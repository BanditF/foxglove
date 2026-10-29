//! The right screen's art on the Screens page (#516, epic #515), in three tiers:
//! - **Showing now:** the right screen's piece, its badge, what happens after a restart,
//!   Previous / Next, and Rotation beside it (the left screen's column is `screens_ui`'s);
//! - **On the keyboard:** the kept places with the "Kept: n of 8 places" meter, and the pieces
//!   that come with the keyboard, with their pictures (`art_builtins`);
//! - **On this PC:** every piece once with one badge (`art_pieces`), Add art… and the filters.
//!
//! Clicks become [`Act`]s the art service carries out, so the previews draw the page with no
//! keyboard. One sending strip (`art_words::strip`) shows what runs and how it ended, on the
//! page and in the tray. Keep on keyboard is one click; a full keyboard asks what to replace
//! (#517).
use super::art_cycle::{Cycle, Follow, Item, Step};
use super::art_pieces::{self, Filter, Kept, PcPiece};
use super::art_service::{self, Choice, Retry, Snapshot};
use super::art_words::{self, Badge, Showing, Tone};
use super::{art_library, theme};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use egui::{Color32, CornerRadius, RichText, Sense, Stroke};

/// What Add art… is picking.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Pick {
    #[default]
    File,
    Folder,
}

/// "The keyboard is full": the piece to keep and the place picked to replace.
#[derive(Clone, Debug, PartialEq)]
struct Replace {
    file: PathBuf,
    name: String,
    slot: Option<u8>,
}

#[derive(Default)]
struct UiState {
    dialog: egui_file_dialog::FileDialog,
    picking: Pick,
    filter: Filter,
    search: String,
    replace: Option<Replace>,
    /// The kept place whose Remove waits for the answer.
    remove: Option<u8>,
    /// A copy or add that failed, in words.
    message: Option<String>,
}
thread_local! { static UI: std::cell::RefCell<UiState> = std::cell::RefCell::new(UiState::default()); }

/// Runs every frame of the settings window: Add art…'s file dialog.
pub fn update(ctx: &egui::Context) {
    art_service::set_wake(crate::ui_wake::UiWake::from_ctx(ctx));
    UI.with_borrow_mut(|state| {
        if art_library::PICK_FILE.swap(false, Ordering::Relaxed) {
            state.picking = Pick::File;
            state.dialog.pick_file();
        }
        state.dialog.update(ctx);
        let Some(path) = state.dialog.take_picked() else {
            return;
        };
        match state.picking {
            Pick::Folder => {
                super::art_sources::add_source(path);
                art_library::refresh();
            }
            // Add art…: into the library, then shown (#520).
            Pick::File => match art_pieces::add_file(&path) {
                Ok(file) => {
                    state.message = None;
                    if let Some(h) = art_service::handle() {
                        h.push(file);
                    }
                }
                Err(e) => state.message = Some(add_failed(&e, &path)),
            },
        }
    });
}

fn add_failed(error: &str, path: &std::path::Path) -> String {
    let name = art_service::file_name(path);
    match art_words::problem(error, None) {
        art_words::Problem::NotArt => format!(
            "{name} isn't art for {}.",
            super::screen_roles::the(super::screen_roles::Role::Art)
        ),
        _ => format!("Couldn't add {name} to this PC."),
    }
}

/// What a click asks the art service (or the library) to do.
#[derive(Clone, Debug, PartialEq)]
enum Act {
    Show(Choice),
    /// Show a PC piece (index into the PC tier): sent until the keyboard restarts.
    ShowPiece(usize),
    /// Keep a PC piece on the keyboard: a free place, or the replace choice when full.
    KeepPiece(usize),
    /// Keep the piece sent from this file, now on screen.
    KeepFile(PathBuf),
    /// Keep `file` in place `slot` (the replace choice's answer).
    KeepIn(PathBuf, u8),
    Push(PathBuf),
    Remove(u8),
    Delete(PathBuf),
    Refresh,
    Cycle(Cycle),
    Next,
    Previous,
    Cancel,
    Again(Retry),
    AddFile,
    AddFolder,
    RemoveSource(PathBuf),
    StandIn(String, Option<u8>),
    /// Hide the firmware-change line.
    Dismiss,
    /// Which way round a piece's ink is (#666), by its file or folder.
    Polarity(PathBuf, super::art_encode::Polarity),
}

#[cfg(test)]
thread_local! {
    /// The previews' art state and library, so they never ask a real keyboard.
    pub static PREVIEW: std::cell::RefCell<Option<(Snapshot, art_library::Library)>> =
        const { std::cell::RefCell::new(None) };
}

/// Opens (or closes) the full-keyboard choice for the previews.
#[cfg(test)]
pub fn preview_replace(file: Option<(&str, Option<u8>)>) {
    UI.with_borrow_mut(|u| {
        u.replace = file.map(|(name, slot)| Replace {
            file: PathBuf::from(format!("art/{name}.bin")),
            name: name.into(),
            slot,
        })
    });
}

/// The art as the page draws it this frame.
pub struct Art {
    pub s: Snapshot,
    pub library: art_library::Library,
    pub pcs: Vec<PcPiece>,
    pub kept: Vec<Option<Kept>>,
    pub unreadable: Vec<String>,
}

impl Art {
    pub fn new(s: Snapshot, library: art_library::Library) -> Art {
        let (found, unreadable) = art_pieces::found(&library);
        let looks = art_pieces::wallpaper_files();
        let is_wallpaper = |file: &str| looks.contains(&file.to_lowercase());
        let pcs = art_pieces::pc_pieces(found, &is_wallpaper);
        let kept = art_pieces::kept(&s, &pcs);
        Art {
            s,
            library,
            pcs,
            kept,
            unreadable,
        }
    }

    /// What the right screen shows: its name and its picture, when Foxglove has one.
    pub fn showing(&self) -> (String, Option<(u64, Arc<egui::ColorImage>)>) {
        (
            art_pieces::showing_name(&self.s),
            art_pieces::showing_picture(&self.s, &self.pcs),
        )
    }
}

