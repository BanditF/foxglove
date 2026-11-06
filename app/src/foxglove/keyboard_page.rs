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

use super::key_layout::KeyLayout;
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
    /// The part's short name: "Left", "Keyboard" (#559).
    pub name: String,
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
    /// The board's parts, for the words that name them (#559).
    pub parts: super::parts::Parts,
    pub actions: Vec<Action>,
    /// The risky action waiting for its second click.
    pub armed: Option<String>,
    /// An action is being sent.
    pub busy: bool,
    pub last: Option<(String, String)>,
    /// The board drawing at the top (#392).
    pub board: BoardView,
    /// The low-battery heads-up is on (#372).
    pub low_battery_notice: bool,
    /// "Update firmware" and its steps (#382).
    pub update: super::firmware_update::View,
    /// The other devices' batteries (#504).
    pub devices: Vec<super::peripherals::Peripheral>,
    /// Their low-battery heads-up is on (#504).
    pub device_notice: bool,
    /// "Read Logitech batteries" is on (#505).
    pub logitech: bool,
    /// The focused keyboard (#800): muted cards with a read-only chip for one Foxglove can't
    /// change. `None` before any keyboard was found.
    pub focus: Option<super::focus_ui::Focused>,
}

/// What a click on the page asks for.
#[derive(Clone, Debug, PartialEq)]
pub enum Intent {
    AskFirmwareAgain,
    Action(Action),
    /// Turns the low-battery heads-up on or off (#372).
    LowBatteryNotice(bool),
    /// A click in "Update firmware" (#382).
    Update(super::firmware_update::Intent),
    /// Turns the other devices' low-battery heads-up on or off (#504).
    DeviceNotice(bool),
    /// Turns "Read Logitech batteries" on or off (#505).
    Logitech(bool),
}

/// The firmware card's lines for the cached builds, one per part (#559): a one-piece board has
/// one line and nothing to disagree with.
pub fn firmware_lines(
    f: &Firmware,
    parts: &super::parts::Parts,
) -> (Vec<Half>, Option<(String, Tone)>, bool) {
    match f {
        Firmware::Known { left, right } => {
            let parts = parts.clone().seen(if right.is_ok() { 2 } else { 1 });
            let half = |name, r: &Result<foxglove_link::protocol::BuildInfo, String>| match r {
                Ok(b) => Half {
                    name,
                    text: format!("{} · {}", b.version, b.sha),
                    tone: Tone::Plain,
                    hover: None,
                },
                Err(e) => Half {
                    name,
                    text: "no reply yet".into(),
                    tone: Tone::Warn,
                    hover: Some(e.clone()),
                },
            };
            if !parts.is_split() {
                return (vec![half(parts.short(0), left)], None, true);
            }
            let note = match (left, right) {
                (Ok(l), Ok(r)) if l.version != r.version || l.sha != r.sha => Some((
                    format!(
                        "The {} run different firmware. Flash both from the same build.",
                        if parts.noun() == "half" {
                            "halves"
                        } else {
                            "parts"
                        }
                    ),
                    Tone::Warn,
                )),
                _ => None,
            };
            (
                vec![half(parts.short(0), left), half(parts.short(1), right)],
                note,
                true,
            )
        }
        Firmware::Unknown | Firmware::Reading => (
            Vec::new(),
            Some(("Asking the keyboard…".into(), Tone::Muted)),
            false,
        ),
        Firmware::NoKeyboard => (
            Vec::new(),
            Some((
                "No keyboard with Foxglove's firmware is connected.".into(),
                Tone::Muted,
            )),
            false,
        ),
        Firmware::NotReported => (
            Vec::new(),
            Some((
                "This keyboard's firmware doesn't report its version.".into(),
                Tone::Muted,
            )),
            false,
        ),
    }
}

/// A risky action clicked once, waiting for the confirming second click.
static CONFIRMING: Mutex<Option<(String, Instant)>> = Mutex::new(None);
const CONFIRM_WITHIN: Duration = Duration::from_secs(4);

fn armed() -> Option<String> {
    let confirming = CONFIRMING.lock().unwrap_or_else(|e| e.into_inner());
    confirming
        .as_ref()
        .filter(|(_, at)| at.elapsed() < CONFIRM_WITHIN)
        .map(|(label, _)| label.clone())
}

/// The page's state from the cached keyboard state (the services keep it current). A focused
/// keyboard Foxglove can't change (#800) gets [`read_only`]'s cards; the batteries of the other
/// devices stay.
pub fn view(ctx: &egui::Context) -> View {
    let focus = super::focus_ui::Focused::now();
    if let Some(f) = focus.clone().filter(|f| f.read_only.is_some()) {
        let link = super::focus_ui::Switcher::now(None)
            .focused()
            .map(|r| r.detail.clone())
            .unwrap_or_default();
        return View {
            devices: super::peripherals::list(),
            device_notice: super::battery::device_notices_on(),
            logitech: super::battery::logitech_on(),
            ..read_only(f, BoardView::default(), link)
        };
    }
    let parts = super::parts::Parts::current();
    let (halves, firmware_note, can_ask_again) = firmware_lines(&ks::firmware(ctx), &parts);
    View {
        halves,
        firmware_note,
        can_ask_again,
        link: super::connection::plain(ctx),
        parts,
        actions: keyboard_actions::actions(),
        armed: armed(),
        busy: ks::invoking(),
        last: ks::last_action(),
        board: BoardView::default(),
        low_battery_notice: super::battery::notices_on(),
        update: super::firmware_update::view(),
        devices: super::peripherals::list(),
        device_notice: super::battery::device_notices_on(),
        logitech: super::battery::logitech_on(),
        focus,
    }
}

/// The firmware card's line for a keyboard Foxglove can't change.
pub const READ_ONLY_FIRMWARE: &str = "Foxglove reads the firmware of keyboards running its own.";
/// The key actions card's line for one.
pub const READ_ONLY_ACTIONS: &str = "Key actions need Foxglove's firmware on the keyboard.";

