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
    /// The duplicate kept place whose Free waits for the answer.
    free: Option<u8>,
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
    /// The "Animate on e-paper" switch (#833), clicked.
    AnimateEpaper(bool),
}

#[cfg(test)]
thread_local! {
    /// The previews' art state and library, so they never ask a real keyboard.
    pub static PREVIEW: std::cell::RefCell<Option<(Snapshot, art_library::Library)>> =
        const { std::cell::RefCell::new(None) };
    /// The previews' e-paper switch, so they never read or write the shared test store.
    pub static PREVIEW_ANIMATE_EPAPER: std::cell::Cell<Option<bool>> =
        const { std::cell::Cell::new(None) };
}

/// The art screen's pace as the page shows it (#833): its refresh class and frame limit, and
/// the user's e-paper switch, which is read only for an e-paper screen (never for the Corne's).
fn page_pace(ctx: &egui::Context) -> super::art_encode::Pace {
    super::art_encode::Pace::current_with(|| animate_epaper_here(ctx))
}

/// The e-paper switch as last read or clicked here: read from the store again each time the
/// page comes back on screen (a frame without it in between), not every frame.
fn animate_epaper_here(ctx: &egui::Context) -> bool {
    #[cfg(test)]
    if let Some(on) = PREVIEW_ANIMATE_EPAPER.get() {
        return on;
    }
    let id = egui::Id::new("foxglove-animate-epaper");
    let frame = ctx.cumulative_frame_nr();
    let kept = ctx
        .data(|d| d.get_temp::<(u64, bool)>(id))
        .filter(|(n, _)| frame.saturating_sub(*n) <= 1);
    let on = kept.map_or_else(art_service::animate_epaper, |(_, on)| on);
    ctx.data_mut(|d| d.insert_temp(id, (frame, on)));
    on
}

/// The switch's label.
pub const EPAPER_SWITCH: &str = "Animate on e-paper";
/// The chip beside it, and the chip's hover.
pub const MAY_GHOST: &str = "may ghost";
pub const GHOST_TIP: &str =
    "E-paper can keep a faint trace of the frame before (ghosting) until the screen's next full refresh.";

/// The switch's hover (and its ⓘ): what it does, at this screen's rate. It doesn't repeat the
/// switch's label, which sits right beside it.
pub fn epaper_tip(pace: super::art_encode::Pace) -> String {
    let rate = match pace.fps(u8::MAX) {
        _ if pace.min_frame_ms == 0 => "at the piece's own rate".to_string(),
        1 => "at most 1 frame a second on this screen".to_string(),
        n => format!("at most {n} frames a second on this screen"),
    };
    format!(
        "Plays each piece's frames, {rate}. Off: only its first frame shows. Pieces kept on \
         the keyboard change when they're kept again."
    )
}

/// "Animate on e-paper" with its ⓘ and "may ghost" beside it (#833), only when the art screen
/// is e-paper: a fast screen (the Corne's) never shows it.
fn epaper_switch(ui: &mut egui::Ui, pace: super::art_encode::Pace, acts: &mut Vec<Act>) {
    if !pace.refresh.is_epaper() {
        return;
    }
    let mut on = pace.animate_epaper;
    ui.add_space(6.0);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        let c = ui.checkbox(&mut on, EPAPER_SWITCH);
        if theme::with_info(ui, c, &epaper_tip(pace)).changed() {
            let id = egui::Id::new("foxglove-animate-epaper");
            let frame = ui.ctx().cumulative_frame_nr();
            ui.ctx().data_mut(|d| d.insert_temp(id, (frame, on)));
            acts.push(Act::AnimateEpaper(on));
        }
        let dot = if on {
            theme::Dot::Warn
        } else {
            theme::Dot::Muted
        };
        theme::hint_chip(ui, MAY_GHOST, Some(GHOST_TIP), Some(dot));
    });
}

/// A kept place's hover when this PC has no file for it, so its tile shows "ART".
pub const NO_PREVIEW_TIP: &str =
    "Kept on the keyboard; its file isn't on this PC, so there's no preview";

/// The refusal's hover.
pub const ALREADY_KEPT_TIP: &str =
    "It's on the keyboard already; keeping it again would fill a second place with the same piece.";

/// Keep's refusal of a piece a place already holds.
pub fn already_kept(name: &str) -> String {
    format!("Already kept as {name}")
}

