//! The Right screen art card on the Screens page (#388, epic #393), and the file picker shared
//! with the tray.
//!
//! As the approved canvas draws it: what's playing (with its saved slot), the library as small
//! 1-bit thumbnails drawn the way the right screen shows them, Push file..., the saved slots
//! behind a one-line summary, and the hotlist as pills. Clicks become [`Act`]s that the art
//! service carries out, so the previews draw the card with no keyboard.
use super::art_cycle::{Cycle, Follow, Item, Step};
use super::{art_library, art_service};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use egui::{Color32, CornerRadius, RichText, Sense, Stroke};

#[derive(Default)]
struct UiState {
    dialog: egui_file_dialog::FileDialog,
    /// Thumbnail textures by file, made once per library scan.
    textures: Vec<(PathBuf, egui::TextureHandle)>,
    save_slot: u8,
    save_name: String,
    /// The file name the Save box started from when it was longer than a slot name holds.
    save_name_cut: Option<String>,
    /// The slot whose Erase was clicked once and waits for the second click.
    confirm_erase: Option<u8>,
    /// The saved slots' list is open.
    slots_open: bool,
}
thread_local! { static UI: std::cell::RefCell<UiState> = std::cell::RefCell::new(UiState::default()); }

pub fn update(ctx: &egui::Context) {
    art_service::set_wake(crate::ui_wake::UiWake::from_ctx(ctx));
    UI.with_borrow_mut(|state| {
        if art_library::PICK_FILE.swap(false, Ordering::Relaxed) {
            state.dialog.pick_file();
        }
        state.dialog.update(ctx);
        if let Some(path) = state.dialog.take_picked() {
            if let Some(h) = art_service::handle() {
                h.push(path);
            }
        }
    });
}

/// What a click on the card asks the art service (or the library) to do.
#[derive(Clone, Debug, PartialEq)]
enum Act {
    Select(i8),
    Push(PathBuf),
    Delete(PathBuf),
    Refresh,
    PlaySlot(u8),
    Save(u8, String),
    Erase(u8),
    Cycle(Cycle),
    Next,
    Cancel,
}

#[cfg(test)]
thread_local! {
    /// The previews' art state and library, so they never ask a real keyboard.
    pub static PREVIEW: std::cell::RefCell<Option<(art_service::Snapshot, art_library::Library)>> =
        const { std::cell::RefCell::new(None) };
}

/// The Right screen art card; nothing until a keyboard with the art relay answers.
pub fn group(ui: &mut egui::Ui) {
    #[cfg(test)]
    if let Some((s, library)) = PREVIEW.with_borrow(|p| p.clone()) {
        if s.connected {
            card(ui, &s, &library);
        }
        return;
    }
    let Some(h) = art_service::handle() else {
        return;
    };
    let s = h.snapshot();
    if !s.connected {
        return;
    }
    let library = art_library::snapshot();
    for act in card(ui, &s, &library) {
        match act {
            Act::Select(n) => {
                let _ = h.select(n);
            }
            Act::Push(p) => {
                let _ = h.push(p);
            }
            Act::Delete(p) => art_library::delete(p),
            Act::Refresh => {
                h.refresh();
                // Wallpaper Engine may have been installed, moved or started (#446): looked
                // for on the wallpaper service's thread, not this one.
                super::wallpaper::refresh();
                art_library::refresh();
                UI.with_borrow_mut(|s| s.textures.clear());
            }
            Act::PlaySlot(n) => {
                let _ = h.play_slot(n);
            }
            Act::Save(n, name) => {
                let _ = h.save_to_slot(n, name);
            }
            Act::Erase(n) => {
                let _ = h.erase_slot(n);
            }
            Act::Cycle(c) => {
                if let Err(e) = h.set_cycle(c) {
                    eprintln!("Foxglove: can't save the hotlist: {e}");
                }
            }
            Act::Next => {
                let _ = h.next();
            }
            Act::Cancel => h.cancel(),
        }
    }
    super::art_library_view::window(ui.ctx(), &h, &s);
}

fn card(ui: &mut egui::Ui, s: &art_service::Snapshot, library: &art_library::Library) -> Vec<Act> {
    let mut acts = Vec::new();
    super::settings_ui::section(ui, "Right screen art", |ui| {
        UI.with_borrow_mut(|state| draw(ui, s, library, state, &mut acts));
    });
    acts
}

