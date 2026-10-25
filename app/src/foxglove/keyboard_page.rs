//! The Keyboard page (#386, epic #393), as the approved canvas draws it: a place for the board
//! drawing at the top, then a Firmware card and an Actions card side by side, and the link to
//! this PC in plain words.
//!
//! [`view`] gathers what the page shows from the cached keyboard state; [`draw`] paints a
//! [`View`] and returns what was clicked, so the previews and tests draw it with no keyboard.
//! Nothing here waits or repeats: the state comes from the services' events, and a click acts.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use egui::{Color32, CornerRadius, RichText, Sense, Stroke};

use super::boards::Drawing;
use super::keyboard_actions::{self, Action, Group};
use super::keyboard_settings::{self as ks, Firmware};
use super::settings_ui::{card, row};
use super::shell::Page;
use super::theme;

/// How a value reads: as is, amber (needs a look), or muted (not known yet).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Plain,
    Warn,
    Muted,
}

/// One half's firmware line.
#[derive(Clone, Debug, PartialEq)]
pub struct Half {
    pub name: &'static str,
    pub text: String,
    pub tone: Tone,
    /// Why it couldn't be read, on hover.
    pub hover: Option<String>,
}

/// Everything the page shows.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct View {
    pub halves: Vec<Half>,
    /// A line under the halves: still asking, no keyboard, or the halves disagree.
    pub firmware_note: Option<(String, Tone)>,
    pub can_ask_again: bool,
    pub link: super::connection::Plain,
    pub actions: Vec<Action>,
    /// The risky action waiting for its second click.
    pub armed: Option<String>,
    /// An action is being sent.
    pub busy: bool,
    pub last: Option<(String, String)>,
    /// The board drawing at the top (#392).
    pub board: BoardView,
}

/// What a click on the page asks for.
#[derive(Clone, Debug, PartialEq)]
pub enum Intent {
    AskFirmwareAgain,
    Action(Action),
}

/// The firmware card's lines for the cached builds.
pub fn firmware_lines(f: &Firmware) -> (Vec<Half>, Option<(String, Tone)>, bool) {
    match f {
        Firmware::Known { left, right } => {
            let half = |name, r: &Result<digitalis::protocol::BuildInfo, String>| match r {
                Ok(b) => Half { name, text: format!("{} · {}", b.version, b.sha), tone: Tone::Plain, hover: None },
                Err(e) => Half { name, text: "no reply yet".into(), tone: Tone::Warn, hover: Some(e.clone()) },
            };
            let note = match (left, right) {
                (Ok(l), Ok(r)) if l.version != r.version || l.sha != r.sha => {
                    Some(("The halves run different firmware. Flash both from the same build.".into(), Tone::Warn))
                }
                _ => None,
            };
            (vec![half("Left", left), half("Right", right)], note, true)
        }
        Firmware::Unknown | Firmware::Reading => (Vec::new(), Some(("Asking the keyboard…".into(), Tone::Muted)), false),
        Firmware::NoKeyboard => {
            (Vec::new(), Some(("No keyboard with Foxglove's firmware is connected.".into(), Tone::Muted)), false)
        }
        Firmware::NotReported => {
            (Vec::new(), Some(("This keyboard's firmware doesn't report its version.".into(), Tone::Muted)), false)
        }
    }
}

/// A risky action clicked once, waiting for the confirming second click.
static CONFIRMING: Mutex<Option<(String, Instant)>> = Mutex::new(None);
const CONFIRM_WITHIN: Duration = Duration::from_secs(4);

fn armed() -> Option<String> {
    let confirming = CONFIRMING.lock().unwrap_or_else(|e| e.into_inner());
    confirming.as_ref().filter(|(_, at)| at.elapsed() < CONFIRM_WITHIN).map(|(label, _)| label.clone())
}

/// The page's state from the cached keyboard state (the services keep it current).
pub fn view(ctx: &egui::Context) -> View {
    let (halves, firmware_note, can_ask_again) = firmware_lines(&ks::firmware(ctx));
    View {
        halves,
        firmware_note,
        can_ask_again,
        link: super::connection::plain(ctx),
        actions: keyboard_actions::actions(),
        armed: armed(),
        busy: ks::invoking(),
        last: ks::last_action(),
        board: BoardView::default(),
    }
}

#[cfg(test)]
thread_local! {
    /// The previews' page, so they never ask a real keyboard.
    pub static PREVIEW: std::cell::RefCell<Option<View>> = const { std::cell::RefCell::new(None) };
}

