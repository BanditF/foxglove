//! The art library view (#59, docs/art-library.md): every piece of right-screen art Foxglove
//! knows about in one grid. That covers built-ins, the keyboard's saved slots, the library
//! folder and the sets bundled with the app, with the same art shown once. Each card shows what
//! is on the keyboard now and what uses the piece (pin, hotlist, layer art).

use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::Arc;

use super::art_cycle::Item;
use super::art_service::{Choice, Handle, Snapshot};
use super::{art_library, art_preview};

/// Sets that ship with the app: the lighting presets' art (#273) and the plugins' (#238, #259).
const BUNDLED: &[(&str, &[u8])] = &[
    ("fury.bin", include_bytes!("../../resources/lighting/art/fury.bin")),
    ("constellations.bin", include_bytes!("../../resources/lighting/art/constellations.bin")),
    ("hearth.bin", include_bytes!("../../resources/lighting/art/hearth.bin")),
    ("moon-garden.bin", include_bytes!("../../resources/lighting/art/moon-garden.bin")),
    ("bandit-focus.bin", include_bytes!("../../../assets/plugins/org.foxglove.focus-timer/focus.bin")),
    ("bandit-onair.bin", include_bytes!("../../../assets/plugins/org.foxglove.broadcast-status/onair.bin")),
];

/// Where a piece lives. A piece can be in several places.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Place {
    /// A built-in set: its index on this firmware and its manifest name, when known.
    BuiltIn(i8, Option<String>),
    Slot(u8),
    Library(PathBuf),
    /// Index into `BUNDLED`.
    Bundled(usize),
}

impl Place {
    fn badge(&self) -> &'static str {
        match self {
            Place::BuiltIn(..) => "built-in",
            Place::Slot(_) => "saved",
            Place::Library(_) => "library",
            Place::Bundled(_) => "bundled",
        }
    }

    /// The hotlist entry for this place, if it can be one.
    fn item(&self) -> Option<Item> {
        match self {
            Place::BuiltIn(_, Some(name)) => Some(Item::BuiltIn(name.clone())),
            Place::BuiltIn(_, None) | Place::Bundled(_) => None,
            Place::Slot(n) => Some(Item::Slot(*n)),
            Place::Library(p) => Some(Item::File(p.clone())),
        }
    }

    /// What choosing this place shows on the keyboard.
    fn choice(&self) -> Option<Choice> {
        match self {
            Place::BuiltIn(n, _) => Some(Choice::BuiltIn(*n)),
            Place::Slot(n) => Some(Choice::Slot(*n)),
            Place::Library(p) => Some(Choice::File(p.clone())),
            Place::Bundled(_) => None,
        }
    }
}

#[derive(Clone)]
pub struct Piece {
    pub label: String,
    pub places: Vec<Place>,
    pub preview: Option<Arc<egui::ColorImage>>,
    pub bytes: Option<usize>,
    /// Frames and fps, when known (from the built-in manifest).
    pub detail: Option<String>,
    hash: Option<u64>,
}

impl Piece {
    /// The quickest way to show it: built-in, then slot, then a push.
    fn best(&self) -> Option<&Place> {
        let rank = |p: &Place| match p {
            Place::BuiltIn(..) => 0,
            Place::Slot(_) => 1,
            Place::Library(_) => 2,
            Place::Bundled(_) => 3,
        };
        self.places.iter().min_by_key(|p| rank(p))
    }
}

/// A library or bundled blob, as the view needs it.
pub struct Blob {
    pub place: Place,
    pub label: String,
    pub preview: Option<Arc<egui::ColorImage>>,
    pub bytes: usize,
    pub hash: u64,
}