/// One library piece as the card shows it.
#[derive(Clone)]
pub struct Tile {
    pub path: PathBuf,
    pub name: String,
    /// The first frame as stored (turned 90°), white = lit.
    pub image: Arc<egui::ColorImage>,
}

/// The library as tiles: each piece once (by content, then by name), sorted by name; files that
/// can't be read are left out and named in the second list, never shown as rows (#388, #367).
pub fn tiles(entries: &[art_library::Entry]) -> (Vec<Tile>, Vec<String>) {
    let mut out: Vec<Tile> = Vec::new();
    let mut unreadable = Vec::new();
    let mut hashes = std::collections::HashSet::new();
    let mut sorted: Vec<&art_library::Entry> = entries.iter().collect();
    sorted.sort_by_key(|e| {
        e.path
            .file_stem()
            .map(|n| n.to_string_lossy().to_lowercase())
    });
    for e in sorted {
        if !art_library::is_right_art_file(&e.path) {
            continue;
        }
        let name = e
            .path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        match &e.preview {
            Err(_) => unreadable.push(name),
            Ok(image) => {
                let same_name = out.iter().any(|t| t.name.eq_ignore_ascii_case(&name));
                if hashes.insert(e.hash) && !same_name {
                    out.push(Tile {
                        path: e.path.clone(),
                        name,
                        image: image.clone(),
                    });
                }
            }
        }
    }
    (out, unreadable)
}

/// The right screen's tones: lit pixels light grey, the rest near black.
const LIT: Color32 = Color32::from_rgb(0xe8, 0xea, 0xe9);
const OFF: Color32 = Color32::from_rgb(0x0b, 0x0d, 0x0c);

/// A stored frame turned upright, in the screen's tones, at full size or halved. The stored
/// background is a set bit (white in `art_preview`) and the ink a clear one; the screen lights
/// the ink, so ink becomes [`LIT`] and the background [`OFF`] (not big white blocks). Halving
/// averages 2×2 and dithers back to 1 bit (ordered 2×2), so a thumbnail stays a 1-bit picture
/// like the device's, not a grey blur.
pub fn thumbnail(stored: &egui::ColorImage, half: bool) -> egui::ColorImage {
    let [sw, sh] = stored.size; // stored: sw = upright height, sh = upright width
    let lit = |x: usize, y: usize| -> f32 {
        // upright (x, y) = stored (sw - 1 - y, x)
        let p = stored.pixels[x * sw + (sw - 1 - y)];
        1.0 - f32::from(p.r().max(p.g()).max(p.b())) / 255.0
    };
    let (w, h) = (sh, sw);
    if !half {
        let pixels = (0..h)
            .flat_map(|y| (0..w).map(move |x| (x, y)))
            .map(|(x, y)| if lit(x, y) >= 0.5 { LIT } else { OFF });
        return egui::ColorImage::new([w, h], pixels.collect());
    }
    const BAYER: [[f32; 2]; 2] = [[0.125, 0.625], [0.875, 0.375]];
    let (tw, th) = (w / 2, h / 2);
    let mut pixels = Vec::with_capacity(tw * th);
    for y in 0..th {
        for x in 0..tw {
            let level = (lit(2 * x, 2 * y)
                + lit(2 * x + 1, 2 * y)
                + lit(2 * x, 2 * y + 1)
                + lit(2 * x + 1, 2 * y + 1))
                / 4.0;
            pixels.push(if level > BAYER[y % 2][x % 2] {
                LIT
            } else {
                OFF
            });
        }
    }
    egui::ColorImage::new([tw, th], pixels)
}

/// What plays on the right now: (name, where it comes from).
pub fn playing(s: &art_service::Snapshot) -> (String, String) {
    let Some(st) = s.status else {
        return ("…".into(), "Waiting for the keyboard".into());
    };
    let count = i16::from(st.right_set_count);
    let set = i16::from(st.right_set);
    if st.flash_slots > 0 && set >= count && set < count + i16::from(st.flash_slots) {
        let slot = (set - count) as u8;
        let name = s
            .slots
            .iter()
            .find(|i| i.slot == slot && i.used)
            .map_or_else(|| format!("slot {}", slot + 1), |i| i.name.clone());
        return (name, format!("Saved on the keyboard · slot {}", slot + 1));
    }
    if let Some(n) = s.showing_built_in() {
        let name = s
            .built_ins()
            .into_iter()
            .find(|(i, _)| *i == n)
            .map_or_else(|| format!("Set {n}"), |(_, name)| name);
        return (name, "Built in".into());
    }
    if let Some(p) = s.showing_file() {
        return (
            p.file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
            "Pushed from this PC".into(),
        );
    }
    let state = digitalis::protocol::RIGHT_STATES
        .get(usize::from(st.right_state))
        .copied()
        .unwrap_or("busy");
    ("…".into(), format!("The right half is {state}"))
}