/// The top of the Keyboard page: the board, then the keyboard's cards, acting on their clicks.
/// Returns the page a click on the board opens.
pub fn top(ui: &mut egui::Ui) -> Option<Page> {
    #[cfg(test)]
    if let Some(v) = PREVIEW.with_borrow(|v| v.clone()) {
        let go = board(ui, &v.board);
        draw(ui, &v);
        return go;
    }
    let board_state = board_view(ui.ctx());
    if board_state.panel.is_some() {
        // The left screen's picture follows the panel's own frames while the page is drawn.
        super::screens::want_preview(crate::ui_wake::UiWake::from_ctx(ui.ctx()));
    }
    let go = board(ui, &board_state);
    show(ui);
    go
}

/// The keyboard's cards, acting on their clicks.
fn show(ui: &mut egui::Ui) {
    let view = view(ui.ctx());
    match draw(ui, &view) {
        Some(Intent::AskFirmwareAgain) => {
            ks::read_firmware_again();
            ui.ctx().request_repaint();
        }
        Some(Intent::Action(action)) => {
            let mut confirming = CONFIRMING.lock().unwrap_or_else(|e| e.into_inner());
            if action.risky && view.armed.as_deref() != Some(action.label.as_str()) {
                *confirming = Some((action.label.clone(), Instant::now()));
                return;
            }
            *confirming = None;
            ks::invoke(action.label.clone(), action.binding, ui.ctx().clone());
        }
        None => {}
    }
}

fn tone_color(ui: &egui::Ui, tone: Tone) -> Color32 {
    let p = theme::palette(ui);
    match tone {
        Tone::Plain => p.text,
        Tone::Warn => p.warn,
        Tone::Muted => p.muted,
    }
}

/// Paints the page's cards; returns the click, if any.
pub fn draw(ui: &mut egui::Ui, v: &View) -> Option<Intent> {
    let mut intent = None;
    ui.columns(2, |cols| {
        card(&mut cols[0], "Firmware", |ui| {
            for h in &v.halves {
                // A build in the mono face; words ("no reply yet") in the body face.
                let mut value = RichText::new(&h.text).color(tone_color(ui, h.tone));
                if h.tone == Tone::Plain {
                    value = value.font(theme::mono(12.5));
                }
                let r = row(ui, h.name, value);
                if let Some(why) = &h.hover {
                    r.on_hover_text(why);
                }
            }
            if let Some((note, tone)) = &v.firmware_note {
                ui.label(RichText::new(note).color(tone_color(ui, *tone)));
            }
            ui.add_space(4.0);
            // "Update keyboard" joins this row with #382.
            if v.can_ask_again {
                ui.horizontal(|ui| {
                    if ui.button("Ask the keyboard again").clicked() {
                        intent = Some(Intent::AskFirmwareAgain);
                    }
                });
            }
        });
        card(&mut cols[1], "Actions", |ui| {
            if let Some(a) = actions(ui, v) {
                intent = Some(Intent::Action(a));
            }
        });
    });
    ui.add_space(10.0);
    card(ui, "Link to this PC", |ui| {
        for (what, how) in &v.link.rows {
            row(ui, what, how.as_str());
        }
        if let Some(fix) = &v.link.unplug_fix {
            ui.add_space(2.0);
            let p = theme::palette(ui);
            ui.label(RichText::new("Don't unplug: the keyboard couldn't type.").color(p.warn));
            ui.label(fix);
        }
    });
    ui.add_space(6.0);
    intent
}

/// The Actions card: what the keymap's keys can do, by group; risky ones ask twice.
fn actions(ui: &mut egui::Ui, v: &View) -> Option<Action> {
    if v.actions.is_empty() {
        ui.weak("Connect the keyboard to list what its keys can do.");
        return None;
    }
    let mut clicked = None;
    for chunk in v.actions.chunk_by(|a, b| a.group == b.group) {
        ui.label(theme::caps_label(ui, group_label(chunk[0].group)));
        ui.horizontal_wrapped(|ui| {
            for action in chunk {
                let armed = v.armed.as_deref() == Some(action.label.as_str());
                // "Lights: Toggle" under LIGHTS reads "Toggle".
                let name = short_label(action);
                let text = if armed { format!("Click again: {name}") } else { name.to_string() };
                let p = theme::palette(ui);
                let mut button = egui::Button::new(text);
                if armed {
                    button = button.fill(p.selected_fill).stroke(Stroke::new(1.0, p.warn));
                } else if action.risky {
                    button = button.stroke(Stroke::new(1.0, p.danger.gamma_multiply(0.6)));
                }
                if ui.add_enabled(!v.busy, button).clicked() {
                    clicked = Some(action.clone());
                }
            }
        });
        ui.add_space(4.0);
    }
    if v.actions.iter().any(|a| a.risky) {
        ui.weak("Risky actions ask for a second click.");
    }
    ui.weak("Bluetooth, output, reset and bootloader keys work only on the keyboard itself.");
    if let Some((label, result)) = &v.last {
        ui.weak(format!("Last: {label} — {result}"));
    }
    clicked
}

