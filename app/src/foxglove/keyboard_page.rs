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
    let go = board(ui);
    #[cfg(test)]
    if let Some(v) = PREVIEW.with_borrow(|v| v.clone()) {
        draw(ui, &v);
        return go;
    }
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

/// The space for the board drawing (#392 replaces it with the drawing from the board
/// profile): both halves, each with its screen, keys and underglow. A screen opens Screens and
/// the underglow opens Lights.
pub fn board(ui: &mut egui::Ui) -> Option<Page> {
    let p = theme::palette(ui);
    let mut go = None;
    theme::card(ui).inner_margin(egui::Margin::same(20)).show(ui, |ui| {
        ui.set_min_width(ui.available_width());
        const KEY: egui::Vec2 = egui::vec2(26.0, 24.0);
        const GAP: f32 = 5.0;
        let keys_w = 6.0 * KEY.x + 5.0 * GAP;
        let half_w = keys_w;
        let between = 56.0;
        let height = 58.0 + 10.0 + 3.0 * KEY.y + 2.0 * GAP + 12.0 + 8.0 + 10.0 + 12.0;
        let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), height), Sense::hover());
        let left_x = rect.center().x - between / 2.0 - half_w;
        for (i, x) in [left_x, rect.center().x + between / 2.0].into_iter().enumerate() {
            let right = i == 1;
            let top = rect.top();
            let screen = egui::Rect::from_center_size(egui::pos2(x + half_w / 2.0, top + 29.0), egui::vec2(120.0, 58.0));
            let id = ui.id().with(("board-screen", i));
            let screen_click = ui.interact(screen, id, Sense::click());
            let painter = ui.painter();
            painter.rect_filled(screen, CornerRadius::same(4), Color32::from_rgb(0x05, 0x07, 0x06));
            if right {
                dither(painter, screen.shrink(3.0));
            } else {
                painter.text(
                    screen.center(),
                    egui::Align2::CENTER_CENTER,
                    "LEFT PANEL",
                    theme::mono(11.0),
                    p.accent,
                );
            }
            let ring = if screen_click.hovered() || screen_click.has_focus() { 2.0 } else { 1.0 };
            painter.rect_stroke(screen, CornerRadius::same(4), Stroke::new(ring, p.accent), egui::StrokeKind::Inside);
            if screen_click.on_hover_text("Open Screens").clicked() {
                go = Some(Page::Screens);
            }
            let keys_top = screen.bottom() + 10.0;
            for r in 0..3 {
                for c in 0..6 {
                    let at = egui::pos2(x + c as f32 * (KEY.x + GAP), keys_top + r as f32 * (KEY.y + GAP));
                    ui.painter().rect(
                        egui::Rect::from_min_size(at, KEY),
                        CornerRadius::same(4),
                        p.control,
                        Stroke::new(1.0, p.control_line),
                        egui::StrokeKind::Inside,
                    );
                }
            }
            let glow_top = keys_top + 3.0 * KEY.y + 2.0 * GAP + 12.0;
            let glow = egui::Rect::from_center_size(egui::pos2(x + half_w / 2.0, glow_top + 4.0), egui::vec2(half_w + 20.0, 8.0));
            let glow_click = ui.interact(glow.expand2(egui::vec2(0.0, 6.0)), ui.id().with(("board-glow", i)), Sense::click());
            let glow_ring = glow_click.hovered() || glow_click.has_focus();
            ui.painter().rect_filled(glow, CornerRadius::same(4), p.accent);
            if glow_ring {
                ui.painter().rect_stroke(glow.expand(2.0), CornerRadius::same(5), Stroke::new(1.5, p.text_strong), egui::StrokeKind::Outside);
            }
            if glow_click.on_hover_text("Open Lights").clicked() {
                go = Some(Page::Lights);
            }
            ui.painter().text(
                egui::pos2(x + half_w / 2.0, glow.bottom() + 10.0),
                egui::Align2::CENTER_TOP,
                if right { "RIGHT" } else { "LEFT · CENTRAL" },
                theme::mono(11.0),
                p.muted,
            );
        }
    });
    ui.add_space(2.0);
    ui.vertical_centered(|ui| ui.weak("Click a screen or the lights to jump to its page."));
    ui.add_space(6.0);
    go
}

/// A 1-bit checker standing in for the right screen's art, the way the device lights pixels.
fn dither(painter: &egui::Painter, rect: egui::Rect) {
    const CELL: f32 = 2.0;
    let cols = (rect.width() / CELL) as usize;
    let rows = (rect.height() / CELL) as usize;
    let lit = Color32::from_rgb(0xf2, 0xf2, 0xf2);
    for r in 0..rows {
        for c in (r % 2..cols).step_by(2) {
            let at = rect.min + egui::vec2(c as f32 * CELL, r as f32 * CELL);
            painter.rect_filled(egui::Rect::from_min_size(at, egui::vec2(CELL, CELL)), 0.0, lit);
        }
    }
}

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
            ..Default::default()
        }
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