fn texture(ui: &egui::Ui, state: &mut UiState, tile: &Tile, half: bool) -> egui::TextureHandle {
    let key = if half {
        tile.path.with_extension("thumb")
    } else {
        tile.path.clone()
    };
    if let Some((_, t)) = state.textures.iter().find(|(p, _)| *p == key) {
        return t.clone();
    }
    let t = ui.ctx().load_texture(
        format!("art-{}", key.display()),
        thumbnail(&tile.image, half),
        egui::TextureOptions::NEAREST,
    );
    state.textures.push((key, t.clone()));
    t
}

fn draw(
    ui: &mut egui::Ui,
    s: &art_service::Snapshot,
    library: &art_library::Library,
    state: &mut UiState,
    acts: &mut Vec<Act>,
) {
    let p = super::theme::palette(ui);
    let (tiles, unreadable) = tiles(&library.entries);
    let showing = s.showing_file().cloned();

    // Playing now: the picture, its name and where it comes from.
    let (name, from) = playing(s);
    ui.horizontal(|ui| {
        let playing_tile = showing
            .as_ref()
            .and_then(|path| tiles.iter().find(|t| t.path == *path));
        match playing_tile {
            Some(tile) => {
                let t = texture(ui, state, tile, false);
                ui.add(egui::Image::new((t.id(), t.size_vec2())).corner_radius(3));
            }
            None => {
                let (rect, _) = ui.allocate_exact_size(egui::vec2(68.0, 140.0), Sense::hover());
                ui.painter().rect(
                    rect,
                    CornerRadius::same(3),
                    OFF,
                    Stroke::new(1.0, p.control_line),
                    egui::StrokeKind::Inside,
                );
                ui.painter().text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    "ART",
                    super::theme::mono(11.0),
                    p.muted,
                );
            }
        }
        ui.vertical(|ui| {
            ui.label(super::theme::caps_label(ui, "Playing"));
            ui.label(
                RichText::new(name)
                    .font(super::theme::hud(16.0))
                    .color(p.text_strong),
            );
            ui.weak(from);
            if s.pinned.is_some() {
                ui.weak("Comes back after the keyboard restarts.");
            }
            ui.add_space(6.0);
            ui.add_enabled_ui(!s.busy, |ui| built_in_picker(ui, s, acts));
        });
    });

    ui.add_space(6.0);
    ui.label(super::theme::caps_label(ui, "Library"));
    if tiles.is_empty() {
        ui.weak("No art in the library yet. Push a file to add one.");
    }
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(8.0, 8.0);
        for tile in &tiles {
            let on = showing.as_ref() == Some(&tile.path);
            let t = texture(ui, state, tile, true);
            let r = library_tile(ui, &t, &tile.name, on);
            if r.clicked() && !on && !s.busy {
                acts.push(Act::Push(tile.path.clone()));
            }
            r.on_hover_text(if on {
                "On the keyboard now"
            } else {
                "Click to show it on the right screen"
            })
            .context_menu(|ui| {
                if ui.button("Move to the Recycle Bin").clicked() {
                    acts.push(Act::Delete(tile.path.clone()));
                    ui.close();
                }
            });
        }
    });
    if !unreadable.is_empty() {
        ui.weak(format!(
            "{} file(s) in the art folder couldn't be read.",
            unreadable.len()
        ))
        .on_hover_text(unreadable.join(", "));
    }
    if let Some(error) = &library.error {
        ui.colored_label(p.danger, error);
    }

    ui.add_space(4.0);
    ui.horizontal_wrapped(|ui| {
        ui.add_enabled_ui(!s.busy, |ui| {
            if ui.button("Push file...").clicked() {
                art_library::PICK_FILE.store(true, Ordering::Relaxed);
            }
        });
        let refresh = ui.button("Refresh");
        let refresh = match art_library::folder() {
            Some(folder) => refresh.on_hover_text(format!("Art folder: {}", folder.display())),
            None => refresh,
        };
        if refresh.clicked() {
            acts.push(Act::Refresh);
        }
        super::art_library_view::button(ui);
    });

    slots(ui, s, state, acts);
    hotlist(ui, s, &tiles, acts);

    if s.busy {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            match s.progress() {
                Some((done, total)) => {
                    let f = if total == 0 {
                        0.0
                    } else {
                        f32::from(done) / f32::from(total)
                    };
                    ui.add(
                        egui::ProgressBar::new(f)
                            .desired_width(180.0)
                            .text(format!("{done} / {total} bytes")),
                    );
                }
                None => {
                    ui.spinner();
                }
            }
            if ui.button("Cancel").clicked() {
                acts.push(Act::Cancel);
            }
        });
    }
    if s.cancelled {
        ui.weak("Cancelled");
    }
    if let Some(error) = &s.error {
        ui.colored_label(p.danger, error);
    }
}