fn group_label(g: Group) -> &'static str {
    g.title()
}

/// An action's label without its group's name in front.
fn short_label(a: &Action) -> &str {
    a.label.strip_prefix(a.group.title()).and_then(|rest| rest.strip_prefix(": ")).unwrap_or(&a.label)
}

/// What the board drawing shows (#392): the board's shape from its profile, the left panel's
/// last frame, the art playing on the right, and the underglow's colour.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BoardView {
    pub drawing: Drawing,
    /// The left panel as the PC last drew it.
    pub panel: Option<super::screens::Frame>,
    /// The right screen's art, as stored (see `art_ui::thumbnail`).
    pub art: Option<std::sync::Arc<egui::ColorImage>>,
    /// The left half's underglow colour; `None` when it's off or not known yet.
    pub glow: Option<Color32>,
    pub left_label: String,
    pub right_label: String,
}

/// The board drawing's state from the cached keyboard state; nothing here asks the keyboard.
fn board_view(ctx: &egui::Context) -> BoardView {
    let drawing = super::boards::current().and_then(|b| b.drawing).unwrap_or_default();
    let snap = super::screens::snapshot();
    let panel = snap.hello.as_ref().and(snap.frame.clone()).filter(|_| !super::screens::paused() && !super::pause::paused());
    let (mut art, mut art_name) = (None, None);
    if let Some(h) = super::art_service::handle() {
        let s = h.snapshot();
        if s.connected {
            art_name = Some(super::art_ui::playing(&s).0);
            let library = super::art_library::snapshot();
            art = s.showing_file().and_then(|path| {
                super::art_ui::tiles(&library.entries).0.into_iter().find(|t| t.path == *path).map(|t| t.image)
            });
        }
    }
    let glow = ks::rgb_state(ctx).map(|s| s.left).filter(|l| l.on).map(|l| super::lights_page::shown_color(l.hue, l.sat, l.brightness));
    let link = super::connection::summary(ctx);
    let left_label = match link.usb {
        Some(true) => "LEFT · USB".to_string(),
        Some(false) => "LEFT · BLUETOOTH".to_string(),
        None => "LEFT".to_string(),
    };
    let right_label = art_name.map_or_else(|| "RIGHT".to_string(), |n| format!("RIGHT · {}", n.to_uppercase()));
    BoardView { drawing, panel, art, glow, left_label, right_label }
}

/// Where the board's clickable parts landed, for the click tests.
#[cfg(test)]
#[derive(Clone, Debug, Default)]
pub struct Hits {
    pub screens: Vec<egui::Rect>,
    pub glows: Vec<egui::Rect>,
    pub keys: Vec<egui::Rect>,
}

#[cfg(test)]
thread_local! {
    pub static LAST_HITS: std::cell::RefCell<Option<Hits>> = const { std::cell::RefCell::new(None) };
}

/// The screens' shapes as the device shows them (width, height): the left panel, and the right
/// screen's art upright (tall: the art is stored turned, see `art_ui::thumbnail`).
const PANEL_SHAPE: egui::Vec2 = egui::vec2(64.0, 52.0);
const ART_SHAPE: egui::Vec2 = egui::vec2(68.0, 140.0);
const SCREEN_HEIGHT: f32 = 72.0;

thread_local! {
    /// The board's screen textures: (the frame or picture they were made from, texture).
    static PANEL_TEXTURE: std::cell::RefCell<Option<(super::screens::Frame, egui::TextureHandle)>> = const { std::cell::RefCell::new(None) };
    static ART_TEXTURE: std::cell::RefCell<Option<(usize, egui::TextureHandle)>> = const { std::cell::RefCell::new(None) };
}

