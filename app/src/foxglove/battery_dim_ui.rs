//! "Save battery: dim lights on battery" on the Lights page: one switch ("Dim on battery" and
//! an ⓘ), the part being dimmed as a chip in the Keyboard card's header ([`chip`], #794 K27),
//! and the levels behind the "Adjust levels…" expander, collapsed. The keyboard does the dimming itself (firmware
//! battery_dim.c, D7 0E): each half caps its underglow by its own cable and charge, on top of the
//! brightness the user picked, so it works without the app. Defaults: 100% on USB, 80% on
//! battery, 12% below 20% charge.
//!
//! No polling: the state is read when the page first shows it and on each new connection, and
//! then follows the keyboard's own D7 8E reports (sent when a half's level or cable changes), plus
//! a fresh read when the right's battery level changes (its unsolicited HELLO).

use std::sync::{Arc, Mutex, Weak};

use egui::RichText;
use foxglove_link::link::{EventSubscription, Link};
use foxglove_link::protocol::{self, BatteryDim, BatteryDimConfig, BatteryDimHalf};

use super::theme;

/// What Foxglove knows about the keyboard's battery dim.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DimState {
    /// Not asked yet.
    Unknown,
    Asking,
    NoKeyboard,
    /// The firmware has no D7 0E (older than this feature): HELLO lacks the capability bit.
    NotSupported,
    /// The firmware has D7 0E, but the read went unanswered twice (a reply lost while the
    /// keyboard switched between USB and Bluetooth, say). Asked again on ↻ or a new connection.
    NoAnswer,
    Known(BatteryDim),
}

/// What the switch and the sliders ask for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Intent {
    Set(BatteryDimConfig),
    Ask,
}

/// The levels being dragged but not sent yet.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Draft {
    pub config: Option<BatteryDimConfig>,
    /// The previews show "Adjust levels…" open.
    #[cfg(test)]
    pub open: bool,
}

/// The switch's whole name; its short label is [`SHORT`].
pub const LABEL: &str = "Save battery: dim lights on battery";
/// The switch's label on the card (#794 K26).
pub const SHORT: &str = "Dim on battery";
/// What the switch does: its hover and ⓘ, after [`LABEL`].
pub const TIP: &str = "Each half dims its own lights while it runs on battery, and a lot more \
                       when its battery is low. On USB it shows your brightness. Saved on the \
                       keyboard, so it works without Foxglove.";
/// The switch off, in its hover (#794 K28).
pub const OFF: &str = "Off: your brightness on battery too.";
/// "Adjust levels…"'s session memory, its header and its hover.
pub const LEVELS_KEY: &str = "battery-dim-levels";
pub const LEVELS: &str = "Adjust levels…";
pub const LEVELS_TIP: &str = "The brightness on power, on battery and on low battery";

/// The switch's hover: the whole label, what it does, and what off means while it is off.
pub fn hover(on: bool) -> String {
    if on {
        format!("{LABEL}: {TIP}")
    } else {
        format!("{LABEL}: {TIP}\n{OFF}")
    }
}

impl Draft {
    /// Drops the drafted levels once the keyboard reports them.
    pub fn settle(&mut self, state: &DimState) {
        if let (DimState::Known(d), Some(c)) = (state, self.config) {
            if d.config == c {
                self.config = None;
            }
        }
    }
}

static STATE: Mutex<(Option<u64>, DimState)> = Mutex::new((None, DimState::Unknown));
static DRAFT: Mutex<Draft> = Mutex::new(Draft {
    config: None,
    #[cfg(test)]
    open: false,
});
/// The report tap on the keyboard's link, for its unsolicited D7 8E and HELLO.
static TAP: Mutex<Option<(Weak<Link>, EventSubscription)>> = Mutex::new(None);

#[cfg(test)]
thread_local! {
    /// The previews' state, so they never ask a real keyboard.
    pub static PREVIEW: std::cell::RefCell<Option<(DimState, Draft)>> =
        const { std::cell::RefCell::new(None) };
}

fn set_state(generation: Option<u64>, state: DimState) {
    *STATE.lock().unwrap_or_else(|e| e.into_inner()) = (generation, state);
}

/// One D7 0E at a time: a request made while one runs is queued (the newest write wins) and
/// sent when that one has answered.
static RUNNING: Mutex<Option<Queued>> = Mutex::new(None);