/// Every piece, merged. A library or bundled blob joins a built-in when the wallpaper index says
/// it is that built-in (`right.builtin`), and blobs with equal bytes join each other.
pub fn pieces(s: &Snapshot, blobs: Vec<Blob>, builtin_of: &dyn Fn(&str) -> Option<String>) -> Vec<Piece> {
    let count = s.status.map_or(0, |st| st.right_set_count).min(128);
    let names = super::art_manifest::sets(count);
    let mut out: Vec<Piece> = (0..count)
        .map(|n| {
            let set = names.map(|sets| &sets[usize::from(n)]);
            Piece {
                label: set.map_or_else(|| format!("Set {n}"), |set| set.label.clone()),
                places: vec![Place::BuiltIn(n as i8, set.map(|set| set.name.clone()))],
                preview: None,
                bytes: None,
                detail: set.map(|set| format!("{} frames, {} fps", set.frames, set.fps)),
                hash: None,
            }
        })
        .collect();
    for slot in s.slots.iter().filter(|i| i.used) {
        out.push(Piece {
            label: slot.name.clone(),
            places: vec![Place::Slot(slot.slot)],
            preview: None,
            bytes: Some(usize::from(slot.len)),
            detail: None,
            hash: None,
        });
    }
    for blob in blobs {
        let file_name = match &blob.place {
            Place::Library(p) => p.file_name().map(|f| f.to_string_lossy().into_owned()),
            _ => None,
        };
        let builtin = file_name.as_deref().and_then(builtin_of);
        let same = out.iter().position(|p| {
            p.hash == Some(blob.hash)
                || builtin.as_ref().is_some_and(|b| {
                    p.places.iter().any(|pl| matches!(pl, Place::BuiltIn(_, Some(n)) if n == b))
                })
        });
        match same {
            Some(i) => {
                let p = &mut out[i];
                p.places.push(blob.place);
                p.preview = p.preview.take().or(blob.preview);
                p.bytes = p.bytes.or(Some(blob.bytes));
                p.hash = p.hash.or(Some(blob.hash));
            }
            None => out.push(Piece {
                label: blob.label,
                places: vec![blob.place],
                preview: blob.preview,
                bytes: Some(blob.bytes),
                detail: None,
                hash: Some(blob.hash),
            }),
        }
    }
    out
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Filter {
    #[default]
    All,
    OnKeyboard,
    Library,
    Hotlist,
}

#[derive(Default)]
struct ViewState {
    open: bool,
    filter: Filter,
    search: String,
    textures: Vec<(u64, egui::TextureHandle)>,
    /// A slot whose Clear was clicked once.
    confirm_clear: Option<u8>,
    message: Option<String>,
}

thread_local! { static VIEW: RefCell<ViewState> = RefCell::new(ViewState::default()); }

/// The "Art library" button for the Right screen art section.
pub fn button(ui: &mut egui::Ui) {
    if ui.button("Art library…").clicked() {
        VIEW.with_borrow_mut(|v| v.open = true);
    }
}

fn bundled_blobs() -> Vec<Blob> {
    BUNDLED
        .iter()
        .enumerate()
        .map(|(i, (name, bytes))| Blob {
            place: Place::Bundled(i),
            label: name.trim_end_matches(".bin").replace('-', " "),
            preview: art_preview::first_frame(bytes).ok().map(|img| Arc::new(upright(&img))),
            bytes: bytes.len(),
            hash: art_library::blob_hash(bytes),
        })
        .collect()
}

/// NVA1 frames are stored turned 90° clockwise (the nice!view is mounted sideways); turn the
/// thumbnail back upright.
fn upright(img: &egui::ColorImage) -> egui::ColorImage {
    let [w, h] = img.size; // stored: w = upright height, h = upright width
    let mut pixels = Vec::with_capacity(w * h);
    for y in 0..w {
        for x in 0..h {
            // upright (x, y) = stored (w - 1 - y, x)
            pixels.push(img.pixels[x * w + (w - 1 - y)]);
        }
    }
    egui::ColorImage::new([h, w], pixels)
}

/// The library window; call every frame from the settings UI.
pub fn window(ctx: &egui::Context, h: &Handle, s: &Snapshot) {
    if !VIEW.with_borrow(|v| v.open) {
        return;
    }
    let library = art_library::snapshot();
    let mut blobs: Vec<Blob> = library
        .entries
        .iter()
        .map(|e| Blob {
            place: Place::Library(e.path.clone()),
            label: e.path.file_stem().unwrap_or_default().to_string_lossy().into_owned(),
            preview: e.preview.as_ref().ok().map(|img| Arc::new(upright(img))),
            bytes: e.bytes,
            hash: e.hash,
        })
        .collect();
    blobs.extend(bundled_blobs());
    let index = super::wallpaper::index();
    let builtin_of = |file: &str| {
        index
            .iter()
            .find(|w| w.right.library.eq_ignore_ascii_case(file))
            .and_then(|w| w.right.builtin.clone())
    };
    let all = pieces(s, blobs, &builtin_of);

    let mut open = true;
    egui::Window::new("Art library")
        .open(&mut open)
        .default_size([640.0, 520.0])
        .show(ctx, |ui| VIEW.with_borrow_mut(|v| contents(ui, v, h, s, &all, library.error.as_deref())));
    if !open {
        VIEW.with_borrow_mut(|v| v.open = false);
    }
}

fn on_screen(s: &Snapshot, place: &Place) -> bool {
    match place {
        Place::BuiltIn(n, _) => s.showing_built_in() == Some(*n),
        Place::Library(p) => s.showing_file() == Some(p),
        // A slot plays like a pushed set; the pin says which.
        Place::Slot(n) => {
            s.pinned == Some(Choice::Slot(*n))
                && s.status.is_some_and(|st| st.right_state == digitalis::protocol::right_state::PLAYING)
        }
        Place::Bundled(_) => false,
    }
}

fn contents(ui: &mut egui::Ui, v: &mut ViewState, h: &Handle, s: &Snapshot, all: &[Piece], error: Option<&str>) {
    ui.horizontal_wrapped(|ui| {
        for (f, text) in [
            (Filter::All, "All"),
            (Filter::OnKeyboard, "On the keyboard"),
            (Filter::Library, "Library"),
            (Filter::Hotlist, "Hotlist"),
        ] {
            ui.selectable_value(&mut v.filter, f, text);
        }
        ui.add(egui::TextEdit::singleline(&mut v.search).hint_text("Search").desired_width(140.0));
        if ui.button("Rescan").clicked() {
            art_library::refresh();
            v.textures.clear();
        }
    });
    if let Some(e) = error {
        ui.colored_label(ui.visuals().error_fg_color, e);
    }
    if let Some(m) = &v.message {
        ui.colored_label(ui.visuals().warn_fg_color, m);
    }
    let in_hotlist = |p: &Piece| p.places.iter().filter_map(Place::item).any(|i| s.cycle.items.contains(&i));
    let search = v.search.to_lowercase();
    let shown: Vec<&Piece> = all
        .iter()
        .filter(|p| match v.filter {
            Filter::All => true,
            Filter::OnKeyboard => {
                p.places.iter().any(|pl| matches!(pl, Place::BuiltIn(..) | Place::Slot(_)))
                    || p.places.iter().any(|pl| on_screen(s, pl))
            }
            Filter::Library => p.places.iter().any(|pl| matches!(pl, Place::Library(_) | Place::Bundled(_))),
            Filter::Hotlist => in_hotlist(p),
        })
        .filter(|p| search.is_empty() || p.label.to_lowercase().contains(&search))
        .collect();
    if shown.is_empty() {
        ui.weak("No art here yet: add .bin files to the library folder, or save a set to the keyboard.");
    }
    egui::ScrollArea::vertical().show(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            for piece in shown {
                card(ui, v, h, s, piece, in_hotlist(piece));
            }
        });
    });
}