/// This frame's art, or `None` before the art service starts. Sorted once per frame and
/// shared by the page's parts (kept in the context, so each preview's context has its own).
pub fn current(ctx: &egui::Context) -> Option<Arc<Art>> {
    let id = egui::Id::new("foxglove-art-this-frame");
    let frame = ctx.cumulative_frame_nr();
    if let Some((_, art)) = ctx
        .data(|d| d.get_temp::<(u64, Arc<Art>)>(id))
        .filter(|(n, _)| *n == frame)
    {
        return Some(art);
    }
    let art = Arc::new(build()?);
    ctx.data_mut(|d| d.insert_temp(id, (frame, art.clone())));
    Some(art)
}

fn build() -> Option<Art> {
    #[cfg(test)]
    if let Some((s, library)) = PREVIEW.with_borrow(|p| p.clone()) {
        return Some(Art::new(s, library));
    }
    let h = art_service::handle()?;
    Some(Art::new(h.snapshot(), art_library::snapshot()))
}

/// Carries out the page's clicks.
fn act(art: &Art, acts: Vec<Act>) {
    #[cfg(test)]
    if PREVIEW.with_borrow(|p| p.is_some()) {
        return;
    }
    let Some(h) = art_service::handle() else {
        return;
    };
    let keep = |file: PathBuf, name: String| match art_pieces::first_free(&art.s) {
        Some(slot) => {
            h.keep(file, slot);
        }
        None => UI.with_borrow_mut(|u| {
            u.replace = Some(Replace {
                file,
                name,
                slot: None,
            })
        }),
    };
    let to_library = |i: usize| {
        let piece = art.pcs.get(i)?;
        match art_pieces::to_library(piece, &art.library) {
            Ok(p) => Some((p, piece.name.clone())),
            Err(e) => {
                UI.with_borrow_mut(|u| {
                    u.message = Some(format!("Couldn't copy {} to this PC.", piece.name))
                });
                eprintln!("Foxglove: art: {e}");
                None
            }
        }
    };
    for a in acts {
        match a {
            Act::Show(c) => {
                let _ = match c {
                    Choice::BuiltIn(n) => h.select(n),
                    Choice::Slot(n) => h.play_slot(n),
                    Choice::File(p) => h.push(p),
                };
            }
            Act::ShowPiece(i) => {
                if let Some((p, _)) = to_library(i) {
                    h.push(p);
                }
            }
            Act::KeepPiece(i) => {
                if let Some((p, name)) = to_library(i) {
                    keep(p, name);
                }
            }
            Act::KeepFile(p) => keep(p.clone(), art_service::file_name(&p)),
            Act::KeepIn(p, slot) => {
                h.keep(p, slot);
            }
            Act::Push(p) => {
                h.push(p);
            }
            Act::Remove(n) => {
                h.erase_slot(n);
            }
            Act::Delete(p) => art_library::delete(p),
            Act::Refresh => {
                h.refresh();
                // Wallpaper Engine may have been installed, moved or started (#446): looked
                // for on the wallpaper service's thread, not this one.
                super::wallpaper::refresh();
                art_library::refresh();
            }
            Act::Cycle(c) => {
                if let Err(e) = h.set_cycle(c) {
                    eprintln!("Foxglove: can't save Rotation: {e}");
                }
            }
            Act::Next => {
                h.next();
            }
            Act::Previous => {
                h.previous();
            }
            Act::Cancel => h.cancel(),
            Act::Again(r) => {
                h.again(r);
            }
            Act::AddFile => art_library::PICK_FILE.store(true, Ordering::Relaxed),
            Act::AddFolder => UI.with_borrow_mut(|u| {
                u.picking = Pick::Folder;
                u.dialog.pick_directory();
            }),
            Act::RemoveSource(f) => {
                super::art_sources::remove_source(&f);
                art_library::refresh();
            }
            Act::StandIn(builtin, slot) => super::art_sources::set_stand_in(&builtin, slot),
            Act::Dismiss => h.dismiss_notice(),
            Act::Polarity(p, polarity) => {
                super::art_polarity::set(&p, polarity);
            }
        }
    }
}

/// The textures of the art pictures, kept in the context they were made for (a new context,
/// as a preview makes, starts empty) and keyed by the picture's content, so a file replaced
/// under the same name gets its new picture.
pub fn texture(
    ctx: &egui::Context,
    key: u64,
    half: bool,
    stored: &egui::ColorImage,
) -> egui::TextureHandle {
    type Cache = HashMap<(u64, bool), egui::TextureHandle>;
    let id = egui::Id::new("foxglove-art-textures");
    if let Some(t) = ctx.data(|d| {
        d.get_temp::<Cache>(id)
            .and_then(|c| c.get(&(key, half)).cloned())
    }) {
        return t;
    }
    let t = ctx.load_texture(
        format!("art-{key:x}-{half}"),
        thumbnail(stored, half),
        egui::TextureOptions::NEAREST,
    );
    ctx.data_mut(|d| {
        let cache = d.get_temp_mut_or_default::<Cache>(id);
        // Old pictures (files edited or removed) go now and then; the page remakes what it shows.
        if cache.len() >= 256 {
            cache.clear();
        }
        cache.insert((key, half), t.clone())
    });
    t
}

/// The right screen's tones: lit pixels light grey, the rest near black.
const LIT: Color32 = Color32::from_rgb(0xe8, 0xea, 0xe9);
const OFF: Color32 = Color32::from_rgb(0x0b, 0x0d, 0x0c);

/// A stored frame turned upright as the connected keyboard's art screen is mounted (#560), in
/// the screen's tones, at full size or halved. See [`thumbnail_turned`].
pub fn thumbnail(stored: &egui::ColorImage, half: bool) -> egui::ColorImage {
    thumbnail_turned(
        stored,
        half,
        super::art_encode::ArtTarget::current().rotation,
    )
}

