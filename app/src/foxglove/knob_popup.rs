//! The knob's popup (#719, option A "Flyout twin", the user's pick): a card in the place, size
//! and look of Windows' volume flyout (bottom centre of the display's work area, just above the
//! taskbar), with the app's icon, name, level and its place in the cycle.
//!
//! - It is drawn on the overlay window (`overlay_window.rs`): the always-on-top, click-through
//!   window that never activates, which covers the chosen display's work area. The card is a
//!   non-interactable egui area, so it takes no focus and no clicks; egui's points carry the
//!   display's DPI.
//! - On master, while Foxglove holds master volume (#855, firmware with D7 16), the turns and a
//!   double click come to Foxglove: it sets master's level and mute and shows this card for
//!   master (Windows shows no flyout: no volume keys went). On firmware without it Foxglove
//!   draws no second popup: Windows' own flyout shows (the knob sends the volume keys), and while
//!   Foxglove holds the knob a small "Knob · Master" tab sits on top of it, on the display
//!   Windows shows its flyout on (the main one).
//! - Timing, as the storyboard has it: a click 2 s; a turn keeps the card up until 1.5 s after
//!   the last detent; master (past the last app, or a hold) 2 s; mute 2 s; "<app> stopped ·
//!   knob: master" 3 s; the tab 2 s after master's level last changed.
//!
//! Polling: none. Start: [`show`] (the knob service, on a knob event or a session/master
//! change). Stop: the card's own deadline, one `request_repaint_after` for it.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use egui::{pos2, vec2, Align2, Color32, FontId, Painter, Pos2, Rect};

/// What the card says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Card {
    /// The knob is on an app: its name, place in the cycle (1-based, of how many), level, mute.
    App {
        name: String,
        place: Option<(usize, usize)>,
        level: Option<u8>,
        muted: bool,
    },
    /// Master: back to it (past the last app, or a hold), with why; or a turn or a double click
    /// on it while Foxglove holds master volume (#855; no note). Windows' level, and its mute.
    Master {
        level: Option<u8>,
        note: &'static str,
        muted: bool,
    },
    /// The target's sound ended; the knob is on master again.
    Stopped { name: String },
    /// The target was hidden from the knob (#952): "<app> hidden" / "knob: <next> · n of m" or
    /// "knob: master".
    Hidden { name: String, note: String },
    /// On master while Foxglove holds the knob: the tab over Windows' own flyout.
    MasterTab,
    /// On a device Foxglove sets the volume of (#1064): Windows' flyout or the volume keys moved
    /// master. Master's card, directly above Windows' flyout; on the first such change of a run
    /// a third row says why Windows' slider goes back ("Windows' slider goes back to 100 % on
    /// this device.").
    Outside {
        level: Option<u8>,
        muted: bool,
        first: bool,
    },
}

impl Card {
    /// The log's words for it.
    pub fn words(&self) -> String {
        match self {
            Card::App {
                name,
                place,
                level,
                muted,
            } => {
                let mut s = name.clone();
                if let Some((i, n)) = place {
                    s.push_str(&format!(" {i} of {n}"));
                }
                if *muted {
                    s.push_str(" muted");
                } else if let Some(l) = level {
                    s.push_str(&format!(" {l} %"));
                }
                s
            }
            Card::Master { muted: true, .. } => "Master volume muted".into(),
            Card::Master {
                note: "",
                level: Some(l),
                ..
            } => format!("Master volume {l} %"),
            Card::Master { note, .. } => format!("Master volume ({note})"),
            Card::Stopped { name } => format!("{name} stopped · knob: master"),
            Card::Hidden { name, note } => format!("{name} hidden · {note}"),
            Card::MasterTab => "Knob · Master tab".into(),
            Card::Outside { muted: true, .. } => "Master volume muted (above the flyout)".into(),
            Card::Outside { level, first, .. } => format!(
                "Master volume {} % (above the flyout{})",
                level.unwrap_or(0),
                if *first { ", with the slider row" } else { "" }
            ),
        }
    }
}