#[derive(Clone, Copy)]
struct Queued {
    /// Another request came in meanwhile.
    again: bool,
    set: Option<BatteryDimConfig>,
}

/// Reads the levels and both halves' state (`set` = `None`) or sets the levels, in the
/// background; `ctx` repaints when the answer is in.
pub fn ask(set: Option<BatteryDimConfig>, ctx: egui::Context) {
    let set = set.filter(|_| !super::pause::paused());
    {
        let mut running = RUNNING.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(q) = running.as_mut() {
            q.again = true;
            q.set = set.or(q.set);
            return;
        }
        *running = Some(Queued { again: false, set });
        let mut s = STATE.lock().unwrap_or_else(|e| e.into_inner());
        if !matches!(s.1, DimState::Known(_)) {
            s.1 = DimState::Asking;
        }
    }
    let _ = super::services::spawn_supervised("battery-dim", move || {
        let mut set = set;
        loop {
            request(set, &ctx);
            let mut running = RUNNING.lock().unwrap_or_else(|e| e.into_inner());
            match *running {
                Some(q) if q.again => {
                    set = q.set;
                    *running = Some(Queued {
                        again: false,
                        set: None,
                    });
                }
                _ => {
                    *running = None;
                    break;
                }
            }
        }
    });
}

/// How long after an unanswered D7 0E the one retry goes out: long enough for a USB/Bluetooth
/// switch to settle (the reply lost at 2026-10-26 23:57:39 came 2 s after a reconnect).
const RETRY_AFTER: std::time::Duration = std::time::Duration::from_secs(1);

/// One D7 0E (retried once if unanswered) and its answer into the cache.
fn request(set: Option<BatteryDimConfig>, ctx: &egui::Context) {
    let (generation, state) = match super::devices::device() {
        None => (super::devices::generation(), DimState::NoKeyboard),
        Some(d) => {
            tap(d.link(), ctx);
            // The connection asked, read after `device()`: it may have just found the keyboard,
            // and a generation read before that made `state` ask a second time (two "no answer"
            // lines at 23:57:39).
            let generation = Some(d.link().state().generation);
            let state = answer(
                || d.battery_dim(set),
                || std::thread::sleep(RETRY_AFTER),
                current(),
            );
            (generation, state)
        }
    };
    set_state(generation, state);
    ctx.request_repaint();
}

/// The state after asking with `ask`: [`DimState::NotSupported`] only when the device says the
/// firmware lacks D7 0E (its HELLO capability bit, checked before any I/O). Any other failure is
/// a lost reply: `pause` once, ask once more, then keep what was `known`, else
/// [`DimState::NoAnswer`]. Bounded: at most two asks per request.
fn answer(
    mut ask: impl FnMut() -> Result<BatteryDim, foxglove_devices::DeviceError>,
    pause: impl FnOnce(),
    known: DimState,
) -> DimState {
    let mut pause = Some(pause);
    loop {
        match ask() {
            Ok(dim) => return DimState::Known(dim),
            Err(foxglove_devices::DeviceError::Unsupported) => return DimState::NotSupported,
            Err(e) => match pause.take() {
                Some(pause) => pause(),
                None => {
                    eprintln!("Foxglove: battery dim: {}", super::devices::describe(&e));
                    return match known {
                        DimState::Known(_) => known,
                        _ => DimState::NoAnswer,
                    };
                }
            },
        }
    }
}

/// Follows the keyboard's own reports on `link`: D7 8E updates the state at once; a HELLO (the
/// right's battery changed) asks again so the right's half is fresh.
fn tap(link: &Arc<Link>, ctx: &egui::Context) {
    let mut tap = TAP.lock().unwrap_or_else(|e| e.into_inner());
    if tap
        .as_ref()
        .is_some_and(|(l, _)| l.upgrade().is_some_and(|l| Arc::ptr_eq(&l, link)))
    {
        return;
    }
    let ctx = ctx.clone();
    // Runs on the link's reader thread: parse, store, repaint; anything slower is spawned.
    let subscription = link.subscribe_reports(move |generation, r| {
        if protocol::is_reply(r, protocol::SETTINGS, protocol::settings::BATTERY_DIM) {
            let dim = BatteryDim::parse(r);
            if dim.status == 0 {
                set_state(Some(generation), DimState::Known(dim));
                ctx.request_repaint();
            }
        } else if protocol::is_reply(r, protocol::PANEL, protocol::panel::HELLO)
            && matches!(current(), DimState::Known(_))
        {
            ask(None, ctx.clone());
        }
    });
    *tap = Some((Arc::downgrade(link), subscription));
}