/// A stored frame turned upright from `rotation` quarter turns clockwise, in the screen's
/// tones, at full size or halved. The stored background is a set bit (white in `art_preview`)
/// and the ink a clear one; the screen lights the ink, so ink becomes [`LIT`] and the
/// background [`OFF`] (not big white blocks). Halving averages 2×2 and dithers back to 1 bit
/// (ordered 2×2), so a thumbnail stays a 1-bit picture like the device's, not a grey blur.
pub fn thumbnail_turned(stored: &egui::ColorImage, half: bool, rotation: u8) -> egui::ColorImage {
    let [sw, sh] = stored.size;
    let turned = rotation % 2 == 1;
    let lit = |x: usize, y: usize| -> f32 {
        // Upright (x, y) in the stored frame, by how it is mounted.
        let (sx, sy) = match rotation % 4 {
            1 => (sw - 1 - y, x),
            2 => (sw - 1 - x, sh - 1 - y),
            3 => (y, sh - 1 - x),
            _ => (x, y),
        };
        let p = stored.pixels[sy * sw + sx];
        1.0 - f32::from(p.r().max(p.g()).max(p.b())) / 255.0
    };
    let (w, h) = if turned { (sh, sw) } else { (sw, sh) };
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

/// What the right screen shows now, for the keyboard page and the tray: its name and picture.
pub fn showing_now(
    s: &Snapshot,
    library: &art_library::Library,
) -> (String, Option<Arc<egui::ColorImage>>) {
    if !s.connected {
        return ("…".into(), None);
    }
    let (name, picture) = Art::new(s.clone(), library.clone()).showing();
    (name, picture.map(|(_, image)| image))
}

fn tone_color(ui: &egui::Ui, tone: Tone) -> Color32 {
    let p = theme::palette(ui);
    match tone {
        Tone::Busy => p.accent,
        Tone::Ok => p.ok,
        Tone::Warn => p.warn,
        Tone::Error => p.danger,
        Tone::Plain => p.muted,
    }
}

/// A badge: its words in the mono face inside a thin rounded line.
pub fn badge_ui(ui: &mut egui::Ui, badge: Badge) -> egui::Response {
    let color = tone_color(ui, badge.tone());
    let galley = ui
        .painter()
        .layout_no_wrap(badge.text(), theme::mono(10.5), color);
    let (rect, r) = ui.allocate_exact_size(galley.size() + egui::vec2(10.0, 4.0), Sense::hover());
    ui.painter().rect_stroke(
        rect,
        CornerRadius::same(3),
        Stroke::new(1.0, color),
        egui::StrokeKind::Inside,
    );
    ui.painter()
        .galley(rect.min + egui::vec2(5.0, 2.0), galley, color);
    r
}

/// The sending strip (#518), on the page and in the tray: a coloured edge, the sentence, a
/// smaller line, a bar while sending, and one button. Returns the button when clicked.
pub fn strip_ui(ui: &mut egui::Ui, strip: &art_words::Strip) -> Option<art_words::Button> {
    let p = theme::palette(ui);
    let edge = tone_color(ui, strip.tone);
    let mut clicked = None;
    let r = egui::Frame::new()
        .fill(p.well)
        .stroke(Stroke::new(1.0, p.line))
        .corner_radius(CornerRadius::same(6))
        .inner_margin(egui::Margin {
            left: 14,
            right: 10,
            top: 8,
            bottom: 8,
        })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.set_max_width((ui.available_width() - 96.0).max(120.0));
                    ui.label(RichText::new(&strip.text).color(p.text_strong));
                    if let Some(d) = &strip.detail {
                        ui.label(RichText::new(d).small().color(p.muted));
                    }
                    if let Some(f) = strip.progress {
                        ui.horizontal(|ui| {
                            ui.add(
                                egui::ProgressBar::new(f)
                                    .desired_width(200.0)
                                    .fill(p.accent),
                            );
                            ui.label(
                                RichText::new(format!("{}%", (f * 100.0) as u32))
                                    .small()
                                    .color(p.muted),
                            );
                        });
                    }
                });
                if let Some(button) = strip.button {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let text = match button {
                            art_words::Button::Cancel => "Cancel",
                            art_words::Button::TryAgain => "Try again",
                        };
                        if ui.button(text).clicked() {
                            clicked = Some(button);
                        }
                    });
                }
            });
        })
        .response;
    let bar = egui::Rect::from_min_size(r.rect.min, egui::vec2(4.0, r.rect.height()));
    ui.painter().rect_filled(
        bar,
        CornerRadius {
            nw: 6,
            sw: 6,
            ne: 0,
            se: 0,
        },
        edge,
    );
    clicked
}

/// The strip for this snapshot, with its click turned into an act. A success line fades: one
/// repaint is asked for when it is due to go, not a timer.
fn strip(ui: &mut egui::Ui, s: &Snapshot, acts: &mut Vec<Act>) {
    let Some(strip) = art_words::strip(s) else {
        return;
    };
    if let Some(o) = s.outcome.as_ref().filter(|o| o.result.is_ok() && !s.busy) {
        ui.ctx()
            .request_repaint_after(art_words::DONE_FOR.saturating_sub(o.at.elapsed()));
    }
    ui.add_space(6.0);
    match strip_ui(ui, &strip) {
        Some(art_words::Button::Cancel) => acts.push(Act::Cancel),
        Some(art_words::Button::TryAgain) => {
            if let Some(r) = s.outcome.as_ref().and_then(|o| o.job.again.clone()) {
                acts.push(Act::Again(r));
            }
        }
        None => {}
    }
}

/// A picture tile: the art (or a dark box with words), its name under it, and an orange ring
/// when it is on the right screen.
fn tile(
    ui: &mut egui::Ui,
    picture: Option<&egui::TextureHandle>,
    name: &str,
    on: bool,
    empty: Option<&str>,
) -> egui::Response {
    let p = theme::palette(ui);
    let pic = egui::vec2(34.0, 70.0);
    let size = egui::vec2(pic.x + 50.0, pic.y + 26.0);
    let sense = if empty.is_some() {
        Sense::hover()
    } else {
        Sense::click()
    };
    let (rect, r) = ui.allocate_exact_size(size, sense);
    let painter = ui.painter();
    let frame = egui::Rect::from_center_size(
        egui::pos2(rect.center().x, rect.top() + 4.0 + pic.y / 2.0),
        pic + egui::vec2(6.0, 6.0),
    );
    let line = if on {
        p.accent
    } else if r.hovered() && empty.is_none() {
        p.control_line.gamma_multiply(1.6)
    } else {
        p.control_line
    };
    painter.rect(
        frame,
        CornerRadius::same(4),
        if empty.is_some() { p.surface } else { OFF },
        Stroke::new(if on { 2.0 } else { 1.0 }, line),
        egui::StrokeKind::Inside,
    );
    let inner = egui::Rect::from_center_size(frame.center(), pic);
    match (picture, empty) {
        (Some(t), _) => {
            painter.image(
                t.id(),
                inner,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                Color32::WHITE,
            );
        }
        (None, words) => {
            painter.text(
                inner.center(),
                egui::Align2::CENTER_CENTER,
                words.unwrap_or("ART"),
                theme::body(11.0),
                p.muted,
            );
        }
    }
    if empty.is_none() {
        let shown: String = if name.chars().count() > 12 {
            name.chars().take(11).chain(['…']).collect()
        } else {
            name.to_string()
        };
        painter.text(
            egui::pos2(rect.center().x, rect.bottom() - 2.0),
            egui::Align2::CENTER_BOTTOM,
            shown,
            theme::body(11.5),
            p.text,
        );
    }
    r
}