/// How long each card shows.
pub const CLICK: Duration = Duration::from_millis(2000);
pub const TURN: Duration = Duration::from_millis(1500);
pub const MASTER: Duration = Duration::from_millis(2000);
pub const MUTE: Duration = Duration::from_millis(2000);
pub const STOPPED: Duration = Duration::from_millis(3000);
pub const TAB: Duration = Duration::from_millis(2000);

struct Shown {
    card: Card,
    until: Instant,
}

/// The card on screen and its deadline.
#[derive(Default)]
struct Slot(Option<Shown>);

impl Slot {
    /// Puts `card` up until `until`; the same card keeps a later deadline (turning keeps it up).
    fn put(&mut self, card: Card, until: Instant) {
        let until = match &self.0 {
            Some(s) if s.card == card => s.until.max(until),
            _ => until,
        };
        self.0 = Some(Shown { card, until });
    }

    /// The card showing at `now`, if any (it goes once its time is up).
    fn current(&mut self, now: Instant) -> Option<(Card, Instant)> {
        match &self.0 {
            Some(s) if s.until > now => Some((s.card.clone(), s.until)),
            Some(_) => {
                self.0 = None;
                None
            }
            None => None,
        }
    }
}

static SHOWN: Mutex<Slot> = Mutex::new(Slot(None));
static WAKE: std::sync::OnceLock<crate::ui_wake::UiWake> = std::sync::OnceLock::new();

/// Shows `card` for `time` from now (a later deadline of the same card is kept: turning keeps
/// it up). Any thread; draws a frame. Notes it in the health view and the log.
pub fn show(card: Card, time: Duration) {
    let until = Instant::now() + time;
    SHOWN
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .put(card.clone(), until);
    let words = card.words();
    eprintln!(
        "Foxglove: knob popup shown: {words} ({:.1} s)",
        time.as_secs_f32()
    );
    super::knob_health::note(|h| {
        h.popups += 1;
        h.last_popup = Some(words);
    });
    if let Some(w) = WAKE.get() {
        w.request_repaint();
    }
}

/// Size of Windows 11's volume flyout, in points.
pub const SIZE: egui::Vec2 = vec2(236.0, 48.0);
/// The master card above the flyout with its third row (#1064): 16 pt taller, grown upwards.
pub const TALL: f32 = 64.0;
/// The third row's room: the card less its 12 pt margins (the plan's limit).
pub const ROW_WIDTH: f32 = 212.0;
/// Its gap above the taskbar (the work area's bottom), in points.
pub const GAP: f32 = 12.0;

/// Where Windows shows its flyout on a work area of `work` (points, the window's own
/// coordinates): centred, `GAP` above the bottom.
pub fn spot(work: Rect) -> Rect {
    Rect::from_min_size(
        pos2(work.center().x - SIZE.x / 2.0, work.bottom() - GAP - SIZE.y),
        SIZE,
    )
}

/// Where the master card goes above Windows' flyout (#1064), where the "Knob · Master" tab
/// sits: its bottom 8 pt above the flyout's top; 64 pt tall with the third row, else 48.
pub fn above(work: Rect, first: bool) -> Rect {
    let flyout = spot(work);
    let h = if first { TALL } else { SIZE.y };
    Rect::from_min_size(pos2(flyout.left(), flyout.top() - 8.0 - h), vec2(SIZE.x, h))
}

/// The third row's type size: 10 pt, or smaller when the words at 10 pt (`width_at_10`) are
/// wider than [`ROW_WIDTH`], so they never run past the card.
pub fn row_size(width_at_10: f32) -> f32 {
    if width_at_10 <= ROW_WIDTH {
        10.0
    } else {
        10.0 * ROW_WIDTH / width_at_10
    }
}