/// The board drawing (#392): both halves drawn from the board profile, each with its screen,
/// keys and underglow. A screen opens Screens and the underglow opens Lights.
pub fn board(ui: &mut egui::Ui, v: &BoardView) -> Option<Page> {
    let p = theme::palette(ui);
    let mut go = None;
    #[cfg(test)]
    let mut hits = Hits::default();
    theme::card(ui).inner_margin(egui::Margin::same(20)).show(ui, |ui| {
        ui.set_min_width(ui.available_width());
        let d = &v.drawing;
        let unit = KEY_UNIT;
        let (w, h) = d.size();
        let half = egui::vec2(w * unit, h * unit);
        let between = 56.0;
        let height = SCREEN_HEIGHT + 12.0 + half.y + 14.0 + 8.0 + 10.0 + 12.0;
        let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), height), Sense::hover());
        let left_x = rect.center().x - between / 2.0 - half.x;
        for (i, x) in [left_x, rect.center().x + between / 2.0].into_iter().enumerate() {
            let right = i == 1;
            let top = rect.top();
            // The screen sits over the inner columns, as on the Corne, shaped like what it shows.
            let shape = if right { ART_SHAPE } else { PANEL_SHAPE };
            let screen = egui::vec2((SCREEN_HEIGHT - 6.0) * shape.x / shape.y + 6.0, SCREEN_HEIGHT);
            let screen_x = if right { x + half.x * 0.25 } else { x + half.x * 0.75 };
            let screen_rect = egui::Rect::from_center_size(egui::pos2(screen_x, top + screen.y / 2.0), screen);
            #[cfg(test)]
            hits.screens.push(screen_rect);
            let click = ui.interact(screen_rect, ui.id().with(("board-screen", i)), Sense::click());
            ui.painter().rect_filled(screen_rect, CornerRadius::same(4), Color32::from_rgb(0x05, 0x07, 0x06));
            let inner = screen_rect.shrink(3.0);
            if right {
                match &v.art {
                    Some(stored) => {
                        let key = std::sync::Arc::as_ptr(stored) as usize;
                        let t = ART_TEXTURE.with_borrow_mut(|cache| match cache {
                            Some((k, t)) if *k == key => t.clone(),
                            _ => {
                                let image = super::art_ui::thumbnail(stored, false);
                                let t = ui.ctx().load_texture("board-art", image, egui::TextureOptions::NEAREST);
                                *cache = Some((key, t.clone()));
                                t
                            }
                        });
                        paint_fitted(ui.painter(), &t, inner);
                    }
                    // Nothing known: a dark screen, not a made-up picture.
                    None => {
                        ui.painter().text(inner.center(), egui::Align2::CENTER_CENTER, "ART", theme::mono(10.0), p.muted);
                    }
                }
            } else {
                match &v.panel {
                    Some(frame) => {
                        let t = PANEL_TEXTURE.with_borrow_mut(|cache| super::screens_ui::frame_texture(ui.ctx(), cache, frame).clone());
                        paint_fitted(ui.painter(), &t, inner);
                    }
                    None => {
                        ui.painter().text(inner.center(), egui::Align2::CENTER_CENTER, "PANEL", theme::mono(11.0), p.accent);
                    }
                }
            }
            let ring = if click.hovered() || click.has_focus() { 2.0 } else { 1.0 };
            ui.painter().rect_stroke(screen_rect, CornerRadius::same(4), Stroke::new(ring, p.accent), egui::StrokeKind::Inside);
            if click.on_hover_text("Open Screens").clicked() {
                go = Some(Page::Screens);
            }
            let keys_top = screen_rect.bottom() + 12.0;
            for k in d.keys(right) {
                let key = egui::Rect::from_min_size(
                    egui::pos2(x + k.min.x * unit, keys_top + k.min.y * unit),
                    k.size() * unit - egui::vec2(GAP, GAP),
                );
                ui.painter().rect(key, CornerRadius::same(4), p.control, Stroke::new(1.0, p.control_line), egui::StrokeKind::Inside);
                #[cfg(test)]
                hits.keys.push(key);
            }
            let glow_top = keys_top + half.y + 14.0;
            let glow = egui::Rect::from_center_size(egui::pos2(x + half.x / 2.0, glow_top + 4.0), egui::vec2(half.x + 20.0, 8.0));
            let glow_click = ui.interact(glow.expand2(egui::vec2(0.0, 6.0)), ui.id().with(("board-glow", i)), Sense::click());
            #[cfg(test)]
            hits.glows.push(glow);
            match v.glow {
                Some(c) => {
                    ui.painter().rect_filled(glow.expand(3.0), CornerRadius::same(6), c.gamma_multiply(0.25));
                    ui.painter().rect_filled(glow, CornerRadius::same(4), c);
                }
                None => {
                    ui.painter().rect(glow, CornerRadius::same(4), p.well, Stroke::new(1.0, p.control_line), egui::StrokeKind::Inside);
                }
            }
            if glow_click.hovered() || glow_click.has_focus() {
                ui.painter().rect_stroke(glow.expand(2.0), CornerRadius::same(5), Stroke::new(1.5, p.text_strong), egui::StrokeKind::Outside);
            }
            let hover = if v.glow.is_some() { "Open Lights" } else { "Lights off or not known yet. Open Lights" };
            if glow_click.on_hover_text(hover).clicked() {
                go = Some(Page::Lights);
            }
            ui.painter().text(
                egui::pos2(x + half.x / 2.0, glow.bottom() + 10.0),
                egui::Align2::CENTER_TOP,
                if right { &v.right_label } else { &v.left_label },
                theme::mono(11.0),
                p.muted,
            );
        }
    });
    #[cfg(test)]
    LAST_HITS.set(Some(hits));
    ui.add_space(2.0);
    ui.vertical_centered(|ui| ui.weak("Click a screen or the lights to jump to its page."));
    ui.add_space(6.0);
    go
}