/// The right screen's column of Showing now: the picture, its name and badge, the restart
/// line, Previous / Next, and Keep on keyboard for a piece that isn't kept.
pub fn now_right(ui: &mut egui::Ui) {
    let Some(art) = current(ui.ctx()) else {
        return;
    };
    let mut acts = Vec::new();
    draw_now_right(ui, &art, &mut acts);
    act(&art, acts);
}

fn draw_now_right(ui: &mut egui::Ui, art: &Art, acts: &mut Vec<Act>) {
    let p = theme::palette(ui);
    let s = &art.s;
    ui.label(theme::caps_label(
        ui,
        &super::screen_roles::name(super::screen_roles::Role::Art),
    ));
    if !s.connected {
        ui.weak("Waiting for the keyboard.");
        return;
    }
    let (name, picture) = art.showing();
    ui.horizontal_top(|ui| {
        // The art screen's shape upright, 140 points tall (#560).
        let t = super::art_encode::ArtTarget::current();
        let (w, h) = if t.rotation % 2 == 1 {
            (t.height, t.width)
        } else {
            (t.width, t.height)
        };
        let size = egui::vec2((140.0 * w as f32 / h.max(1) as f32).min(240.0), 140.0);
        match &picture {
            Some((key, image)) => {
                let t = texture(ui.ctx(), *key, false, image);
                ui.add(egui::Image::new((t.id(), size)).corner_radius(3));
            }
            None => {
                let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
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
                    theme::mono(11.0),
                    p.muted,
                );
            }
        }
        ui.vertical(|ui| {
            ui.label(
                RichText::new(&name)
                    .font(theme::hud(17.0))
                    .color(p.text_strong),
            );
            let Some(st) = s.status else {
                return;
            };
            let now = art_words::showing(&st);
            if let Some(b) = art_words::showing_badge(now) {
                badge_ui(ui, b);
            }
            let remembered = s.pin_now().as_ref().is_some_and(|pin| match (pin, now) {
                (Choice::BuiltIn(n), Showing::BuiltIn(m)) => *n == m,
                (Choice::Slot(n), Showing::Kept(m)) => *n == m,
                (Choice::File(_), Showing::Sent) => s.showing_file().is_some(),
                _ => false,
            });
            match art_words::after_restart(now, remembered) {
                Some(line) => ui.weak(line),
                None => ui.weak(art_words::right_state_words(st.right_state)),
            };
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                ui.add_enabled_ui(!s.busy, |ui| {
                    if ui.button("‹ Previous").clicked() {
                        acts.push(Act::Previous);
                    }
                    if ui.button("Next ›").clicked() {
                        acts.push(Act::Next);
                    }
                    if let (Showing::Sent, Some(file)) = (now, s.showing_file()) {
                        if st.flash_slots > 0
                            && ui
                                .add(theme::primary_button(ui, "Keep on keyboard"))
                                .clicked()
                        {
                            acts.push(Act::KeepFile(file.clone()));
                        }
                    }
                });
            });
        });
    });
    strip(ui, s, acts);
}

/// Which Rotation pills are lit, from its settings: Off, Every n min, Lights wake, Match the
/// wallpaper.
pub fn rotation_pills(c: &Cycle) -> [bool; 4] {
    let moves =
        c.steps_on(Step::Minutes) || c.steps_on(Step::LightsWake) || c.follow == Follow::Wallpaper;
    [
        !moves,
        c.steps_on(Step::Minutes),
        c.steps_on(Step::LightsWake),
        c.follow == Follow::Wallpaper,
    ]
}

/// Rotation's column of Showing now: what changes the right screen, and when. It steps through
/// what the keyboard holds (`art_cycle::rotation`), so it has no list of its own.
pub fn rotation(ui: &mut egui::Ui) {
    let Some(art) = current(ui.ctx()) else {
        return;
    };
    if !art.s.connected {
        return;
    }
    let mut acts = Vec::new();
    draw_rotation(ui, &art, &mut acts);
    act(&art, acts);
}