/// Why Keep won't run `act`: the piece is already kept in another place (the same bytes).
fn refusal(art: &Art, act: &Act) -> Option<String> {
    let (piece, into) = match act {
        Act::KeepPiece(i) => (Some(*i), None),
        Act::KeepFile(p) => (art_pieces::piece_of_file(&art.pcs, p), None),
        Act::KeepIn(p, slot) => (art_pieces::piece_of_file(&art.pcs, p), Some(*slot)),
        _ => return None,
    };
    let k = art_pieces::kept_as(&art.s, &art.kept, piece?).filter(|k| Some(k.slot) != into)?;
    Some(already_kept(&k.name))
}

/// Opens (or closes) the free-duplicate choice for the previews.
#[cfg(test)]
pub fn preview_free(slot: Option<u8>) {
    UI.with_borrow_mut(|u| u.free = slot);
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
    /// Pieces that can't go on the art screen, with the reason (#340).
    pub refused: Vec<(String, String)>,
}

impl Art {
    pub fn new(s: Snapshot, library: art_library::Library) -> Art {
        let art_pieces::Scanned {
            found,
            unreadable,
            refused,
        } = art_pieces::found(&library);
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
            refused,
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
        if let Some(why) = refusal(art, &a) {
            UI.with_borrow_mut(|u| u.message = Some(why));
            continue;
        }
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
            Act::AnimateEpaper(on) => {
                if let Err(e) = art_service::set_animate_epaper(on) {
                    eprintln!("Foxglove: {e}");
                }
                // A keyboard that paces its own art (#842) plays its built-in and kept pieces
                // at the new choice too.
                h.tell_animate(on);
                // A piece sent from this PC goes again at the new pace; kept ones stay as kept.
                let sent = art.s.status.map(|st| art_words::showing(&st)) == Some(Showing::Sent);
                if let Some(file) = art.s.showing_file().filter(|_| sent) {
                    h.push(file.clone());
                }
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
            let keep = match (now, s.showing_file()) {
                (Showing::Sent, Some(file)) if art_pieces::places(s) > 0 => {
                    Some(Act::KeepFile(file.clone()))
                }
                _ => None,
            };
            // The same bytes kept already: a second place would hold a copy, so no Keep. The
            // refusal takes the restart line's place: "Not kept on the keyboard" would be wrong.
            let refused = keep.as_ref().and_then(|k| refusal(art, k));
            match (&refused, art_words::after_restart(now, remembered)) {
                (Some(why), _) => ui.weak(why).on_hover_text(ALREADY_KEPT_TIP),
                (None, Some(line)) => ui.weak(line),
                (None, None) => ui.weak(art_words::right_state_words(st.right_state)),
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
                    if let Some(keep) = keep.filter(|_| refused.is_none()) {
                        if ui
                            .add(theme::primary_button(ui, "Keep on keyboard"))
                            .clicked()
                        {
                            acts.push(keep);
                        }
                    }
                });
            });
        });
    });
    // Under the picture, across the column, so "may ghost" stays beside the switch.
    epaper_switch(ui, page_pace(ui.ctx()), acts);
    strip(ui, s, acts);
}