/// A library tile: the half-size picture over its name; an orange ring when it's on the keyboard.
fn library_tile(
    ui: &mut egui::Ui,
    t: &egui::TextureHandle,
    name: &str,
    on: bool,
) -> egui::Response {
    let p = super::theme::palette(ui);
    let size = egui::vec2(t.size_vec2().x.max(64.0) + 12.0, t.size_vec2().y + 30.0);
    let (rect, r) = ui.allocate_exact_size(size, Sense::click());
    let line = if on {
        p.accent
    } else if r.hovered() || r.has_focus() {
        p.control_line.gamma_multiply(1.6)
    } else {
        p.line
    };
    let painter = ui.painter();
    painter.rect(
        rect,
        CornerRadius::same(6),
        p.control,
        Stroke::new(if on { 2.0 } else { 1.0 }, line),
        egui::StrokeKind::Inside,
    );
    let img = egui::Rect::from_min_size(
        egui::pos2(rect.center().x - t.size_vec2().x / 2.0, rect.top() + 6.0),
        t.size_vec2(),
    );
    painter.image(
        t.id(),
        img,
        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
        Color32::WHITE,
    );
    let shown: String = if name.chars().count() > 11 {
        name.chars().take(10).chain(['…']).collect()
    } else {
        name.to_string()
    };
    painter.text(
        egui::pos2(rect.center().x, rect.bottom() - 7.0),
        egui::Align2::CENTER_BOTTOM,
        shown,
        super::theme::body(11.5),
        p.text,
    );
    r
}

fn built_in_picker(ui: &mut egui::Ui, s: &art_service::Snapshot, acts: &mut Vec<Act>) {
    let built_ins = s.built_ins();
    if built_ins.is_empty() {
        return;
    }
    let showing = s.showing_built_in();
    let current = built_ins
        .iter()
        .find(|(n, _)| Some(*n) == showing)
        .map_or_else(|| "Choose built-in".to_string(), |(_, name)| name.clone());
    egui::ComboBox::from_id_salt("right-art-built-in")
        .selected_text(current)
        .show_ui(ui, |ui| {
            if ui.selectable_label(false, "Random").clicked() {
                acts.push(Act::Select(-1));
            }
            for (n, name) in built_ins {
                // Selected = what the right half shows now.
                if ui.selectable_label(Some(n) == showing, name).clicked() {
                    acts.push(Act::Select(n));
                }
            }
        });
}

