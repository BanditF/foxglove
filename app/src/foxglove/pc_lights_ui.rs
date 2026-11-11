//! The Lights page's "Other devices" card (#423): each lighting device Foxglove found, how
//! it's controlled and what it can do (chips), Identify, a colour per zone, Off and "Match
//! keyboard", and "Add a device…" for gear the catalog doesn't know.
//!
//! The controls follow the device's capabilities: zone pickers only for per-zone or per-LED
//! colour, swatches and Match only for devices that take a colour, Off and Identify only for
//! lights that switch. A Windows Dynamic Lighting device (#691) gets the same card once
//! Foxglove drives it: a picker per lamp for 2 to 12 lamps (more is a per-key board: one
//! "All lamps" row, per-key colour lives in Scenes), and one muted line on what Windows lets
//! Foxglove do with it.
//!
//! "Use OpenRGB's devices" (#471, off by default) adds the controllers a running OpenRGB
//! drives as rows like any other, "Through OpenRGB"; its host and port sit under the toggle.
//!
//! Since #795 each device is one row, collapsed by default and remembered for the session: its
//! name, how it's controlled, its state as flat chips and Identify; zones, colours, Calibrate…
//! and Off open under it. OpenRGB and Add a device sit in a collapsed "More sources". Every
//! sentence the card had is still there, on screen or one hover or click away.
//!
//! [`show`] reads the worker's snapshot and sends clicks to it; [`draw`] paints a [`View`] and
//! returns [`Intent`]s, so the previews and tests need no hardware. Colours are picked first
//! and written on Apply: dragging a picker sends nothing.

use std::collections::BTreeMap;
use std::sync::Mutex;

use egui::{Color32, RichText, Sense, Stroke};
use foxglove_devices::pc_lighting::{
    self as pc_lighting, openrgb, CatalogDevice, Control, Protocol,
};

use super::other_keyboards::{self as others, Keyboards};
use super::pc_lights::{self, Msg, Place, Row, Snapshot, Status, Tone};
use super::pc_lights_add::{self as add, Wizard};
use super::theme;

/// What the card shows: the worker's snapshot. The focused keyboard isn't listed: it has the
/// page's Keyboard card (#533); the others are, with "Show in Foxglove" (#820).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct View {
    pub pc: Snapshot,
    /// Each LampArray's kind and lamp positions by VID/PID, for the thumbnails (#804).
    pub lamps: BTreeMap<(u16, u16), pc_lights::LampLayout>,
    /// The keyboards the focus knows (#820).
    pub keyboards: Keyboards,
}

/// What a click asks for.
#[derive(Clone, Debug, PartialEq)]
pub enum Intent {
    /// Zone index and colour, the zones the user changed.
    Apply {
        key: String,
        colors: Vec<(usize, [u8; 3])>,
    },
    Off(String),
    Identify(String),
    Match(String, bool),
    /// Add a device: blink one zone (only ever from a click).
    TestZone {
        vid: u16,
        pid: u16,
        path: String,
        protocol: Protocol,
        zone: u8,
    },
    Save(Box<CatalogDevice>),
    Remove(u16, u16),
    /// Put text on the clipboard (a catalog entry to share).
    Copy(String),
    /// Open the colour calibrator on a device (by key).
    Calibrate(String),
    /// Open it at a picked colour on a zone, to add a match for it.
    CalibrateColor(String, Option<String>, [u8; 3]),
    /// New OpenRGB settings (#471): the toggle, or a host and port to connect to.
    OpenRgb(openrgb::Settings),
    /// Connect to OpenRGB again at the same address.
    OpenRgbConnect,
    /// Open Windows' Dynamic Lighting settings (another app has a device's lights, #795 O10).
    OpenDynamicLighting,
    /// "Show in Foxglove": make this keyboard (by its focus key) the focus (#820).
    Show(String),
}

/// The colours being picked per device (by key), until Apply.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Draft {
    pub colors: BTreeMap<String, Vec<Option<[u8; 3]>>>,
    /// The device whose "Remove" was clicked in its More menu, waiting for the second click.
    pub removing: Option<String>,
    /// OpenRGB's host and port as being typed, until Connect.
    pub openrgb: Option<(String, String)>,
}

impl Draft {
    /// The changed zones of a device, as Apply sends them.
    pub fn changes(&self, row: &Row) -> Vec<(usize, [u8; 3])> {
        let Some(draft) = self.colors.get(&row.key) else {
            return Vec::new();
        };
        draft
            .iter()
            .zip(&row.zones)
            .enumerate()
            .filter_map(|(i, (d, (_, now)))| d.filter(|d| Some(*d) != *now).map(|d| (i, d)))
            .collect()
    }
}

static DRAFT: Mutex<Option<(Draft, Wizard)>> = Mutex::new(None);

#[cfg(test)]
thread_local! {
    /// The previews' card and wizard, so they never touch real devices.
    pub static PREVIEW: std::cell::RefCell<Option<(View, Wizard)>> = const { std::cell::RefCell::new(None) };
}

/// Drops a pending "Remove" confirm: the page was left.
pub fn forget_remove() {
    if let Some((draft, _)) = DRAFT.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        draft.removing = None;
    }
}

/// The card, and what its clicks do.
pub fn show(ui: &mut egui::Ui) {
    #[cfg(test)]
    if let Some((v, mut wizard)) = PREVIEW.with_borrow(|v| v.clone()) {
        draw(ui, &v, &mut Draft::default(), &mut wizard);
        return;
    }
    pc_lights::page_shown(ui.ctx());
    let pc = pc_lights::snapshot();
    let view = View {
        keyboards: Keyboards::now(&pc.devices),
        pc,
        lamps: pc_lights::lamp_layouts(),
    };
    let mut guard = DRAFT.lock().unwrap_or_else(|e| e.into_inner());
    let (draft, wizard) = guard.get_or_insert_with(Default::default);
    for intent in draw(ui, &view, draft, wizard) {
        act(ui.ctx(), intent, &view.pc.devices, draft);
    }
}

/// The worker request a row's Apply, Off or Identify is: the keyboard card sends the same for a
/// focused LampArray keyboard's key (#825). `None` for the rest.
pub fn row_msg(intent: &Intent) -> Option<Msg> {
    match intent {
        Intent::Apply { key, colors } => Some(Msg::Set {
            key: key.clone(),
            colors: colors.clone(),
        }),
        Intent::Off(key) => Some(Msg::Off(key.clone())),
        Intent::Identify(key) => Some(Msg::Identify(key.clone())),
        _ => None,
    }
}

/// What a click asks of the worker (or the window), with `devices` the rows it may name and
/// `draft` the colours being picked: Other devices' rows, and the keyboard card for a focused
/// LampArray keyboard (#825). Queues requests only; no device is asked here.
pub fn act(ctx: &egui::Context, intent: Intent, devices: &[Row], draft: &mut Draft) {
    if let Some(msg) = row_msg(&intent) {
        if let Intent::Apply { key, .. } | Intent::Off(key) = &intent {
            draft.colors.remove(key);
        }
        pc_lights::send(msg);
        return;
    }
    match intent {
        Intent::Apply { .. } | Intent::Off(_) | Intent::Identify(_) => {}
        Intent::Match(key, on) => {
            // What the keyboard last reported, should the worker have none yet.
            if let (true, Some(k)) = (on, keyboard_shows()) {
                // The accent the keyboard is sent may not be what it shows (#616): say
                // which one the case follows.
                if let Some(accent) = super::accent::followed() {
                    eprintln!(
                        "Foxglove: Match keyboard on for {key}: following the keyboard's \
                             report (effect {}, hue {}, sat {}, brightness {}), not the accent \
                             {} it is sent",
                        k.effect,
                        k.hue,
                        k.sat,
                        k.brightness,
                        pc_lights::hex(accent)
                    );
                }
                pc_lights::send(Msg::Keyboard(k));
            }
            pc_lights::send(Msg::Match(key, on));
        }
        Intent::TestZone {
            vid,
            pid,
            path,
            protocol,
            zone,
        } => pc_lights::send(Msg::TestZone {
            vid,
            pid,
            path,
            protocol,
            zone,
        }),
        Intent::Save(device) => pc_lights::send(Msg::SaveLocal(device)),
        Intent::Remove(vid, pid) => pc_lights::send(Msg::RemoveLocal(vid, pid)),
        Intent::Copy(text) => ctx.copy_text(text),
        Intent::Calibrate(key) => {
            if let Some(row) = devices.iter().find(|r| r.key == key) {
                super::calibrate_ui::open(super::calibrate_ui::lights_target(row));
            }
        }
        Intent::CalibrateColor(key, zone, color) => {
            if let Some(row) = devices.iter().find(|r| r.key == key) {
                let target = super::calibrate_ui::lights_target(row);
                super::calibrate_ui::open_at(target, zone, Some(color));
            }
        }
        Intent::OpenRgb(settings) => {
            draft.openrgb = None;
            pc_lights::send(Msg::OpenRgbSettings(settings));
        }
        Intent::OpenRgbConnect => pc_lights::send(Msg::OpenRgbConnect),
        Intent::OpenDynamicLighting => {
            super::notifications_ui::open_settings_page(super::lights_owner::DL_SETTINGS)
        }
        // The switcher's own pick: C1 saves and announces it, and the pages follow.
        Intent::Show(key) => super::focus_ui::pick(ctx, &key),
    }
}

/// Off's hover, on a row and on the keyboard card (#825).
pub const OFF_TIP: &str = "Turn every zone dark";

/// Identify's hover, on a row and on the keyboard card (#825).
pub const IDENTIFY_TIP: &str = "Blink this device for a moment, then put its colours back";

#[cfg(test)]
thread_local! {
    /// Where the keyboard card's lights buttons landed this frame, by label, for the click
    /// tests (#825).
    pub static LAST_CARD_BUTTONS: std::cell::RefCell<Vec<(&'static str, egui::Rect)>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// A button of the keyboard card's lights, noted for the click tests.
fn card_button(
    ui: &mut egui::Ui,
    label: &'static str,
    enabled: bool,
    button: egui::Button,
) -> egui::Response {
    let r = ui.add_enabled(enabled, button);
    #[cfg(test)]
    LAST_CARD_BUTTONS.with_borrow_mut(|b| b.push((label, r.rect)));
    #[cfg(not(test))]
    let _ = label;
    r
}

/// The keyboard card's lights for a focused keyboard Foxglove drives through Windows Dynamic
/// Lighting (#825): its row's own controls, so the clicks are the row's requests for its key
/// ([`row_msg`]): the colour for all lamps (a picker per lamp for 2 to 12), Apply colours, Off,
/// Identify and Calibrate…. Muted, never blank, while Foxglove doesn't drive it (Windows lists
/// it but Foxglove hasn't opened it): the header's Windows chip says why.
pub fn card_lights(
    ui: &mut egui::Ui,
    row: &Row,
    busy: bool,
    draft: &mut Draft,
    out: &mut Vec<Intent>,
) {
    let driven = driven(row);
    ui.add_enabled_ui(driven && !busy, |ui| {
        let colours = row.caps.takes_color() && zones(ui, row, draft, true, out);
        let changes = draft.changes(row);
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            if colours {
                let apply = theme::primary_button(ui, "Apply colours");
                if card_button(ui, "Apply colours", !changes.is_empty(), apply).clicked() {
                    out.push(Intent::Apply {
                        key: row.key.clone(),
                        colors: changes.clone(),
                    });
                }
            }
            if row.caps.switchable() {
                let off =
                    card_button(ui, "Off", true, egui::Button::new("Off")).on_hover_text(OFF_TIP);
                if off.clicked() {
                    out.push(Intent::Off(row.key.clone()));
                }
                let identify = card_button(ui, "Identify", true, egui::Button::new("Identify"))
                    .on_hover_text(IDENTIFY_TIP);
                if identify.clicked() {
                    out.push(Intent::Identify(row.key.clone()));
                }
            }
            if colours && calibrate_button(ui).clicked() {
                out.push(Intent::Calibrate(row.key.clone()));
            }
        });
    });
}

/// What "Match keyboard" starts from: the keyboard's lights as it last reported them
/// (RGB_STATE), never the colour Foxglove meant to send it, such as the accent the keymap
/// can't take (#616).
fn keyboard_shows() -> Option<pc_lights::KeyboardLights> {
    let left = super::keyboard_settings::cached_rgb_state()?.left;
    Some(pc_lights::KeyboardLights::from_half(&left))
}

/// What the control kind is called on the page.
pub fn control_text(c: Control) -> &'static str {
    match c {
        Control::Driver(_) => "Built-in support",
        Control::LampArray => "Windows lighting",
        Control::NotYet => "Not controllable yet",
        Control::OpenRgb => "Through OpenRGB",
    }
}

/// What a device Foxglove can't drive yet does instead (#614): it keeps its own effect, named
/// when the catalog lists one (the first of its `effects` is the one it shows on its own).
pub fn not_yet_text(effects: &[String]) -> String {
    match effects.first().map(|e| e.trim()).filter(|e| !e.is_empty()) {
        Some(effect) => {
            format!("Lights not controllable yet: it keeps its own effect ({effect}).")
        }
        None => "Lights not controllable yet: it keeps its own effect.".into(),
    }
}