fn draw_rotation(ui: &mut egui::Ui, art: &Art, acts: &mut Vec<Act>) {
    let s = &art.s;
    let Some(st) = s.status else {
        return;
    };
    ui.label(theme::caps_label(ui, "Rotation"));
    ui.label(format!(
        "Changes {}",
        super::screen_roles::the(super::screen_roles::Role::Art)
    ));
    let mut cycle = s.cycle.clone();
    let mut changed = false;
    let lit = rotation_pills(&cycle);
    ui.horizontal_wrapped(|ui| {
        // A pill that doesn't fit moves to the next row rather than wrapping its words.
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
        if theme::pill(ui, lit[0], "Off").clicked() && !lit[0] {
            for step in [Step::Minutes, Step::LightsWake] {
                cycle.set_step(step, false);
            }
            cycle.follow = Follow::Nothing;
            changed = true;
        }
        if theme::pill(ui, lit[1], &format!("Every {} min", cycle.minutes)).clicked() {
            cycle.set_step(Step::Minutes, !lit[1]);
            changed = true;
        }
        let wake = ui.add_enabled(s.wake_events, theme::pill_button(ui, lit[2], "Lights wake"));
        if wake
            .on_hover_text("Steps when the keyboard's lights come back on.")
            .on_disabled_hover_text("Needs keyboard firmware that reports when the lights come back on.")
            .clicked()
        {
            cycle.set_step(Step::LightsWake, !lit[2]);
            changed = true;
        }
        let engine = super::wallpaper_engine::presence();
        let hover = if engine.installed() {
            "Shows the art that goes with the Wallpaper Engine wallpaper on screen (from the art folder's wallpapers.json)."
        } else {
            "Shows the art that goes with your Windows desktop wallpaper, when its file is named after a wallpaper in the art folder's wallpapers.json. Works best with Wallpaper Engine."
        };
        if theme::pill(ui, lit[3], "Wallpaper").on_hover_text(hover).clicked() {
            cycle.follow = if lit[3] { Follow::Nothing } else { Follow::Wallpaper };
            changed = true;
        }
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
    if cycle.follow == Follow::Wallpaper {
        let monitors = super::wallpaper::monitors();
        if !monitors.is_empty() {
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
        if let Some(hint) = super::wallpaper_engine::presence().hint() {
            ui.weak(hint);
        }
    }
    let pieces = super::art_cycle::rotation(&st).len();
    ui.weak(format!(
        "Steps through the keyboard's {pieces} pieces. Its art keys step through the same ones."
    ));
    layer_art(ui, art, &mut cycle, &mut changed);
    if changed {
        acts.push(Act::Cycle(cycle));
    }
}

/// Art for a layer while it's active, over everything else (art_triggers).
fn layer_art(ui: &mut egui::Ui, art: &Art, cycle: &mut Cycle, changed: &mut bool) {
    let s = &art.s;
    let count = s.status.map_or(0, |st| st.right_set_count);
    let names = super::art_manifest::sets(count);
    let label = |item: &Item| match item {
        Item::BuiltIn(name) => names
            .and_then(|sets| sets.iter().find(|set| &set.name == name))
            .map_or_else(|| name.clone(), |set| set.label.clone()),
        Item::Slot(n) => art_service::name_of(&Choice::Slot(*n), s),
        Item::File(p) => art_service::file_name(p),
    };
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
                            if ui.selectable_label(false, &info.name).clicked() {
                                picked = Some(Some(Item::Slot(info.slot)));
                            }
                        }
                    });
                if let Some(a) = picked {
                    cycle.set_layer_art(layer, a);
                    *changed = true;
                }
                ui.end_row();
            }
        });
        if layer_names.len() <= 1 {
            ui.weak("Layers appear once the keymap has been read.");
        }
    });
}

/// The meter: "Kept: n of m places" and a box per place, lit when used.
fn meter(ui: &mut egui::Ui, used: usize, total: usize) {
    let p = theme::palette(ui);
    ui.horizontal(|ui| {
        ui.label(RichText::new(format!("Kept: {used} of {total} places")).color(p.text_strong));
        let (rect, _) =
            ui.allocate_exact_size(egui::vec2(total as f32 * 26.0, 12.0), Sense::hover());
        for i in 0..total {
            let r = egui::Rect::from_min_size(
                rect.min + egui::vec2(i as f32 * 26.0, 0.0),
                egui::vec2(22.0, 12.0),
            );
            let (fill, line) = if i < used {
                (p.accent, p.accent)
            } else {
                (p.surface, p.control_line)
            };
            ui.painter().rect(
                r,
                CornerRadius::same(2),
                fill,
                Stroke::new(1.0, line),
                egui::StrokeKind::Inside,
            );
        }
    });
}

/// "On the keyboard": the kept places and the pieces that come with the keyboard, as the
/// keyboard reports them (read on connect).
pub fn keyboard_card(ui: &mut egui::Ui) {
    let Some(art) = current(ui.ctx()) else {
        return;
    };
    if !art.s.connected {
        return;
    }
    let mut acts = Vec::new();
    super::settings_ui::section(ui, "On the keyboard", |ui| {
        UI.with_borrow_mut(|state| draw_keyboard(ui, &art, state, &mut acts));
    });
    act(&art, acts);
}

fn draw_keyboard(ui: &mut egui::Ui, art: &Art, state: &mut UiState, acts: &mut Vec<Act>) {
    let s = &art.s;
    let Some(st) = s.status else {
        ui.weak("Reading the keyboard…");
        return;
    };
    if let Some(notice) = &s.notice {
        ui.horizontal_wrapped(|ui| {
            ui.colored_label(theme::palette(ui).warn, notice);
            if ui.small_button("OK").clicked() {
                acts.push(Act::Dismiss);
            }
        });
        ui.add_space(4.0);
    }
    if let Some((used, total)) = art_pieces::meter(s) {
        meter(ui, used, total);
    }
    ui.weak("Plays without this PC.");
    let now = art_words::showing(&st);
    if !art.kept.is_empty() {
        ui.add_space(6.0);
        ui.label(theme::caps_label(ui, "Kept on the keyboard (yours)"));
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(6.0, 6.0);
            for (n, place) in art.kept.iter().enumerate() {
                let Some(k) = place else {
                    tile(ui, None, "", false, Some("free"));
                    continue;
                };
                let piece = k.piece.and_then(|i| art.pcs.get(i));
                let t = piece.map(|p| texture(ui.ctx(), p.key, true, &p.image));
                let on = now == Showing::Kept(n as u8);
                let r = tile(ui, t.as_ref(), &k.name, on, None);
                if r.clicked() && !on && !s.busy {
                    acts.push(Act::Show(Choice::Slot(k.slot)));
                }
                let hover = if piece.is_some() {
                    "Click to show it. Right-click for more."
                } else {
                    "Click to show it. This PC has no copy of it. Right-click for more."
                };
                r.on_hover_text(hover).context_menu(|ui| {
                    if ui.button("Show now").clicked() {
                        acts.push(Act::Show(Choice::Slot(k.slot)));
                        ui.close();
                    }
                    if ui.button("Remove from keyboard…").clicked() {
                        state.remove = Some(k.slot);
                        ui.close();
                    }
                    stand_in_menu(ui, s, k.slot, acts);
                });
            }
        });
    }
    let built_ins = s.built_ins();
    if !built_ins.is_empty() {
        ui.add_space(6.0);
        ui.label(theme::caps_label(ui, "Comes with the keyboard"));
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(6.0, 6.0);
            for (n, name) in &built_ins {
                let picture = super::art_builtins::picture(*n, st.right_set_count);
                let t = picture
                    .as_ref()
                    .map(|p| texture(ui.ctx(), p.key, true, &p.image));
                let on = now == Showing::BuiltIn(*n);
                let r = tile(ui, t.as_ref(), name, on, None);
                if r.clicked() && !on && !s.busy {
                    acts.push(Act::Show(Choice::BuiltIn(*n)));
                }
                r.on_hover_text("Click to show it");
            }
        });
        if super::art_manifest::sets(st.right_set_count).is_none() {
            ui.weak("This keyboard runs firmware Foxglove doesn't know, so its own pieces have no names or pictures here.");
        }
    }
    ui.add_space(4.0);
    ui.weak("Click a piece to show it. Right-click a kept one to remove it.");
    remove_dialog(ui, art, state, acts);
    replace_dialog(ui, art, state, acts);
}