/// The right half's flash slots (firmware #123): one summary line, the list behind a click.
fn slots(ui: &mut egui::Ui, s: &art_service::Snapshot, state: &mut UiState, acts: &mut Vec<Act>) {
    use digitalis::protocol::right_state;
    let Some(st) = s.status.filter(|st| st.flash_slots > 0) else {
        return;
    };
    let used = s.slots.iter().filter(|i| i.used).count();
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.label(format!(
            "Saved on the keyboard: {used} of {}",
            st.flash_slots
        ));
        let text = if state.slots_open {
            "Hide slots"
        } else {
            "Show slots"
        };
        if ui.small_button(text).clicked() {
            state.slots_open = !state.slots_open;
        }
    });
    if !state.slots_open {
        return;
    }
    ui.add_enabled_ui(!s.busy, |ui| {
        for info in s.slots.iter().filter(|i| i.used) {
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("{}", info.slot + 1)).font(super::theme::mono(12.5)));
                ui.label(&info.name);
                let playing = i16::from(st.right_set) == i16::from(st.right_set_count) + i16::from(info.slot);
                if playing {
                    ui.weak("playing");
                } else if ui.small_button("Play").clicked() {
                    acts.push(Act::PlaySlot(info.slot));
                }
                let confirming = state.confirm_erase == Some(info.slot);
                if ui.small_button(if confirming { "Erase? Click again" } else { "Erase" }).clicked() {
                    if confirming {
                        state.confirm_erase = None;
                        acts.push(Act::Erase(info.slot));
                    } else {
                        state.confirm_erase = Some(info.slot);
                    }
                }
            });
        }
        let empty = usize::from(st.flash_slots).saturating_sub(used);
        if empty > 0 {
            ui.weak(format!("{empty} empty"));
        }
        // Saving copies the pushed set now playing on the right into flash.
        let can_save = st.right_state == right_state::PLAYING;
        ui.add_enabled_ui(can_save, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label("Save what's playing to");
                egui::ComboBox::from_id_salt("art-save-slot")
                    .selected_text(format!("slot {}", state.save_slot + 1))
                    .show_ui(ui, |ui| {
                        for n in 0..st.flash_slots {
                            let taken = st.flash_used & (1 << n) != 0;
                            let text = format!("slot {}{}", n + 1, if taken { " (replace)" } else { "" });
                            ui.selectable_value(&mut state.save_slot, n, text);
                        }
                    });
                if state.save_name.is_empty() {
                    if let Some(art_service::Choice::File(p)) = &s.pinned {
                        let stem = p.file_stem().unwrap_or_default().to_string_lossy().into_owned();
                        let (name, cut) = fit_save_name(&stem);
                        state.save_name = name;
                        state.save_name_cut = cut;
                    }
                }
                let edit = ui.add(egui::TextEdit::singleline(&mut state.save_name).hint_text("name").desired_width(120.0));
                if edit.changed() {
                    state.save_name = fit_save_name(&state.save_name).0;
                    state.save_name_cut = None;
                }
                let used = state.save_name.len();
                let full = digitalis::protocol::art::NAME_LEN;
                let counter = format!("{used}/{full}");
                if used >= full {
                    ui.colored_label(ui.visuals().warn_fg_color, counter)
                        .on_hover_text("A slot name holds 16 characters; it can't take more.");
                } else {
                    ui.weak(counter);
                }
                if ui.button("Save").clicked() {
                    acts.push(Act::Save(state.save_slot, state.save_name.trim().to_string()));
                }
            });
        });
        if let Some(from) = &state.save_name_cut {
            ui.colored_label(
                ui.visuals().warn_fg_color,
                format!("\"{from}\" is longer than the 16 characters a slot name holds; edit the name above if you like."),
            );
        }
        if !can_save {
            ui.weak("Push a file first; saving copies the set now playing on the right.");
        }
    });
}

/// `name` cut to what a slot keeps, and the original when something was cut.
fn fit_save_name(name: &str) -> (String, Option<String>) {
    let fit = digitalis::protocol::art_name_fit(name);
    (
        fit.to_string(),
        (fit.len() < name.len()).then(|| name.to_string()),
    )
}

/// Which hotlist pills are lit, from the cycle settings.
pub fn hotlist_pills(c: &Cycle) -> [bool; 5] {
    let cycles = c.steps_on(Step::Minutes)
        || c.steps_on(Step::LightsWake)
        || c.steps_on(Step::Key)
        || c.follow == Follow::Wallpaper;
    [
        !cycles,
        c.steps_on(Step::Minutes),
        c.follow == Follow::Wallpaper,
        c.steps_on(Step::LightsWake),
        c.steps_on(Step::Key),
    ]
}