/// The zone chip's hover on a device Foxglove can't drive yet (#614): its zones are how its
/// lights are divided, not something Foxglove sets.
pub const ZONES_NOT_YET_TIP: &str =
    "How the device's lights are divided. Foxglove can't set them yet.";

/// The zone chip's (and the open row's light line's) hover.
pub fn zones_tip(row: &Row) -> &'static str {
    match row.control {
        Control::NotYet => ZONES_NOT_YET_TIP,
        _ => super::light_zones::TIP,
    }
}

/// What the card is for: its title's hover (#795 O1).
pub const ABOUT: &str =
    "Case lights, screens and other lighting devices on this PC, and what each can do.";

/// The labels of the switches whose ⓘ follows them.
pub const MATCH_SHORT: &str = "Match keyboard";
pub const OPENRGB_SHORT: &str = "Use OpenRGB's devices";

/// "Match keyboard" hover (#616), on the checkbox and its ⓘ.
pub const MATCH_TIP: &str =
    "Show the colour the keyboard's lights show, as the keyboard reports it. \
                         While the keyboard plays a rainbow (Spectrum, Swirl) there's no colour to \
                         copy: this device keeps its colours and the line below says so. It \
                         doesn't go dark when the keyboard dims. Picking a colour here stops it.";

/// "Use OpenRGB's devices" hover (#471).
pub const OPENRGB_TIP: &str = "List the devices a running OpenRGB drives (memory, graphics cards, \
                               motherboards, fans) here, so Foxglove and its scenes can light \
                               them. Turn on OpenRGB's SDK server first.";

/// The toggle's hover while it's off (#795 O24).
pub const OPENRGB_OFF: &str = "Off: Foxglove doesn't connect to OpenRGB.";

/// What a typed OpenRGB address must be (#795 O26: the ⓘ of "Host or port not valid").
pub const OPENRGB_ADDRESS: &str = "Host: an IP address or localhost; port: 1 to 65535.";

/// The rare sources' expander (#795 O23–O30): OpenRGB and Add a device, collapsed by default.
pub const MORE_SOURCES: &str = "pc-more-sources";

/// A device row's expander key (#795): its own, so each row is remembered on its own.
pub fn row_key(device: &str) -> String {
    format!("pc-device:{device}")
}

/// Paints the card; returns what was asked for this frame.
///
/// The casual viewer sees each device's name, its state as chips and Identify, one row each
/// (collapsed by default, remembered for the session); zones, colours, Calibrate… and Off are
/// one click away. Conflicts stay on screen as one amber bar each, and the rare sources
/// (OpenRGB, Add a device) sit in a collapsed "More sources" (docs/design/
/// device-focus-layouts-thumbnails-lights-declutter.md, rows O1–O31).
pub fn draw(ui: &mut egui::Ui, v: &View, draft: &mut Draft, wizard: &mut Wizard) -> Vec<Intent> {
    let mut out = Vec::new();
    let s = &v.pc;
    super::settings_ui::card_with_hover(ui, "Other devices", ABOUT, |ui| {
        let p = theme::palette(ui);
        for c in &s.conflicts {
            ui.add_space(2.0);
            conflict_bar(ui, c);
        }
        // A result with nowhere narrower to go (your own catalog file that can't be saved): the
        // top of the card, where amber goes.
        if let Some(st) = placed(&s.results, &Place::Card) {
            ui.add_space(2.0);
            status_here(ui, st, None);
        }
        // The keyboards that aren't the focus and have no device row, first (#820); the focused
        // one's own row isn't listed again.
        let keyboards = v.keyboards.own_rows(&s.devices);
        let listed: Vec<&Row> = s.devices.iter().filter(|r| v.keyboards.listed(r)).collect();
        if listed.is_empty() && keyboards.is_empty() {
            ui.add_space(4.0);
            ui.label(RichText::new("No PC or peripheral lights found.").color(p.muted));
        }
        for k in keyboards {
            ui.add_space(4.0);
            let detail = v.keyboards.detail(k);
            if others::row(ui, k, &detail, v.keyboards.layouts.get(&k.key)) {
                out.push(Intent::Show(k.key.clone()));
            }
        }
        for row in listed {
            ui.add_space(4.0);
            let status = placed(&s.results, &Place::Row(row.key.clone()));
            let lamps = super::scene_room::vid_pid(&row.key).and_then(|k| v.lamps.get(&k));
            let show = v.keyboards.show_on(row);
            device(ui, row, status, lamps, show, s.busy, draft, &mut out);
        }
        ui.add_space(4.0);
        ui.separator();
        more_sources(ui, s, draft, wizard, &mut out);
    });
    out
}

/// `place`'s own result while it's shown: a confirmation fades after
/// [`pc_lights::DONE_FOR`], a note or a problem stays until the next result for that place.
pub fn placed<'a>(results: &'a [Status], place: &Place) -> Option<&'a Status> {
    results.iter().find(|st| &st.place == place && st.shown())
}

/// What a result says where it shows: without the device's name when the row already shows
/// it ("HP OMEN 45L case lights: colours set" on that row is "Colours set"), and at most
/// [`theme::BAR_MAX_CHARS`] characters, cut at a word with "…" (the whole of it is the hover).
pub fn status_short(text: &str, name: Option<&str>) -> String {
    let text = name
        .and_then(|n| text.strip_prefix(n))
        .and_then(|t| t.strip_prefix(": "))
        .unwrap_or(text);
    let mut chars = text.chars();
    let first = chars.next().map(|c| c.to_uppercase().collect::<String>());
    let text = format!("{}{}", first.unwrap_or_default(), chars.as_str());
    if text.chars().count() <= theme::BAR_MAX_CHARS {
        return text;
    }
    let room: String = text.chars().take(theme::BAR_MAX_CHARS - 1).collect();
    let cut = room.rfind(' ').filter(|&i| i > 0).unwrap_or(room.len());
    format!("{}…", room[..cut].trim_end_matches([',', ';', ':']))
}

/// A confirmation, beside what was clicked: a flat chip with a green dot (rule 2: status chips
/// are never outlined), the whole line on hover. It fades: one repaint is asked for when it's
/// due to go, not a timer (as the art strip's, #511).
pub fn done_chip(ui: &mut egui::Ui, st: &Status, name: Option<&str>) {
    let short = status_short(&st.text, name);
    theme::hint_chip(ui, &short, Some(&st.text), Some(theme::Dot::Ok));
    ui.ctx()
        .request_repaint_after(pc_lights::DONE_FOR.saturating_sub(st.at.elapsed()));
}

/// A note (muted) or a problem (amber), under what was clicked: an info bar that stays until
/// the next result (rule 5: amber is never collapsed), the whole line on its ⓘ.
pub fn status_bar(ui: &mut egui::Ui, st: &Status, name: Option<&str>) {
    let level = match st.tone {
        Tone::Problem => theme::Level::Warn,
        Tone::Note | Tone::Done => theme::Level::Info,
    };
    theme::info_bar(ui, level, &status_short(&st.text, name), &st.text, None);
}

/// A result in its place: a fading chip for a confirmation, else a bar that stays.
pub fn status_here(ui: &mut egui::Ui, st: &Status, name: Option<&str>) {
    if st.tone == Tone::Done {
        ui.horizontal(|ui| done_chip(ui, st, name));
    } else {
        status_bar(ui, st, name);
    }
}

/// The keyboard card's own results (its picks drawn through Windows, #749 b), at the card's top.
pub fn keyboard_status(ui: &mut egui::Ui) {
    #[cfg(test)]
    let status = PREVIEW.with_borrow(|v| {
        v.as_ref()
            .and_then(|(v, _)| placed(&v.pc.results, &Place::Keyboard).cloned())
    });
    #[cfg(not(test))]
    let status = pc_lights::result(&Place::Keyboard);
    if let Some(st) = status.as_ref().filter(|st| st.shown()) {
        status_here(ui, st, None);
        ui.add_space(6.0);
    }
}

/// A vendor app that may override a device's colours (#558, #795 O2): one amber bar, the
/// sentence and the hint on its ⓘ.
fn conflict_bar(ui: &mut egui::Ui, c: &pc_lights::Conflicting) {
    let sentence = format!(
        "{} is running and may override Foxglove's colours on {}.",
        c.app, c.device
    );
    let full = match &c.hint {
        Some(hint) => format!("{sentence}\n{hint}"),
        None => sentence,
    };
    theme::info_bar(
        ui,
        theme::Level::Warn,
        &conflict_short(&c.app, &c.device),
        &full,
        None,
    );
}

/// The conflict bar's line: "{app} may override the {device}", the device's name cut to fit the
/// bar's 60 characters (then the app's, should that alone be too long).
pub fn conflict_short(app: &str, device: &str) -> String {
    let cut = |s: &str, room: usize| -> String {
        if s.chars().count() > room {
            s.chars()
                .take(room.saturating_sub(1))
                .chain(['…'])
                .collect()
        } else {
            s.to_string()
        }
    };
    const MID: &str = " may override the ";
    let app = cut(app, theme::BAR_MAX_CHARS / 2);
    let room = theme::BAR_MAX_CHARS - MID.chars().count() - app.chars().count();
    format!("{app}{MID}{}", cut(device, room))
}

/// "More sources" (#795 O23–O30): OpenRGB and Add a device, collapsed. Its summary says how
/// OpenRGB is; while it's on and not connected, an amber chip keeps that on screen.
fn more_sources(
    ui: &mut egui::Ui,
    s: &pc_lights::Snapshot,
    draft: &mut Draft,
    wizard: &mut Wizard,
    out: &mut Vec<Intent>,
) {
    let o = &s.openrgb;
    if wizard.step != add::Step::Closed {
        // A flow opened on purpose (O30) stays in sight while it runs.
        super::settings_ui::set_expander_open(ui.ctx(), MORE_SOURCES, true);
    }
    let summary = openrgb_summary(s);
    let down = o.settings.enabled && !o.connected;
    let said = placed(&s.results, &Place::Sources);
    let ex = super::settings_ui::expander_with(
        ui,
        MORE_SOURCES,
        "More sources",
        &summary,
        |ui| {
            if down {
                theme::hint_chip(
                    ui,
                    "OpenRGB not connected",
                    o.status.as_deref(),
                    Some(theme::Dot::Warn),
                );
            }
            // Add a device's and your entries' results, on the header, so a saved entry still
            // says so once the flow closes.
            if let Some(st) = said.filter(|st| st.tone == Tone::Done) {
                done_chip(ui, st, None);
            }
        },
        |ui| {
            let mut asked = Vec::new();
            openrgb_row(ui, o, draft, &mut asked);
            ui.add_space(6.0);
            ui.separator();
            add::draw(ui, s, wizard, &mut asked);
            asked
        },
    );
    out.extend(ex.body.into_iter().flatten());
    if let Some(st) = said.filter(|st| st.tone != Tone::Done) {
        status_bar(ui, st, None);
    }
}

/// More sources' summary: "OpenRGB off", "OpenRGB: 2 devices", or "OpenRGB on" while it isn't
/// connected (the amber chip beside it says so).
pub fn openrgb_summary(s: &pc_lights::Snapshot) -> String {
    let o = &s.openrgb;
    if !o.settings.enabled {
        return "OpenRGB off".into();
    }
    if !o.connected {
        return "OpenRGB on".into();
    }
    let n = s
        .devices
        .iter()
        .filter(|r| matches!(r.control, Control::OpenRgb))
        .count();
    format!("OpenRGB: {n} device{}", if n == 1 { "" } else { "s" })
}