fn current() -> DimState {
    STATE.lock().unwrap_or_else(|e| e.into_inner()).1
}

/// The cached state; asks the keyboard on first look and after each new connection.
fn state(ctx: &egui::Context) -> DimState {
    let generation = super::devices::generation();
    let (seen, s) = *STATE.lock().unwrap_or_else(|e| e.into_inner());
    let stale = s == DimState::Unknown
        || (seen != generation && s != DimState::Asking)
        || (generation.is_none() && matches!(s, DimState::Known(_)));
    if stale {
        ask(None, ctx.clone());
    }
    s
}

/// The switch and its levels, inside the Lights page's Keyboard card.
pub fn show(ui: &mut egui::Ui) {
    #[cfg(test)]
    if let Some((s, mut d)) = PREVIEW.with_borrow(|v| *v) {
        if d.open {
            super::settings_ui::set_expander_open(ui.ctx(), LEVELS_KEY, true);
        }
        draw(ui, &s, &mut d);
        return;
    }
    let ctx = ui.ctx().clone();
    let s = state(&ctx);
    let mut draft = DRAFT.lock().unwrap_or_else(|e| e.into_inner());
    draft.settle(&s);
    for intent in draw(ui, &s, &mut draft) {
        match intent {
            Intent::Set(c) => ask(Some(c), ctx.clone()),
            Intent::Ask => ask(None, ctx.clone()),
        }
    }
}

/// One part in words: "Right half on battery · 80%", "Keyboard on USB · 100%",
/// "Right half on low battery (15%) · 12%".
pub fn half_text(name: &str, h: &BatteryDimHalf) -> String {
    let power = if h.usb {
        "on USB".to_string()
    } else if h.low {
        match h.charge {
            Some(c) => format!("on low battery ({c}%)"),
            None => "on low battery".to_string(),
        }
    } else {
        "on battery".to_string()
    };
    format!("{name} {power} · {}%", h.level)
}

/// The Keyboard card's header chip while dimming is on (#794 K27), from the cached state: the
/// part being dimmed ("Right on battery · dims to 80%"), else "On USB · 100%"; every part's
/// line on its hover.
pub fn chip(ui: &mut egui::Ui) {
    #[cfg(test)]
    let s = match PREVIEW.with_borrow(|v| *v) {
        Some((s, _)) => s,
        None => return,
    };
    #[cfg(not(test))]
    let s = state(ui.ctx());
    let DimState::Known(d) = s else { return };
    let config = DRAFT.lock().unwrap_or_else(|e| e.into_inner()).config;
    if !config.unwrap_or(d.config).enabled {
        return;
    }
    let parts = super::parts::Parts::current();
    let lines = status_lines(&d, &parts);
    theme::hint_chip(
        ui,
        &chip_text(&d, &parts),
        Some(&lines.join("\n")),
        Some(theme::Dot::Muted),
    );
}

/// [`chip`]'s words: the first part on battery and the level it dims to, in its short name.
pub fn chip_text(d: &BatteryDim, parts: &super::parts::Parts) -> String {
    let parts = parts.clone().seen(if d.right.is_some() { 2 } else { 1 });
    let mut halves = vec![(parts.short(0), d.left)];
    if let Some((h, _)) = d.right.filter(|_| parts.is_split()) {
        halves.push((parts.short(1), h));
    }
    match halves.into_iter().find(|(_, h)| !h.usb) {
        Some((name, h)) => {
            let power = if h.low {
                "on low battery"
            } else {
                "on battery"
            };
            format!("{name} {power} · dims to {}%", h.level)
        }
        None => format!("On USB · {}%", d.left.level),
    }
}

/// The parts' state, the one on battery first (that's the one being dimmed). A one-piece board
/// has one line (#559). The last line says when the right half still has to take the levels.
pub fn status_lines(d: &BatteryDim, parts: &super::parts::Parts) -> Vec<String> {
    let parts = parts.clone().seen(if d.right.is_some() { 2 } else { 1 });
    let mut halves = vec![(parts.name(0), Some(d.left))];
    if parts.is_split() {
        halves.push((parts.name(1), d.right.map(|(h, _)| h)));
    }
    halves.sort_by_key(|(_, h)| h.is_none_or(|h| h.usb));
    let mut lines: Vec<String> = halves
        .into_iter()
        .map(|(name, h)| match h {
            Some(h) => half_text(&name, &h),
            None => format!("{name}: no data yet"),
        })
        .collect();
    lines.extend(pending_line(d, &parts));
    lines
}