/// Which Rotation pills are lit, from its settings: Off, Every n min, Lights wake, Match the
/// wallpaper. On e-paper (#833) lights wake doesn't apply: it isn't offered, and a saved one
/// doesn't count as moving.
pub fn rotation_pills(c: &Cycle, epaper: bool) -> [bool; 4] {
    let wake = c.steps_on(Step::LightsWake) && !epaper;
    let moves = c.steps_on(Step::Minutes) || wake || c.follow == Follow::Wallpaper;
    [
        !moves,
        c.steps_on(Step::Minutes),
        wake,
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
    // On e-paper Rotation steps by minutes; lights wake isn't offered (#833).
    let epaper = super::art_encode::Pace::current_with(|| false)
        .refresh
        .is_epaper();
    let lit = rotation_pills(&cycle, epaper);
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
        if !epaper {
            let wake = ui.add_enabled(s.wake_events, theme::pill_button(ui, lit[2], "Lights wake"));
            if wake
                .on_hover_text("Steps when the keyboard's lights come back on.")
                .on_disabled_hover_text("Needs keyboard firmware that reports when the lights come back on.")
                .clicked()
            {
                cycle.set_step(Step::LightsWake, !lit[2]);
                changed = true;
            }
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
    if super::art_playlist::active(&cycle) {
        let pieces = cycle.items.len();
        ui.weak(format!(
            "Steps through the playlist's {pieces} pieces. Its art keys step through the same ones."
        ));
    } else {
        let pieces = super::art_cycle::rotation(&st).len();
        ui.weak(format!(
            "Steps through the keyboard's {pieces} pieces. Its art keys step through the same ones."
        ));
    }
    playlist(ui, art, &mut cycle, &mut changed);
    layer_art(ui, art, &mut cycle, &mut changed);
    if changed {
        acts.push(Act::Cycle(cycle));
    }
}

/// A playlist entry in the user's words.
fn item_label(item: &Item, s: &Snapshot) -> String {
    let count = s.status.map_or(0, |st| st.right_set_count);
    match item {
        Item::BuiltIn(name) => super::art_manifest::sets(count)
            .and_then(|sets| sets.iter().find(|set| &set.name == name))
            .map_or_else(|| name.clone(), |set| set.label.clone()),
        Item::Slot(n) => art_service::name_of(&Choice::Slot(*n), s),
        Item::File(p) => art_service::file_name(p),
    }
}

/// The playlist (#522): more art than the keyboard keeps, in the order Rotation steps it. Its
/// pieces from this PC go into the places lent to it ahead of their turn (`art_playlist`).
fn playlist(ui: &mut egui::Ui, art: &Art, cycle: &mut Cycle, changed: &mut bool) {
    let s = &art.s;
    let Some(st) = s.status else {
        return;
    };
    egui::CollapsingHeader::new("Playlist")
        .id_salt("art-playlist")
        .default_open(cycle.playlist)
        .show(ui, |ui| {
            ui.weak("Steps through more art than the keyboard keeps; each piece is staged ahead, so steps are instant.");
            let mut on = cycle.playlist;
            if ui.checkbox(&mut on, "Play the playlist").changed() {
                cycle.playlist = on;
                if on && cycle.places.is_empty() {
                    cycle.places = super::art_playlist::lend(&st, super::art_playlist::MAX_PLACES);
                }
                *changed = true;
            }
            let mut remove = None;
            let mut earlier = None;
            egui::Grid::new("art-playlist-items")
                .num_columns(3)
                .spacing(egui::vec2(8.0, 4.0))
                .show(ui, |ui| {
                    for (i, item) in cycle.items.iter().enumerate() {
                        let here = cycle.playlist && i == cycle.position % cycle.items.len().max(1);
                        let label = format!("{}. {}", i + 1, item_label(item, s));
                        if here {
                            ui.label(
                                RichText::new(format!("{label} · now"))
                                    .color(theme::palette(ui).accent),
                            )
                            .on_hover_text("Rotation's piece now");
                        } else {
                            ui.label(label);
                        }
                        if i > 0 {
                            if ui.small_button("↑").on_hover_text("Earlier").clicked() {
                                earlier = Some(i);
                            }
                        } else {
                            ui.label("");
                        }
                        if ui
                            .small_button("Remove")
                            .on_hover_text("Remove from the playlist")
                            .clicked()
                        {
                            remove = Some(i);
                        }
                        ui.end_row();
                    }
                });
            if let Some(i) = earlier {
                cycle.items.swap(i - 1, i);
                *changed = true;
            }
            if let Some(i) = remove {
                cycle.items.remove(i);
                cycle.position = 0;
                *changed = true;
            }
            let mut add = None;
            egui::ComboBox::from_id_salt("art-playlist-add")
                .selected_text("Add a piece…")
                .show_ui(ui, |ui| {
                    for p in &art.pcs {
                        if let Some(f) = p.file() {
                            if ui.selectable_label(false, &p.name).clicked() {
                                add = Some(Item::File(f.clone()));
                            }
                        }
                    }
                    for set in super::art_manifest::sets(st.right_set_count).unwrap_or_default() {
                        if ui.selectable_label(false, &set.label).clicked() {
                            add = Some(Item::BuiltIn(set.name.clone()));
                        }
                    }
                    for info in s
                        .slots
                        .iter()
                        .filter(|i| i.used && !cycle.places.contains(&i.slot))
                    {
                        if ui.selectable_label(false, &info.name).clicked() {
                            add = Some(Item::Slot(info.slot));
                        }
                    }
                });
            if let Some(item) = add {
                cycle.items.push(item);
                *changed = true;
            }
            ui.horizontal_wrapped(|ui| {
                let lent: Vec<String> = cycle.places.iter().map(|n| (n + 1).to_string()).collect();
                ui.label(if lent.is_empty() {
                    "No places lent".to_string()
                } else {
                    format!("Places lent: {}", lent.join(" and "))
                });
                let more = super::art_playlist::lend(&st, 8)
                    .into_iter()
                    .rev()
                    .find(|n| !cycle.places.contains(n));
                if cycle.places.len() < super::art_playlist::MAX_PLACES {
                    let lend = ui.add_enabled(more.is_some(), egui::Button::new("Lend a place").small());
                    if lend
                        .on_hover_text("A free place on the keyboard for the playlist's pieces.")
                        .on_disabled_hover_text("The keyboard's places are all in use.")
                        .clicked()
                    {
                        if let Some(n) = more {
                            cycle.places.push(n);
                            cycle.places.sort_unstable();
                            *changed = true;
                        }
                    }
                }
                if !cycle.places.is_empty()
                    && ui
                        .small_button("Give one back")
                        .on_hover_text("The place keeps what it holds, as one of yours.")
                        .clicked()
                {
                    cycle.places.remove(0);
                    *changed = true;
                }
            });
            // How the last swap went (the health rows, #705).
            if cycle.playlist {
                let health = super::art_health::now();
                if let Some(row) = super::art_health::rows_of(&health, std::time::Instant::now()).get(2) {
                    let p = theme::palette(ui);
                    let colour = match row.level {
                        super::art_health::Level::Ok => p.text,
                        super::art_health::Level::Warn => p.warn,
                        super::art_health::Level::Fail => p.danger,
                    };
                    ui.colored_label(colour, format!("Last swap: {}", row.detail));
                }
            }
        });
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
    if !s.cycle.places.is_empty() {
        let lent: Vec<String> = s.cycle.places.iter().map(|n| (n + 1).to_string()).collect();
        ui.weak(format!(
            "Places {} are lent to the playlist: it changes what they hold.",
            lent.join(" and ")
        ));
    }
    let now = art_words::showing(&st);
    let dups = art_pieces::duplicates(s, &art.kept);
    #[cfg(test)]
    KEPT_TILES.with_borrow_mut(Vec::clear);
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
                #[cfg(test)]
                KEPT_TILES.with_borrow_mut(|t| t.push((k.slot, r.rect)));
                if r.clicked() && !on && !s.busy {
                    acts.push(Act::Show(Choice::Slot(k.slot)));
                }
                let dup = dups.iter().find(|d| d.slot == k.slot);
                r.on_hover_text(kept_tip(piece.is_some(), dup))
                    .context_menu(|ui| {
                        if ui.button("Show now").clicked() {
                            acts.push(Act::Show(Choice::Slot(k.slot)));
                            ui.close();
                        }
                        if dup.is_some() && ui.button(FREE_BUTTON).clicked() {
                            state.free = Some(k.slot);
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
        // The same piece in two places: offer the copy's place back.
        for d in &dups {
            ui.horizontal_wrapped(|ui| {
                ui.colored_label(theme::palette(ui).warn, same_piece_line(d));
                if ui
                    .add_enabled(!s.busy, egui::Button::new(FREE_BUTTON).small())
                    .clicked()
                {
                    state.free = Some(d.slot);
                }
            });
        }
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
    free_dialog(ui, art, &dups, state, acts);
    replace_dialog(ui, art, state, acts);
}

#[cfg(test)]
thread_local! {
    /// Where the kept tiles were drawn last frame, by place, so a preview can hover one.
    pub static KEPT_TILES: std::cell::RefCell<Vec<(u8, egui::Rect)>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// The duplicate's button and menu entry.
pub const FREE_BUTTON: &str = "Free this place…";

/// A kept tile's hover: how to use it, and why it has no picture or what it copies.
pub fn kept_tip(has_file: bool, dup: Option<&art_pieces::Duplicate>) -> String {
    let act = "Click to show it. Right-click for more.";
    match dup {
        Some(d) => format!("The same piece as {}. {act}", d.of_name),
        None if has_file => act.to_string(),
        None => format!("{NO_PREVIEW_TIP}.\n{act}"),
    }
}

/// The line under the kept tiles for a duplicate.
pub fn same_piece_line(d: &art_pieces::Duplicate) -> String {
    format!("{} is the same piece as {}.", d.name, d.of_name)
}

/// Free this place asks first; it goes through the same erase as Remove, and the piece stays
/// kept in the other place.
fn free_dialog(
    ui: &mut egui::Ui,
    art: &Art,
    dups: &[art_pieces::Duplicate],
    state: &mut UiState,
    acts: &mut Vec<Act>,
) {
    let Some(slot) = state.free else {
        return;
    };
    let Some(d) = dups.iter().find(|d| d.slot == slot) else {
        state.free = None;
        return;
    };
    let mut open = true;
    egui::Window::new(free_title(d))
        .id(egui::Id::new("art-free"))
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .open(&mut open)
        .show(ui.ctx(), |ui| {
            ui.label(free_body(d));
            if let Some(line) = used_for(&art.s, d.slot) {
                ui.colored_label(theme::palette(ui).warn, line);
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui
                    .add(theme::primary_button(ui, "Free the place"))
                    .clicked()
                {
                    acts.push(Act::Remove(slot));
                    state.free = None;
                }
                if ui.button("Cancel").clicked() {
                    state.free = None;
                }
            });
        });
    if !open {
        state.free = None;
    }
}

pub fn free_title(d: &art_pieces::Duplicate) -> String {
    format!("Free {}'s place?", d.name)
}

pub fn free_body(d: &art_pieces::Duplicate) -> String {
    format!(
        "It's the same piece as {}, which stays kept, so nothing is lost.",
        d.of_name
    )
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
    // #340: when no piece was made for this board, one note says so for all of them; a card
    // says "fitted" only when the board has pieces of both kinds.
    let fitted = Fitted::of(&art.pcs);
    if fitted == Fitted::All {
        ui.horizontal(|ui| {
            let note = ui.weak(FITTED_NOTE);
            theme::with_info(ui, note, super::art_encode::FIT_RULE);
        });
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
                let tile = Tile {
                    badge: *badge,
                    on,
                    can_send,
                    has_places,
                    capacity: art_pieces::capacity(s),
                    fitted_chip: fitted == Fitted::Some,
                };
                pc_tile(ui, piece, *i, tile, acts);
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
    if !art.refused.is_empty() {
        // #340: what can't be made for this board's art screen, and why.
        ui.weak(refused_line(
            art.refused.len(),
            &super::screen_roles::the(super::screen_roles::Role::Art),
        ))
        .on_hover_text(
            art.refused
                .iter()
                .map(|(name, why)| format!("{name}: {why}"))
                .collect::<Vec<_>>()
                .join("\n"),
        );
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

/// A PC card's state: its badge, whether it shows now, what the keyboard allows, and the most
/// a piece can be (the keyboard's art buffer).
#[derive(Clone, Copy)]
struct Tile {
    badge: Badge,
    on: bool,
    can_send: bool,
    has_places: bool,
    capacity: usize,
    /// Show the "fitted" chip on a fitted piece (only when native and fitted pieces mix).
    fitted_chip: bool,
}

/// The note above the PC cards when none was made for this board (#340).
pub const FITTED_NOTE: &str = "Art isn't made for this board: it's fitted, never stretched";

/// How many of the PC pieces were fitted to this board's art screen (#340).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fitted {
    None,
    /// Some, beside pieces made for it: each fitted card gets a chip.
    Some,
    /// Every one: one note for all.
    All,
}

impl Fitted {
    pub fn of(pcs: &[PcPiece]) -> Fitted {
        let n = pcs.iter().filter(|p| p.fitted.is_some()).count();
        match n {
            0 => Fitted::None,
            n if n == pcs.len() => Fitted::All,
            _ => Fitted::Some,
        }
    }
}

/// "1 piece can't go on the art screen." / "3 pieces can't go on the art screen."
pub fn refused_line(n: usize, the_screen: &str) -> String {
    let pieces = if n == 1 { "piece" } else { "pieces" };
    format!("{n} {pieces} can't go on {the_screen}.")
}

/// A thumbnail's size on a PC card: the picture's own shape, at most 70 points tall and as
/// wide as the card allows (#340). The Corne's halved 34x70 thumbnail stays 34x70.
fn tile_picture_size(size: [usize; 2]) -> egui::Vec2 {
    let (w, h) = (size[0].max(1) as f32, size[1].max(1) as f32);
    let scale = f32::min(70.0 / h, (PC_TILE - 26.0) / w);
    egui::vec2(w * scale, h * scale)
}

/// A size in KiB as the art words say it: "16 KiB", "12.5 KiB".
pub fn kib(bytes: usize) -> String {
    if bytes.is_multiple_of(1024) {
        format!("{} KiB", bytes / 1024)
    } else {
        format!("{:.1} KiB", bytes as f32 / 1024.0)
    }
}

/// A PC piece's card: the picture, its name, its one badge, and Keep on keyboard.
fn pc_tile(ui: &mut egui::Ui, piece: &PcPiece, index: usize, tile: Tile, acts: &mut Vec<Act>) {
    let Tile {
        badge,
        on,
        can_send,
        has_places,
        capacity,
        fitted_chip,
    } = tile;
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
                ui.add(egui::Image::new((t.id(), tile_picture_size(t.size()))));
                ui.label(RichText::new(&piece.name).color(p.text_strong));
                if let Some(rule) = piece.fitted.as_deref().filter(|_| fitted_chip) {
                    // #340: drawn for another screen, fitted by the one rule.
                    theme::hint_chip(ui, "fitted", Some(rule), None);
                }
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
                        let over = piece.bytes.saturating_sub(capacity);
                        ui.label(
                            RichText::new(format!("over {}", kib(capacity)))
                                .small()
                                .color(p.danger),
                        )
                        .on_hover_text(format!(
                            "{} is {}: {} over the {} the keyboard takes.",
                            piece.name,
                            kib(piece.bytes),
                            kib(over),
                            kib(capacity)
                        ));
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
        assert_eq!(rotation_pills(&c, false), [true, false, false, false]);
        c.set_step(Step::Minutes, true);
        c.follow = Follow::Wallpaper;
        assert_eq!(rotation_pills(&c, false), [false, true, false, true]);
        // #833: on e-paper a saved lights wake isn't offered and doesn't move the art.
        let mut wake = Cycle::default();
        wake.set_step(Step::LightsWake, true);
        assert_eq!(rotation_pills(&wake, false), [false, false, true, false]);
        assert_eq!(rotation_pills(&wake, true), [true, false, false, false]);
        wake.set_step(Step::Minutes, true);
        assert_eq!(rotation_pills(&wake, true), [false, true, false, false]);
    }

    /// A made-up board's e-paper art screen (#833): 88x184, partial refresh, 1 s a frame.
    pub fn epaper_screens() -> Vec<crate::foxglove::screen_roles::Screen> {
        use crate::foxglove::screen_roles::{Role, Screen};
        vec![Screen {
            id: 0,
            role: Role::Art,
            name: "Art screen".into(),
            width: 88,
            height: 184,
            format: "1bit".into(),
            rotation: 0,
            refresh: crate::foxglove::boards::Refresh::EpaperPartial,
            min_frame_ms: 1000,
            max_blob: None,
            slots: None,
        }]
    }

    /// Every text Showing now's art and Rotation columns paint (hover text not included), on
    /// the art screens `screens`, with the e-paper switch `animate`.
    fn now_texts(
        screens: Vec<crate::foxglove::screen_roles::Screen>,
        animate: bool,
    ) -> Vec<String> {
        use crate::foxglove::art_encode::TEST_SCREENS;
        use crate::foxglove::theme::headless;
        TEST_SCREENS.set(Some(screens));
        PREVIEW_ANIMATE_EPAPER.set(Some(animate));
        let (s, lib) = sample();
        let art = Art::new(s, lib);
        let ctx = headless::ctx(crate::foxglove::theme::Choice::Dark);
        ctx.memory_mut(|m| m.set_everything_is_visible(true));
        let mut shapes = Vec::new();
        for t in 0..2 {
            shapes = headless::frame(&ctx, f64::from(t), vec![], |ui| {
                let mut acts = Vec::new();
                draw_now_right(ui, &art, &mut acts);
                draw_rotation(ui, &art, &mut acts);
                assert!(acts.is_empty(), "drawing alone asks for nothing");
            });
        }
        TEST_SCREENS.set(None);
        PREVIEW_ANIMATE_EPAPER.set(None);
        headless::texts(&shapes)
    }

    /// #833: an e-paper art screen gets "Animate on e-paper" with "may ghost" beside it, and
    /// Rotation offers Every n min but not Lights wake.
    #[test]
    fn epaper_gets_the_switch_and_no_lights_wake() {
        for animate in [false, true] {
            let t = now_texts(epaper_screens(), animate);
            assert!(t.iter().any(|t| t == EPAPER_SWITCH), "{t:?}");
            assert!(t.iter().any(|t| t == MAY_GHOST), "{t:?}");
            assert!(t.iter().any(|t| t.starts_with("Every ")), "{t:?}");
            assert!(!t.iter().any(|t| t == "Lights wake"), "{t:?}");
        }
        let pace = crate::foxglove::art_encode::Pace::of(&epaper_screens()[0], false);
        assert_eq!(
            epaper_tip(pace),
            "Plays each piece's frames, at most 1 frame a second on this screen. Off: only its \
             first frame shows. Pieces kept on the keyboard change when they're kept again."
        );
        assert!(GHOST_TIP.contains("ghosting"));
    }

    /// #833: the Corne's screen is fast: no e-paper switch (its setting isn't even read, so a
    /// stored "on" changes nothing), and Rotation as before, Lights wake included.
    #[test]
    fn the_cornes_screens_page_is_unchanged() {
        use crate::foxglove::art_encode::Pace;
        use crate::foxglove::fixtures::users_desk;
        use crate::foxglove::screen_roles::{of_board, old_firmware};
        let corne = of_board(&users_desk::board());
        for screens in [corne.clone(), old_firmware()] {
            let off = now_texts(screens.clone(), false);
            assert_eq!(
                off,
                now_texts(screens.clone(), true),
                "the switch's value doesn't matter"
            );
            assert!(
                !off.iter().any(|t| t == EPAPER_SWITCH || t == MAY_GHOST),
                "{off:?}"
            );
            assert!(off.iter().any(|t| t == "Lights wake"), "{off:?}");
        }
        crate::foxglove::art_encode::TEST_SCREENS.set(Some(corne));
        let pace = Pace::current_with(|| panic!("the Corne's page reads the e-paper switch"));
        crate::foxglove::art_encode::TEST_SCREENS.set(None);
        assert!(!pace.refresh.is_epaper());
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

    /// #340: a card's picture keeps its shape; the Corne's halved art stays 34x70, a wide
    /// screen's fits the card's width, and sizes read in KiB.
    #[test]
    fn tile_pictures_keep_their_shape() {
        assert_eq!(super::tile_picture_size([34, 70]), egui::vec2(34.0, 70.0));
        let wide = super::tile_picture_size([160, 56]);
        assert!((wide.x - (PC_TILE - 26.0)).abs() < 0.01 && wide.y < 70.0);
        assert!((wide.x / wide.y - 160.0 / 56.0).abs() < 0.01);
        assert_eq!(super::kib(16384), "16 KiB");
        assert_eq!(super::kib(12800), "12.5 KiB");
    }

    /// #340: one note when every piece is fitted, a chip per card only when they mix; the
    /// refused line counts in words.
    #[test]
    fn fitted_pieces_are_noted_once_or_chipped() {
        use super::{refused_line, Fitted};
        let (_, lib) = sample();
        let mut pcs = Art::new(Snapshot::default(), lib).pcs;
        assert_eq!(Fitted::of(&pcs), Fitted::None);
        for p in &mut pcs {
            p.fitted = Some("rule".into());
        }
        assert_eq!(Fitted::of(&pcs), Fitted::All);
        pcs[0].fitted = None;
        assert_eq!(Fitted::of(&pcs), Fitted::Some);
        assert_eq!(Fitted::of(&[]), Fitted::None);
        assert_eq!(
            refused_line(1, "the art screen"),
            "1 piece can't go on the art screen."
        );
        assert_eq!(
            refused_line(3, "the art screen"),
            "3 pieces can't go on the art screen."
        );
        assert!(super::FITTED_NOTE.chars().count() <= 60);
    }

    /// The sample with moon-garden sent from this PC (place 4 keeps it), and hearth-moongarde
    /// in place 1: a library copy of moon-garden's bytes under another name.
    fn garden_twice() -> Art {
        let (mut s, mut l) = sample();
        let garden = l
            .entries
            .iter()
            .find(|e| e.path.ends_with("moon-garden.bin"))
            .unwrap()
            .clone();
        l.entries.push(art_library::Entry {
            path: "art/hearth-moongarden.bin".into(),
            ..garden.clone()
        });
        s.slots[0].name = "hearth-moongarde".into();
        s.slots[0].len = garden.bytes as u16;
        s.sent = Some(("art/moon-garden.bin".into(), 0));
        Art::new(s, l)
    }

    /// Keep refuses a piece a place already holds, by any of its files, and says which place;
    /// replacing that same place is no second copy.
    #[test]
    fn keep_refuses_a_piece_already_kept() {
        let art = garden_twice();
        let garden = PathBuf::from("art/moon-garden.bin");
        let first = Some("Already kept as hearth-moongarde".to_string());
        assert_eq!(refusal(&art, &Act::KeepFile(garden.clone())), first);
        assert_eq!(
            refusal(&art, &Act::KeepFile("art/hearth-moongarden.bin".into())),
            first
        );
        let i = art_pieces::piece_of_file(&art.pcs, &garden).unwrap();
        assert_eq!(refusal(&art, &Act::KeepPiece(i)), first);
        assert_eq!(refusal(&art, &Act::KeepIn(garden.clone(), 0)), None);
        assert_eq!(refusal(&art, &Act::KeepIn(garden, 6)), first);
        assert_eq!(refusal(&art, &Act::KeepFile("art/hearth.bin".into())), None);
        assert_eq!(refusal(&art, &Act::Remove(3)), None);
    }

    /// Showing now offers no Keep for a sent piece that is kept already: it says where, in place
    /// of the "Not kept on the keyboard" restart line.
    #[test]
    fn showing_now_says_already_kept_instead_of_keep() {
        use crate::foxglove::theme::headless;
        let art = garden_twice();
        let ctx = headless::ctx(crate::foxglove::theme::Choice::Dark);
        let mut shapes = Vec::new();
        for t in 0..2 {
            shapes = headless::frame(&ctx, f64::from(t), vec![], |ui| {
                draw_now_right(ui, &art, &mut Vec::new());
            });
        }
        let t = headless::texts(&shapes);
        assert!(
            t.iter().any(|t| t == "Already kept as hearth-moongarde"),
            "{t:?}"
        );
        assert!(!t.iter().any(|t| t == "Keep on keyboard"), "{t:?}");
        assert!(
            !t.iter().any(|t| t.starts_with("Not kept on the keyboard")),
            "no restart line that says it isn't kept: {t:?}"
        );
    }

    /// The KEPT list: a place with no file here says why it has no picture on hover; the
    /// duplicate gets its line and Free this place…, whose confirm erases only that place.
    #[test]
    fn kept_list_explains_missing_files_and_frees_duplicates() {
        use crate::foxglove::theme::headless;
        assert_eq!(
            NO_PREVIEW_TIP,
            "Kept on the keyboard; its file isn't on this PC, so there's no preview"
        );
        assert!(kept_tip(false, None).starts_with(NO_PREVIEW_TIP));
        assert!(!kept_tip(true, None).contains("preview"));

        let art = garden_twice();
        let ctx = headless::ctx(crate::foxglove::theme::Choice::Dark);
        let mut state = UiState::default();
        let at = std::cell::Cell::new(None);
        let run = |t: f64, events: Vec<egui::Event>, state: &mut UiState| {
            let mut acts = Vec::new();
            let shapes = headless::frame(&ctx, t, events, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    draw_keyboard(ui, &art, state, &mut acts);
                });
            });
            // Where "Free the place" was painted, for the click.
            at.set(shapes.iter().find_map(|s| match s {
                egui::epaint::Shape::Text(t) if t.galley.text() == "Free the place" => {
                    Some(t.pos + t.galley.rect.center().to_vec2())
                }
                _ => None,
            }));
            (headless::texts(&shapes), acts)
        };
        let (t, _) = run(0.0, vec![], &mut state);
        let line = "moon-garden is the same piece as hearth-moongarde.";
        assert!(t.iter().any(|t| t == line), "{t:?}");
        // Hover bandit-radar (place 3): no file on this PC.
        let radar = KEPT_TILES.with_borrow(|t| t.iter().find(|(n, _)| *n == 2).unwrap().1);
        run(1.0, headless::hover(radar.center()), &mut state);
        run(2.0, vec![], &mut state);
        let (t, _) = run(2.5, vec![], &mut state);
        assert!(t.iter().any(|t| t.starts_with(NO_PREVIEW_TIP)), "{t:?}");

        // Free this place… asks first; Free the place erases place 4 only.
        state.free = Some(3);
        // A new window lays itself out unseen first.
        run(3.0, vec![], &mut state);
        let (t, acts) = run(3.5, vec![], &mut state);
        assert!(acts.is_empty());
        assert!(t.iter().any(|t| t == "Free moon-garden's place?"), "{t:?}");
        assert!(
            t.iter().any(|t| {
                t
                == "It's the same piece as hearth-moongarde, which stays kept, so nothing is lost."
            }),
            "{t:?}"
        );
        let button = at.get().expect("Free the place was drawn");
        let (_, acts) = run(4.0, headless::click(button), &mut state);
        assert_eq!(
            acts,
            [Act::Remove(3)],
            "the existing erase, for place 4 only"
        );
        assert_eq!(state.free, None);
        // A non-duplicate place never gets the confirm.
        let mut other = UiState {
            free: Some(1),
            ..Default::default()
        };
        run(5.0, vec![], &mut other);
        assert_eq!(other.free, None);
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
                    fit: super::super::art_encode::Fit::AsMade,
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