/// "Use OpenRGB's devices" (#471): the toggle; while it's on, the server's host and port, a
/// Connect for typed ones, "try again" and how the connection is.
fn openrgb_row(
    ui: &mut egui::Ui,
    v: &pc_lights::OpenRgbView,
    draft: &mut Draft,
    out: &mut Vec<Intent>,
) {
    let p = theme::palette(ui);
    let s = &v.settings;
    let mut on = s.enabled;
    let tip = if s.enabled {
        OPENRGB_TIP.to_string()
    } else {
        format!("{OPENRGB_TIP}\n{OPENRGB_OFF}")
    };
    let changed = ui
        .horizontal(|ui| {
            let c = ui.checkbox(
                &mut on,
                RichText::new(OPENRGB_SHORT).color(p.text_strong).strong(),
            );
            theme::with_info(ui, c, &tip).changed()
        })
        .inner;
    if changed {
        draft.openrgb = None;
        out.push(Intent::OpenRgb(openrgb::Settings {
            enabled: on,
            ..s.clone()
        }));
    }
    if !s.enabled {
        return;
    }
    let (host, port) = draft
        .openrgb
        .get_or_insert_with(|| (s.host.clone(), s.port.to_string()));
    let typed = port
        .trim()
        .parse::<u16>()
        .ok()
        .map(|port| openrgb::Settings {
            host: host.trim().to_string(),
            port,
            ..s.clone()
        });
    let valid = typed.as_ref().filter(|t| t.addr().is_some());
    let mut connect = false;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        ui.label(theme::caps_label(ui, "Host"));
        ui.add(egui::TextEdit::singleline(host).desired_width(120.0))
            .on_hover_text("OpenRGB's address: an IP address, or localhost");
        ui.label(theme::caps_label(ui, "Port"));
        ui.add(
            egui::TextEdit::singleline(port)
                .desired_width(56.0)
                .char_limit(5),
        );
        connect = ui
            .add_enabled(
                valid.is_some_and(|t| t != s) || !v.connected && valid.is_some(),
                egui::Button::new("Connect"),
            )
            .on_hover_text("Connect to OpenRGB at this address")
            .clicked();
    });
    if valid.is_none() {
        theme::info_bar(
            ui,
            theme::Level::Warn,
            "Host or port not valid",
            OPENRGB_ADDRESS,
            None,
        );
    }
    let mut again = s.reconnect;
    if ui
        .checkbox(&mut again, "Reconnect automatically")
        .on_hover_text("A few tries, a little longer apart each time, then Foxglove stops")
        .changed()
    {
        out.push(Intent::OpenRgb(openrgb::Settings {
            reconnect: again,
            ..s.clone()
        }));
    }
    if let Some(status) = &v.status {
        ui.label(
            RichText::new(status)
                .color(if v.connected { p.muted } else { p.warn })
                .font(theme::mono(12.0)),
        );
    }
    if connect {
        match valid.cloned() {
            // The same address, not connected now: try it again.
            Some(t) if &t == s => out.push(Intent::OpenRgbConnect),
            Some(t) => out.push(Intent::OpenRgb(t)),
            None => {}
        }
    }
}

/// A row of read-only chips: flat, no outline (the flat-chip rule, #795).
pub fn chips(ui: &mut egui::Ui, chips: &[String]) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(6.0, 4.0);
        for c in chips {
            theme::hint_chip(ui, c, None, None);
        }
    });
}

/// Whether Foxglove drives the device: a driver's, or a Dynamic Lighting device it opened
/// (#691; Windows' access to it is known only then).
pub fn driven(row: &Row) -> bool {
    match row.control {
        Control::Driver(_) | Control::OpenRgb => true,
        Control::LampArray => row.lamp_access.is_some(),
        Control::NotYet => false,
    }
}

/// The most lamps that get a picker each; more is a per-key board (lighting-zones.md, hide
/// rules): one row for all lamps.
pub const MAX_LAMP_PICKERS: usize = 12;

/// Whether a device gets a colour picker per zone (or lamp).
fn pickers(row: &Row) -> bool {
    row.caps.per_zone()
        && (!matches!(row.control, Control::LampArray | Control::OpenRgb)
            || row.zones.len() <= MAX_LAMP_PICKERS)
}

/// Whether the row already shows each zone by name: a colour picker per zone, drawn (the
/// device takes colour from Foxglove now, its effect uses colour, and it has few enough zones).
/// Same conditions as the row's own drawing, from the device's data alone.
fn named_pickers(row: &Row) -> bool {
    let drawn = match row.control {
        Control::Driver(_) | Control::OpenRgb => true,
        Control::LampArray => row.lamp_access.is_some(),
        Control::NotYet => false,
    };
    drawn
        && row.caps.takes_color()
        && effect_of(row.control).is_some_and(|e| e.uses_colour())
        && pickers(row)
}

/// Whether a device row offers **Show zones** (#611, the Hub's cut on #775): only where the
/// drawing adds what the row can't show, so not when each zone already has a named picker on
/// the row. A per-LED strip past [`MAX_LAMP_PICKERS`] (one "All" row) or a device Foxglove
/// can't drive yet keeps it.
pub fn offers_zones(row: &Row) -> bool {
    row.caps.zones.is_some()
        && super::zones_ui::drawable(row.caps.kind(false))
        && !named_pickers(row)
}

/// What Windows lets Foxglove do with a Dynamic Lighting device, in one line (#691): from
/// `IsEnabled` / `IsAvailable` and whether Foxglove has package identity (background control,
/// #179). `None` when there's nothing to say.
pub fn lamp_line(access: pc_lighting::LampAccess, identity: bool) -> Option<&'static str> {
    if !access.enabled {
        return Some("Turned off in Windows' Dynamic Lighting settings.");
    }
    match (access.available, identity) {
        (false, true) => Some(
            "Another app has these lights now. Settings > Personalization > Dynamic Lighting \
             sets which app comes first.",
        ),
        (false, false) => {
            Some("Windows shows Foxglove's colours here while Foxglove's window is in front.")
        }
        (true, false) => Some(
            "Windows shows Foxglove's colours while Foxglove's window is in front; in the \
             background its own Dynamic Lighting takes over.",
        ),
        (true, true) => None,
    }
}

/// A Windows lighting device's state as a chip (#795 O9–O13).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LampChip {
    /// The chip: "off in Windows", "another app has it", "in front only", …
    pub text: &'static str,
    pub dot: theme::Dot,
    /// The sentence it stands for, word for word: the chip's hover.
    pub hover: &'static str,
    /// Whether the row offers "Open Dynamic Lighting settings" (another app comes first).
    pub settings: bool,
}

/// What Windows does with a Dynamic Lighting device, as a chip for its row: amber where the
/// user has something to change, grey where it's how Windows works. `None` for other devices,
/// and for one Foxglove fully drives.
pub fn lamp_chip(row: &Row, identity: bool) -> Option<LampChip> {
    use theme::Dot;
    if !matches!(row.control, Control::LampArray) {
        return None;
    }
    let chip = |text, dot, hover, settings| LampChip {
        text,
        dot,
        hover,
        settings,
    };
    Some(match row.lamp_access {
        Some(access) => {
            let hover = lamp_line(access, identity)?;
            if !access.enabled {
                chip("off in Windows", Dot::Warn, hover, false)
            } else if !access.available && identity {
                chip("another app has it", Dot::Warn, hover, true)
            } else {
                chip("in front only", Dot::Muted, hover, false)
            }
        }
        None if row.zones.is_empty() => chip(
            "reading lamps…",
            Dot::Muted,
            "Windows lighting: reading its lamps from Windows.",
            false,
        ),
        None => chip(
            "not in Windows right now",
            Dot::Muted,
            "Windows lists no Dynamic Lighting device for it right now.",
            false,
        ),
    })
}

/// A device's light summary as a chip for its collapsed row (#795 O5): "5 zones", "24 LEDs",
/// "2 lamps", "1 zone", "on/off"; the whole summary is on the open row.
pub fn light_chip(row: &Row) -> Option<String> {
    use pc_lighting::zones::{self as z, Kind};
    let caps = &row.caps;
    match caps.kind(false) {
        Kind::None => None,
        Kind::OnOff => Some("on/off".into()),
        Kind::Single => Some("1 zone".into()),
        Kind::Zones => z::scene_words(caps.color, caps.count, caps.zones.as_ref(), false)
            .or_else(|| caps.summary(false)),
        Kind::PerLed => {
            let n = caps
                .zones
                .as_ref()
                .map(|z| z.list.len() as u32)
                .filter(|n| *n > 0)
                .unwrap_or(caps.count);
            let noun = if matches!(row.control, Control::LampArray) {
                "lamps"
            } else {
                "LEDs"
            };
            Some(format!("{n} {noun}"))
        }
        Kind::Unknown => caps.summary(false),
    }
}

/// The open row's light line (#611): the summary ("5 lighting zones", "Per-LED lighting, 24
/// lamps") with what it means on hover, and **Show zones** where the drawing adds to the row
/// ([`offers_zones`]).
fn light_line(ui: &mut egui::Ui, row: &Row) {
    let caps = &row.caps;
    let p = theme::palette(ui);
    let Some(summary) = caps.summary(false) else {
        return;
    };
    let mut open = false;
    ui.horizontal_wrapped(|ui| {
        ui.add(egui::Label::new(RichText::new(summary).color(p.text)).wrap())
            .on_hover_text(zones_tip(row));
        if offers_zones(row) {
            open = super::zones_ui::toggle(ui, &row.key, caps.kind(false));
        }
    });
    if open {
        let names: Vec<String> = row.zones.iter().map(|(n, _)| n.clone()).collect();
        super::zones_ui::device(ui, caps, &names);
    }
}

/// The collapsed row's muted line (#795 O4, O14): how it's controlled, "· added by you",
/// "· calibrated". A device Foxglove can't drive says on hover that it keeps its own effect
/// ([`not_yet_text`]).
pub fn row_note(row: &Row) -> String {
    let mut note = control_text(row.control).to_string();
    if row.entry.is_some() {
        note.push_str(" · added by you");
    }
    if row.calibrated {
        note.push_str(" · calibrated");
    }
    note
}

/// One device (#795): a row collapsed by default, remembered for the session. The row has the
/// name, how it's controlled, its state as flat chips (amber stays here, closed or not),
/// Identify and the ⋯ menu; open, the light line, the zone pickers and swatches, Match
/// keyboard, Calibrate…, Off and Apply.
///
/// The device's thumbnail (#804) leads the row, tinted with its colours now; a click on it is
/// Identify where the row has Identify, the same request as the button. On a row too narrow
/// for it, it gives way (the battery list's size, then none: [`thumb::row_size`]) from what the
/// row's other contents needed last pass.
///
/// A keyboard that isn't the focus (`show`, #820) has "Show in Foxglove" at the row's end.
#[allow(clippy::too_many_arguments)]
fn device(
    ui: &mut egui::Ui,
    row: &Row,
    status: Option<&Status>,
    lamps: Option<&pc_lights::LampLayout>,
    show: Option<&super::keyboard_focus::Keyboard>,
    busy: bool,
    draft: &mut Draft,
    out: &mut Vec<Intent>,
) {
    use super::device_thumb::{self as thumb, Thumb};
    let p = theme::palette(ui);
    let driven = driven(row);
    let identify = driven && row.caps.switchable();
    let identity = super::scene_lamps::identity();
    let note = row_note(row);
    let mut asked = Vec::new();
    let mut removing = None;
    let mut thumb_clicked = false;
    // What the row's contents other than the thumbnail needed last pass, and this pass's room.
    let need_id = thumb::id(&row.key).with("rest");
    let rest = ui.ctx().data(|d| d.get_temp::<f32>(need_id));
    let room = std::cell::Cell::new(0.0_f32);
    let used = std::cell::Cell::new(0.0_f32);
    let edge = std::cell::Cell::new(0.0_f32);
    let ex = super::settings_ui::expander_lead(
        ui,
        &row_key(&row.key),
        thumb::ROW.y + 4.0,
        |ui| {
            room.set(ui.available_width());
            edge.set(ui.max_rect().right());
            if let Some(size) = thumb::row_size(room.get(), rest) {
                let r = thumb::row_thumb(
                    ui,
                    size,
                    &row.key,
                    Thumb::of_row(row, lamps),
                    identify && !busy,
                );
                thumb_clicked = r.clicked();
                ui.add_space(thumb::GAP);
                used.set(size.x + thumb::GAP + ui.spacing().item_spacing.x);
            }
        },
        &row.name,
        "",
        |ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            let note = ui.add(
                egui::Label::new(RichText::new(note).color(p.muted))
                    .truncate()
                    .selectable(false),
            );
            if matches!(row.control, Control::NotYet) {
                note.on_hover_text(not_yet_text(&row.caps.effects));
            }
            if let Some(chip) = light_chip(row) {
                theme::hint_chip(ui, &chip, Some(zones_tip(row)), None);
            }
            match row.caps.summary(false) {
                None if row.caps.screen_chips().is_empty() => {
                    theme::hint_chip(ui, "No lights Foxglove knows of", None, None);
                }
                _ => {}
            }
            for screen in row.caps.screen_chips() {
                theme::hint_chip(ui, &screen, None, None);
            }
            let lamp = lamp_chip(row, identity);
            if let Some(chip) = lamp {
                theme::hint_chip(ui, chip.text, Some(chip.hover), Some(chip.dot));
            }
            // What the last click on this device did: on its row, open or closed.
            if let Some(st) = status.filter(|st| st.tone == Tone::Done) {
                done_chip(ui, st, Some(&row.name));
            }
            let left = ui
                .with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    // Room for the buttons' outline at the row's clipped edge.
                    ui.add_space(2.0);
                    if let Some(k) = show {
                        // At the end, where a keyboard's own row has it.
                        if others::link(ui, k) {
                            asked.push(Intent::Show(k.key.clone()));
                        }
                    }
                    if let Some(entry) = &row.entry {
                        more(ui, row, entry, &mut removing, &mut asked);
                    }
                    if identify {
                        let identify = ui
                            .add_enabled(!busy, egui::Button::new("Identify"))
                            .on_hover_text(IDENTIFY_TIP);
                        if identify.clicked() {
                            asked.push(Intent::Identify(row.key.clone()));
                        }
                    }
                    if lamp.is_some_and(|c| c.settings)
                        && ui.button(super::lights_owner::DL_BUTTON).clicked()
                    {
                        asked.push(Intent::OpenDynamicLighting);
                    }
                    ui.available_width()
                })
                .inner;
            // Room to spare (or, past the row's edge, how far over it went).
            // (egui widens the row's own rect with what overflows it: measure from its edge.)
            let over = ui.min_rect().right() - edge.get();
            let slack = if over > 0.5 { -over } else { left.max(0.0) };
            let need = room.get() - used.get() - slack;
            if rest.is_none_or(|r| (r - need).abs() > 0.5) {
                ui.ctx().data_mut(|d| d.insert_temp(need_id, need));
                if thumb::row_size(room.get(), rest) != thumb::row_size(room.get(), Some(need)) {
                    ui.ctx().request_discard("a device thumbnail's size");
                }
            }
        },
        |ui| {
            let mut asked = Vec::new();
            light_line(ui, row);
            let colours = match row.control {
                Control::Driver(_) | Control::OpenRgb => row.caps.takes_color(),
                Control::LampArray => row.lamp_access.is_some() && row.caps.takes_color(),
                Control::NotYet => false,
            };
            let colours = colours && zones(ui, row, draft, false, &mut asked);
            actions(ui, row, busy, driven, colours, draft, &mut asked);
            asked
        },
    );
    if thumb_clicked {
        out.push(Intent::Identify(row.key.clone()));
    }
    out.extend(asked);
    out.extend(ex.body.into_iter().flatten());
    // A note or a problem stays under the row, closed or open (amber is never collapsed).
    if let Some(st) = status.filter(|st| st.tone != Tone::Done) {
        status_bar(ui, st, Some(&row.name));
    }
    if removing.is_some() {
        draft.removing = removing;
    }
    // Remove's second click: the device's entry goes only once it's confirmed here, closed or
    // open.
    if draft.removing.as_deref() == Some(row.key.as_str()) {
        let device = row.entry.as_deref().and_then(add::parse_entry);
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("Forget your entry for this device?").color(p.warn));
            if let Some(device) = &device {
                if ui.button("Remove").clicked() {
                    draft.removing = None;
                    out.push(Intent::Remove(device.vid, device.pid));
                }
            }
            if ui.button("Keep").clicked() {
                draft.removing = None;
            }
        });
    }
}