fn card(ui: &mut egui::Ui, v: &mut ViewState, h: &Handle, s: &Snapshot, piece: &Piece, hotlisted: bool) {
    let showing = piece.places.iter().any(|pl| on_screen(s, pl));
    let pinned = piece.places.iter().filter_map(Place::choice).any(|c| s.pinned.as_ref() == Some(&c));
    let layer = piece
        .places
        .iter()
        .filter_map(Place::item)
        .find_map(|i| s.cycle.layers.iter().find(|l| l.art == i).map(|l| l.layer));
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.set_width(150.0);
        ui.vertical(|ui| {
            match &piece.preview {
                Some(img) => {
                    let id = piece.hash.unwrap_or(0);
                    let index = v.textures.iter().position(|(h, _)| *h == id).unwrap_or_else(|| {
                        let t = ui.ctx().load_texture(format!("art-lib-{id}"), (**img).clone(), egui::TextureOptions::NEAREST);
                        v.textures.push((id, t));
                        v.textures.len() - 1
                    });
                    ui.image((v.textures[index].1.id(), egui::vec2(68.0, 140.0)));
                }
                None => {
                    let (rect, _) = ui.allocate_exact_size(egui::vec2(68.0, 140.0), egui::Sense::hover());
                    ui.painter().rect_stroke(rect, 2.0, ui.visuals().widgets.noninteractive.bg_stroke, egui::StrokeKind::Inside);
                    ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, "no preview", egui::FontId::proportional(10.0), ui.visuals().weak_text_color());
                }
            }
            ui.strong(&piece.label);
            if showing {
                ui.colored_label(ui.visuals().selection.bg_fill, "● on screen");
            }
            let mut tags: Vec<String> = Vec::new();
            for pl in &piece.places {
                tags.push(match pl {
                    Place::Slot(n) => format!("slot {}", n + 1),
                    other => other.badge().to_string(),
                });
            }
            if pinned {
                tags.push("pinned".into());
            }
            if hotlisted {
                tags.push("hotlist".into());
            }
            if let Some(l) = layer {
                tags.push(format!("layer {l} art"));
            }
            ui.weak(tags.join(" · "));
            let mut info = Vec::new();
            if let Some(b) = piece.bytes {
                info.push(format!("{:.1} KiB", b as f32 / 1024.0));
            }
            if let Some(d) = &piece.detail {
                info.push(d.clone());
            }
            if !info.is_empty() {
                ui.weak(info.join(", "));
            }
            let too_big = piece.bytes.is_some_and(|b| b > 16 * 1024);
            if too_big {
                ui.colored_label(ui.visuals().warn_fg_color, "over 16 KiB: too big to push");
            }
            ui.horizontal_wrapped(|ui| {
                let best = piece.best().cloned();
                if ui
                    .add_enabled(!s.busy && !too_big && best.is_some(), egui::Button::new("Show"))
                    .clicked()
                {
                    match best {
                        Some(Place::BuiltIn(n, _)) => {
                            h.select(n);
                        }
                        Some(Place::Slot(n)) => {
                            h.play_slot(n);
                        }
                        Some(Place::Library(p)) => {
                            h.push(p);
                        }
                        Some(Place::Bundled(i)) => match copy_to_library(i) {
                            Ok(p) => {
                                h.push(p);
                            }
                            Err(e) => v.message = Some(e),
                        },
                        None => {}
                    }
                }
                let item = piece.places.iter().find_map(Place::item);
                if let Some(item) = item {
                    let text = if hotlisted { "★ hotlist" } else { "☆ hotlist" };
                    if ui.small_button(text).clicked() {
                        let mut cycle = s.cycle.clone();
                        if hotlisted {
                            cycle.items.retain(|i| piece.places.iter().filter_map(Place::item).all(|p| &p != i));
                            cycle.position = cycle.position.min(cycle.items.len().saturating_sub(1));
                        } else {
                            cycle.items.push(item);
                        }
                        if let Err(e) = h.set_cycle(cycle) {
                            v.message = Some(e);
                        }
                    }
                }
                for pl in &piece.places {
                    match pl {
                        Place::Slot(n) => {
                            let armed = v.confirm_clear == Some(*n);
                            if ui.small_button(if armed { "Clear? Click again" } else { "Clear slot" }).clicked() {
                                if armed {
                                    h.erase_slot(*n);
                                    v.confirm_clear = None;
                                } else {
                                    v.confirm_clear = Some(*n);
                                }
                            }
                        }
                        Place::Library(p) => {
                            if ui.small_button("Recycle").on_hover_text("Moves the file to the Recycle Bin").clicked() {
                                art_library::delete(p.clone());
                            }
                        }
                        Place::Bundled(i)
                            if !piece.places.iter().any(|p| matches!(p, Place::Library(_)))
                                && ui.small_button("Add to library").clicked() =>
                        {
                            if let Err(e) = copy_to_library(*i) {
                                v.message = Some(e);
                            }
                        }
                        _ => {}
                    }
                }
            });
        });
    });
}

