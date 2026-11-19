//! Edits show at once (slice L3 of "lights live in Foxglove's window", docs:
//! review/ux-plan-lights-live.md section 3): what the user picked on the Keyboard card but the
//! keyboard hasn't reported yet, as the lights the drawing shows meanwhile.
//!
//! Pure: the page hands in its draft and what it sent; this says what to draw and when the
//! keyboard's report takes over again. Never a timer: the report's own event (or one bounded
//! repaint at [`GIVE_UP`]) ends it.

use std::time::{Duration, Instant};

use foxglove_link::protocol::{RgbHalf, RgbState};

use super::lights_live::Draft as Shown;
use super::lights_page::{Draft, Intent};

/// Without a report this long after a change, the drawing goes back to what the keyboard said
/// (a refused or lost change must not stay drawn as done).
pub const GIVE_UP: Duration = Duration::from_secs(5);

/// One change the card sent to the keyboard.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Change {
    On(bool),
    Effect(u8),
    Speed(u8),
    Colour { hue: u16, sat: u8, brightness: u8 },
}

impl Change {
    /// `h` with this change made.
    pub fn apply(self, mut h: RgbHalf) -> RgbHalf {
        match self {
            Change::On(on) => h.on = on,
            Change::Effect(e) => h.effect = e,
            Change::Speed(s) => h.speed = s,
            Change::Colour {
                hue,
                sat,
                brightness,
            } => (h.hue, h.sat, h.brightness) = (hue, sat, brightness),
        }
        h
    }
}

/// A change on its way: what, and when it was sent.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sent {
    pub change: Change,
    pub at: Instant,
}

/// The change an intent asks the keyboard for, if it is one.
pub fn change_of(intent: &Intent) -> Option<Change> {
    match *intent {
        Intent::Lights(on) => Some(Change::On(on)),
        Intent::Effect(e) => Some(Change::Effect(e)),
        Intent::Speed(s) => Some(Change::Speed(s)),
        Intent::Apply {
            hue,
            sat,
            brightness,
        } => Some(Change::Colour {
            hue,
            sat,
            brightness,
        }),
        _ => None,
    }
}

/// The keyboard has answered since `sent` (a report read after it), or it has been too long.
/// Any newer read counts, even one already on its way with the old state: a rare flicker, and
/// the next report puts the change back.
pub fn settled(sent: &Sent, last_read: Option<Instant>, now: Instant) -> bool {
    last_read.is_some_and(|r| r > sent.at) || now.saturating_duration_since(sent.at) >= GIVE_UP
}

/// Lets go of `sent` once settled, and of a speed pick with it: the keyboard has had its say, so
/// a refused speed is not kept drawn. Returns whether the change settled.
pub fn settle(draft: &mut Draft, last_read: Option<Instant>, now: Instant) -> bool {
    let Some(sent) = draft.sent else {
        return false;
    };
    if !settled(&sent, last_read, now) {
        return false;
    }
    draft.sent = None;
    if matches!(sent.change, Change::Speed(_)) {
        draft.speed = None;
    }
    true
}