/// "⋯": the device's less used actions, for a device you added (Copy entry, Remove). Remove
/// asks again under the device before it forgets the entry.
fn more(
    ui: &mut egui::Ui,
    row: &Row,
    entry: &str,
    removing: &mut Option<String>,
    out: &mut Vec<Intent>,
) {
    ui.menu_button("⋯", |ui| {
        if ui
            .button("Copy entry")
            .on_hover_text(
                "Copy this device's catalog entry (TOML) to share it, or to contribute it \
                 to Foxglove's lighting-catalog.toml",
            )
            .clicked()
        {
            out.push(Intent::Copy(entry.to_string()));
        }
        if add::parse_entry(entry).is_some()
            && ui
                .button("Remove…")
                .on_hover_text("Forget your entry for this device")
                .clicked()
        {
            *removing = Some(row.key.clone());
        }
    })
    .response
    .on_hover_text("Copy entry, Remove");
}

/// The effect a PC light runs from Foxglove, from the effect table (#670): its class's
/// steady colour. A device Foxglove can't drive has none.
pub fn effect_of(control: Control) -> Option<&'static super::effect_controls::EffectSpec> {
    use super::effect_controls::{spec, EffectClass};
    match control {
        Control::Driver(_) | Control::OpenRgb => spec(EffectClass::PcLights, "static"),
        Control::LampArray => spec(EffectClass::LampArray, "static"),
        Control::NotYet => None,
    }
}

/// The zone pickers and the swatch row; false when the device's effect takes no colour (then
/// nothing is drawn).
/// The colour group's caps label sits beside the swatches on a row, above them on the keyboard
/// card (rule 4, as its COLOUR).
fn zones(
    ui: &mut egui::Ui,
    row: &Row,
    draft: &mut Draft,
    above: bool,
    out: &mut Vec<Intent>,
) -> bool {
    // Only the controls its effect uses (#670); every one Foxglove drives takes a colour.
    if !effect_of(row.control).is_some_and(|e| e.uses_colour()) {
        return false;
    }
    let picks = draft
        .colors
        .entry(row.key.clone())
        .or_insert_with(|| vec![None; row.zones.len()]);
    picks.resize(row.zones.len(), None);
    if pickers(row) {
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 14.0;
            for (i, (name, now)) in row.zones.iter().enumerate() {
                ui.vertical(|ui| {
                    ui.set_width(118.0);
                    let shown = picks[i].or(*now);
                    let zone = row.zone_ids.get(i).map(String::as_str);
                    ui.horizontal(|ui| {
                        if let Some(c) = zone_swatch(ui, row, i, name, shown) {
                            picks[i] = Some(c);
                        }
                        if shown.is_none() {
                            ui.weak("not set");
                        } else if picks[i].is_some_and(|c| Some(c) != *now) {
                            ui.label(RichText::new("changed").color(theme::palette(ui).accent));
                        }
                    });
                    ui.label(RichText::new(name).size(12.5));
                    if let Some(c) = shown {
                        if super::calibrate_ui::not_matched(ui, &row.key, zone, c) {
                            let zone = zone.map(String::from);
                            out.push(Intent::CalibrateColor(row.key.clone(), zone, c));
                        }
                    }
                });
            }
        });
    }
    ui.add_space(4.0);
    let caps = match (row.caps.per_zone(), row.control) {
        (false, _) => "Colour",
        (true, Control::LampArray) => "All lamps",
        (true, Control::OpenRgb) => "All LEDs",
        (true, _) => "All zones",
    };
    if above {
        ui.label(theme::caps_label(ui, caps));
    }
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 10.0;
        if !above {
            ui.label(theme::caps_label(ui, caps));
        }
        for (name, rgb) in super::lights_page::SWATCHES {
            let all = picks.iter().all(|c| *c == Some(rgb));
            if swatch(ui, super::calibrate::screen_color(&row.key, None, rgb), all)
                .on_hover_text(name)
                .clicked()
            {
                picks.iter_mut().for_each(|c| *c = Some(rgb));
            }
        }
    });
    true
}

/// The open row's last line: Match keyboard (with its ⓘ) on the left; Calibrate…, Off and
/// Apply colours on the right.
fn actions(
    ui: &mut egui::Ui,
    row: &Row,
    busy: bool,
    driven: bool,
    colours: bool,
    draft: &mut Draft,
    out: &mut Vec<Intent>,
) {
    let switch = driven && row.caps.switchable();
    if !colours && !switch {
        return;
    }
    let mut matching = row.match_keyboard;
    let changes = draft.changes(row);
    let mut apply = false;
    let mut toggled = false;
    super::settings_ui::split_row(
        ui,
        |ui| {
            ui.add_enabled_ui(!busy, |ui| {
                if colours {
                    apply = ui
                        .add_enabled(
                            !changes.is_empty(),
                            theme::primary_button(ui, "Apply colours"),
                        )
                        .clicked();
                }
                if switch {
                    if ui.button("Off").on_hover_text(OFF_TIP).clicked() {
                        out.push(Intent::Off(row.key.clone()));
                    }
                    if row.caps.takes_color() && calibrate_button(ui).clicked() {
                        out.push(Intent::Calibrate(row.key.clone()));
                    }
                }
            });
        },
        |ui| {
            if !colours {
                return;
            }
            ui.add_enabled_ui(!busy, |ui| {
                let c = ui.checkbox(&mut matching, MATCH_SHORT);
                toggled = theme::with_info(ui, c, MATCH_TIP).changed();
            });
        },
    );
    if toggled {
        out.push(Intent::Match(row.key.clone(), matching));
    }
    if apply {
        out.push(Intent::Apply {
            key: row.key.clone(),
            colors: changes,
        });
    }
}

/// A zone's colour swatch (#795 O15, O16): a click opens its popup, with the colour picker and
/// the `#rrggbb` box to type an exact colour. Drawn through the device's screen side; a pick
/// stands for what it's sent. Returns a pick, in the device's colours.
fn zone_swatch(
    ui: &mut egui::Ui,
    row: &Row,
    i: usize,
    name: &str,
    shown: Option<[u8; 3]>,
) -> Option<[u8; 3]> {
    use egui::widgets::color_picker::{color_picker_color32, Alpha};
    let zone = row.zone_ids.get(i).map(String::as_str);
    let [r, g, b] = super::calibrate::on_screen(&row.key, zone, shown.unwrap_or([0x80; 3]));
    let mut color = Color32::from_rgb(r, g, b);
    let popup_id = ui.make_persistent_id(("pc-zone-popup", &row.key, i));
    let open = egui::Popup::is_id_open(ui.ctx(), popup_id);
    let button =
        zone_button(ui, color, open).on_hover_text(format!("{name}: pick a colour, then Apply"));
    let mut picked = None;
    egui::Popup::menu(&button)
        .id(popup_id)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show(|ui| {
            ui.spacing_mut().slider_width = 275.0;
            if color_picker_color32(ui, &mut color, Alpha::Opaque) {
                let [r, g, b, _] = color.to_array();
                picked = Some(super::calibrate::from_screen(&row.key, zone, [r, g, b]));
            }
            ui.horizontal(|ui| {
                ui.label(theme::caps_label(ui, "Hex"));
                if let Some(c) = hex_box(ui, (&row.key, i), name, shown) {
                    picked = Some(c);
                }
            });
        });
    picked
}

/// The swatch a zone's popup opens from: its colour, outlined as every clickable thing is.
fn zone_button(ui: &mut egui::Ui, color: Color32, open: bool) -> egui::Response {
    let size = ui.spacing().interact_size;
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    response.widget_info(|| egui::WidgetInfo::new(egui::WidgetType::ColorButton));
    if ui.is_rect_visible(rect) {
        let visuals = if open {
            &ui.visuals().widgets.open
        } else {
            ui.style().interact(&response)
        };
        let rect = rect.expand(visuals.expansion);
        let radius = egui::CornerRadius::same(3);
        ui.painter().rect_filled(rect, radius, color);
        ui.painter()
            .rect_stroke(rect, radius, visuals.bg_stroke, egui::StrokeKind::Inside);
    }
    response
}

/// A zone's colour as `#rrggbb` to type (#617): an exact colour, such as a saved one to put
/// back. The text being typed stays while the box has focus; a whole colour is a pick (sent on
/// Apply, like the picker's). The value is the device's colour, before the screen's mapping.
fn hex_box(
    ui: &mut egui::Ui,
    id: (&str, usize),
    name: &str,
    shown: Option<[u8; 3]>,
) -> Option<[u8; 3]> {
    let id = ui.make_persistent_id(("pc-zone-hex", id.0, id.1));
    let current = shown.map(pc_lights::hex).unwrap_or_default();
    let mut text = ui
        .data_mut(|d| d.get_temp::<String>(id))
        .unwrap_or_else(|| current.clone());
    let r = ui
        .add(
            egui::TextEdit::singleline(&mut text)
                .desired_width(72.0)
                .char_limit(7)
                .font(egui::TextStyle::Monospace)
                .hint_text("#rrggbb"),
        )
        .on_hover_text(format!("{name}: type a colour as #rrggbb, then Apply"));
    let picked = r.changed().then(|| parse_typed(&text)).flatten();
    if r.has_focus() {
        ui.data_mut(|d| d.insert_temp(id, text));
    } else {
        ui.data_mut(|d| d.remove::<String>(id));
    }
    picked
}

/// A typed colour: `#rrggbb` or `rrggbb`, any case.
pub fn parse_typed(text: &str) -> Option<[u8; 3]> {
    let t = text.trim();
    pc_lights::parse_hex(&format!("#{}", t.strip_prefix('#').unwrap_or(t)))
}

/// "Calibrate…": opens the colour calibrator on a device.
fn calibrate_button(ui: &mut egui::Ui) -> egui::Response {
    ui.button("Calibrate…").on_hover_text(
        "Match how this device shows colours to your screen or another device: white, red, \
         green, blue and grey, about a minute",
    )
}