const CARD: Color32 = Color32::from_rgb(0x2c, 0x2c, 0x2e);
const EDGE: Color32 = Color32::from_rgb(0x3c, 0x3c, 0x40);
const TRACK: Color32 = Color32::from_rgb(0x5a, 0x5a, 0x5e);
const TEXT: Color32 = Color32::from_rgb(0xf2, 0xf2, 0xf2);
const DIM: Color32 = Color32::from_rgb(0xa8, 0xa8, 0xae);
const GREY: Color32 = Color32::from_rgb(0x60, 0x60, 0x66);

/// An app's colour for its round icon: steady per name (the exe's icon comes later).
fn tint(name: &str) -> Color32 {
    let h = name
        .bytes()
        .fold(0u32, |h, b| h.wrapping_mul(31).wrapping_add(u32::from(b)));
    let palette = [
        Color32::from_rgb(0x1d, 0xb9, 0x54),
        Color32::from_rgb(0x58, 0x65, 0xf2),
        Color32::from_rgb(0xe0, 0x6c, 0x3c),
        Color32::from_rgb(0xc0, 0x4c, 0xa8),
        Color32::from_rgb(0x2e, 0x9c, 0xc8),
        Color32::from_rgb(0xb8, 0x98, 0x2a),
    ];
    palette[(h % palette.len() as u32) as usize]
}

fn bar(p: &Painter, r: Rect, level: u8, fill: Color32) {
    p.rect_filled(r, 2.0, TRACK);
    let w = r.width() * f32::from(level.min(100)) / 100.0;
    p.rect_filled(Rect::from_min_size(r.min, vec2(w, r.height())), 2.0, fill);
}

fn round_icon(p: &Painter, c: Pos2, fill: Color32, glyph: &str) {
    p.circle_filled(c, 12.0, fill);
    p.text(
        c,
        Align2::CENTER_CENTER,
        glyph,
        FontId::proportional(14.0),
        Color32::WHITE,
    );
}