/// "Use instead of a built-in": a kept place shown wherever that built-in is chosen (a pin, a
/// layer, a preset), app-side (`art_sources` stand-ins).
fn stand_in_menu(ui: &mut egui::Ui, s: &Snapshot, slot: u8, acts: &mut Vec<Act>) {
    let names = super::art_manifest::sets(s.status.map_or(0, |st| st.right_set_count));
    let Some(sets) = names else {
        return;
    };
    ui.menu_button("Use instead of…", |ui| {
        for set in sets {
            let current = super::art_sources::stand_in(&set.name) == Some(slot);
            if ui.selectable_label(current, &set.label).clicked() {
                acts.push(Act::StandIn(set.name.clone(), (!current).then_some(slot)));
                ui.close();
            }
        }
    });
}

/// Remove from keyboard asks first, and says plainly when the keyboard holds the only copy
/// (a kept place can't be read back to this PC).
fn remove_dialog(ui: &mut egui::Ui, art: &Art, state: &mut UiState, acts: &mut Vec<Act>) {
    let Some(slot) = state.remove else {
        return;
    };
    let Some(k) = art.kept.iter().flatten().find(|k| k.slot == slot) else {
        state.remove = None;
        return;
    };
    let mut open = true;
    egui::Window::new(format!("Remove {} from the keyboard?", k.name))
        .id(egui::Id::new("art-remove"))
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .open(&mut open)
        .show(ui.ctx(), |ui| {
            if k.piece.is_some() {
                ui.label("This PC has a copy (same name and size); you can keep it again later.");
            } else {
                ui.colored_label(
                    theme::palette(ui).warn,
                    "This PC has no copy of it, and the keyboard can't send it back: removing it loses it.",
                );
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.add(theme::primary_button(ui, "Remove")).clicked() {
                    acts.push(Act::Remove(slot));
                    state.remove = None;
                }
                if ui.button("Cancel").clicked() {
                    state.remove = None;
                }
            });
        });
    if !open {
        state.remove = None;
    }
}

/// What a kept place is used for, in a line for the replace choice.
fn used_for(s: &Snapshot, slot: u8) -> Option<String> {
    let mut uses = Vec::new();
    if s.pinned == Some(Choice::Slot(slot)) {
        uses.push("shown after a restart".to_string());
    }
    for l in s.cycle.layers.iter().filter(|l| l.art == Item::Slot(slot)) {
        let name = super::layer_names::remembered()
            .get(usize::from(l.layer))
            .cloned()
            .flatten()
            .unwrap_or_else(|| format!("layer {}", l.layer));
        uses.push(format!("the art for {name}"));
    }
    (!uses.is_empty()).then(|| format!("Used as {}.", uses.join(" and ")))
}

/// "The keyboard is full" (#517): pick a place to replace, or just show the piece for now.
/// Nothing is overwritten without this.
fn replace_dialog(ui: &mut egui::Ui, art: &Art, state: &mut UiState, acts: &mut Vec<Act>) {
    let Some(mut replace) = state.replace.clone() else {
        return;
    };
    let s = &art.s;
    let p = theme::palette(ui);
    let mut close = false;
    let mut open = true;
    egui::Window::new("The keyboard is full")
        .id(egui::Id::new("art-replace"))
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .open(&mut open)
        .show(ui.ctx(), |ui| {
            ui.set_min_width(690.0);
            let total = art.kept.len();
            ui.label(format!(
                "It keeps {total} pieces. Pick one to replace with {}.",
                replace.name
            ));
            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                for k in art.kept.iter().flatten() {
                    let piece = k.piece.and_then(|i| art.pcs.get(i));
                    let t = piece.map(|pc| texture(ui.ctx(), pc.key, true, &pc.image));
                    if tile(ui, t.as_ref(), &k.name, replace.slot == Some(k.slot), None).clicked() {
                        replace.slot = Some(k.slot);
                    }
                }
            });
            let picked = replace
                .slot
                .and_then(|n| art.kept.iter().flatten().find(|k| k.slot == n));
            if let Some(k) = picked {
                if k.piece.is_some() {
                    ui.weak(format!(
                        "This PC has a copy of {} (same name and size); you can keep it again later.",
                        k.name
                    ));
                } else {
                    ui.colored_label(
                        p.warn,
                        format!("This PC has no copy of {}: replacing it loses it.", k.name),
                    );
                }
                match used_for(s, k.slot) {
                    Some(line) => ui.colored_label(p.warn, line),
                    None => ui.colored_label(p.ok, "Not used by a layer or after a restart."),
                };
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if let Some(k) = picked {
                    if ui
                        .add(theme::primary_button(ui, &format!("Replace {}", k.name)))
                        .clicked()
                    {
                        acts.push(Act::KeepIn(replace.file.clone(), k.slot));
                        close = true;
                    }
                }
                if ui.button("Just show it for now").clicked() {
                    acts.push(Act::Push(replace.file.clone()));
                    close = true;
                }
                if ui.button("Cancel").clicked() {
                    close = true;
                }
            });
        });
    state.replace = (!close && open).then_some(replace);
    let _ = s;
}

/// "On this PC": every piece once with one badge, Add art…, the filters and search.
pub fn pc_card(ui: &mut egui::Ui) {
    let Some(art) = current(ui.ctx()) else {
        return;
    };
    let mut acts = Vec::new();
    super::settings_ui::section(ui, "On this PC", |ui| {
        UI.with_borrow_mut(|state| draw_pc(ui, &art, state, &mut acts));
    });
    act(&art, acts);
}