/// A small round colour swatch, ringed when every zone has it.
fn swatch(ui: &mut egui::Ui, color: Color32, selected: bool) -> egui::Response {
    let p = theme::palette(ui);
    let (rect, response) = ui.allocate_exact_size(egui::vec2(22.0, 22.0), Sense::click());
    let painter = ui.painter();
    let c = rect.center();
    if selected {
        painter.circle_stroke(c, 12.0, Stroke::new(2.0, p.text_strong));
    } else if response.hovered() || response.has_focus() {
        painter.circle_stroke(c, 12.0, Stroke::new(2.0, p.accent));
    }
    painter.circle_filled(c, 9.5, color);
    response
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use foxglove_devices::pc_lighting::{Color, LampArrayInfo, LightCaps, Screen, Unknown};

    fn case(colors: [Option<[u8; 3]>; 5], matching: bool) -> Row {
        let names = [
            "Diamond display",
            "Interior light",
            "Front bottom fan",
            "Front middle fan",
            "Front top fan",
        ];
        Row {
            key: "103c:84fd".into(),
            name: "HP OMEN 45L case lights".into(),
            control: Control::Driver(Protocol::OmenTracer),
            zones: names
                .iter()
                .zip(colors)
                .map(|(n, c)| (n.to_string(), c))
                .collect(),
            match_keyboard: matching,
            caps: LightCaps {
                color: Color::PerZone,
                count: 5,
                effects: ["breathing", "colour cycle", "blinking", "wave", "radial"]
                    .map(String::from)
                    .to_vec(),
                brightness: true,
                // As the catalog gives it (#611): five areas.
                zones: Some(pc_lighting::Zones {
                    list: names
                        .iter()
                        .map(|n| pc_lighting::Zone {
                            area: Some(n.to_string()),
                            ..Default::default()
                        })
                        .collect(),
                    dead: vec![0],
                    source: pc_lighting::ZoneSource::Catalog,
                    reported: None,
                }),
                ..Default::default()
            },
            entry: None,
            zone_ids: ["1", "2", "5", "6", "7"].map(String::from).to_vec(),
            calibration: Some(Default::default()),
            calibrated: false,
            size: None,
            zone_positions: vec![],
            zone_roles: vec![],
            kind: foxglove_devices::pc_lighting::DeviceKind::Case,
            shape: None,
            lamp_access: None,
            calibration_hint: None,
        }
    }

    pub fn unknown_strip() -> Unknown {
        Unknown {
            vid: 0x1209,
            pid: 0xa1e5,
            name: "Desk strip".into(),
            usage_pages: vec![0xff00, 0x0001],
            path: "strip".into(),
        }
    }

    /// The previews' sample: the keyboard, the 45L with colours, the cooler, the G502 with its
    /// LampArray attributes, the case's vendor tool running, two unknown devices.
    pub fn sample() -> View {
        let orange = Some([0xf0, 0x8a, 0x3c]);
        let pine = Some([0x5f, 0xc4, 0x8e]);
        View {
            pc: Snapshot {
                devices: vec![
                    case([orange, pine, orange, orange, orange], false),
                    Row {
                        key: "03f0:7397".into(),
                        name: "HP OMEN liquid cooler".into(),
                        control: Control::NotYet,
                        zones: vec![],
                        match_keyboard: false,
                        caps: LightCaps {
                            screens: vec![Screen {
                                images: true,
                                animations: true,
                                ..Default::default()
                            }],
                            // The effect it keeps, as the catalog has it (#614).
                            effects: vec!["rainbow".into()],
                            ..Default::default()
                        },
                        entry: None,
                        zone_ids: vec![],
                        calibration: None,
                        calibrated: false,
                        size: None,
                        zone_positions: vec![],
                        zone_roles: vec![],
                        // As the catalog has it (#804: its thumbnail is a cooler's ring).
                        kind: pc_lighting::DeviceKind::Cooler,
                        shape: None,
                        lamp_access: None,
                        calibration_hint: None,
                    },
                    Row {
                        key: "046d:c332".into(),
                        name: "G502 Proteus Spectrum".into(),
                        control: Control::LampArray,
                        zones: vec![
                            ("Lamp 1 · Logo".into(), orange),
                            ("Lamp 2 · Accent".into(), pine),
                        ],
                        match_keyboard: false,
                        caps: LightCaps::from_lamp_array(LampArrayInfo {
                            lamp_count: 2,
                            positions: true,
                            min_update_us: 33_000,
                            kind: 2,
                        }),
                        entry: None,
                        zone_ids: vec!["0".into(), "1".into()],
                        calibration: None,
                        calibrated: false,
                        size: None,
                        zone_positions: vec![],
                        zone_roles: vec![],
                        kind: Default::default(),
                        shape: None,
                        lamp_access: Some(Default::default()),
                        calibration_hint: None,
                    },
                ],
                conflicts: vec![pc_lights::Conflicting {
                    app: "OMEN Light Studio".into(),
                    device: "HP OMEN 45L case lights".into(),
                    key: "103c:84fd".into(),
                    hint: Some("Settings > Apps > Installed apps > OMEN Light Studio".into()),
                }],
                // The last click: Apply on the case's row.
                status: Some(Status::new(
                    Place::Row("103c:84fd".into()),
                    Tone::Done,
                    "HP OMEN 45L case lights: colours set",
                )),
                results: vec![Status::new(
                    Place::Row("103c:84fd".into()),
                    Tone::Done,
                    "HP OMEN 45L case lights: colours set",
                )],
                busy: false,
                unknown: vec![
                    unknown_strip(),
                    Unknown {
                        vid: 0x045e,
                        pid: 0x0b12,
                        name: "Xbox Wireless Controller".into(),
                        usage_pages: vec![0x0001],
                        path: "pad".into(),
                    },
                ],
                lamp_applies: Default::default(),
                keyboard_lamp_arrays: Vec::new(),
                openrgb: Default::default(),
                local_file: Some(
                    r"C:\Users\you\AppData\Roaming\Foxglove\lighting-catalog.local.toml".into(),
                ),
            },
            // The mouse's two lamps where Windows places them (#804's thumbnail).
            lamps: crate::foxglove::fixtures::users_desk::lamps(),
            keyboards: Default::default(),
        }
    }

    /// Before Foxglove has set anything, no Light Studio, matching the keyboard.
    pub fn sample_fresh() -> View {
        View {
            pc: Snapshot {
                devices: vec![case([None; 5], true)],
                ..Default::default()
            },
            lamps: Default::default(),
            keyboards: Default::default(),
        }
    }

    /// The second desk (`fixtures`): another vendor's case board driven by a fake driver, and
    /// a lightbar on Dynamic Lighting. (The keyboard has the page's own card, #533.)
    pub fn sample_second_desk() -> View {
        use crate::foxglove::fixtures::second_desk as desk;
        let mut bar = desk::lightbar_row();
        bar.caps = LightCaps::from_lamp_array(desk::lightbar_info());
        for (i, z) in pc_lighting::lamp_array::lamp_zones(&desk::lightbar_desc().lamps)
            .into_iter()
            .enumerate()
        {
            bar.zones
                .push((z.name, (i % 3 == 0).then_some([0x5f, 0xc4, 0x8e])));
            bar.zone_ids.push(z.id.to_string());
        }
        bar.lamp_access = Some(pc_lighting::LampAccess {
            enabled: true,
            available: false,
        });
        View {
            pc: Snapshot {
                devices: vec![desk::case_row(), bar],
                ..Default::default()
            },
            lamps: desk::lamps(),
            keyboards: Default::default(),
        }
    }

    /// An OpenRGB controller as the worker lists it (#471): `leds` LEDs, a zone each.
    pub fn openrgb_row(name: &str, leds: usize, color: Option<[u8; 3]>) -> Row {
        Row {
            key: format!("openrgb:{name}:#0"),
            name: name.into(),
            control: Control::OpenRgb,
            zones: (0..leds)
                .map(|i| (format!("LED {}", i + 1), color))
                .collect(),
            match_keyboard: false,
            caps: LightCaps {
                color: Color::PerLed,
                count: leds as u32,
                effects: vec!["Direct".into(), "Static".into(), "Rainbow".into()],
                ..Default::default()
            },
            entry: None,
            zone_ids: (0..leds).map(|i| i.to_string()).collect(),
            calibration: None,
            calibrated: false,
            size: None,
            zone_positions: vec![],
            zone_roles: vec![],
            kind: Default::default(),
            shape: None,
            lamp_access: None,
            calibration_hint: None,
        }
    }

    /// The second desk's case board with "Use OpenRGB's devices" on (#471): a four-LED memory
    /// module and a 40-LED fan hub through OpenRGB.
    pub fn sample_openrgb() -> View {
        let mut v = sample_second_desk();
        v.pc.devices.truncate(1);
        v.pc.devices
            .push(openrgb_row("Memory module", 4, Some([0xf0, 0x8a, 0x3c])));
        v.pc.devices.push(openrgb_row("ARGB fan hub", 40, None));
        v.pc.openrgb = pc_lights::OpenRgbView {
            settings: openrgb::Settings {
                enabled: true,
                ..Default::default()
            },
            connected: true,
            status: Some("Connected to OpenRGB at 127.0.0.1:6742: 2 devices".into()),
        };
        v
    }

    /// One device with zones (#795's previews): the 45L with its colours, nothing else.
    pub fn sample_one() -> View {
        let mut v = sample();
        v.pc.devices.truncate(1);
        v.pc.conflicts.clear();
        v.pc.status = None;
        v.pc.results.clear();
        v
    }

    /// Seven devices (#795's previews): [`sample`]'s three (Light Studio running), the second
    /// desk's case board and lightbar, and two OpenRGB controllers, OpenRGB connected.
    pub fn sample_seven() -> View {
        let mut v = sample();
        let desk = sample_second_desk();
        v.pc.devices.extend(desk.pc.devices);
        let rgb = sample_openrgb();
        v.pc.devices
            .push(openrgb_row("Memory module", 4, Some([0xf0, 0x8a, 0x3c])));
        v.pc.devices.push(strip_row());
        v.pc.openrgb = rgb.pc.openrgb;
        v
    }

    /// "Use OpenRGB's devices" on, and OpenRGB not answering (#795's amber state).
    pub fn sample_openrgb_down() -> View {
        let mut v = sample_second_desk();
        v.pc.openrgb = pc_lights::OpenRgbView {
            settings: openrgb::Settings {
                enabled: true,
                reconnect: true,
                ..Default::default()
            },
            connected: false,
            status: Some(
                "Can't reach OpenRGB at 127.0.0.1:6742 (connection refused); trying again in 4 s"
                    .into(),
            ),
        };
        v
    }

    /// #471: off by default, the card says so and lists nothing of OpenRGB's; on, its
    /// controllers are rows like any other ("Through OpenRGB") with pickers up to 12 LEDs and
    /// one "All LEDs" row past that, and the host and port sit under the toggle.
    #[test]
    fn openrgb_is_off_until_turned_on_then_its_devices_are_rows() {
        let text = shown_open(&sample_second_desk());
        assert!(text.contains("Use OpenRGB's devices"), "{text}");
        // #795 O24: the off line is the toggle's hover.
        assert!(
            reachable(&sample_second_desk()).contains("Foxglove doesn't connect to OpenRGB"),
            "{text}"
        );
        assert!(!text.contains("Through OpenRGB") && !text.contains("HOST"));
        let text = shown_open(&sample_openrgb());
        for want in [
            "Memory module",
            "ARGB fan hub",
            "Through OpenRGB",
            "LED 4",
            "ALL LEDS",
            "HOST",
            "127.0.0.1",
            "6742",
            "Reconnect automatically",
            "Connected to OpenRGB at 127.0.0.1:6742: 2 devices",
        ] {
            assert!(text.contains(want), "{want}: {text}");
        }
        assert!(!text.contains("LED 40"), "40 LEDs: one row, no picker each");
        assert_eq!(
            text.matches("Through OpenRGB").count(),
            2,
            "one label per device"
        );
        assert!(driven(&openrgb_row("x", 1, None)));
        assert!(effect_of(Control::OpenRgb).is_some_and(|e| e.uses_colour()));
    }

    /// A typed host that is not an IP address or localhost says why (no name lookups).
    #[test]
    fn a_typed_openrgb_address_is_checked_before_connect() {
        let v = sample_openrgb();
        let mut draft = Draft {
            openrgb: Some(("openrgb.example".into(), "6742".into())),
            ..Draft::default()
        };
        let text = shown_open_with(&v, &mut draft);
        // #795 O26: one amber line, the sentence on its ⓘ.
        assert!(text.contains("Host or port not valid"), "{text}");
        assert!(!text.contains("an IP address or localhost"), "{text}");
        // Typed another address: the draft holds it until Connect.
        let mut draft = Draft {
            openrgb: Some(("192.168.1.20".into(), "6743".into())),
            ..Draft::default()
        };
        let text = shown_open_with(&v, &mut draft);
        assert!(!text.contains("Host or port not valid"), "{text}");
        assert!(text.contains("192.168.1.20") && text.contains("Connect"));
    }

    #[test]
    fn the_second_desk_shows_its_own_devices_and_no_one_elses() {
        let v = sample_second_desk();
        let text = shown_open(&v);
        for want in [
            "Nimbus Strata case lights",
            "Top light bar",
            "Glass glow",
            "Lumen desk lightbar",
            "Windows lighting",
            // The fake driver: shown as driven, with its zone pickers.
            "Built-in support",
            "Apply colours",
        ] {
            assert!(text.contains(want), "{want}: {text}");
        }
        for not in [
            "OMEN",
            "Light Studio",
            "Corne",
            "G502",
            "left half",
            "right half",
        ] {
            assert!(!text.contains(not), "{not}: {text}");
        }
    }

    /// #558: a conflict is the catalog's app on its device, one line each, nobody's by name.
    #[test]
    fn conflicting_apps_warn_one_line_per_app_and_device() {
        use crate::foxglove::fixtures::second_desk as desk;
        let mut v = sample_second_desk();
        let one = |app: &str, device: &str| pc_lights::Conflicting {
            app: app.into(),
            device: device.into(),
            key: String::new(),
            hint: Some("Quit it from its tray icon.".into()),
        };
        v.pc.conflicts = vec![
            one(desk::CONFLICT_APP, "Nimbus Strata case lights"),
            one("Lumen Studio", "Lumen desk lightbar"),
        ];
        // #795 O2: one amber bar each, on screen; the sentence and the hint on its ⓘ.
        let casual = shown(&v);
        assert!(
            casual.contains("Nimbus Glow may override the Nimbus Strata case lights"),
            "{casual}"
        );
        assert!(casual.contains("Lumen Studio may override the Lumen desk lightbar"));
        assert!(!casual.contains("is running"), "{casual}");
        let text = reachable(&v);
        assert!(
            text.contains(
                "Nimbus Glow is running and may override Foxglove's colours on Nimbus Strata case lights."
            ),
            "{text}"
        );
        assert!(text.contains("Quit it from its tray icon."), "{text}");
        assert!(
            text.contains("Lumen Studio is running and may override Foxglove's colours on Lumen desk lightbar."),
            "{text}"
        );
        assert!(!text.contains("OMEN") && !text.contains("Light Studio"));
        // None running: no bar.
        let text = reachable(&sample_second_desk());
        assert!(
            !text.contains("is running") && !text.contains("may override"),
            "{text}"
        );
    }

    #[test]
    fn apply_sends_only_the_zones_that_changed() {
        let row = case([Some([1, 1, 1]), None, None, None, None], false);
        let mut d = Draft::default();
        assert!(d.changes(&row).is_empty());
        d.colors.insert(
            row.key.clone(),
            vec![
                Some([1, 1, 1]),
                Some([2, 2, 2]),
                None,
                None,
                Some([3, 3, 3]),
            ],
        );
        assert_eq!(d.changes(&row), vec![(1, [2, 2, 2]), (4, [3, 3, 3])]);
    }

    /// #617: each zone shows its colour as `#rrggbb` to type; a typed colour is a pick.
    #[test]
    fn each_zone_has_a_hex_box() {
        let v = sample();
        let case =
            v.pc.devices
                .iter()
                .find(|r| r.caps.per_zone() && r.zones.iter().any(|z| z.1.is_some()))
                .expect("a per-zone device with colours in the sample");
        // #795 O16: in each zone's swatch popup, one click away; not on the open row.
        let open = shown_open(&v);
        assert!(!open.contains("#f08a3c"), "{open}");
        let text = reachable(&v);
        assert!(
            text.contains("Diamond display: type a colour as #rrggbb, then Apply"),
            "{text}"
        );
        for c in case.zones.iter().filter_map(|z| z.1) {
            assert!(
                text.contains(&pc_lights::hex(c)),
                "{} in {text}",
                pc_lights::hex(c)
            );
        }
        assert_eq!(parse_typed("#A57500"), Some([0xa5, 0x75, 0x00]));
        assert_eq!(parse_typed(" a57500 "), Some([0xa5, 0x75, 0x00]));
        assert_eq!(parse_typed("#a575"), None);
        assert_eq!(parse_typed("red"), None);
    }

    /// #691: a Dynamic Lighting device Foxglove drives has the card any light has, from the
    /// effect table's LampArray row: its lamps by name, All lamps, Apply, Match, Identify and
    /// Off, and one line on what Windows lets Foxglove do. No "coming soon".
    #[test]
    fn a_lamp_array_gets_the_card_from_the_effect_table() {
        let text = shown_open(&sample());
        for want in [
            "G502 Proteus Spectrum",
            "Windows lighting",
            "Lamp 1 · Logo",
            "Lamp 2 · Accent",
            "ALL LAMPS",
            // #795 O11: a chip, the sentence on its hover.
            "in front only",
        ] {
            assert!(text.contains(want), "{want}: {text}");
        }
        assert!(reachable(&sample())
            .contains("Windows shows Foxglove's colours while Foxglove's window is in front"));
        assert!(!text.contains("coming soon"), "{text}");
        assert_eq!(
            text.matches("Match keyboard").count(),
            2,
            "the case and the mouse"
        );
        assert_eq!(text.matches("Identify").count(), 2);
        // The effect table decides: the LampArray's one effect takes a colour.
        let mouse = &sample().pc.devices[2];
        assert!(effect_of(mouse.control).is_some_and(|e| e.uses_colour()));
        // Another vendor's 12-lamp lightbar, which Windows keeps from Foxglove right now.
        let text = reachable(&sample_second_desk());
        assert!(text.contains("Lamp 12 · Accent"), "{text}");
        assert!(
            text.contains("while Foxglove's window is in front."),
            "{text}"
        );
    }

    /// #611's hide rule: more than 12 lamps is a per-key board, so one All lamps row and no
    /// picker per lamp; a lamp array not opened yet says why, with no controls.
    #[test]
    fn a_big_lamp_array_gets_one_row_and_an_unopened_one_no_controls() {
        let mut v = sample_second_desk();
        let bar = &mut v.pc.devices[1];
        bar.zones = (0..24).map(|i| (format!("Lamp {}", i + 1), None)).collect();
        bar.zone_ids = (0..24).map(|i| i.to_string()).collect();
        bar.lamp_access = Some(Default::default());
        let text = shown_open(&v);
        assert!(
            text.contains("ALL LAMPS") && !text.contains("Lamp 24"),
            "{text}"
        );
        v.pc.devices[1].lamp_access = None;
        // #795 O12, O13: grey chips on the row, the sentences on hover.
        assert!(shown(&v).contains("not in Windows right now"));
        let text = reachable(&v);
        assert!(
            text.contains("Windows lists no Dynamic Lighting device"),
            "{text}"
        );
        v.pc.devices[1].zones.clear();
        assert!(shown(&v).contains("reading lamps…"));
        let text = reachable(&v);
        assert!(text.contains("reading its lamps"), "{text}");
        let text = shown_open(&v);
        assert_eq!(text.matches("Match keyboard").count(), 1, "the case only");
    }

    /// A 40-LED fan hub through OpenRGB, its LEDs reported (no picker each: one All LEDs row).
    pub fn strip_row() -> Row {
        let mut row = openrgb_row("ARGB fan hub", 40, None);
        row.caps.zones = Some(pc_lighting::Zones::from_report(&[40]));
        row
    }

    /// #611's hide rules on Other devices (with the Hub's cut on #775): a device whose zones
    /// already show as named colour pickers (the case's 5 zones, the mouse's 2 lamps) gets no
    /// Show zones; a per-LED strip with one All row keeps it, with the source line and no
    /// zone-looks line; one colour gets no link and no pickers. No device offers Check zones
    /// (that's the keyboard's probe).
    #[test]
    fn device_zone_drawings_follow_the_hide_rules_per_kind() {
        use crate::foxglove::zones_ui::{CHECK, HIDE, PREVIEW_OPEN, SHOW};
        use foxglove_devices::pc_lighting::zones::ZONE_LOOKS;
        let mut v = sample();
        assert!(v.pc.devices.iter().all(|r| !offers_zones(r)));
        PREVIEW_OPEN.set(v.pc.devices.iter().map(|r| r.key.clone()).collect());
        let text = shown_open(&v);
        PREVIEW_OPEN.set(vec![]);
        for not in [SHOW, HIDE, ZONE_LOOKS, "From Foxglove's device list"] {
            assert!(!text.contains(not), "named pickers: {not}: {text}");
        }
        assert!(text.contains("Diamond display") && text.contains("Lamp 1 · Logo"));
        // The same mouse before Windows opens it: no pickers drawn, so the drawing shows its lamps.
        v.pc.devices[2].lamp_access = None;
        assert!(offers_zones(&v.pc.devices[2]));
        v.pc.devices[2].lamp_access = Some(Default::default());
        // A per-LED strip: one All LEDs row, so Show zones stays.
        v.pc.devices.push(strip_row());
        let closed = shown_open(&v);
        assert_eq!(closed.matches(SHOW).count(), 1, "the strip only: {closed}");
        PREVIEW_OPEN.set(v.pc.devices.iter().map(|r| r.key.clone()).collect());
        let open = shown_open(&v);
        PREVIEW_OPEN.set(vec![]);
        for want in [HIDE, "Reported by the device", "ALL LEDS"] {
            assert!(open.contains(want), "{want}: {open}");
        }
        assert!(
            !open.contains(ZONE_LOOKS) && !open.contains(CHECK),
            "{open}"
        );
        // One colour: no link, no pickers.
        let mut one = sample_fresh();
        one.pc.devices[0].caps.color = Color::Single;
        one.pc.devices[0].caps.zones = None;
        PREVIEW_OPEN.set(vec![one.pc.devices[0].key.clone()]);
        let text = shown_open(&one);
        PREVIEW_OPEN.set(vec![]);
        assert!(text.contains("1 zone (whole device)"), "{text}");
        for not in [SHOW, HIDE, ZONE_LOOKS, "Front top fan"] {
            assert!(!text.contains(not), "one colour: {not}: {text}");
        }
    }

    #[test]
    fn the_lamp_line_says_what_windows_lets_foxglove_do() {
        use pc_lighting::LampAccess;
        let on = LampAccess::default();
        let taken = LampAccess {
            available: false,
            ..on
        };
        let off = LampAccess {
            enabled: false,
            ..on
        };
        assert!(lamp_line(off, true).unwrap().contains("Turned off"));
        assert!(lamp_line(taken, true).unwrap().contains("Another app"));
        assert!(lamp_line(taken, false).unwrap().contains("in front."));
        assert!(lamp_line(on, false).unwrap().contains("background"));
        assert_eq!(lamp_line(on, true), None);
    }

    #[test]
    fn controls_have_plain_names() {
        assert_eq!(
            control_text(Control::Driver(Protocol::OmenTracer)),
            "Built-in support"
        );
        assert_eq!(control_text(Control::LampArray), "Windows lighting");
        assert_eq!(control_text(Control::NotYet), "Not controllable yet");
        assert_eq!(control_text(Control::OpenRgb), "Through OpenRGB");
    }

    /// #614: a device Foxglove can't drive keeps its own effect, named from the catalog when
    /// it lists one (the first), generic when it lists none.
    #[test]
    fn a_device_not_controllable_yet_keeps_its_own_effect() {
        let effects = |e: &[&str]| e.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            not_yet_text(&effects(&["rainbow"])),
            "Lights not controllable yet: it keeps its own effect (rainbow)."
        );
        assert_eq!(
            not_yet_text(&effects(&["colour cycle", "breathing"])),
            "Lights not controllable yet: it keeps its own effect (colour cycle)."
        );
        assert_eq!(
            not_yet_text(&[]),
            "Lights not controllable yet: it keeps its own effect."
        );
        assert_eq!(
            not_yet_text(&effects(&[" "])),
            "Lights not controllable yet: it keeps its own effect."
        );
        // From the data: the bundled cooler's entry says rainbow, the unconfirmed one nothing.
        use crate::foxglove::fixtures::users_desk;
        let cooler = users_desk::catalog_row(users_desk::COOLER);
        assert_eq!(cooler.control, Control::NotYet);
        assert_eq!(
            not_yet_text(&cooler.caps.effects),
            "Lights not controllable yet: it keeps its own effect (rainbow)."
        );
        let other = users_desk::catalog_row((0x03f0, 0x02be));
        assert_eq!(
            not_yet_text(&other.caps.effects),
            "Lights not controllable yet: it keeps its own effect."
        );
        // Its zone chip says the zones are how its lights are divided, not what Foxglove sets.
        assert_eq!(
            zones_tip(&cooler),
            "How the device's lights are divided. Foxglove can't set them yet."
        );
        let case = users_desk::catalog_row(users_desk::CASE);
        assert_eq!(zones_tip(&case), super::super::light_zones::TIP);
    }

    /// What a click on a device's thumbnail asks for (#804): two frames, the press and the
    /// release, over the thumbnail where the first frame put it.
    fn click_thumb(v: &View, key: &str, busy: bool) -> Vec<Intent> {
        use crate::foxglove::device_thumb;
        let mut v = v.clone();
        v.pc.busy = busy;
        let ctx = test_ctx();
        let mut draft = Draft::default();
        let mut wizard = Wizard::default();
        let mut frame = |events: Vec<egui::Event>| {
            let mut asked = Vec::new();
            let raw = egui::RawInput {
                events,
                ..Default::default()
            };
            let mut out = ctx.run_ui(raw, |ui| {
                asked = draw(ui, &v, &mut draft, &mut wizard);
            });
            out.textures_delta.clear();
            asked
        };
        frame(vec![]);
        let at = ctx
            .read_response(device_thumb::id(key))
            .expect("the row has a thumbnail")
            .rect
            .center();
        let button = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        let mut asked = frame(vec![egui::Event::PointerMoved(at), button(true)]);
        asked.extend(frame(vec![button(false)]));
        asked
    }

    /// #804: a driven device's thumbnail is Identify, the same request as its button; one
    /// Foxglove can't drive (the cooler) has a thumbnail that does nothing, and none is
    /// clickable while the worker is busy (as the button isn't).
    #[test]
    fn a_click_on_the_thumbnail_identifies_the_device() {
        let v = sample();
        let case = v.pc.devices[0].key.clone();
        let cooler = v.pc.devices[1].key.clone();
        let mouse = v.pc.devices[2].key.clone();
        assert_eq!(
            click_thumb(&v, &case, false),
            vec![Intent::Identify(case.clone())]
        );
        assert_eq!(
            click_thumb(&v, &mouse, false),
            vec![Intent::Identify(mouse)]
        );
        assert_eq!(click_thumb(&v, &cooler, false), vec![]);
        assert_eq!(click_thumb(&v, &case, true), vec![]);
        // The hover says so; the cooler's names its shape.
        let text = reachable(&v);
        assert!(
            text.contains(crate::foxglove::device_thumb::IDENTIFY),
            "{text}"
        );
    }

    /// A headless context with the app's fonts and theme.
    pub fn test_ctx() -> egui::Context {
        let ctx = egui::Context::default();
        let mut fonts = egui::FontDefinitions::default();
        crate::platform::add_phosphor_to_fonts(&mut fonts);
        theme::add_fonts(&mut fonts);
        ctx.set_fonts(fonts);
        theme::apply_choice(&ctx, theme::Choice::Dark);
        ctx
    }

    /// The card as the casual viewer sees it: every device row and More sources closed.
    pub fn shown(v: &View) -> String {
        frame_text(
            v,
            &mut Draft::default(),
            &mut Wizard::default(),
            false,
            false,
        )
    }

    /// The card with every device row and More sources opened (no popup, no tooltip).
    fn shown_open(v: &View) -> String {
        shown_open_with(v, &mut Draft::default())
    }

    fn shown_open_with(v: &View, draft: &mut Draft) -> String {
        frame_text(v, draft, &mut Wizard::default(), true, false)
    }

    /// Everything the card puts within one hover or one click for `v` (#795): what the casual
    /// viewer sees, the rows opened, and a frame with egui's "everything is visible", where
    /// every tooltip, ⓘ, popup (a zone's swatch, the ⋯ menu) and zone drawing shows.
    pub fn reachable(v: &View) -> String {
        reachable_with(v, &mut Draft::default(), &mut Wizard::default())
    }

    fn reachable_with(v: &View, draft: &mut Draft, wizard: &mut Wizard) -> String {
        let mut text = frame_text(v, draft, &mut wizard.clone(), false, false);
        text.push_str(&frame_text(v, draft, &mut wizard.clone(), true, false));
        text.push_str(&frame_text(v, draft, &mut wizard.clone(), false, true));
        text
    }

    /// The text of the card's last frame of three (a new tooltip's first frame only measures
    /// it), with the rows `open` and egui's "everything is visible" as `everything` says.
    fn frame_text(
        v: &View,
        draft: &mut Draft,
        wizard: &mut Wizard,
        open: bool,
        everything: bool,
    ) -> String {
        use crate::foxglove::settings_ui::set_expander_open;
        let ctx = test_ctx();
        if open {
            for row in &v.pc.devices {
                set_expander_open(&ctx, &row_key(&row.key), true);
            }
            set_expander_open(&ctx, MORE_SOURCES, true);
        }
        ctx.memory_mut(|m| m.set_everything_is_visible(everything));
        let mut text = String::new();
        for _ in 0..3 {
            let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
                draw(ui, v, draft, wizard);
            });
            out.textures_delta.clear();
            text.clear();
            for clipped in &out.shapes {
                collect_text(&clipped.shape, &mut text);
            }
        }
        text
    }

    fn collect_text(shape: &egui::Shape, out: &mut String) {
        match shape {
            egui::Shape::Text(t) => {
                out.push_str(t.galley.text());
                out.push('\n');
            }
            egui::Shape::Vec(v) => v.iter().for_each(|s| collect_text(s, out)),
            _ => {}
        }
    }

    #[test]
    fn controls_follow_capabilities() {
        let mut v = sample_fresh();
        let text = shown_open(&v);
        assert!(text.contains("Apply colours") && text.contains("Front top fan"));
        assert!(text.contains("Identify") && text.contains("Match keyboard"));
        assert!(
            text.contains("Calibrate…"),
            "a driven colour device can be calibrated"
        );
        // The same board, said to take only one colour: no zone pickers, still a colour.
        v.pc.devices[0].caps.color = Color::Single;
        let text = shown_open(&v);
        assert!(!text.contains("Front top fan"), "{text}");
        assert!(text.contains("Match keyboard") && text.contains("COLOUR"));
        // No lights at all: no colour controls, no Off or Identify.
        v.pc.devices[0].caps = LightCaps::default();
        let text = shown_open(&v);
        assert!(!text.contains("Match keyboard") && !text.contains("Identify"));
        assert!(!text.contains("Calibrate…"));
        assert!(text.contains("No lights Foxglove knows of"));
    }

    /// The keyboard has its own card (#533): no "Foxglove firmware" row here. A device you added
    /// keeps Copy entry and Remove behind ⋯ (More ▸ before #795), and Remove asks again before it forgets.
    #[test]
    fn the_keyboard_is_not_listed_and_more_holds_the_entry_actions() {
        let mut v = sample();
        let text = reachable(&v);
        assert!(!text.contains("Foxglove firmware"), "{text}");
        // Any text: the menu shows for an entry; Remove… only for one that parses. #795 O22:
        // the menu is "⋯" on the row, closed or open.
        v.pc.devices[0].entry = Some("[[device]]".into());
        let text = shown(&v);
        assert!(text.contains("⋯"), "{text}");
        assert!(!text.contains("Copy entry") && !text.contains("Forget your entry"));
        let mut draft = Draft {
            removing: Some(v.pc.devices[0].key.clone()),
            ..Draft::default()
        };
        // On screen with the row closed.
        let text = frame_text(&v, &mut draft, &mut Wizard::default(), false, false);
        assert!(
            text.contains("Forget your entry for this device?"),
            "{text}"
        );
        assert!(text.contains("Keep"));
    }

    /// A state of the card for the inventory test: the view, the draft and the wizard.
    struct Case {
        what: &'static str,
        view: View,
        draft: Draft,
        wizard: Wizard,
        /// The sentences the card had in this state (docs/design/...-declutter.md, O1–O31).
        wants: Vec<String>,
    }

    /// #795: nothing is removed. Every sentence the Other devices card showed before the
    /// declutter (the inventory's rows O1–O31, in the old words) still renders somewhere in its
    /// state: on the card, on an opened row or in More sources, or as hover text in a headless
    /// frame with everything visible (tooltips, the ⓘ, the swatch popups, the ⋯ menu). The only
    /// words that went are the "More ▸" menu's name, now "⋯" (O22).
    #[test]
    fn every_other_devices_sentence_is_still_reachable() {
        use super::super::lights_page::SWATCHES;
        use super::super::pc_lights_add::{tests::sample_zones, Step, ADD_TIP};
        use crate::foxglove::zones_ui::{HIDE, SHOW};
        let s = |v: &[&str]| v.iter().map(|t| t.to_string()).collect::<Vec<_>>();
        let case = |what, view, wants| Case {
            what,
            view,
            draft: Draft::default(),
            wizard: Wizard::default(),
            wants,
        };
        let calibrate_tip = "Match how this device shows colours to your screen or another \
                             device: white, red, green, blue and grey, about a minute";
        let lamp = |access: pc_lighting::LampAccess| {
            let mut v = sample();
            v.pc.devices[2].lamp_access = Some(access);
            v
        };
        let off_in_windows = lamp(pc_lighting::LampAccess {
            enabled: false,
            available: true,
        });
        let mut unopened = sample();
        unopened.pc.devices[2].lamp_access = None;
        let mut reading = unopened.clone();
        reading.pc.devices[2].zones.clear();
        let mut added = sample();
        added.pc.devices[0].entry = Some(
            CatalogDevice::new("Desk strip".into(), 0x1209, 0xa1e5, Protocol::None, vec![])
                .to_toml(),
        );
        let mut calibrated = sample();
        calibrated.pc.devices[0].calibrated = true;
        let mut strip = sample();
        strip.pc.devices.push(strip_row());
        let mut changed = Draft::default();
        changed.colors.insert(
            sample().pc.devices[0].key.clone(),
            vec![Some([1, 2, 3]), None, None, None, None],
        );
        let mut one_colour = sample_fresh();
        one_colour.pc.devices[0].caps.color = Color::Single;
        one_colour.pc.devices[0].caps.zones = None;
        let mut no_lights = sample_fresh();
        no_lights.pc.devices[0].caps = LightCaps::default();
        let swatch_names: Vec<String> = SWATCHES.iter().map(|(n, _)| n.to_string()).collect();
        let mut cases = vec![
            case(
                "the user's desk, Light Studio running",
                sample(),
                [
                    // O1, O2
                    s(&[
                        ABOUT,
                        "OMEN Light Studio is running and may override Foxglove's colours on \
                         HP OMEN 45L case lights.",
                        "Settings > Apps > Installed apps > OMEN Light Studio",
                    ]),
                    // O4, O5, O6
                    s(&[
                        "HP OMEN 45L case lights",
                        "Built-in support",
                        "HP OMEN liquid cooler",
                        "Not controllable yet",
                        "G502 Proteus Spectrum",
                        "Windows lighting",
                        "5 lighting zones",
                        "Per-LED lighting, 2 lamps",
                        super::super::light_zones::TIP,
                        "Screen · images & animations",
                    ]),
                    // O8
                    s(&[
                        "Calibrate…",
                        calibrate_tip,
                        "Off",
                        "Turn every zone dark",
                        "Identify",
                        "Blink this device for a moment, then put its colours back",
                    ]),
                    // O11, O14
                    s(&[
                        "Windows shows Foxglove's colours while Foxglove's window is in front; \
                         in the background its own Dynamic Lighting takes over.",
                        // #614: the cooler's hover names the effect it keeps.
                        "Lights not controllable yet: it keeps its own effect (rainbow).",
                    ]),
                    // O15, O16
                    s(&[
                        "Diamond display",
                        "Front top fan",
                        "Lamp 1 · Logo",
                        "Diamond display: pick a colour, then Apply",
                        "Diamond display: type a colour as #rrggbb, then Apply",
                        "#f08a3c",
                        "#5fc48e",
                    ]),
                    // O18, O19, O20
                    s(&[
                        "ALL ZONES",
                        "ALL LAMPS",
                        "Apply colours",
                        "Match keyboard",
                        MATCH_TIP,
                    ]),
                    swatch_names.clone(),
                    // O23, O24, O29
                    s(&["Use OpenRGB's devices", OPENRGB_TIP, OPENRGB_OFF]),
                    s(&["Add a device…", ADD_TIP]),
                    // O31
                    s(&["HP OMEN 45L case lights: colours set"]),
                ]
                .concat(),
            ),
            // O3
            case(
                "nothing found",
                View::default(),
                s(&["No PC or peripheral lights found."]),
            ),
            // O4: added by you, calibrated.
            case(
                "a calibrated device",
                calibrated,
                s(&["Built-in support", "· calibrated"]),
            ),
            // O6: one colour, no lights.
            case(
                "one colour",
                one_colour,
                s(&["1 zone (whole device)", "COLOUR"]),
            ),
            case(
                "no lights",
                no_lights,
                s(&["No lights Foxglove knows of"]),
            ),
            // O7
            case(
                "a per-LED strip",
                strip,
                s(&[SHOW, HIDE, "Reported by the device", "ALL LEDS"]),
            ),
            // O9
            case(
                "turned off in Windows",
                off_in_windows,
                s(&["Turned off in Windows' Dynamic Lighting settings."]),
            ),
            // O11 without package identity, the lightbar Windows keeps.
            case(
                "the second desk",
                sample_second_desk(),
                s(&[
                    "Windows shows Foxglove's colours here while Foxglove's window is in front.",
                    "Lamp 12 · Accent",
                ]),
            ),
            // O12, O13
            case(
                "Windows lists no device",
                unopened,
                s(&["Windows lists no Dynamic Lighting device for it right now."]),
            ),
            case(
                "reading lamps",
                reading,
                s(&["Windows lighting: reading its lamps from Windows."]),
            ),
            // O15: not set.
            case(
                "nothing set yet",
                sample_fresh(),
                s(&["not set"]),
            ),
            // O22
            case(
                "a device you added",
                added.clone(),
                s(&[
                    "· added by you",
                    "Copy entry, Remove",
                    "Copy entry",
                    "Copy this device's catalog entry (TOML) to share it, or to contribute it \
                     to Foxglove's lighting-catalog.toml",
                    "Remove…",
                    "Forget your entry for this device",
                ]),
            ),
            // O25, O27, O28
            case(
                "OpenRGB on",
                sample_openrgb(),
                s(&[
                    "Through OpenRGB",
                    "HOST",
                    "PORT",
                    "OpenRGB's address: an IP address, or localhost",
                    "Connect",
                    "Reconnect automatically",
                    "A few tries, a little longer apart each time, then Foxglove stops",
                    "Connected to OpenRGB at 127.0.0.1:6742: 2 devices",
                    OPENRGB_TIP,
                ]),
            ),
            case(
                "OpenRGB down",
                sample_openrgb_down(),
                // Connect is enabled while it isn't connected: its hover shows then.
                s(&[
                    "Can't reach OpenRGB at 127.0.0.1:6742 (connection refused); trying again in 4 s",
                    "Connect to OpenRGB at this address",
                ]),
            ),
        ];
        // O15: a changed zone.
        cases.push(Case {
            what: "a zone changed",
            view: sample(),
            draft: changed,
            wizard: Wizard::default(),
            wants: s(&["changed"]),
        });
        // O21
        cases.push(Case {
            what: "Remove clicked",
            view: added.clone(),
            draft: Draft {
                removing: Some(added.pc.devices[0].key.clone()),
                ..Draft::default()
            },
            wizard: Wizard::default(),
            wants: s(&["Forget your entry for this device?", "Remove", "Keep"]),
        });
        // O26
        cases.push(Case {
            what: "a typed address that isn't one",
            view: sample_openrgb(),
            draft: Draft {
                openrgb: Some(("openrgb.example".into(), "6742".into())),
                ..Draft::default()
            },
            wizard: Wizard::default(),
            wants: s(&["Host: an IP address or localhost; port: 1 to 65535."]),
        });
        // O30: the wizard, unchanged.
        let mut setup = Wizard::default();
        setup.pick(unknown_strip());
        let review = Wizard {
            step: Step::Review,
            ..setup.clone()
        };
        for (what, view, wizard, wants) in [
            (
                "Add a device: pick",
                sample(),
                Wizard {
                    step: Step::Pick,
                    ..Wizard::default()
                },
                s(&[
                    "Add a device: which one?",
                    "Desk strip",
                    "1209:A1E5",
                    "Choose",
                    "Cancel",
                ]),
            ),
            (
                "Add a device: pick, nothing unknown",
                sample_fresh(),
                Wizard {
                    step: Step::Pick,
                    ..Wizard::default()
                },
                s(&["Every HID device on this PC is recognised already."]),
            ),
            (
                "Add a device: set up",
                sample(),
                setup,
                s(&[
                    "Add a device: 1209:A1E5",
                    "Name",
                    "Protocol",
                    "Foxglove will list it and remember its name, but can't light it.",
                    "Review",
                ]),
            ),
            (
                "Add a device: name the zones",
                sample(),
                sample_zones(),
                s(&["Name the zones: zone", "What lit up?", "Done naming"]),
            ),
            (
                "Add a device: review",
                sample(),
                review,
                s(&[
                    "Add a device: review",
                    "Saved to",
                    "Copy entry shares it later.",
                    "Save",
                ]),
            ),
        ] {
            cases.push(Case {
                what,
                view,
                draft: Draft::default(),
                wizard,
                wants,
            });
        }
        for mut case in cases {
            let text = reachable_with(&case.view, &mut case.draft, &mut case.wizard);
            for want in &case.wants {
                assert!(
                    text.contains(want.as_str()),
                    "{}: {want:?} is no longer on the card or in its hovers:\n{text}",
                    case.what
                );
            }
        }
        // O10 needs package identity, which a test can't have: its chip, from the same data.
        let mut taken = sample().pc.devices[2].clone();
        taken.lamp_access = Some(pc_lighting::LampAccess {
            enabled: true,
            available: false,
        });
        let chip = lamp_chip(&taken, true).expect("a chip");
        assert_eq!(chip.text, "another app has it");
        assert_eq!(chip.dot, theme::Dot::Warn);
        assert!(chip.settings, "with the Dynamic Lighting settings button");
        assert_eq!(
            chip.hover,
            "Another app has these lights now. Settings > Personalization > Dynamic Lighting \
             sets which app comes first."
        );
    }

    /// #795: the casual viewer sees each device's name, how it's controlled, its state as chips
    /// and Identify, one row each; the explanations, the zones and the colours are one hover or
    /// one click away. Amber stays on screen: the conflict bar, and OpenRGB's chip while it's on
    /// and down.
    #[test]
    fn rows_are_collapsed_and_amber_stays_on_screen() {
        let casual = shown(&sample());
        for want in [
            "Other devices",
            "OMEN Light Studio may override the HP OMEN 45L case lights",
            "HP OMEN 45L case lights",
            "Built-in support",
            "5 zones",
            "2 lamps",
            "in front only",
            "Identify",
            "More sources",
            "OpenRGB off",
            "Colours set",
        ] {
            assert!(casual.contains(want), "{want}: {casual}");
        }
        for hidden in [
            ABOUT,
            "is running and may override",
            "5 lighting zones",
            "Diamond display",
            "Apply colours",
            "Calibrate…",
            "Match keyboard",
            "Use OpenRGB's devices",
            "Add a device…",
            "in the background its own Dynamic Lighting takes over",
        ] {
            assert!(
                !casual.contains(hidden),
                "{hidden} is on the card: {casual}"
            );
        }
        assert_eq!(
            casual.matches("Identify").count(),
            2,
            "the case and the mouse"
        );
        // OpenRGB on: its count; on and down: an amber chip on the closed header.
        assert!(shown(&sample_openrgb()).contains("OpenRGB: 2 devices"));
        let down = shown(&sample_openrgb_down());
        assert!(
            down.contains("OpenRGB on") && down.contains("OpenRGB not connected"),
            "{down}"
        );
        assert!(
            !down.contains("Can't reach"),
            "the status is the chip's hover: {down}"
        );
        assert!(!shown(&sample_openrgb()).contains("not connected"));
        // Each row is remembered on its own: opening one opens only it.
        let ctx = test_ctx();
        let v = sample();
        crate::foxglove::settings_ui::set_expander_open(&ctx, &row_key(&v.pc.devices[0].key), true);
        assert!(crate::foxglove::settings_ui::expander_open(
            &ctx,
            &row_key(&v.pc.devices[0].key)
        ));
        assert!(!crate::foxglove::settings_ui::expander_open(
            &ctx,
            &row_key(&v.pc.devices[2].key)
        ));
        assert!(!crate::foxglove::settings_ui::expander_open(
            &ctx,
            MORE_SOURCES
        ));
    }

    /// A3b: a result is short where it shows: no device name its row already shows, a capital
    /// first, and at most the info bar's 60 characters, cut at a word.
    #[test]
    fn a_result_reads_short_on_its_row() {
        let name = Some("HP OMEN 45L case lights");
        assert_eq!(
            status_short("HP OMEN 45L case lights: colours set", name),
            "Colours set"
        );
        assert_eq!(
            status_short("That device isn't connected now", name),
            "That device isn't connected now"
        );
        assert_eq!(
            status_short("Zone 3: blinked, back to its colour", None),
            "Zone 3: blinked, back to its colour"
        );
        let long = "Match keyboard: the keyboard plays Swirl, which HP OMEN 45L case lights \
                    can't match from Foxglove, so it keeps its colours";
        let short = status_short(long, name);
        assert!(short.chars().count() <= theme::BAR_MAX_CHARS, "{short}");
        assert!(
            short.ends_with('…') && short.starts_with("Match keyboard: the keyboard plays Swirl"),
            "{short}"
        );
        assert!(!short.contains(" …"), "cut at a word: {short}");
    }

    /// A3b: a confirmation fades after DONE_FOR; a note or a problem stays until the next
    /// result; each shows only in its own place.
    #[test]
    fn confirmations_fade_and_problems_stay() {
        let row = Place::Row("103c:84fd".into());
        let mut done = Status::new(row.clone(), Tone::Done, "x: colours set");
        assert!(placed(std::slice::from_ref(&done), &row).is_some());
        assert!(
            placed(std::slice::from_ref(&done), &Place::Card).is_none(),
            "only on its row"
        );
        assert!(placed(std::slice::from_ref(&done), &Place::Row("other".into())).is_none());
        let Some(old) = std::time::Instant::now()
            .checked_sub(pc_lights::DONE_FOR + std::time::Duration::from_secs(1))
        else {
            return; // a clock this young can't go back that far
        };
        done.at = old;
        assert!(
            placed(std::slice::from_ref(&done), &row).is_none(),
            "a confirmation fades"
        );
        for tone in [Tone::Note, Tone::Problem] {
            let mut st = Status::new(row.clone(), tone, "x: not connected");
            st.at = old;
            assert!(
                placed(std::slice::from_ref(&st), &row).is_some(),
                "{tone:?} stays"
            );
        }
    }

    /// A3b: the result shows next to what was clicked. A confirmation is a chip on the device's
    /// row (closed rows too), a problem an amber bar under it; neither is a loose line at the
    /// card's foot, and the card-wide ones go to its top.
    #[test]
    fn results_show_on_the_row_that_acted() {
        let mut v = sample();
        let case = "103c:84fd".to_string();
        let casual = shown(&v);
        let chip = casual
            .find("Colours set")
            .expect("the chip on the case's row");
        let mouse = casual.find("G502").expect("the mouse's row");
        assert!(
            chip < mouse,
            "on the case's row, before the next row: {casual}"
        );
        assert!(
            !casual.contains("HP OMEN 45L case lights: colours set"),
            "the hover only"
        );

        v.pc.said(Status::new(
            Place::Row(case.clone()),
            Tone::Problem,
            "HP OMEN 45L case lights: not connected",
        ));
        let casual = shown(&v);
        let bar = casual
            .find("Not connected")
            .expect("the bar under the case's row");
        assert!(bar < casual.find("G502").unwrap(), "{casual}");
        let reach = reachable(&v);
        assert!(
            reach.contains("HP OMEN 45L case lights: not connected"),
            "the ⓘ has it all"
        );

        // Faded: nothing left anywhere on the card.
        if let Some(old) = std::time::Instant::now()
            .checked_sub(pc_lights::DONE_FOR + std::time::Duration::from_secs(1))
        {
            let mut st = Status::new(
                Place::Row(case),
                Tone::Done,
                "HP OMEN 45L case lights: colours set",
            );
            st.at = old;
            v.pc.said(st);
            assert!(!reachable(&v).contains("olours set"), "faded");
        }

        // Card-wide: above the first row, after the conflicts.
        v.pc.said(Status::new(
            Place::Card,
            Tone::Problem,
            "Can't save C:\\x\\lighting-catalog.local.toml: access denied",
        ));
        let casual = shown(&v);
        let save = casual.find("Can't save").expect("the card's bar");
        let first_row = casual.find("Built-in support").expect("the case's row");
        assert!(save < first_row, "above the first row: {casual}");

        // Add a device's result: on More sources' header, open or closed.
        v.pc.said(Status::new(
            Place::Sources,
            Tone::Done,
            "Desk strip: saved as your own device",
        ));
        let casual = shown(&v);
        assert!(
            casual.contains("Desk strip: saved as your own device"),
            "{casual}"
        );
        assert!(casual.find("Desk strip: saved").unwrap() > casual.find("More sources").unwrap());

        // The keyboard card's own results don't land on Other devices.
        v.pc.said(Status::new(
            Place::Keyboard,
            Tone::Done,
            "Eyelash Corne: shown through Windows Dynamic Lighting",
        ));
        assert!(!reachable(&v).contains("shown through Windows"));
    }

    /// The conflict bar's line fits the bar's 60 characters, a long name cut with "…".
    #[test]
    fn the_conflict_line_fits_the_info_bar() {
        assert_eq!(
            conflict_short("OMEN Light Studio", "HP OMEN 45L case lights"),
            "OMEN Light Studio may override the HP OMEN 45L case lights"
        );
        for (app, device) in [
            (
                "Nimbus Glow",
                "Nimbus Strata case lights with a very long name indeed",
            ),
            (
                "An app with a name far longer than anyone would give it",
                "A device with a long name too",
            ),
        ] {
            let line = conflict_short(app, device);
            assert!(line.chars().count() <= theme::BAR_MAX_CHARS, "{line}");
            assert!(
                line.contains(" may override the ") && line.ends_with('…'),
                "{line}"
            );
        }
    }

    /// The collapsed row's chips (#795 O5): the zone count, LEDs or lamps, one zone, on/off.
    #[test]
    fn the_light_chip_says_how_finely_in_a_few_words() {
        let v = sample();
        assert_eq!(light_chip(&v.pc.devices[0]).as_deref(), Some("5 zones"));
        assert_eq!(light_chip(&v.pc.devices[1]), None);
        assert_eq!(light_chip(&v.pc.devices[2]).as_deref(), Some("2 lamps"));
        assert_eq!(light_chip(&strip_row()).as_deref(), Some("40 LEDs"));
        let mut one = sample_fresh().pc.devices[0].clone();
        one.caps.color = Color::Single;
        one.caps.zones = None;
        assert_eq!(light_chip(&one).as_deref(), Some("1 zone"));
        one.caps.color = Color::OnOff;
        assert_eq!(light_chip(&one).as_deref(), Some("on/off"));
    }
}
