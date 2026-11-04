//! Who has the keyboard's lights (#491): the owner byte of every D7 8C (SET_FRAME) and D7 8F
//! (SET_EFFECT) report, replies and the keyboard's own events alike.
//!
//! The firmware's values (widgets/host_frames.h): 0 the keyboard's own effect, 3 the host link
//! (Foxglove's frames or effect), and 1 and 2 kept for a HID LampArray over USB and BLE (#115,
//! #239), which is how Windows Dynamic Lighting, or an app Windows lets in, drives a keyboard.
//! Firmware before LampArray only ever says 0 or 3. LampArray firmware is to say when it takes
//! the lights and when it lets go with SET_FRAME's unsolicited D7 8C (event 1, owner 1 or 2, then
//! 0), the event a host that stepped aside waits for.
//!
//! [`Tracker`] keeps the owner from those reports alone: it reads what the keyboard sends and
//! never asks. A host watches the link's reports (`link::Link::subscribe_reports`) and hands
//! each to [`Tracker::observe`]; nothing polls. The one-owner rule spans both commands: frames
//! and an effect replace each other quietly (host_link.h), so a "0" from one of them while the
//! other holds the lights for the host link leaves the host link as the owner.

use crate::protocol::{settings, SETTINGS};

/// Who drives the keyboard's lights, as its owner byte says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Owner {
    /// 0: the keyboard's own effect.
    Keyboard,
    /// 1: a HID LampArray over USB: Windows Dynamic Lighting, or an app it lets in.
    LampArrayUsb,
    /// 2: the same over Bluetooth.
    LampArrayBle,
    /// 3: the host link: frames or an effect from the PC's Foxglove.
    HostLink,
    /// A byte this crate doesn't know yet: someone else.
    Other(u8),
}

impl Owner {
    pub const fn from_byte(b: u8) -> Owner {
        match b {
            0 => Owner::Keyboard,
            1 => Owner::LampArrayUsb,
            2 => Owner::LampArrayBle,
            3 => Owner::HostLink,
            other => Owner::Other(other),
        }
    }

    pub const fn byte(self) -> u8 {
        match self {
            Owner::Keyboard => 0,
            Owner::LampArrayUsb => 1,
            Owner::LampArrayBle => 2,
            Owner::HostLink => 3,
            Owner::Other(b) => b,
        }
    }

    /// The lights are a LampArray's (Windows Dynamic Lighting).
    pub const fn lamp_array(self) -> bool {
        matches!(self, Owner::LampArrayUsb | Owner::LampArrayBle)
    }

    /// Someone other than the keyboard and the host link has the lights: a host-link sender
    /// steps aside until they are free again.
    pub const fn elsewhere(self) -> bool {
        matches!(
            self,
            Owner::LampArrayUsb | Owner::LampArrayBle | Owner::Other(_)
        )
    }
}

/// Which command a report answered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// SET_FRAME (D7 8C).
    Frames,
    /// SET_EFFECT (D7 8F).
    Effect,
}

/// One owner byte as the keyboard sent it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Said {
    pub owner: Owner,
    pub source: Source,
    /// Sent by the keyboard on its own (byte 6), not a reply.
    pub event: bool,
}

const FRAME_REPLY: u8 = 0x80 | settings::FRAME;
const EFFECT_REPLY: u8 = 0x80 | settings::EFFECT;

/// The owner byte of a D7 8C or D7 8F report; `None` for any other report and for firmware
/// without the command (status 4, whose owner byte means nothing).
pub fn said(r: &[u8]) -> Option<Said> {
    if r.len() < 7 || r[0] != SETTINGS {
        return None;
    }
    let source = match r[1] {
        FRAME_REPLY => Source::Frames,
        EFFECT_REPLY => Source::Effect,
        _ => return None,
    };
    if r[2] == 4 {
        return None;
    }
    Some(Said {
        owner: Owner::from_byte(r[3]),
        source,
        // D7 8F has no events yet (its byte 6 is always 0).
        event: source == Source::Frames && r[6] != 0,
    })
}

/// The keyboard's lights' owner, from the reports seen.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Tracker {
    owner: Option<Owner>,
    /// While the host link owns them: through which command.
    via: Option<Source>,
}

impl Tracker {
    /// The owner last said; `None` before any report (or after [`Tracker::forget`]).
    pub fn owner(&self) -> Option<Owner> {
        self.owner
    }