/// "The right half takes these levels when it next connects.", while it does.
fn pending_line(d: &BatteryDim, parts: &super::parts::Parts) -> Option<String> {
    (d.right.is_some() && !d.right_synced).then(|| {
        format!(
            "{} takes these levels when it next connects.",
            super::parts::upper_first(&parts.the(1))
        )
    })
}

/// The switch greyed while its state isn't known (#794 K31): a disabled "Dim on battery", a
/// short chip, and the whole line on both their hovers.
fn greyed(ui: &mut egui::Ui, chip: &str, line: &str) {
    ui.add_enabled(false, egui::Checkbox::new(&mut false, SHORT))
        .on_disabled_hover_text(line);
    theme::hint_chip(ui, chip, Some(line), Some(theme::Dot::Muted));
}

/// Paints the switch, the state and "Adjust…"; returns what was asked for this frame.
pub fn draw(ui: &mut egui::Ui, s: &DimState, draft: &mut Draft) -> Vec<Intent> {
    let mut out = Vec::new();
    let d = match s {
        DimState::Known(d) => *d,
        DimState::Unknown | DimState::Asking => {
            ui.horizontal_wrapped(|ui| {
                ui.spinner();
                greyed(
                    ui,
                    "asking the keyboard…",
                    "Save battery: asking the keyboard…",
                );
            });
            return out;
        }
        DimState::NoKeyboard => {
            ui.horizontal_wrapped(|ui| {
                greyed(
                    ui,
                    "needs Foxglove firmware",
                    "Save battery: dim lights on battery: needs a keyboard with Foxglove's firmware",
                );
            });
            return out;
        }
        DimState::NotSupported | DimState::NoAnswer => {
            let (chip, text) = if *s == DimState::NotSupported {
                (
                    "needs newer firmware",
                    "Save battery: dim lights on battery: needs newer keyboard firmware",
                )
            } else {
                (
                    "no answer",
                    "Save battery: dim lights on battery: the keyboard didn't answer",
                )
            };
            ui.horizontal_wrapped(|ui| {
                greyed(ui, chip, text);
                if ui.small_button("↻").on_hover_text("Refresh").clicked() {
                    out.push(Intent::Ask);
                }
            });
            return out;
        }
    };
    let mut config = draft.config.unwrap_or(d.config);
    let mut on = config.enabled;
    let changed = ui
        .horizontal(|ui| {
            let c = ui.checkbox(&mut on, SHORT);
            theme::with_info(ui, c, &hover(config.enabled)).changed()
        })
        .inner;
    if changed {
        config.enabled = on;
        draft.config = Some(config);
        out.push(Intent::Set(config));
    }
    // The right half behind on the levels is state: it stays on screen (K29).
    if config.enabled {
        if let Some(line) = pending_line(&d, &super::parts::Parts::current()) {
            ui.indent("battery-dim-state", |ui| {
                ui.label(RichText::new(line).color(theme::palette(ui).muted));
            });
        }
    }
    let summary = format!("{}% · {}% · {}%", config.power, config.battery, config.low);
    let e = super::settings_ui::expander(ui, LEVELS_KEY, LEVELS, &summary, |ui| {
        adjust(ui, &mut config, draft, &mut out);
    });
    e.header.on_hover_text(LEVELS_TIP);
    out
}