/// Paints a screen's texture into `rect`, as large as fits with its shape kept.
fn paint_fitted(painter: &egui::Painter, t: &egui::TextureHandle, rect: egui::Rect) {
    let size = t.size_vec2();
    let scale = (rect.width() / size.x).min(rect.height() / size.y);
    let target = egui::Rect::from_center_size(rect.center(), size * scale);
    let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
    painter.image(t.id(), target, uv, Color32::WHITE);
}

/// Key size and gap on the board drawing, in points.
const KEY_UNIT: f32 = 31.0;
const GAP: f32 = 5.0;

#[cfg(test)]
pub mod tests {
    use super::*;
    use digitalis::protocol::BuildInfo;

    fn build(half: u8, sha: &str) -> BuildInfo {
        let version = digitalis::protocol::FirmwareVersion { major: 0, minor: 1, patch: 0, profile: 0 };
        BuildInfo { half, version, sha: sha.into() }
    }

    #[test]
    fn firmware_lines_in_plain_words() {
        let (halves, note, ask) = firmware_lines(&Firmware::Known {
            left: Ok(build(0, "e255d32")),
            right: Err("no reply from the keyboard".into()),
        });
        assert_eq!(halves[0].name, "Left");
        assert!(halves[0].text.ends_with("· e255d32"), "{}", halves[0].text);
        assert_eq!((halves[1].text.as_str(), halves[1].tone), ("no reply yet", Tone::Warn));
        assert_eq!(halves[1].hover.as_deref(), Some("no reply from the keyboard"));
        assert_eq!(note, None);
        assert!(ask);

        let (_, note, _) = firmware_lines(&Firmware::Known { left: Ok(build(0, "aaaaaaa")), right: Ok(build(1, "bbbbbbb")) });
        assert_eq!(note.map(|n| n.1), Some(Tone::Warn), "the halves differ");

        for f in [Firmware::Unknown, Firmware::Reading, Firmware::NoKeyboard, Firmware::NotReported] {
            let (halves, note, ask) = firmware_lines(&f);
            assert!(halves.is_empty() && note.is_some() && !ask, "{f:?}");
        }
    }

    /// The sample the previews draw: both halves seen, the right one not answering yet.
    pub fn sample() -> View {
        let (halves, firmware_note, can_ask_again) = firmware_lines(&Firmware::Known {
            left: Ok(build(0, "e255d32")),
            right: Err("no reply from the keyboard".into()),
        });
        let action = |label: &str, group| Action { label: label.into(), group, binding: (0, 0, 0), hold_ms: 0, risky: false };
        View {
            halves,
            firmware_note,
            can_ask_again,
            link: super::super::connection::Plain {
                rows: vec![
                    ("Left half", "on USB".into()),
                    ("Bluetooth to this PC", "connected".into()),
                    ("Bluetooth profile", "1 · this PC".into()),
                ],
                unplug_fix: None,
            },
            actions: vec![
                action("Lights: Toggle", Group::Lights),
                action("Lights: Next effect", Group::Lights),
                action("Lights: Brightness +", Group::Lights),
                action("Toggle NUMBER", Group::Layers),
                action("Go to FUNCTION", Group::Layers),
                action("Caps word", Group::Other),
            ],
            armed: None,
            busy: false,
            last: Some(("Lights: Toggle".into(), "done".into())),
            board: sample_board(),
        }
    }