/// The hotlist (#60) as the canvas draws it: what moves it on as pills, then the list and Next.
fn hotlist(ui: &mut egui::Ui, s: &art_service::Snapshot, tiles: &[Tile], acts: &mut Vec<Act>) {
    let Some(st) = s.status else {
        return;
    };
    ui.add_space(6.0);
    ui.label(super::theme::caps_label(ui, "Hotlist"));
    let mut cycle = s.cycle.clone();
    let mut changed = false;
    let lit = hotlist_pills(&cycle);
    ui.horizontal_wrapped(|ui| {
        // A pill that doesn't fit moves to the next row rather than wrapping its words.
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
        if super::theme::pill(ui, lit[0], "Don't cycle").clicked() && !lit[0] {
            for step in [Step::Minutes, Step::LightsWake, Step::Key] {
                cycle.set_step(step, false);
            }
            cycle.follow = Follow::Nothing;
            changed = true;
        }
        if super::theme::pill(ui, lit[1], &format!("Every {} min", cycle.minutes)).clicked() {
            cycle.set_step(Step::Minutes, !lit[1]);
            changed = true;
        }
        let engine = super::wallpaper_engine::presence();
        let hover = if engine.installed() {
            "Shows the art that goes with the Wallpaper Engine wallpaper on screen (from the art folder's wallpapers.json)."
        } else {
            "Shows the art that goes with your Windows desktop wallpaper, when its file is named after a wallpaper in the art folder's wallpapers.json. Works best with Wallpaper Engine."
        };
        if super::theme::pill(ui, lit[2], "Match the wallpaper")
            .on_hover_text(hover)
            .clicked()
        {
            cycle.follow = if lit[2] { Follow::Nothing } else { Follow::Wallpaper };
            changed = true;
        }
        let wake = ui.add_enabled(s.wake_events, super::theme::pill_button(ui, lit[3], "When the lights wake"));
        if wake.on_disabled_hover_text("Needs keyboard firmware that reports when the lights come back on.").clicked() {
            cycle.set_step(Step::LightsWake, !lit[3]);
            changed = true;
        }
        ui.add_enabled(false, super::theme::pill_button(ui, lit[4], "On the art key"))
            .on_disabled_hover_text("Needs keyboard firmware with the &art_next key.");
    });
    if lit[1] {
        ui.horizontal(|ui| {
            ui.label("Every");
            changed |= ui
                .add(
                    egui::DragValue::new(&mut cycle.minutes)
                        .range(1..=1440)
                        .suffix(" min"),
                )
                .changed();
        });
    }
    // Wallpaper Engine's monitors, only when it's here; otherwise one short hint (#446).
    let monitors = if cycle.follow == Follow::Wallpaper {
        super::wallpaper::monitors()
    } else {
        Vec::new()
    };
    if cycle.follow == Follow::Wallpaper && !monitors.is_empty() {
        ui.horizontal(|ui| {
            ui.label("Wallpaper of");
            // "//?/DISPLAY#DEL40EA#..." -> "DEL40EA" (the monitor's model).
            let short = |m: &str| m.split('#').nth(1).unwrap_or(m).to_string();
            let shown = cycle
                .wallpaper_monitor
                .as_deref()
                .map_or_else(|| "any monitor".into(), short);
            egui::ComboBox::from_id_salt("art-wallpaper-monitor")
                .selected_text(shown)
                .show_ui(ui, |ui| {
                    changed |= ui
                        .selectable_value(&mut cycle.wallpaper_monitor, None, "any monitor")
                        .changed();
                    for m in monitors {
                        let label = short(&m);
                        changed |= ui
                            .selectable_value(&mut cycle.wallpaper_monitor, Some(m), label)
                            .changed();
                    }
                });
        });
    }
    if cycle.follow == Follow::Wallpaper {
        if let Some(hint) = super::wallpaper_engine::presence().hint() {
            ui.weak(hint);
        }
    }

    let names = super::art_manifest::sets(st.right_set_count);
    let label = |item: &Item| match item {
        Item::BuiltIn(name) => names
            .and_then(|sets| sets.iter().find(|set| &set.name == name))
            .map_or_else(|| name.clone(), |set| set.label.clone()),
        Item::Slot(n) => s.slots.iter().find(|i| i.slot == *n && i.used).map_or_else(
            || format!("slot {} (empty)", n + 1),
            |i| format!("saved \"{}\"", i.name),
        ),
        Item::File(p) => p
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned(),
    };
    let mut remove = None;
    ui.add_space(2.0);
    ui.horizontal_wrapped(|ui| {
        for (i, item) in cycle.items.iter().enumerate() {
            let text = if i == cycle.position {
                format!("▶ {}", label(item))
            } else {
                label(item)
            };
            let r = ui
                .add(egui::Button::new(text).small())
                .on_hover_text("Click to take it off the hotlist");
            if r.clicked() {
                remove = Some(i);
            }
        }
        if cycle.items.is_empty() {
            ui.weak("Nothing on the hotlist yet.");
        }
    });
    if let Some(i) = remove {
        cycle.items.remove(i);
        cycle.position = cycle.position.min(cycle.items.len().saturating_sub(1));
        changed = true;
    }
    ui.horizontal(|ui| {
        let mut add = None;
        egui::ComboBox::from_id_salt("art-hotlist-add")
            .selected_text("Add to hotlist")
            .show_ui(ui, |ui| {
                for set in names.unwrap_or_default() {
                    if ui.selectable_label(false, &set.label).clicked() {
                        add = Some(Item::BuiltIn(set.name.clone()));
                    }
                }
                for info in s.slots.iter().filter(|i| i.used) {
                    if ui
                        .selectable_label(false, format!("saved \"{}\"", info.name))
                        .clicked()
                    {
                        add = Some(Item::Slot(info.slot));
                    }
                }
                for tile in tiles {
                    if ui.selectable_label(false, &tile.name).clicked() {
                        add = Some(Item::File(tile.path.clone()));
                    }
                }
            });
        if let Some(item) = add.filter(|i| !cycle.items.contains(i)) {
            cycle.items.push(item);
            changed = true;
        }
        if ui
            .add_enabled(
                !cycle.items.is_empty() && !s.busy,
                egui::Button::new("Next"),
            )
            .clicked()
        {
            acts.push(Act::Next);
        }
    });
    if names.is_none() {
        ui.weak("Built-ins can be added once the art manifest matches this keyboard's firmware.");
    }

    // Art for a layer while it's active, over everything else (art_triggers).
    egui::CollapsingHeader::new("Layer art").id_salt("art-layer-art").show(ui, |ui| {
        ui.weak("Shown while that layer is active (after 0.4 s, so a quick tap doesn't flash it), then the art before comes back.");
        let layer_names = super::layer_names::remembered();
        egui::Grid::new("art-layer-art-grid").num_columns(2).show(ui, |ui| {
            for (layer, name) in layer_names.iter().enumerate().skip(1) {
                let Ok(layer) = u8::try_from(layer) else { break };
                ui.label(name.clone().unwrap_or_else(|| format!("Layer {layer}")));
                let current = cycle.layer_art(layer).cloned();
                let mut picked: Option<Option<Item>> = None;
                egui::ComboBox::from_id_salt(("art-layer", layer))
                    .selected_text(current.as_ref().map_or_else(|| "none".into(), label))
                    .show_ui(ui, |ui| {
                        if ui.selectable_label(current.is_none(), "none").clicked() {
                            picked = Some(None);
                        }
                        for set in names.unwrap_or_default() {
                            if ui.selectable_label(false, &set.label).clicked() {
                                picked = Some(Some(Item::BuiltIn(set.name.clone())));
                            }
                        }
                        for info in s.slots.iter().filter(|i| i.used) {
                            if ui.selectable_label(false, format!("saved \"{}\"", info.name)).clicked() {
                                picked = Some(Some(Item::Slot(info.slot)));
                            }
                        }
                    });
                if let Some(art) = picked {
                    cycle.set_layer_art(layer, art);
                    changed = true;
                }
                ui.end_row();
            }
        });
        if layer_names.len() <= 1 {
            ui.weak("Layers appear once the keymap has been read.");
        }
    });

    if changed {
        acts.push(Act::Cycle(cycle));
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;

    #[test]
    fn long_names_are_cut_visibly_and_short_ones_left_alone() {
        assert_eq!(
            fit_save_name("hearth-moongarden"),
            ("hearth-moongarde".into(), Some("hearth-moongarden".into()))
        );
        assert_eq!(fit_save_name("fury"), ("fury".into(), None));
        assert_eq!(
            fit_save_name("exactly 16 chars"),
            ("exactly 16 chars".into(), None)
        );
    }

    fn entry(path: &str, hash: u64, ok: bool) -> art_library::Entry {
        let image = Arc::new(egui::ColorImage::new([4, 2], vec![Color32::WHITE; 8]));
        art_library::Entry {
            path: PathBuf::from(path),
            preview: if ok {
                Ok(image)
            } else {
                Err("Invalid NVA1 art blob".into())
            },
            bytes: 10,
            hash,
        }
    }

    /// Each piece once, no `.left` companions and no "Invalid NVA1 art blob" rows (#388).
    #[test]
    fn the_library_lists_each_piece_once() {
        let (tiles, unreadable) = tiles(&[
            entry("art/hud.bin", 1, true),
            entry("art/hud.left.bin", 2, false),
            entry("art/courier.bin", 3, true),
            entry("art/Courier copy.bin", 3, true),
            entry("art/HUD.BIN", 4, true),
            entry("art/broken.bin", 5, false),
            entry("art/notes.txt", 6, true),
        ]);
        let names: Vec<_> = tiles.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, ["courier", "hud"]);
        assert_eq!(
            unreadable,
            ["broken"],
            "the .left companion never shows, even as unreadable"
        );
    }

    #[test]
    fn thumbnails_are_upright_and_one_bit() {
        // Stored 4 wide x 2 high: upright is 2 wide x 4 tall.
        // White is the stored background, black the ink the screen lights.
        let mut px = vec![Color32::WHITE; 8];
        px[3] = Color32::BLACK; // stored (3, 0) = upright (0, 0)
        let stored = egui::ColorImage::new([4, 2], px);
        let full = thumbnail(&stored, false);
        assert_eq!(full.size, [2, 4]);
        assert_eq!(full.pixels[0], LIT);
        assert_eq!(full.pixels.iter().filter(|p| **p == LIT).count(), 1);

        let grey = egui::ColorImage::new(
            [8, 8],
            (0..64)
                .map(|i| {
                    if (i + i / 8) % 2 == 0 {
                        Color32::WHITE
                    } else {
                        Color32::BLACK
                    }
                })
                .collect(),
        );
        let half = thumbnail(&grey, true);
        assert_eq!(half.size, [4, 4]);
        assert!(half.pixels.iter().all(|p| *p == LIT || *p == OFF), "1-bit");
        let lit = half.pixels.iter().filter(|p| **p == LIT).count();
        assert_eq!(lit, 8, "a 50% grey dithers to half lit");
        let ink = thumbnail(
            &egui::ColorImage::new([4, 4], vec![Color32::BLACK; 16]),
            true,
        );
        assert!(ink.pixels.iter().all(|p| *p == LIT));
        let background = thumbnail(
            &egui::ColorImage::new([4, 4], vec![Color32::WHITE; 16]),
            true,
        );
        assert!(background.pixels.iter().all(|p| *p == OFF));
    }

    #[test]
    fn hotlist_pills_follow_the_cycle() {
        let mut c = Cycle::default();
        assert_eq!(hotlist_pills(&c), [true, false, false, false, false]);
        c.set_step(Step::Minutes, true);
        c.follow = Follow::Wallpaper;
        assert_eq!(hotlist_pills(&c), [false, true, true, false, false]);
    }

    /// The previews' art state: the right half playing "hearth" from the library; "fury" saved in slot 1.
    pub fn sample() -> (art_service::Snapshot, art_library::Library) {
        let blob = |name: &str| -> Vec<u8> {
            std::fs::read(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("resources/lighting/art")
                    .join(name),
            )
            .expect("bundled art")
        };
        let entries = [
            "constellations.bin",
            "fury.bin",
            "hearth.bin",
            "moon-garden.bin",
        ]
        .into_iter()
        .map(|name| {
            let bytes = blob(name);
            art_library::Entry {
                path: PathBuf::from("art").join(name),
                preview: super::super::art_preview::first_frame(&bytes)
                    .map(Arc::new)
                    .map_err(str::to_owned),
                bytes: bytes.len(),
                hash: art_library::blob_hash(&bytes),
            }
        })
        .collect();
        let status = digitalis::protocol::ArtStatus {
            link: 2,
            right_state: digitalis::protocol::right_state::PLAYING,
            right_set: 2,
            right_set_count: 6,
            flash_slots: 8,
            flash_used: 1,
            ..Default::default()
        };
        let mut cycle = Cycle::default();
        cycle.set_step(Step::LightsWake, true);
        cycle.items = vec![
            Item::File(PathBuf::from("art/fury.bin")),
            Item::File(PathBuf::from("art/hearth.bin")),
        ];
        cycle.position = 1;
        let snapshot = art_service::Snapshot {
            connected: true,
            status: Some(status),
            pinned: Some(art_service::Choice::File(PathBuf::from("art/hearth.bin"))),
            slots: vec![digitalis::protocol::SlotInfo {
                slot: 0,
                used: true,
                writes: 1,
                len: 3599,
                name: "fury".into(),
            }],
            cycle,
            wake_events: true,
            ..Default::default()
        };
        (
            snapshot,
            art_library::Library {
                entries,
                ..Default::default()
            },
        )
    }
}