/// The three levels and the threshold, each sent when its drag ends.
fn adjust(
    ui: &mut egui::Ui,
    config: &mut BatteryDimConfig,
    draft: &mut Draft,
    out: &mut Vec<Intent>,
) {
    let rows: [(&str, &str, u8, u8); 4] = [
        ("On power", "Of your brightness, on USB", 1, 100),
        ("On battery", "Of your brightness, on battery", 1, 100),
        (
            "Low battery",
            "Of your brightness, below the threshold",
            1,
            100,
        ),
        (
            "Low battery threshold",
            "The charge where the low level starts",
            0,
            50,
        ),
    ];
    let mut values = [config.power, config.battery, config.low, config.low_below];
    let (mut changed, mut send) = (false, false);
    ui.add_enabled_ui(config.enabled, |ui| {
        for ((name, hint, min, max), value) in rows.into_iter().zip(values.iter_mut()) {
            ui.horizontal(|ui| {
                ui.label(name).on_hover_text(hint);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(RichText::new(format!("{value}%")).font(theme::mono(12.5)));
                });
            });
            ui.spacing_mut().slider_width = ui.available_width();
            let r = ui.add(egui::Slider::new(value, min..=max).show_value(false));
            changed |= r.changed();
            send |= r.drag_stopped() || (r.changed() && !r.dragged());
        }
        if ui
            .small_button("Defaults")
            .on_hover_text("100% on power, 80% on battery, 12% below 20% charge")
            .clicked()
        {
            let d = BatteryDimConfig::default();
            values = [d.power, d.battery, d.low, d.low_below];
            changed = true;
            send = true;
        }
    });
    [config.power, config.battery, config.low, config.low_below] = values;
    if changed {
        draft.config = Some(*config);
    }
    if send && config.valid() {
        out.push(Intent::Set(*config));
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;

    fn half(usb: bool, low: bool, level: u8, charge: Option<u8>) -> BatteryDimHalf {
        BatteryDimHalf {
            usb,
            low,
            level,
            charge,
        }
    }

    /// The previews' keyboard: left on USB, right on battery at 61%, both at the defaults.
    pub fn sample() -> DimState {
        DimState::Known(BatteryDim {
            status: 0,
            config: BatteryDimConfig::default(),
            left: half(true, false, 100, Some(95)),
            right: Some((half(false, false, 80, Some(61)), 1)),
            right_synced: true,
            right_redraws: None,
        })
    }

    /// The right half below 20%.
    pub fn sample_low() -> DimState {
        DimState::Known(BatteryDim {
            right: Some((half(false, true, 12, Some(15)), 1)),
            ..match sample() {
                DimState::Known(d) => d,
                _ => unreachable!(),
            }
        })
    }

    #[test]
    fn halves_in_words() {
        assert_eq!(
            half_text("Right half", &half(false, false, 80, Some(61))),
            "Right half on battery · 80%"
        );
        assert_eq!(
            half_text("Left half", &half(true, false, 100, None)),
            "Left half on USB · 100%"
        );
        assert_eq!(
            half_text("Right half", &half(false, true, 12, Some(15))),
            "Right half on low battery (15%) · 12%"
        );
    }

    #[test]
    fn the_half_on_battery_comes_first() {
        let DimState::Known(d) = sample() else {
            unreachable!()
        };
        let corne = crate::foxglove::fixtures::users_desk::board().parts();
        assert_eq!(
            status_lines(&d, &corne),
            vec!["Right half on battery · 80%", "Left half on USB · 100%"]
        );
        let no_right = BatteryDim { right: None, ..d };
        assert_eq!(
            status_lines(&no_right, &corne),
            vec!["Left half on USB · 100%", "Right half: no data yet"]
        );
        let behind = BatteryDim {
            right_synced: false,
            ..d
        };
        let lines = status_lines(&behind, &corne);
        assert_eq!(lines.len(), 3, "says the right is behind");
        assert_eq!(
            lines[2],
            "The right half takes these levels when it next connects."
        );
        // #559: a one-piece board is one line, the keyboard's.
        let plank = crate::foxglove::fixtures::second_desk::board().parts();
        assert_eq!(
            status_lines(&no_right, &plank),
            vec!["Keyboard on USB · 100%"]
        );
        // #794 K27: the Keyboard card's header chip names the part being dimmed.
        assert_eq!(chip_text(&d, &corne), "Right on battery · dims to 80%");
        let DimState::Known(low) = sample_low() else {
            unreachable!()
        };
        assert_eq!(
            chip_text(&low, &corne),
            "Right on low battery · dims to 12%"
        );
        assert_eq!(chip_text(&no_right, &plank), "On USB · 100%");
    }

    #[test]
    fn the_draft_waits_for_the_keyboard() {
        let mut draft = Draft {
            config: Some(BatteryDimConfig {
                battery: 50,
                ..Default::default()
            }),
            open: true,
        };
        draft.settle(&sample());
        assert!(draft.config.is_some(), "the keyboard still says 80");
        let DimState::Known(d) = sample() else {
            unreachable!()
        };
        draft.settle(&DimState::Known(BatteryDim {
            config: BatteryDimConfig {
                battery: 50,
                ..Default::default()
            },
            ..d
        }));
        assert_eq!(draft.config, None);
        assert!(draft.open);
    }

    /// Draws one frame headless and returns what it asked for.
    fn frame(s: DimState, draft: &mut Draft, click: Option<egui::Pos2>) -> Vec<Intent> {
        let ctx = crate::foxglove::theme::headless::ctx(crate::foxglove::theme::Choice::Dark);
        let mut out = Vec::new();
        for i in 0..2 {
            let mut raw = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(480.0, 400.0),
                )),
                ..Default::default()
            };
            if let (1, Some(p)) = (i, click) {
                raw.events = vec![
                    egui::Event::PointerMoved(p),
                    egui::Event::PointerButton {
                        pos: p,
                        button: egui::PointerButton::Primary,
                        pressed: true,
                        modifiers: Default::default(),
                    },
                    egui::Event::PointerButton {
                        pos: p,
                        button: egui::PointerButton::Primary,
                        pressed: false,
                        modifiers: Default::default(),
                    },
                ];
            }
            let mut frame = ctx.run_ui(raw, |ui| {
                egui::CentralPanel::default().show(ui, |ui| {
                    out = draw(ui, &s, draft);
                });
            });
            // Nothing paints these frames; egui wants its texture changes acknowledged.
            frame.textures_delta.clear();
        }
        out
    }

    #[test]
    fn closed_by_default_with_only_the_switch() {
        let mut draft = Draft::default();
        assert_eq!(frame(sample(), &mut draft, None), vec![]);
        assert!(!draft.open, "the levels stay behind Adjust…");
    }

    #[test]
    fn the_switch_turns_it_off() {
        let mut draft = Draft::default();
        // The checkbox is the first row of the panel.
        let out = frame(sample(), &mut draft, Some(egui::pos2(16.0, 16.0)));
        assert_eq!(
            out,
            vec![Intent::Set(BatteryDimConfig {
                enabled: false,
                ..Default::default()
            })]
        );
        assert_eq!(draft.config.map(|c| c.enabled), Some(false));
    }

    #[test]
    fn older_firmware_says_so() {
        let mut draft = Draft::default();
        assert_eq!(frame(DimState::NotSupported, &mut draft, None), vec![]);
        assert_eq!(frame(DimState::NoAnswer, &mut draft, None), vec![]);
        assert_eq!(frame(DimState::NoKeyboard, &mut draft, None), vec![]);
    }

    /// Asks answered from a script, counting asks and pauses.
    fn scripted(
        replies: Vec<Result<BatteryDim, foxglove_devices::DeviceError>>,
        known: DimState,
    ) -> (DimState, usize, usize) {
        let mut replies = replies.into_iter();
        let (mut asks, mut pauses) = (0, 0);
        let state = super::answer(
            || {
                asks += 1;
                replies.next().expect("asked more than scripted")
            },
            || pauses += 1,
            known,
        );
        (state, asks, pauses)
    }

    /// Right after a reconnect (USB to Bluetooth) the first reply is lost: one retry reads it,
    /// and the firmware with the capability bit is never called old.
    #[test]
    fn a_dim_read_lost_after_a_reconnect_is_asked_once_more() {
        use foxglove_devices::DeviceError::{Timeout, Unsupported};
        let dim = match sample() {
            DimState::Known(d) => d,
            _ => unreachable!(),
        };
        assert_eq!(
            scripted(vec![Err(Timeout), Ok(dim)], DimState::Asking),
            (DimState::Known(dim), 2, 1)
        );
        // Unanswered twice: no third ask, and no "newer firmware" either.
        assert_eq!(
            scripted(vec![Err(Timeout), Err(Timeout)], DimState::Asking),
            (DimState::NoAnswer, 2, 1)
        );
        // What was known stays.
        assert_eq!(
            scripted(vec![Err(Timeout), Err(Timeout)], sample()),
            (sample(), 2, 1)
        );
        // Only a missing capability bit (Unsupported, before any I/O) means older firmware,
        // and it is not retried.
        assert_eq!(
            scripted(vec![Err(Unsupported)], DimState::Asking),
            (DimState::NotSupported, 1, 0)
        );
        assert_eq!(
            scripted(vec![Ok(dim)], DimState::Asking),
            (DimState::Known(dim), 1, 0)
        );
    }
}