    /// The board as the previews draw it: the Corne's profile, a panel frame, art, orange lights.
    pub fn sample_board() -> BoardView {
        let corne = super::super::boards::find(0x1d50, 0x615e, "Eyelash Corne").and_then(|b| b.drawing);
        let (art, library) = super::super::art_ui::tests::sample();
        let image = art.showing_file().and_then(|p| {
            super::super::art_ui::tiles(&library.entries).0.into_iter().find(|t| t.path == *p).map(|t| t.image)
        });
        BoardView {
            drawing: corne.unwrap_or_default(),
            panel: super::super::theme_preview::sample_screens().frame,
            art: image,
            glow: Some(Color32::from_rgb(0xf0, 0x8a, 0x3c)),
            left_label: "LEFT · USB".into(),
            right_label: "RIGHT · HEARTH".into(),
        }
    }

    /// No keyboard: what the page says with nothing connected.
    pub fn sample_none() -> View {
        let (halves, firmware_note, can_ask_again) = firmware_lines(&Firmware::NoKeyboard);
        View {
            halves,
            firmware_note,
            can_ask_again,
            link: super::super::connection::Plain {
                rows: vec![("Left half", "not connected".into()), ("Bluetooth to this PC", "not known yet".into())],
                unplug_fix: None,
            },
            board: BoardView { left_label: "LEFT".into(), right_label: "RIGHT".into(), ..Default::default() },
            ..Default::default()
        }
    }

    /// A frame of the board drawing with `events`; returns the page a click asked for.
    fn board_frame(ctx: &egui::Context, v: &BoardView, events: Vec<egui::Event>) -> Option<Page> {
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(900.0, 500.0))),
            events,
            ..Default::default()
        };
        let mut go = None;
        let mut out = ctx.run_ui(raw, |ui| go = board(ui, v));
        // Nothing paints these frames; egui wants its texture changes acknowledged.
        out.textures_delta.clear();
        go
    }

    fn click(ctx: &egui::Context, v: &BoardView, at: egui::Pos2) -> Option<Page> {
        let button = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        board_frame(ctx, v, vec![egui::Event::PointerMoved(at), button(true)]);
        board_frame(ctx, v, vec![button(false)])
    }

    #[test]
    fn clicks_on_the_board_open_their_pages() {
        let ctx = egui::Context::default();
        let mut fonts = egui::FontDefinitions::default();
        theme::add_fonts(&mut fonts);
        ctx.set_fonts(fonts);
        theme::apply_choice(&ctx, theme::Choice::Dark);
        let v = sample_board();
        assert_eq!(board_frame(&ctx, &v, vec![]), None);
        let hits = LAST_HITS.with_borrow(|h| h.clone()).expect("the board was laid out");
        for (i, screen) in hits.screens.iter().enumerate() {
            assert_eq!(click(&ctx, &v, screen.center()), Some(Page::Screens), "screen {i}");
        }
        for (i, glow) in hits.glows.iter().enumerate() {
            assert_eq!(click(&ctx, &v, glow.center()), Some(Page::Lights), "underglow {i}");
        }
        // A key opens nothing.
        assert_eq!(click(&ctx, &v, hits.keys[0].center()), None);
    }

    #[test]
    fn actions_drop_their_group_name() {
        let a = |label: &str, group| Action { label: label.into(), group, binding: (0, 0, 0), hold_ms: 0, risky: false };
        assert_eq!(short_label(&a("Lights: Toggle", Group::Lights)), "Toggle");
        assert_eq!(short_label(&a("Toggle NUMBER", Group::Layers)), "Toggle NUMBER");
        assert_eq!(short_label(&a("Lightsaber", Group::Lights)), "Lightsaber");
    }

    #[test]
    fn no_jargon_on_the_page() {
        for v in [sample(), sample_none()] {
            let mut words: Vec<String> = v.halves.iter().map(|h| h.text.clone()).collect();
            words.extend(v.firmware_note.map(|n| n.0));
            words.extend(v.link.rows.iter().map(|(a, b)| format!("{a} {b}")));
            for w in words {
                let w = w.to_lowercase();
                for jargon in ["endpoint", "active profile", "unknown (", "blob"] {
                    assert!(!w.contains(jargon), "{w}");
                }
            }
        }
    }
}