fn draw_pc(ui: &mut egui::Ui, art: &Art, state: &mut UiState, acts: &mut Vec<Act>) {
    let p = theme::palette(ui);
    let s = &art.s;
    let can_send = s.connected && !s.busy;
    let has_places = art_pieces::meter(s).is_some();
    ui.horizontal_wrapped(|ui| {
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
        for (f, text) in Filter::ALL {
            if theme::pill(ui, state.filter == f, text).clicked() {
                state.filter = f;
            }
        }
        ui.add(egui::TextEdit::singleline(&mut state.search).hint_text("Search").desired_width(140.0));
        {
            ui.menu_button("Add art…", |ui| {
                if ui.button("A file…").clicked() {
                    acts.push(Act::AddFile);
                    ui.close();
                }
                if ui
                    .button("A folder of frames…")
                    .on_hover_text("Folders of frame_N.png + meta.txt sets (like art-src/bandit/out), converted when you show or keep one.")
                    .clicked()
                {
                    acts.push(Act::AddFolder);
                    ui.close();
                }
            })
            .response
            .on_hover_text(format!("Adds art to this PC and shows it on {}.", super::screen_roles::the(super::screen_roles::Role::Art)));
            if ui.button("Refresh").clicked() {
                acts.push(Act::Refresh);
            }
        }
    });
    if let Some(m) = &state.message {
        ui.colored_label(p.warn, m);
    }
    ui.add_space(6.0);
    let shown: Vec<(usize, Badge)> = art
        .pcs
        .iter()
        .enumerate()
        .map(|(i, piece)| (i, art_pieces::badge(s, piece, &art.kept, i)))
        .filter(|(i, b)| state.filter.keeps(&art.pcs[*i], *b, &state.search))
        .collect();
    if art.pcs.is_empty() {
        ui.weak("No art on this PC yet. Add art… adds a .bin file and shows it.");
    } else if shown.is_empty() {
        ui.weak("Nothing matches.");
    }
    // Rows of cards: egui wraps single widgets, not framed groups, so the rows are counted.
    let gap = 8.0;
    let per_row = (((ui.available_width() + gap) / (PC_TILE + gap)).floor() as usize).max(1);
    for row in shown.chunks(per_row) {
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(gap, gap);
            for (i, badge) in row {
                let piece = &art.pcs[*i];
                let on = art_pieces::on_screen(s, piece, &art.kept, *i);
                pc_tile(ui, piece, *i, *badge, on, can_send, has_places, acts);
            }
        });
        ui.add_space(gap / 2.0);
    }
    if !art.unreadable.is_empty() {
        ui.weak(format!(
            "{} file(s) in the art folder couldn't be read.",
            art.unreadable.len()
        ))
        .on_hover_text(art.unreadable.join(", "));
    }
    if art.library.error.is_some() {
        ui.colored_label(p.warn, "Couldn't read the art folder on this PC.");
    }
    let sources = super::art_sources::load().sources;
    if !sources.is_empty() {
        egui::CollapsingHeader::new("Folders of frames")
            .id_salt("art-sources")
            .show(ui, |ui| {
                for folder in sources {
                    ui.horizontal(|ui| {
                        let n = art
                            .library
                            .sources
                            .iter()
                            .filter(|s| s.folder.starts_with(&folder))
                            .count();
                        ui.label(format!("{} ({n} pieces)", folder.display()));
                        if ui.small_button("Remove").clicked() {
                            acts.push(Act::RemoveSource(folder.clone()));
                        }
                    });
                }
            });
    }
}

/// A PC card's outer width: its content, margins and line.
const PC_TILE: f32 = 136.0;

/// A PC piece's card: the picture, its name, its one badge, and Keep on keyboard.
#[allow(clippy::too_many_arguments)]
fn pc_tile(
    ui: &mut egui::Ui,
    piece: &PcPiece,
    index: usize,
    badge: Badge,
    on: bool,
    can_send: bool,
    has_places: bool,
    acts: &mut Vec<Act>,
) {
    let p = theme::palette(ui);
    let line = if on { p.accent } else { p.line };
    let r = egui::Frame::new()
        .fill(p.surface_raised)
        .stroke(Stroke::new(if on { 2.0 } else { 1.0 }, line))
        .corner_radius(CornerRadius::same(6))
        .inner_margin(egui::Margin::symmetric(8, 8))
        .show(ui, |ui| {
            ui.set_width(PC_TILE - 18.0);
            ui.vertical_centered(|ui| {
                let t = texture(ui.ctx(), piece.key, true, &piece.image);
                ui.add(egui::Image::new((t.id(), egui::vec2(34.0, 70.0))));
                ui.label(RichText::new(&piece.name).color(p.text_strong));
                badge_ui(ui, badge);
                match badge {
                    Badge::Sending(n) => {
                        ui.add(
                            egui::ProgressBar::new(f32::from(n) / 100.0)
                                .desired_width(90.0)
                                .fill(p.accent),
                        );
                    }
                    Badge::TooBig => {
                        ui.label(RichText::new("over 16 KiB").small().color(p.danger));
                    }
                    Badge::ThisPc
                        if has_places
                            && ui
                                .add_enabled(can_send, egui::Button::new("Keep on keyboard"))
                                .clicked() =>
                    {
                        acts.push(Act::KeepPiece(index));
                    }
                    _ => {}
                }
            });
        })
        .response;
    let r = ui.interact(r.rect, ui.id().with(("art-pc", piece.key)), Sense::click());
    let showable = can_send && !matches!(badge, Badge::TooBig | Badge::Sending(_));
    if r.clicked() && showable && !on {
        acts.push(Act::ShowPiece(index));
    }
    let art = super::screen_roles::the(super::screen_roles::Role::Art);
    r.on_hover_text(if on {
        format!("On {art} now")
    } else {
        format!("Click to show it on {art} until the keyboard restarts")
    })
    .context_menu(|ui| {
        if ui
            .add_enabled(showable, egui::Button::new("Show now"))
            .clicked()
        {
            acts.push(Act::ShowPiece(index));
            ui.close();
        }
        if has_places
            && badge == Badge::ThisPc
            && ui
                .add_enabled(can_send, egui::Button::new("Keep on keyboard"))
                .clicked()
        {
            acts.push(Act::KeepPiece(index));
            ui.close();
        }
        if let Some(path) = piece.polarity_path() {
            polarity_menu(ui, path, acts);
        }
        if let Some(file) = piece.file() {
            if ui
                .button(RichText::new("Move to the Recycle Bin").color(p.danger))
                .clicked()
            {
                acts.push(Act::Delete(file.clone()));
                ui.close();
            }
        }
    });
}