    /// A report from the keyboard; true when the owner changed.
    pub fn observe(&mut self, r: &[u8]) -> bool {
        said(r).is_some_and(|s| self.apply(s))
    }

    /// One owner byte; true when the owner changed.
    pub fn apply(&mut self, s: Said) -> bool {
        let before = self.owner;
        match s.owner {
            Owner::HostLink => {
                self.owner = Some(Owner::HostLink);
                self.via = Some(s.source);
            }
            Owner::Keyboard
                if self.owner == Some(Owner::HostLink) && self.via != Some(s.source) =>
            {
                // The other command still holds them: an effect replaced the frames (or the
                // frames the effect) quietly, so this "0" is only about its own command.
            }
            other => {
                self.owner = Some(other);
                self.via = None;
            }
        }
        self.owner != before
    }

    /// A new connection: nothing known until the keyboard next says.
    pub fn forget(&mut self) {
        *self = Tracker::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(cmd: u8, status: u8, owner: u8, event: u8) -> [u8; 32] {
        let mut r = [0u8; 32];
        r[..7].copy_from_slice(&[SETTINGS, cmd, status, owner, 30, 1, event]);
        r
    }

    #[test]
    fn owner_bytes_round_trip() {
        for b in 0..=255u8 {
            assert_eq!(Owner::from_byte(b).byte(), b);
        }
        assert!(Owner::LampArrayUsb.elsewhere() && Owner::LampArrayBle.elsewhere());
        assert!(Owner::Other(9).elsewhere());
        assert!(!Owner::Keyboard.elsewhere() && !Owner::HostLink.elsewhere());
        assert!(Owner::LampArrayBle.lamp_array() && !Owner::Other(9).lamp_array());
    }

    #[test]
    fn frame_and_effect_replies_and_events_carry_the_owner() {
        assert_eq!(
            said(&report(0x8C, 0, 3, 0)),
            Some(Said {
                owner: Owner::HostLink,
                source: Source::Frames,
                event: false
            })
        );
        assert_eq!(
            said(&report(0x8C, 0, 1, 1)),
            Some(Said {
                owner: Owner::LampArrayUsb,
                source: Source::Frames,
                event: true
            })
        );
        let effect = said(&report(0x8F, 0, 3, 1)).unwrap();
        assert_eq!(effect.source, Source::Effect);
        assert!(!effect.event, "D7 8F has no events");
        // Refused requests still say who has the lights; firmware without the command doesn't.
        assert_eq!(said(&report(0x8C, 5, 3, 0)).unwrap().owner, Owner::HostLink);
        assert_eq!(said(&report(0x8C, 4, 0, 0)), None);
        // Other reports, and the requests themselves.
        assert_eq!(said(&report(0x8E, 0, 3, 0)), None);
        assert_eq!(said(&report(0x0C, 0, 3, 0)), None);
        assert_eq!(said(&[SETTINGS, 0x8C, 0]), None);
    }

    #[test]
    fn the_tracker_follows_the_one_owner_rule() {
        let mut t = Tracker::default();
        assert_eq!(t.owner(), None);
        assert!(t.observe(&report(0x8C, 0, 3, 0)), "frames take the lights");
        assert!(
            !t.observe(&report(0x8C, 0, 3, 0)),
            "the same again is no change"
        );
        // An effect replaces the frames quietly; releasing the frames afterwards says 0 for
        // the frames only.
        assert!(!t.observe(&report(0x8F, 0, 3, 0)));
        assert!(!t.observe(&report(0x8C, 0, 0, 0)));
        assert_eq!(t.owner(), Some(Owner::HostLink));
        assert!(t.observe(&report(0x8F, 0, 0, 0)), "the effect's release");
        assert_eq!(t.owner(), Some(Owner::Keyboard));
        // Windows takes them (the keyboard's event), then lets go.
        assert!(t.observe(&report(0x8C, 0, 1, 1)));
        assert_eq!(t.owner(), Some(Owner::LampArrayUsb));
        assert!(
            t.observe(&report(0x8F, 0, 0, 0)),
            "free again, whichever says"
        );
        assert_eq!(t.owner(), Some(Owner::Keyboard));
        assert!(t.observe(&report(0x8C, 0, 2, 0)));
        assert!(
            t.observe(&report(0x8C, 0, 3, 0)),
            "the host link has them again"
        );
        assert!(
            !t.observe(&report(0x91, 0, 1, 1)),
            "other reports change nothing"
        );
        t.forget();
        assert_eq!(t.owner(), None);
    }
}