/// Writes a bundled set into the library folder (so it can be pushed, hotlisted or saved) and
/// returns its path.
fn copy_to_library(i: usize) -> Result<PathBuf, String> {
    let (name, bytes) = BUNDLED.get(i).ok_or("no such set")?;
    let folder = art_library::folder().ok_or("No settings directory")?;
    std::fs::create_dir_all(&folder).map_err(|e| e.to_string())?;
    let path = folder.join(name);
    if !path.exists() {
        std::fs::write(&path, bytes).map_err(|e| format!("couldn't copy {name}: {e}"))?;
    }
    art_library::refresh();
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blob(place: Place, hash: u64) -> Blob {
        Blob { place, label: "x".into(), preview: None, bytes: 100, hash }
    }

    #[test]
    fn same_bytes_merge_and_wallpaper_builtins_join() {
        let s = Snapshot::default(); // no status: no built-ins listed
        let lib = |n: &str| Place::Library(PathBuf::from(n));
        let all = pieces(&s, vec![blob(lib("a.bin"), 1), blob(Place::Bundled(0), 1), blob(lib("b.bin"), 2)], &|_| None);
        assert_eq!(all.len(), 2, "a.bin and the bundled copy are one piece");
        assert_eq!(all[0].places, vec![lib("a.bin"), Place::Bundled(0)]);
        assert_eq!(all[0].best(), Some(&lib("a.bin")), "a library file pushes; bundled needs a copy");
    }

    #[test]
    fn bundled_sets_decode_and_turn_upright() {
        for b in bundled_blobs() {
            let img = b.preview.unwrap_or_else(|| panic!("{} decodes", b.label));
            assert_eq!(img.size, [68, 140], "{} is portrait", b.label);
            assert!(b.bytes <= 12 * 1024, "{} stays under the library target", b.label);
        }
    }

    #[test]
    fn upright_turns_a_stored_frame_back() {
        // stored 3 wide x 2 high; the top-right stored pixel is the upright top-left
        let px = |v: u8| egui::Color32::from_gray(v);
        let stored = egui::ColorImage::new([3, 2], vec![px(1), px(2), px(3), px(4), px(5), px(6)]);
        let up = upright(&stored);
        assert_eq!(up.size, [2, 3]);
        assert_eq!(up.pixels, vec![px(3), px(6), px(2), px(5), px(1), px(4)]);
    }
}