/// The piece's ink (#666): Auto finds its background, As is takes its dark pixels as the
/// drawing, Invert its light ones. The keyboard's Dark / Light (Screens) then decides the look.
fn polarity_menu(ui: &mut egui::Ui, path: &std::path::Path, acts: &mut Vec<Act>) {
    use super::art_encode::Polarity;
    let now = super::art_polarity::of(path);
    ui.menu_button(format!("Ink: {}", now.label()), |ui| {
        for p in Polarity::ALL {
            let hint = match p {
                Polarity::Auto => "Finds the background: the drawing is the ink, whichever way round it was drawn.",
                Polarity::AsIs => "The dark pixels are the drawing.",
                Polarity::Invert => "The light pixels are the drawing.",
            };
            if ui.radio(now == p, p.label()).on_hover_text(hint).clicked() {
                acts.push(Act::Polarity(path.to_path_buf(), p));
                ui.close();
            }
        }
    })
    .response
    .on_hover_text("Which way round this piece is drawn. Dark or light screens: Screens, Showing now.");
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// #560: an art screen mounted upright is shown as stored; one on its side is turned.
    #[test]
    fn thumbnails_follow_how_the_art_screen_is_mounted() {
        let mut px = vec![Color32::WHITE; 8];
        px[1] = Color32::BLACK;
        let img = egui::ColorImage::new([4, 2], px);
        let upright = thumbnail_turned(&img, false, 0);
        assert_eq!(upright.size, [4, 2]);
        assert_eq!(upright.pixels[1], LIT);
        assert_eq!(upright.pixels.iter().filter(|p| **p == LIT).count(), 1);
        assert_eq!(thumbnail_turned(&img, false, 1).size, [2, 4]);
        assert_eq!(thumbnail_turned(&img, false, 2).pixels[6], LIT);
    }

    #[test]
    fn thumbnails_are_upright_and_one_bit() {
        // Stored 4 wide x 2 high: upright is 2 wide x 4 tall.
        // White is the stored background, black the ink the screen lights.
        let mut px = vec![Color32::WHITE; 8];
        px[3] = Color32::BLACK; // stored (3, 0) = upright (0, 0)
        let stored = egui::ColorImage::new([4, 2], px);
        let full = thumbnail_turned(&stored, false, 1);
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
        let half = thumbnail_turned(&grey, true, 1);
        assert_eq!(half.size, [4, 4]);
        assert!(half.pixels.iter().all(|p| *p == LIT || *p == OFF), "1-bit");
        let lit = half.pixels.iter().filter(|p| **p == LIT).count();
        assert_eq!(lit, 8, "a 50% grey dithers to half lit");
        let ink = thumbnail_turned(
            &egui::ColorImage::new([4, 4], vec![Color32::BLACK; 16]),
            true,
            1,
        );
        assert!(ink.pixels.iter().all(|p| *p == LIT));
        let background = thumbnail_turned(
            &egui::ColorImage::new([4, 4], vec![Color32::WHITE; 16]),
            true,
            1,
        );
        assert!(background.pixels.iter().all(|p| *p == OFF));
    }

    #[test]
    fn rotation_pills_follow_its_settings() {
        let mut c = Cycle::default();
        assert_eq!(rotation_pills(&c), [true, false, false, false]);
        c.set_step(Step::Minutes, true);
        c.follow = Follow::Wallpaper;
        assert_eq!(rotation_pills(&c), [false, true, false, true]);
    }

    /// The pictures' textures live in the context they were made in, keyed by content: a new
    /// context (as each preview makes) gets its own, which the page's blank thumbnails in the
    /// art-flow audit's renders lacked.
    #[test]
    fn textures_belong_to_their_context() {
        let image = egui::ColorImage::new([4, 2], vec![Color32::WHITE; 8]);
        let a = egui::Context::default();
        let first = texture(&a, 7, true, &image);
        assert_eq!(texture(&a, 7, true, &image).id(), first.id(), "made once");
        assert_ne!(
            texture(&a, 8, true, &image).id(),
            first.id(),
            "new content, new picture"
        );
        let b = egui::Context::default();
        let other = texture(&b, 7, true, &image);
        assert!(
            b.tex_manager().read().meta(other.id()).is_some(),
            "a second context makes its own texture"
        );
    }

    fn bundled(name: &str) -> Vec<u8> {
        std::fs::read(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("resources/lighting/art")
                .join(name),
        )
        .expect("bundled art")
    }

    /// The previews' art: the one built-in (the bundled manifest, so names and pictures), five kept
    /// places, "hearth" sent from this PC and showing.
    pub fn sample() -> (Snapshot, art_library::Library) {
        let files = [
            "constellations.bin",
            "fury.bin",
            "hearth.bin",
            "moon-garden.bin",
        ];
        let entries = files
            .into_iter()
            .map(|name| {
                let bytes = bundled(name);
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
        let slot = |n: u8, name: &str, len: usize| foxglove_link::protocol::SlotInfo {
            slot: n,
            used: true,
            writes: 1,
            len: len as u16,
            name: name.into(),
        };
        let slots = vec![
            slot(0, "fury", bundled("fury.bin").len()),
            slot(1, "constellations", bundled("constellations.bin").len()),
            slot(2, "bandit-radar", 3600),
            slot(3, "moon-garden", bundled("moon-garden.bin").len()),
            slot(4, "skyline", 4100),
        ];
        let status = foxglove_link::protocol::ArtStatus {
            link: 2,
            right_state: foxglove_link::protocol::right_state::PLAYING,
            right_set: -1,
            right_set_count: 1,
            right_capacity: 16384,
            flash_slots: 8,
            flash_used: 0b1_1111,
            ..Default::default()
        };
        let mut cycle = Cycle::default();
        cycle.set_step(Step::LightsWake, true);
        let snapshot = Snapshot {
            connected: true,
            status: Some(status),
            pinned: Some(Choice::File(PathBuf::from("art/hearth.bin"))),
            sent: Some((PathBuf::from("art/hearth.bin"), 0)),
            slots,
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