/// The lights to draw instead of the report's: the pick not applied yet (colour, brightness,
/// speed) and the change sent but not reported. `None` when the report is what is shown.
pub fn shown(draft: &Draft, rgb: &RgbState, sent: Option<&Sent>) -> Option<Shown> {
    let mut left = rgb.left;
    let mut right = rgb.right.map(|(h, _)| h);
    let mut change = |c: Change| {
        left = c.apply(left);
        right = right.map(|h| c.apply(h));
    };
    if let Some((hue, sat, brightness)) = draft.color.filter(|_| draft.edited) {
        change(Change::Colour {
            hue,
            sat,
            brightness,
        });
    }
    if let Some(s) = draft.speed.filter(|s| *s != rgb.left.speed) {
        change(Change::Speed(s));
    }
    if let Some(s) = sent {
        change(s.change);
    }
    let same = left == rgb.left && right == rgb.right.map(|(h, _)| h);
    (!same).then_some(Shown {
        left: Some(left),
        right,
        sent: sent.is_some(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn half(effect: u8, hue: u16, brightness: u8) -> RgbHalf {
        RgbHalf {
            on: true,
            hue,
            sat: 100,
            brightness,
            effect,
            speed: 3,
        }
    }

    fn state() -> RgbState {
        RgbState {
            left: half(0, 30, 60),
            right: Some((half(0, 30, 60), 0)),
            colour_effects: Some(0x0B),
        }
    }

    fn sent(change: Change, at: Instant) -> Sent {
        Sent { change, at }
    }

    #[test]
    fn nothing_picked_draws_the_report() {
        let rgb = state();
        assert_eq!(shown(&Draft::default(), &rgb, None), None);
        // A pick the report already shows is not a draft.
        let d = Draft {
            color: Some((30, 100, 60)),
            edited: true,
            ..Draft::default()
        };
        assert_eq!(shown(&d, &rgb, None), None);
    }

    #[test]
    fn a_slider_drag_is_drawn_but_not_called_sending() {
        let d = Draft {
            color: Some((30, 100, 90)),
            edited: true,
            ..Draft::default()
        };
        let s = shown(&d, &state(), None).unwrap();
        assert_eq!(s.left.unwrap().brightness, 90);
        assert_eq!(s.right.unwrap().brightness, 90);
        assert!(!s.sent);
        // Not edited: the page follows the report, no draft.
        let d = Draft { edited: false, ..d };
        assert_eq!(shown(&d, &state(), None), None);
    }

    #[test]
    fn a_speed_pick_is_drawn_until_the_keyboard_reports_it() {
        let d = Draft {
            speed: Some(5),
            ..Draft::default()
        };
        assert_eq!(shown(&d, &state(), None).unwrap().left.unwrap().speed, 5);
        let mut rgb = state();
        rgb.left.speed = 5;
        rgb.right = Some((rgb.left, 0));
        assert_eq!(shown(&d, &rgb, None), None);
    }

    #[test]
    fn a_sent_change_is_drawn_and_called_sending() {
        let s = sent(Change::Effect(3), Instant::now());
        let d = shown(&Draft::default(), &state(), Some(&s)).unwrap();
        assert_eq!(d.left.unwrap().effect, 3);
        assert!(d.sent);
        let off = sent(Change::On(false), Instant::now());
        let d = shown(&Draft::default(), &state(), Some(&off)).unwrap();
        assert!(!d.left.unwrap().on);
    }

    #[test]
    fn an_unreported_right_half_stays_unreported() {
        let mut rgb = state();
        rgb.right = None;
        let s = sent(Change::Effect(1), Instant::now());
        let d = shown(&Draft::default(), &rgb, Some(&s)).unwrap();
        assert_eq!(d.right, None);
    }

    #[test]
    fn the_report_takes_over_when_it_arrives_or_after_too_long() {
        let t0 = Instant::now() + Duration::from_secs(10);
        let s = sent(Change::Effect(3), t0);
        assert!(!settled(&s, None, t0 + Duration::from_millis(100)));
        // The report that was already there before the change doesn't count.
        let before = t0 - Duration::from_millis(5);
        assert!(!settled(&s, Some(before), t0 + GIVE_UP / 2));
        assert!(settled(
            &s,
            Some(t0 + Duration::from_millis(40)),
            t0 + GIVE_UP / 2
        ));
        // A refused change never gets a report: it is let go.
        assert!(settled(&s, None, t0 + GIVE_UP));
    }

    #[test]
    fn a_refused_pick_snaps_back_to_what_the_keyboard_says() {
        // Sent: drawn as asked. Settled with the old report: the report is drawn again.
        let t0 = Instant::now();
        let s = sent(
            Change::Colour {
                hue: 200,
                sat: 100,
                brightness: 90,
            },
            t0,
        );
        let rgb = state();
        let during = shown(&Draft::default(), &rgb, Some(&s)).unwrap();
        assert_eq!(during.left.unwrap().hue, 200);
        let later = t0 + Duration::from_millis(30);
        assert!(settled(&s, Some(later), later));
        assert_eq!(
            shown(&Draft::default(), &rgb, None),
            None,
            "the report wins"
        );
    }

    #[test]
    fn a_refused_speed_snaps_back_too() {
        let t0 = Instant::now();
        let mut d = Draft {
            speed: Some(5),
            sent: Some(sent(Change::Speed(5), t0)),
            ..Draft::default()
        };
        let rgb = state(); // the keyboard still says its own speed
        assert_eq!(
            shown(&d, &rgb, d.sent.as_ref())
                .unwrap()
                .left
                .unwrap()
                .speed,
            5
        );
        assert!(!settle(&mut d, None, t0 + Duration::from_millis(10)));
        assert_eq!(d.speed, Some(5));
        let later = t0 + Duration::from_millis(50);
        assert!(settle(&mut d, Some(later), later));
        assert_eq!(d.speed, None);
        assert_eq!(shown(&d, &rgb, d.sent.as_ref()), None, "the report wins");
    }

    #[test]
    fn a_drag_changes_the_drawn_colours_before_any_report() {
        use crate::foxglove::lights_live::{look, Report};
        let rgb = state();
        let report = Report {
            connected: true,
            rgb: Some(rgb),
            dim: [100, 100],
            ..Report::default()
        };
        let layout = crate::foxglove::key_layout::resolve(
            Some(&crate::foxglove::fixtures::users_desk::board()),
            &[],
            0,
        );
        let t = foxglove_scene::SceneTime(0);
        let at_rest = look(&report, None, None, None, None).colours(&layout, t);
        let d = Draft {
            color: Some((200, 100, 90)),
            edited: true,
            ..Draft::default()
        };
        let drafted = shown(&d, &rgb, None);
        let l = look(&report, None, None, None, drafted.as_ref());
        assert_ne!(l.colours(&layout, t), at_rest, "drawn before the report");
        assert!(!l.sending, "a pick not applied is not being sent");
        // Solid is still: no frame is asked for, drafted or not.
        assert_eq!(l.next_frame(), None);
        // Applied: sent, then the report takes over and the look is the report's again.
        let s = sent(
            Change::Colour {
                hue: 200,
                sat: 100,
                brightness: 90,
            },
            Instant::now(),
        );
        let sending = shown(&Draft::default(), &rgb, Some(&s));
        assert!(look(&report, None, None, None, sending.as_ref()).sending);
        let after = look(
            &report,
            None,
            None,
            None,
            shown(&Draft::default(), &rgb, None).as_ref(),
        );
        assert!(!after.sending);
        assert_eq!(after.colours(&layout, t), at_rest);
        assert_eq!(after.next_frame(), None);
    }

    #[test]
    fn intents_that_are_not_lights_are_no_change() {
        assert_eq!(change_of(&Intent::Refresh), None);
        assert_eq!(change_of(&Intent::Follow(true)), None);
        assert_eq!(change_of(&Intent::Effect(2)), Some(Change::Effect(2)));
    }
}