/// Paints `card` in its place on a work area of `work` (points).
pub fn paint(p: &Painter, work: Rect, card: &Card) {
    use egui_phosphor::regular as icon;
    let s = spot(work);
    if *card == Card::MasterTab {
        let tab = Rect::from_min_size(pos2(s.left() + 12.0, s.top() - 18.0), vec2(96.0, 18.0));
        p.rect_filled(tab, 5.0, CARD);
        p.text(
            tab.center(),
            Align2::CENTER_CENTER,
            "Knob · Master",
            FontId::proportional(11.0),
            TEXT,
        );
        return;
    }
    let s = match card {
        Card::Outside { first, .. } => above(work, *first),
        _ => s,
    };
    p.rect_filled(s.expand(1.0), 9.0, EDGE);
    p.rect_filled(s, 8.0, CARD);
    if let Card::Outside { first: true, .. } = card {
        let row = super::knob_master::SLIDER_ROW;
        let at_10 = p
            .layout_no_wrap(row.to_string(), FontId::proportional(10.0), DIM)
            .size()
            .x;
        p.text(
            pos2(s.left() + 12.0, s.bottom() - 12.0),
            Align2::LEFT_CENTER,
            row,
            FontId::proportional(row_size(at_10)),
            DIM,
        );
    }
    let (glyph, fill, title, note, level, muted) = match card {
        Card::App {
            name,
            place,
            level,
            muted,
        } => (
            // Muted: the slash takes the badge; the title stays plain.
            if *muted {
                icon::SPEAKER_SLASH.to_string()
            } else {
                name.chars()
                    .next()
                    .map(|c| c.to_uppercase().to_string())
                    .unwrap_or_default()
            },
            tint(name),
            name.clone(),
            if *muted {
                "muted".to_string()
            } else {
                place
                    .map(|(i, n)| format!("{i} of {n}"))
                    .unwrap_or_default()
            },
            *level,
            *muted,
        ),
        Card::Outside { level, muted, .. } => (
            if *muted {
                icon::SPEAKER_SLASH
            } else {
                icon::SPEAKER_HIGH
            }
            .to_string(),
            GREY,
            "Master volume".to_string(),
            if *muted {
                "muted".to_string()
            } else {
                String::new()
            },
            *level,
            *muted,
        ),
        Card::Master { level, note, muted } => (
            // Muted as an app's card shows it: slash in the badge (#855).
            if *muted {
                icon::SPEAKER_SLASH
            } else {
                icon::SPEAKER_HIGH
            }
            .to_string(),
            GREY,
            "Master volume".to_string(),
            if *muted {
                "muted".to_string()
            } else {
                note.to_string()
            },
            *level,
            *muted,
        ),
        Card::Stopped { name } => (
            icon::SPEAKER_X.to_string(),
            GREY,
            format!("{name} stopped"),
            "knob: master".to_string(),
            None,
            false,
        ),
        Card::Hidden { name, note } => (
            icon::EYE_SLASH.to_string(),
            GREY,
            format!("{name} hidden"),
            note.clone(),
            None,
            false,
        ),
        Card::MasterTab => unreachable!(),
    };
    round_icon(p, pos2(s.left() + 24.0, s.center().y), fill, &glyph);
    let left = s.left() + 46.0;
    let title_w = p
        .layout_no_wrap(title.clone(), FontId::proportional(13.0), TEXT)
        .size()
        .x;
    let note_w = p
        .layout_no_wrap(note.clone(), FontId::proportional(10.0), DIM)
        .size()
        .x;
    p.text(
        pos2(left, s.top() + 15.0),
        Align2::LEFT_CENTER,
        title,
        FontId::proportional(13.0),
        TEXT,
    );
    if !note.is_empty() {
        let fits = left + title_w + 8.0 + note_w <= s.right() - 12.0;
        // Beside the title when it fits; else on the second row when that is free (a long
        // name with a level keeps only the level).
        let at = if fits {
            Some((pos2(s.right() - 12.0, s.top() + 15.0), Align2::RIGHT_CENTER))
        } else if level.is_none() {
            Some((pos2(left, s.top() + 33.0), Align2::LEFT_CENTER))
        } else {
            None
        };
        if let Some((at, align)) = at {
            p.text(at, align, note, FontId::proportional(10.0), DIM);
        }
    }
    if let Some(level) = level {
        bar(
            p,
            Rect::from_min_max(
                pos2(left, s.top() + 31.0),
                pos2(s.right() - 44.0, s.top() + 35.0),
            ),
            level,
            if muted {
                TRACK
            } else {
                // The level is Foxglove's own: the palette's accent, as on the other surfaces.
                super::theme::DARK.accent
            },
        );
        p.text(
            pos2(s.right() - 12.0, s.top() + 33.0),
            Align2::RIGHT_CENTER,
            level.to_string(),
            FontId::proportional(13.0),
            if muted { DIM } else { TEXT },
        );
    }
}