/// The page for a focused keyboard Foxglove can't change (#800): the same cards, muted, each
/// with the read-only chip; `board` its drawing ([`read_only_board`]), `link` how it was found
/// ("VIA · USB · read-only").
pub fn read_only(focus: super::focus_ui::Focused, board: BoardView, link: String) -> View {
    View {
        firmware_note: Some((READ_ONLY_FIRMWARE.into(), Tone::Muted)),
        link: super::connection::Plain {
            rows: vec![("Found over".into(), link)],
            unplug_fix: None,
        },
        parts: super::parts::Parts::of(None),
        board,
        focus: Some(focus),
        ..Default::default()
    }
}

/// The board drawing of a focused keyboard Foxglove can't change: its own layout (B1's ladder,
/// `keyboard_focus::layout`), with no screens or underglow of Foxglove's firmware.
pub fn read_only_board(focus: &super::focus_ui::Focused, layout: KeyLayout) -> BoardView {
    BoardView {
        split: layout.parts.len() > 1,
        layout,
        left_label: "KEYBOARD".into(),
        right_label: String::new(),
        title: focus.name.clone(),
        read_only: focus.read_only.clone(),
        ..Default::default()
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
    super::firmware_update::update_dialog(ui.ctx());
    // #504: the controllers' levels stay fresh while the Batteries card is on screen.
    super::peripherals::visible(ui.ctx());
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
        Some(Intent::LowBatteryNotice(on)) => {
            if let Err(e) = super::battery::set_notices(on) {
                eprintln!("Foxglove: can't save the battery setting: {e}");
            }
        }
        Some(Intent::Update(i)) => super::firmware_update::act(i, ui.ctx()),
        Some(Intent::DeviceNotice(on)) => {
            if let Err(e) = super::battery::set_device_notices(on) {
                eprintln!("Foxglove: can't save the battery setting: {e}");
            }
        }
        Some(Intent::Logitech(on)) => {
            if let Err(e) = super::battery::set_logitech(on) {
                eprintln!("Foxglove: can't save the battery setting: {e}");
            }
            super::peripherals::hidpp_setting_changed();
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
    let read_only = v.focus.as_ref().and_then(|f| f.read_only.as_deref());
    if let Some(why) = read_only {
        ui.columns(2, |cols| {
            for (col, title, line) in [
                (0, "Firmware", READ_ONLY_FIRMWARE),
                (1, "Key actions", READ_ONLY_ACTIONS),
            ] {
                card(&mut cols[col], title, |ui| {
                    super::focus_ui::read_only_chip(ui, why);
                    ui.add_space(2.0);
                    ui.weak(line);
                });
            }
        });
        ui.add_space(10.0);
        card(ui, "Connection", |ui| {
            for (what, how) in &v.link.rows {
                row(ui, what, how.as_str());
            }
        });
        ui.add_space(10.0);
        if let Some(i) = devices(ui, v) {
            intent = Some(i);
        }
        ui.add_space(6.0);
        return intent;
    }
    ui.columns(2, |cols| {
        card(&mut cols[0], "Firmware", |ui| {
            for h in &v.halves {
                // A build in the mono face; words ("no reply yet") in the body face.
                let mut value = RichText::new(&h.text).color(tone_color(ui, h.tone));
                if h.tone == Tone::Plain {
                    value = value.font(theme::mono(12.5));
                }
                let r = row(ui, &h.name, value);
                if let Some(why) = &h.hover {
                    r.on_hover_text(why);
                }
            }
            if let Some((note, tone)) = &v.firmware_note {
                ui.label(RichText::new(note).color(tone_color(ui, *tone)));
            }
            ui.add_space(4.0);
            if v.can_ask_again {
                ui.horizontal(|ui| {
                    if ui
                        .button("Refresh")
                        .on_hover_text(ask_again(&v.parts))
                        .clicked()
                    {
                        intent = Some(Intent::AskFirmwareAgain);
                    }
                });
            }
            // #382: "Update firmware…", then its confirm step and progress, in this card.
            if let Some(i) = super::firmware_update::draw(ui, &v.update) {
                intent = Some(Intent::Update(i));
            }
            // #372: a Windows notification below 15% and 5%, once each until it charges.
            let mut on = v.low_battery_notice;
            let (label, hover) = low_battery_words(&v.parts);
            if ui.checkbox(&mut on, label).on_hover_text(hover).changed() {
                intent = Some(Intent::LowBatteryNotice(on));
            }
        });
        card(&mut cols[1], "Key actions", |ui| {
            if let Some(a) = actions(ui, v) {
                intent = Some(Intent::Action(a));
            }
        });
    });
    ui.add_space(10.0);
    card(ui, "Connection", |ui| {
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
    ui.add_space(10.0);
    if let Some(i) = devices(ui, v) {
        intent = Some(i);
    }
    ui.add_space(6.0);
    intent
}

/// "Ask both halves for their firmware again", "Ask the keyboard for its firmware again".
fn ask_again(parts: &super::parts::Parts) -> String {
    let theirs = if parts.is_split() { "their" } else { "its" };
    format!("Ask {} for {theirs} firmware again", parts.all())
}

/// The low-battery heads-up's switch and hover, in the board's words (#559).
fn low_battery_words(parts: &super::parts::Parts) -> (String, String) {
    if parts.is_split() {
        let noun = parts.noun();
        (
            format!("Tell me when a {noun}'s battery runs low"),
            format!("One notification below 15% and one below 5%. They come back after the {noun} charges past 20%."),
        )
    } else {
        (
            "Tell me when the keyboard's battery runs low".into(),
            "One notification below 15% and one below 5%. They come back after it charges past 20%.".into(),
        )
    }
}

/// Foxglove (#504): the Batteries card, the batteries of what else is connected to this PC.
/// Named for what it shows (#533): "devices" on the Lights page means devices with lights.
fn devices(ui: &mut egui::Ui, v: &View) -> Option<Intent> {
    let mut intent = None;
    card(ui, "Batteries", |ui| {
        let p = theme::palette(ui);
        ui.label(
            RichText::new(if v.logitech {
                "Batteries of the Bluetooth devices, game controllers and Logitech devices \
                 connected to this PC."
            } else {
                "Batteries of the Bluetooth devices and game controllers connected to this PC."
            })
            .color(p.muted),
        );
        ui.add_space(4.0);
        if v.devices.is_empty() {
            ui.label(RichText::new("No connected device reports its battery.").color(p.muted));
        }
        for d in &v.devices {
            let tone = if d.percent < super::battery::THRESHOLDS[0] && !d.charging {
                Tone::Warn
            } else {
                Tone::Plain
            };
            // #804: its thumbnail, the shape from its Bluetooth category, lights unknown.
            ui.horizontal(|ui| {
                use super::device_thumb::{self as thumb, BtCategory, Thumb};
                let category = d.category.unwrap_or(BtCategory::Other);
                thumb::show(ui, thumb::SMALL, &Thumb::bluetooth(category));
                row(
                    ui,
                    &d.name,
                    RichText::new(super::peripherals::level_text(d)).color(tone_color(ui, tone)),
                )
                .on_hover_text(d.kind.label());
            });
        }
        ui.add_space(4.0);
        let mut on = v.device_notice;
        if ui
            .checkbox(&mut on, "Tell me when a device's battery runs low")
            .on_hover_text(if v.parts.noun() == "half" {
                "One notification below 15% and one below 5%, as for the halves."
            } else {
                "One notification below 15% and one below 5%, as for the keyboard."
            })
            .changed()
        {
            intent = Some(Intent::DeviceNotice(on));
        }
        let mut logitech = v.logitech;
        if ui
            .checkbox(&mut logitech, "Read Logitech batteries")
            .on_hover_text(
                "Ask Logitech mice, keyboards and headsets (on a receiver, a cable or \
                 Bluetooth) for their battery: when one connects, and at most once a minute \
                 while this list is open. Off: Foxglove doesn't open them.",
            )
            .changed()
        {
            intent = Some(Intent::Logitech(logitech));
        }
    });
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
                let text = if armed {
                    format!("Click again: {name}")
                } else {
                    name.to_string()
                };
                let p = theme::palette(ui);
                let mut button = egui::Button::new(text);
                if armed {
                    button = button
                        .fill(p.selected_fill)
                        .stroke(Stroke::new(1.0, p.warn));
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
    a.label
        .strip_prefix(a.group.title())
        .and_then(|rest| rest.strip_prefix(": "))
        .unwrap_or(&a.label)
}

/// What the board drawing shows (#392): the board's keys from its one layout (#796: the
/// keyboard's own, else its profile's, else a guess), the left panel's last frame, the art
/// playing on the right, and the underglow's colour.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BoardView {
    pub layout: KeyLayout,
    /// A split: the layout's first part is the left half, its second the right. Else one piece
    /// (#559).
    pub split: bool,
    /// The left panel as the PC last drew it.
    pub panel: Option<super::screens::Frame>,
    /// The right screen's art, as stored (see `art_ui::thumbnail`).
    pub art: Option<std::sync::Arc<egui::ColorImage>>,
    /// The left half's underglow colour; `None` when it's off or not known yet.
    pub glow: Option<Color32>,
    /// The left half plays an effect that makes its own colours (#670): the strip is a
    /// rainbow, not `glow`'s stored colour.
    pub glow_rainbow: bool,
    pub left_label: String,
    pub right_label: String,
    /// The focused keyboard's name, over the drawing (#800); empty draws no title.
    pub title: String,
    /// A keyboard Foxglove can't change: the read-only chip's hover. Its drawing has no screens
    /// or underglow.
    pub read_only: Option<String>,
    /// The screens drawn on the board, from its own screens and profile ([`board_screens`],
    /// #822); none for a board without screens.
    pub screens: Vec<BoardScreen>,
}

/// A screen the board drawing shows (#822): what it is for, the part it sits on, where along
/// that part (0 its left edge, 1 its right; `None` over the inner columns) and its shape
/// upright (#560).
#[derive(Clone, Debug, PartialEq)]
pub struct BoardScreen {
    pub role: super::screen_roles::Role,
    pub part: usize,
    pub along: Option<f32>,
    pub shape: egui::Vec2,
}

/// Where a one-piece board's profile puts a screen (its `half`): "left", "middle", "right".
fn along(place: &str) -> Option<f32> {
    match place {
        "left" => Some(0.25),
        "middle" | "center" | "centre" => Some(0.5),
        "right" => Some(0.75),
        _ => None,
    }
}

/// The screens drawn for the host-link keyboard when nothing says (no profile, none
/// described): the older firmware's, on its board's parts (#563); a part the drawing doesn't
/// have draws none.
fn old_firmware_screens() -> Vec<BoardScreen> {
    let old = super::boards::old_firmware_board();
    board_screens(
        &super::screen_roles::old_firmware(),
        old,
        &super::parts::Parts::of(old),
    )
}

/// The screens the board drawing shows (#822), from the board's screens (the firmware's, else
/// its profile's: `screen_roles`) and where its profile puts each (`half`: the part on a split,
/// the place along a one-piece board). The drawing shows the info and the art screen, the first
/// of each, so a board with neither draws no screens.
pub fn board_screens(
    screens: &[super::screen_roles::Screen],
    board: Option<&super::boards::Board>,
    parts: &super::parts::Parts,
) -> Vec<BoardScreen> {
    use super::screen_roles::Role;
    let split = parts.is_split();
    [Role::Info, Role::Art]
        .into_iter()
        .filter_map(|role| {
            let s = super::screen_roles::first(screens, role)?;
            let place = board.and_then(|b| {
                b.screens
                    .iter()
                    .find(|p| Role::of_kind(&p.kind) == Some(role))
                    .map(|p| p.half.as_str())
            });
            let (w, h) = s.upright();
            let shape = if w > 0 && h > 0 {
                egui::vec2(f32::from(w), f32::from(h))
            } else {
                fallback_shape(role)
            };
            let default_part = usize::from(split && role == Role::Art);
            Some(BoardScreen {
                role,
                part: if split {
                    place.and_then(|p| parts.index(p)).unwrap_or(default_part)
                } else {
                    0
                },
                along: place.filter(|_| !split).and_then(along),
                shape,
            })
        })
        .collect()
}

/// The board drawing's state from the cached keyboard state; nothing here asks the keyboard. A
/// focused keyboard Foxglove can't change is drawn from its own layout (#800).
fn board_view(ctx: &egui::Context) -> BoardView {
    let focused = super::keyboard_focus::current();
    let focus = focused.as_ref().map(super::focus_ui::Focused::of);
    if let (Some(k), Some(f)) = (&focused, &focus) {
        if f.read_only.is_some() {
            return read_only_board(f, super::keyboard_focus::layout(k));
        }
    }
    let board = super::boards::current();
    let layout = super::key_layout::current();
    let parts = super::parts::Parts::of(board.as_ref());
    let snap = super::screens::snapshot();
    let panel = snap
        .hello
        .as_ref()
        .and(snap.frame.clone())
        .filter(|_| !super::screens::paused() && !super::pause::paused());
    let (mut art, mut art_name) = (None, None);
    if let Some(h) = super::art_service::handle() {
        let s = h.snapshot();
        if s.connected {
            let (name, image) = super::art_ui::showing_now(&s, &super::art_library::snapshot());
            art_name = Some(name);
            art = image;
        }
    }
    let rgb = ks::rgb_state(ctx);
    let lit = rgb.map(|s| s.left).filter(|l| l.on);
    let glow = lit.map(|l| super::lights_page::shown_color(l.hue, l.sat, l.brightness));
    // A colour Swirl (#626) glows in its colour; a rainbow effect glows as a rainbow.
    let glow_rainbow = lit.is_some_and(|l| {
        super::effect_controls::underglow_in(rgb.as_ref(), l.effect)
            .is_some_and(super::lights_page::KeyboardEffect::rainbow)
    });
    let link = super::connection::summary(ctx);
    let first = parts.short(0).to_uppercase();
    let left_label = match link.usb {
        Some(true) => format!("{first} · USB"),
        Some(false) => format!("{first} · BLUETOOTH"),
        None => first,
    };
    let second = parts.short(1).to_uppercase();
    let right_label = art_name.map_or_else(
        || second.clone(),
        |n| format!("{second} · {}", n.to_uppercase()),
    );
    let screens = match super::screen_roles::current() {
        Some(s) => board_screens(&s, board.as_ref(), &parts),
        None => old_firmware_screens(),
    };
    BoardView {
        screens,
        layout,
        split: parts.is_split(),
        panel,
        art,
        glow,
        glow_rainbow,
        left_label,
        right_label,
        title: focus.map(|f| f.name).unwrap_or_default(),
        read_only: None,
    }
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

/// A screen's shape when the keyboard doesn't say (width, height): the older firmware's
/// screen of that role, upright (its board's profile, #563); square if none is known.
fn fallback_shape(role: super::screen_roles::Role) -> egui::Vec2 {
    super::screen_roles::old_firmware_size(role)
        .filter(|(w, h)| *w > 0 && *h > 0)
        .map_or(egui::vec2(1.0, 1.0), |(w, h)| {
            egui::vec2(f32::from(w), f32::from(h))
        })
}

const SCREEN_HEIGHT: f32 = 72.0;

thread_local! {
    /// The board's screen textures: (the frame or picture they were made from, texture).
    static PANEL_TEXTURE: std::cell::RefCell<Option<(super::screens::Frame, egui::TextureHandle)>> = const { std::cell::RefCell::new(None) };
    static ART_TEXTURE: std::cell::RefCell<Option<(usize, egui::TextureHandle)>> = const { std::cell::RefCell::new(None) };
}

/// The board drawing (#392): both halves drawn from the keyboard's layout (#796), each with its
/// screen, keys (turned where the layout turns them) and underglow, and the chip saying where
/// the layout came from. A screen opens Screens and the underglow opens Lights.
pub fn board(ui: &mut egui::Ui, v: &BoardView) -> Option<Page> {
    let p = theme::palette(ui);
    let mut go = None;
    #[cfg(test)]
    let mut hits = Hits::default();
    theme::card(ui)
        .inner_margin(egui::Margin::same(20))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            let layout = &v.layout;
            if !v.title.is_empty() {
                // The focused keyboard's name (#800).
                ui.label(theme::heading(ui, &v.title))
                    .on_hover_text(super::focus_ui::TITLE_HOVER);
                ui.add_space(4.0);
            }
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                super::key_layout::source_chip(ui, layout);
                if let Some(why) = &v.read_only {
                    super::focus_ui::read_only_chip(ui, why);
                }
            });
            if let Some(note) = layout.source.note() {
                ui.weak(note);
            }
            ui.add_space(6.0);
            let unit = KEY_UNIT;
            let half = layout.size * unit;
            let between = 56.0;
            // A keyboard without Foxglove's firmware has no screens or underglow to show.
            let firmware = v.read_only.is_none();
            let drawn = if v.split { 2 } else { 1 };
            // Only screens on a part the drawing has; a board without screens draws its keys
            // at the top (#822).
            let shown: Vec<&BoardScreen> = v.screens.iter().filter(|s| s.part < drawn).collect();
            let band = if shown.is_empty() {
                0.0
            } else {
                SCREEN_HEIGHT + 12.0
            };
            let height = if firmware {
                band + half.y + 14.0 + 8.0 + 10.0 + 12.0
            } else {
                half.y + 14.0 + 12.0
            };
            let (rect, _) =
                ui.allocate_exact_size(egui::vec2(ui.available_width(), height), Sense::hover());
            let left_x = rect.center().x - between / 2.0 - half.x;
            // A split's halves side by side; a one-piece board's whole drawing in the middle.
            let places = if v.split {
                vec![left_x, rect.center().x + between / 2.0]
            } else {
                vec![rect.center().x - half.x / 2.0]
            };
            let mut screen_no = 0;
            for (i, x) in places.into_iter().enumerate() {
                let right = i == 1;
                let top = rect.top();
                if !firmware {
                    for k in layout.part(i) {
                        let key = egui::Rect::from_min_size(
                            egui::pos2(x + k.rect.min.x * unit, top + k.rect.min.y * unit),
                            k.rect.size() * unit - egui::vec2(GAP, GAP),
                        );
                        ui.painter().add(
                            egui::epaint::RectShape::new(
                                key,
                                CornerRadius::same(4),
                                p.control,
                                Stroke::new(1.0, p.control_line),
                                egui::StrokeKind::Inside,
                            )
                            .with_angle(k.r.to_radians()),
                        );
                        #[cfg(test)]
                        hits.keys.push(key);
                    }
                    if i == 0 {
                        ui.painter().text(
                            egui::pos2(x + half.x / 2.0, top + half.y + 2.0),
                            egui::Align2::CENTER_TOP,
                            &v.left_label,
                            theme::mono(11.0),
                            p.muted,
                        );
                    }
                    continue;
                }
                let mut here: Vec<&BoardScreen> =
                    shown.iter().copied().filter(|s| s.part == i).collect();
                here.sort_by(|a, b| a.along.unwrap_or(0.5).total_cmp(&b.along.unwrap_or(0.5)));
                // Each shaped like what it shows (#560).
                let sizes: Vec<egui::Vec2> = here
                    .iter()
                    .map(|s| {
                        egui::vec2(
                            (SCREEN_HEIGHT - 6.0) * s.shape.x / s.shape.y + 6.0,
                            SCREEN_HEIGHT,
                        )
                    })
                    .collect();
                // One screen sits where the profile puts it, else over the inner columns, as on
                // the Corne; more sit side by side over the part's middle.
                let centres: Vec<f32> = if let [only] = here.as_slice() {
                    let inner = if right { 0.25 } else { 0.75 };
                    vec![x + half.x * only.along.unwrap_or(inner)]
                } else {
                    let gap = 12.0;
                    let total = sizes.iter().fold(-gap, |t, s| t + s.x + gap);
                    let mut at = x + half.x / 2.0 - total / 2.0;
                    sizes
                        .iter()
                        .map(|s| {
                            let c = at + s.x / 2.0;
                            at += s.x + gap;
                            c
                        })
                        .collect()
                };
                for ((s, size), cx) in here.iter().zip(&sizes).zip(centres) {
                    let screen_rect =
                        egui::Rect::from_center_size(egui::pos2(cx, top + size.y / 2.0), *size);
                    #[cfg(test)]
                    hits.screens.push(screen_rect);
                    let click = ui.interact(
                        screen_rect,
                        ui.id().with(("board-screen", screen_no)),
                        Sense::click(),
                    );
                    screen_no += 1;
                    if screen(ui, v, s.role, screen_rect, click) {
                        go = Some(Page::Screens);
                    }
                }
                let keys_top = top + band;
                for k in layout.part(i) {
                    let key = egui::Rect::from_min_size(
                        egui::pos2(x + k.rect.min.x * unit, keys_top + k.rect.min.y * unit),
                        k.rect.size() * unit - egui::vec2(GAP, GAP),
                    );
                    ui.painter().add(
                        egui::epaint::RectShape::new(
                            key,
                            CornerRadius::same(4),
                            p.control,
                            Stroke::new(1.0, p.control_line),
                            egui::StrokeKind::Inside,
                        )
                        .with_angle(k.r.to_radians()),
                    );
                    #[cfg(test)]
                    hits.keys.push(key);
                }
                let glow_top = keys_top + half.y + 14.0;
                let glow = egui::Rect::from_center_size(
                    egui::pos2(x + half.x / 2.0, glow_top + 4.0),
                    egui::vec2(half.x + 20.0, 8.0),
                );
                let glow_click = ui.interact(
                    glow.expand2(egui::vec2(0.0, 6.0)),
                    ui.id().with(("board-glow", i)),
                    Sense::click(),
                );
                #[cfg(test)]
                hits.glows.push(glow);
                match v.glow {
                    Some(_) if v.glow_rainbow => {
                        super::effect_controls::paint_rainbow(ui.painter(), glow, true);
                    }
                    Some(c) => {
                        ui.painter().rect_filled(
                            glow.expand(3.0),
                            CornerRadius::same(6),
                            c.gamma_multiply(0.25),
                        );
                        ui.painter().rect_filled(glow, CornerRadius::same(4), c);
                    }
                    None => {
                        ui.painter().rect(
                            glow,
                            CornerRadius::same(4),
                            p.well,
                            Stroke::new(1.0, p.control_line),
                            egui::StrokeKind::Inside,
                        );
                    }
                }
                if glow_click.hovered() || glow_click.has_focus() {
                    ui.painter().rect_stroke(
                        glow.expand(2.0),
                        CornerRadius::same(5),
                        Stroke::new(1.5, p.text_strong),
                        egui::StrokeKind::Outside,
                    );
                }
                let hover = if v.glow.is_some() {
                    "Open Lights"
                } else {
                    "Lights off or not known yet. Open Lights"
                };
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
    if v.read_only.is_none() {
        ui.vertical_centered(|ui| ui.weak("Click a screen or the lights to jump to its page."));
    }
    ui.add_space(6.0);
    go
}

/// Paints one of the board's screens into `screen_rect`: the panel's last frame or the art
/// playing, else the role's placeholder; returns whether it was clicked.
fn screen(
    ui: &egui::Ui,
    v: &BoardView,
    role: super::screen_roles::Role,
    screen_rect: egui::Rect,
    click: egui::Response,
) -> bool {
    let p = theme::palette(ui);
    ui.painter().rect_filled(
        screen_rect,
        CornerRadius::same(4),
        Color32::from_rgb(0x05, 0x07, 0x06),
    );
    let inner = screen_rect.shrink(3.0);
    if role == super::screen_roles::Role::Art {
        match &v.art {
            Some(stored) => {
                let key = std::sync::Arc::as_ptr(stored) as usize;
                let t = ART_TEXTURE.with_borrow_mut(|cache| match cache {
                    Some((k, t)) if *k == key => t.clone(),
                    _ => {
                        let image = super::art_ui::thumbnail(stored, false);
                        let t = ui.ctx().load_texture(
                            "board-art",
                            image,
                            egui::TextureOptions::NEAREST,
                        );
                        *cache = Some((key, t.clone()));
                        t
                    }
                });
                paint_fitted(ui.painter(), &t, inner);
            }
            // Nothing known: a dark screen, not a made-up picture.
            None => {
                ui.painter().text(
                    inner.center(),
                    egui::Align2::CENTER_CENTER,
                    "ART",
                    theme::mono(10.0),
                    p.muted,
                );
            }
        }
    } else {
        match &v.panel {
            Some(frame) => {
                let t = PANEL_TEXTURE.with_borrow_mut(|cache| {
                    super::screens_ui::frame_texture(ui.ctx(), cache, frame).clone()
                });
                paint_fitted(ui.painter(), &t, inner);
            }
            None => {
                ui.painter().text(
                    inner.center(),
                    egui::Align2::CENTER_CENTER,
                    "PANEL",
                    theme::mono(11.0),
                    p.accent,
                );
            }
        }
    }
    let ring = if click.hovered() || click.has_focus() {
        2.0
    } else {
        1.0
    };
    ui.painter().rect_stroke(
        screen_rect,
        CornerRadius::same(4),
        Stroke::new(ring, p.accent),
        egui::StrokeKind::Inside,
    );
    click.on_hover_text("Open Screens").clicked()
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
    use foxglove_link::protocol::BuildInfo;

    fn build(half: u8, sha: &str) -> BuildInfo {
        let version = foxglove_link::protocol::FirmwareVersion {
            major: 0,
            minor: 1,
            patch: 0,
            profile: 0,
        };
        BuildInfo {
            half,
            version,
            sha: sha.into(),
        }
    }

    fn corne() -> crate::foxglove::parts::Parts {
        crate::foxglove::fixtures::users_desk::board().parts()
    }

    #[test]
    fn firmware_lines_in_plain_words() {
        let (halves, note, ask) = firmware_lines(
            &Firmware::Known {
                left: Ok(build(0, "e255d32")),
                right: Err("no reply from the keyboard".into()),
            },
            &corne(),
        );
        assert_eq!(halves[0].name, "Left");
        assert!(halves[0].text.ends_with("· e255d32"), "{}", halves[0].text);
        assert_eq!(
            (halves[1].text.as_str(), halves[1].tone),
            ("no reply yet", Tone::Warn)
        );
        assert_eq!(
            halves[1].hover.as_deref(),
            Some("no reply from the keyboard")
        );
        assert_eq!(note, None);
        assert!(ask);

        let (_, note, _) = firmware_lines(
            &Firmware::Known {
                left: Ok(build(0, "aaaaaaa")),
                right: Ok(build(1, "bbbbbbb")),
            },
            &corne(),
        );
        assert_eq!(
            note,
            Some((
                "The halves run different firmware. Flash both from the same build.".into(),
                Tone::Warn
            )),
            "the halves differ"
        );

        for f in [
            Firmware::Unknown,
            Firmware::Reading,
            Firmware::NoKeyboard,
            Firmware::NotReported,
        ] {
            let (halves, note, ask) = firmware_lines(&f, &corne());
            assert!(halves.is_empty() && note.is_some() && !ask, "{f:?}");
        }
    }

    /// #559: the second desk's one-piece board runs the Keyboard page with no half anywhere:
    /// one firmware line, the connection rows, the low-battery switch and the drawing.
    #[test]
    fn a_one_piece_board_has_no_halves_on_the_page() {
        use crate::foxglove::fixtures::second_desk;
        let plank = second_desk::board().parts();
        let (lines, note, ask) = firmware_lines(
            &Firmware::Known {
                left: Ok(build(0, "e255d32")),
                right: Err("no right half on this board".into()),
            },
            &plank,
        );
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].name, "Keyboard");
        assert!(note.is_none() && ask);
        let mut words: Vec<String> = lines
            .iter()
            .map(|h| format!("{} {}", h.name, h.text))
            .collect();
        let (label, hover) = low_battery_words(&plank);
        words.extend([label, hover, ask_again(&plank)]);
        for w in &words {
            assert!(!w.to_lowercase().contains("half"), "{w}");
        }
        // The Corne's words stay.
        assert_eq!(
            low_battery_words(&corne()).0,
            "Tell me when a half's battery runs low"
        );
        assert_eq!(
            ask_again(&corne()),
            "Ask both halves for their firmware again"
        );
        // The drawing: one piece, its whole board in the middle of the card.
        let view = BoardView {
            layout: crate::foxglove::key_layout::from_drawing(
                &second_desk::board().drawing.unwrap(),
                false,
                crate::foxglove::key_layout::LayoutSource::Profile,
            ),
            split: false,
            left_label: "KEYBOARD · USB".into(),
            // #560: its 128x32 info screen, drawn in its own shape; #822: with its art screen,
            // where its profile puts them.
            screens: board_screens(
                &crate::foxglove::screen_roles::of_board(&second_desk::board()),
                Some(&second_desk::board()),
                &plank,
            ),
            ..Default::default()
        };
        let ctx = crate::foxglove::pc_lights_ui::tests::test_ctx();
        board_frame(&ctx, &view, vec![]);
        let hits = LAST_HITS.take().unwrap();
        assert_eq!(hits.keys.len(), second_desk::LAMPS, "61 keys, drawn once");
        assert_eq!(hits.screens.len(), 2, "its info and art screens");
        let screen = hits.screens[0];
        assert!(
            screen.width() > screen.height() * 3.0,
            "a 4:1 info screen, not the Corne's: {screen:?}"
        );
    }

    /// #822: the board's screens come from its own screens and profile. A one-piece VIA board
    /// (no profile, nothing described) and a profile without screens draw none, with the keys
    /// at the top; the Corne keeps its info screen on the left half and its art screen on the
    /// right; a profile that puts both on one part gets them side by side.
    #[test]
    fn screen_placeholders_come_from_the_board() {
        use crate::foxglove::parts::Parts;
        use crate::foxglove::screen_roles::{of_board, Role};
        use crate::foxglove::via_host::{layout_of, tests::k5_pro_opened};
        let ctx = crate::foxglove::pc_lights_ui::tests::test_ctx();

        let (via, layout) = k5_pro_opened(1);
        let via =
            crate::foxglove::key_layout::resolve(None, &[layout_of(&via, &layout).unwrap()], 0);
        let one_piece = BoardView {
            layout: via,
            split: false,
            screens: board_screens(&[], None, &Parts::of(None)),
            left_label: "KEYBOARD".into(),
            ..Default::default()
        };
        assert!(one_piece.screens.is_empty());
        board_frame(&ctx, &one_piece, vec![]);
        let hits = LAST_HITS.take().unwrap();
        assert!(
            hits.screens.is_empty(),
            "no placeholder on a board without screens"
        );
        let top = |keys: &[egui::Rect]| keys.iter().map(|k| k.top()).fold(f32::MAX, f32::min);
        let bare_top = top(&hits.keys);
        // No empty band where a screen would be: with a screen the keys sit a band lower.
        board_frame(
            &ctx,
            &BoardView {
                screens: sample_board().screens,
                ..one_piece.clone()
            },
            vec![],
        );
        let hits = LAST_HITS.take().unwrap();
        assert_eq!(
            hits.screens.len(),
            1,
            "a one-piece drawing has no right half"
        );
        assert!((top(&hits.keys) - bare_top - (SCREEN_HEIGHT + 12.0)).abs() < 0.01);
        // Even with the Corne's sample screens around, a view built for it carries none.
        let spread = BoardView {
            split: false,
            screens: Vec::new(),
            ..sample_board()
        };
        board_frame(&ctx, &spread, vec![]);
        assert!(LAST_HITS.take().unwrap().screens.is_empty());

        // A profile without screens, or with only a screen its firmware draws itself.
        let mut bare = crate::foxglove::fixtures::second_desk::board();
        bare.screens.retain(|s| s.kind == "display");
        assert!(board_screens(&of_board(&bare), Some(&bare), &bare.parts()).is_empty());
        bare.screens.clear();
        assert!(board_screens(&of_board(&bare), Some(&bare), &bare.parts()).is_empty());

        // The Corne: the info screen over the left half's inner columns, the art screen upright
        // over the right's, as before.
        let corne = crate::foxglove::fixtures::users_desk::board();
        let screens = board_screens(&of_board(&corne), Some(&corne), &corne.parts());
        assert_eq!(
            screens,
            [
                BoardScreen {
                    role: Role::Info,
                    part: 0,
                    along: None,
                    shape: egui::vec2(64.0, 52.0),
                },
                BoardScreen {
                    role: Role::Art,
                    part: 1,
                    along: None,
                    shape: egui::vec2(68.0, 140.0),
                },
            ]
        );
        board_frame(&ctx, &sample_board(), vec![]);
        let hits = LAST_HITS.take().unwrap();
        assert_eq!(hits.screens.len(), 2);
        let (left, right) = (hits.screens[0], hits.screens[1]);
        assert!(left.center().x < right.center().x);
        assert!(left.width() > left.height() && right.width() < right.height());

        // A split whose profile puts both screens on its left half: side by side there.
        let mut both_left = corne.clone();
        both_left.screens[1].half = "left".into();
        let screens = board_screens(&of_board(&both_left), Some(&both_left), &both_left.parts());
        assert!(screens.iter().all(|s| s.part == 0), "{screens:?}");
        board_frame(
            &ctx,
            &BoardView {
                screens,
                ..sample_board()
            },
            vec![],
        );
        let hits = LAST_HITS.take().unwrap();
        assert_eq!(hits.screens.len(), 2);
        assert!(!hits.screens[0].intersects(hits.screens[1]));
        let right_keys = hits.keys[21..]
            .iter()
            .map(|k| k.left())
            .fold(f32::MAX, f32::min);
        assert!(hits.screens.iter().all(|s| s.center().x < right_keys));
    }

    /// The sample the previews draw: both halves seen, the right one not answering yet.
    pub fn sample() -> View {
        let (halves, firmware_note, can_ask_again) = firmware_lines(
            &Firmware::Known {
                left: Ok(build(0, "e255d32")),
                right: Err("no reply from the keyboard".into()),
            },
            &corne(),
        );
        let action = |label: &str, group| Action {
            label: label.into(),
            group,
            binding: (0, 0, 0),
            hold_ms: 0,
            risky: false,
            behavior: "",
        };
        View {
            halves,
            firmware_note,
            can_ask_again,
            link: super::super::connection::Plain {
                rows: vec![
                    ("Left half".into(), "on USB".into()),
                    ("Bluetooth to this PC".into(), "connected".into()),
                    ("Bluetooth slot".into(), "1 · this PC".into()),
                ],
                unplug_fix: None,
            },
            parts: corne(),
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
            low_battery_notice: true,
            update: super::super::firmware_update::tests::sample(),
            devices: super::super::peripherals::tests::sample(),
            device_notice: true,
            logitech: false,
            focus: None,
        }
    }

    /// The board as the previews draw it: the Corne's profile, a panel frame, art, orange lights.
    pub fn sample_board() -> BoardView {
        let corne = crate::foxglove::fixtures::users_desk::board();
        let (art, library) = super::super::art_ui::tests::sample();
        let (_, image) = super::super::art_ui::showing_now(&art, &library);
        BoardView {
            layout: crate::foxglove::key_layout::resolve(Some(&corne), &[], 0),
            split: true,
            panel: super::super::theme_preview::sample_screens().frame,
            art: image,
            glow: Some(Color32::from_rgb(0xf0, 0x8a, 0x3c)),
            glow_rainbow: false,
            left_label: "LEFT · USB".into(),
            right_label: "RIGHT · HEARTH".into(),
            title: String::new(),
            read_only: None,
            screens: board_screens(
                &crate::foxglove::screen_roles::of_board(&corne),
                Some(&corne),
                &corne.parts(),
            ),
        }
    }

    /// No keyboard: what the page says with nothing connected.
    pub fn sample_none() -> View {
        let (halves, firmware_note, can_ask_again) =
            firmware_lines(&Firmware::NoKeyboard, &corne());
        View {
            halves,
            firmware_note,
            can_ask_again,
            link: super::super::connection::Plain {
                rows: vec![
                    ("Left half".into(), "not connected".into()),
                    ("Bluetooth to this PC".into(), "not known yet".into()),
                ],
                unplug_fix: None,
            },
            parts: corne(),
            board: BoardView {
                left_label: "LEFT".into(),
                right_label: "RIGHT".into(),
                // What `board_view` draws with nothing connected.
                screens: old_firmware_screens(),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    /// #796 AC 1: with the keyboard's Studio layout, the page draws each half's keys where the
    /// overlay draws them (the same offsets between keys, at the page's key size), turned, with
    /// the "Layout from the keyboard" chip.
    #[test]
    fn a_studio_layout_draws_on_the_page_as_on_the_overlay() {
        use crate::foxglove::key_layout::tests::{corne_from_keyboard, corne_studio};
        let overlay = crate::protocols::zmk::physical_keys(&corne_studio().layouts[0]);
        let layout = corne_from_keyboard();
        let view = BoardView {
            layout: layout.clone(),
            ..sample_board()
        };
        let ctx = crate::foxglove::pc_lights_ui::tests::test_ctx();
        board_frame(&ctx, &view, vec![]);
        let hits = LAST_HITS.take().unwrap();
        assert_eq!(hits.keys.len(), 42);
        let mut drawn = hits.keys.iter();
        for part in &layout.parts {
            let keys: Vec<&egui::Rect> = drawn.by_ref().take(part.len()).collect();
            let o = |i: usize| egui::pos2(overlay[part[i].index].x, overlay[part[i].index].y);
            for i in 0..part.len() {
                let page = keys[i].min - keys[0].min;
                let theirs = (o(i) - o(0)) * KEY_UNIT;
                assert!(
                    (page - theirs).length() < 0.01,
                    "key {}: {page:?} vs {theirs:?}",
                    part[i].index
                );
                let size = egui::vec2(overlay[part[i].index].w, overlay[part[i].index].h);
                assert!(
                    (keys[i].size() - (size * KEY_UNIT - egui::vec2(GAP, GAP))).length() < 0.01
                );
            }
        }
        // Without it, the profile's keys exactly as before (#392).
        board_frame(&ctx, &sample_board(), vec![]);
        let hits = LAST_HITS.take().unwrap();
        let d = crate::foxglove::fixtures::users_desk::board()
            .drawing
            .unwrap();
        let first = hits.keys[0].min;
        for (k, want) in hits.keys.iter().zip(d.keys(false)) {
            assert!(
                ((k.min - first) - (want.min - d.keys(false)[0].min) * KEY_UNIT).length() < 0.01
            );
        }
    }

    /// A frame of the board drawing with `events`; returns the page a click asked for.
    fn board_frame(ctx: &egui::Context, v: &BoardView, events: Vec<egui::Event>) -> Option<Page> {
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(900.0, 500.0),
            )),
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
        let hits = LAST_HITS
            .with_borrow(|h| h.clone())
            .expect("the board was laid out");
        for (i, screen) in hits.screens.iter().enumerate() {
            assert_eq!(
                click(&ctx, &v, screen.center()),
                Some(Page::Screens),
                "screen {i}"
            );
        }
        for (i, glow) in hits.glows.iter().enumerate() {
            assert_eq!(
                click(&ctx, &v, glow.center()),
                Some(Page::Lights),
                "underglow {i}"
            );
        }
        // A key opens nothing.
        assert_eq!(click(&ctx, &v, hits.keys[0].center()), None);
    }

    #[test]
    fn actions_drop_their_group_name() {
        let a = |label: &str, group| Action {
            label: label.into(),
            group,
            binding: (0, 0, 0),
            hold_ms: 0,
            risky: false,
            behavior: "",
        };
        assert_eq!(short_label(&a("Lights: Toggle", Group::Lights)), "Toggle");
        assert_eq!(
            short_label(&a("Toggle NUMBER", Group::Layers)),
            "Toggle NUMBER"
        );
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