/// Draws the card showing now on the overlay window. `on_main_display`: whether the overlay
/// covers the display Windows shows its flyout on (the tab goes only there). Called each frame.
pub fn draw(ctx: &egui::Context, on_main_display: impl FnOnce() -> bool) {
    let _ = WAKE.set(crate::ui_wake::UiWake::from_ctx(ctx));
    let now = Instant::now();
    let shown = SHOWN.lock().unwrap_or_else(|e| e.into_inner()).current(now);
    let Some((card, until)) = shown else {
        return;
    };
    if card == Card::MasterTab && !on_main_display() {
        return;
    }
    // The overlay's work area, in the window's own points.
    let work = ctx
        .input(|i| i.viewport().inner_rect)
        .map(|r| Rect::from_min_size(Pos2::ZERO, r.size()))
        .unwrap_or_else(|| ctx.content_rect());
    egui::Area::new(egui::Id::new("foxglove-knob-popup"))
        .order(egui::Order::Tooltip)
        .interactable(false)
        .fixed_pos(Pos2::ZERO)
        .show(ctx, |ui| paint(ui.painter(), work, &card));
    // Its one deadline: the frame that takes it down.
    ctx.request_repaint_after(until.saturating_duration_since(now));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_card_sits_where_windows_flyout_does() {
        // A 1920 x 1032-point work area (a 1080p display at 100 %, taskbar at the bottom).
        let work = Rect::from_min_size(Pos2::ZERO, vec2(1920.0, 1032.0));
        let s = spot(work);
        assert_eq!(s.center().x, 960.0);
        assert_eq!(s.bottom(), 1032.0 - GAP);
        assert_eq!(s.size(), SIZE);
        // At 150 % the window is smaller in points; the card keeps its size in points.
        let work = Rect::from_min_size(Pos2::ZERO, vec2(1706.0, 918.0));
        assert_eq!(spot(work).size(), SIZE);
        assert_eq!(spot(work).bottom(), 918.0 - GAP);
        assert_eq!(spot(work).center().x, 853.0);
    }

    #[test]
    fn a_turn_keeps_the_card_up_and_another_card_replaces_it() {
        let app = Card::App {
            name: "Spotify".into(),
            place: None,
            level: Some(44),
            muted: false,
        };
        let now = Instant::now();
        let mut slot = Slot::default();
        slot.put(app.clone(), now + TURN);
        slot.put(app.clone(), now + Duration::from_millis(100));
        assert_eq!(
            slot.current(now),
            Some((app.clone(), now + TURN)),
            "kept the later deadline"
        );
        assert!(
            slot.current(now + TURN).is_none(),
            "gone 1.5 s after the last detent"
        );
        slot.put(app, now + CLICK);
        slot.put(Card::MasterTab, now + Duration::from_millis(10));
        assert_eq!(slot.current(now).unwrap().0, Card::MasterTab);
        assert!(slot.current(now + Duration::from_millis(10)).is_none());
    }

    #[test]
    fn the_outside_card_sits_above_the_flyout_and_its_row_fits() {
        let work = Rect::from_min_size(Pos2::ZERO, vec2(1920.0, 1032.0));
        let flyout = spot(work);
        let plain = above(work, false);
        let tall = above(work, true);
        assert_eq!(plain.size(), SIZE);
        assert_eq!(tall.height(), TALL);
        assert_eq!(tall.bottom(), plain.bottom(), "grows upwards");
        assert!(plain.bottom() < flyout.top(), "above Windows' flyout");
        assert_eq!(plain.center().x, flyout.center().x);
        // The row fits 212 pt: at 10 pt, or at the size that fits (the plan's words are about
        // 223 pt at 10 pt in Foxglove's font).
        let row = super::super::knob_master::SLIDER_ROW;
        let ctx = super::super::pc_lights_ui::tests::test_ctx();
        let mut width = 0.0;
        let mut frame = ctx.run_ui(egui::RawInput::default(), |ui| {
            let at = |size: f32| {
                ui.painter()
                    .layout_no_wrap(row.to_string(), FontId::proportional(size), DIM)
                    .size()
                    .x
            };
            width = at(row_size(at(10.0)));
        });
        frame.textures_delta.clear();
        assert!(width <= ROW_WIDTH + 0.5, "{width} pt");
        assert_eq!(row_size(200.0), 10.0);
        assert!((row_size(224.0) - 10.0 * 212.0 / 224.0).abs() < 1e-4);
    }

    #[test]
    fn the_log_says_what_showed() {
        let card = Card::App {
            name: "Spotify".into(),
            place: Some((1, 3)),
            level: Some(42),
            muted: false,
        };
        assert_eq!(card.words(), "Spotify 1 of 3 42 %");
        assert_eq!(
            Card::Stopped {
                name: "Spotify".into()
            }
            .words(),
            "Spotify stopped · knob: master"
        );
        assert_eq!(
            Card::Master {
                level: Some(64),
                note: "held: master",
                muted: false
            }
            .words(),
            "Master volume (held: master)"
        );
        let turned = Card::Master {
            level: Some(66),
            note: "",
            muted: false,
        };
        assert_eq!(turned.words(), "Master volume 66 %");
        let muted = Card::Master {
            level: Some(66),
            note: "",
            muted: true,
        };
        assert_eq!(muted.words(), "Master volume muted");
    }
}
