//! Health of the art keys (#60, #701), as plain data for the shared "Keyboard health" view
//! (`health.read`, the app's health page, `foxglove-lab health`; #705): whether Foxglove holds
//! `&art_next` / `&art_prev`, how many presses it stepped, and how it last gave them back.
//! The art service notes each event as it happens (a connect, a press, pause, quit, a worker
//! restart); [`rows`] only reads. No polling.
//!
//! The rows are the Keyboard health view's shared type (`foxglove_link::health::HealthRow`, #705),
//! registered with `health::register` as "art_keys".

// The shared health view (#705) picks these up; until it lands only the tests read them.
#![allow(dead_code)]

use std::sync::Mutex;

/// The subsystem these rows belong to in the health view.
pub const SUBSYSTEM: &str = "screens.art_keys";

/// Why Foxglove last gave the art keys back to the keyboard.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HandBack {
    /// Foxglove was paused (#404); resuming takes them again.
    Pause,
    /// Foxglove quit and the keyboard answered.
    Quit,
    /// Foxglove quit but the keyboard didn't answer: the keys do nothing until its next USB
    /// disconnect or endpoint change.
    QuitFailed(String),
    /// The keyboard went (USB disconnect, endpoint change): it takes them back itself.
    Disconnected,
    /// The art worker restarted after a panic and took them again on the next connect.
    Recovered,
}

/// The art keys' health now.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ArtKeysHealth {
    /// The connected keyboard has the art keys (HELLO bit 16); `None` with no keyboard.
    pub supported: Option<bool>,
    /// Foxglove told the keyboard it takes them (D7 0D 01) and hasn't given them back.
    pub held: bool,
    /// Presses that reached Foxglove while it held the keys (D7 8D 01).
    pub presses: u32,
    /// Of those, the ones that stepped Rotation.
    pub stepped: u32,
    /// Why the last press that didn't step didn't (paused, busy, nothing else to show).
    pub last_refusal: Option<String>,
    pub last_hand_back: Option<HandBack>,
}

impl ArtKeysHealth {
    /// A keyboard connected, with or without the art keys.
    pub fn connected(&mut self, supported: bool) {
        self.supported = Some(supported);
    }

    /// What the keyboard was told: `true` takes the keys; `false` gives them back for `why`.
    pub fn told(&mut self, held: bool, why: Option<HandBack>) {
        self.held = held;
        if let Some(why) = why.filter(|_| !held) {
            self.last_hand_back = Some(why);
        }
    }

    /// The keyboard went: it takes its keys back itself.
    pub fn disconnected(&mut self) {
        self.supported = None;
        if std::mem::take(&mut self.held) {
            self.last_hand_back = Some(HandBack::Disconnected);
        }
    }

    /// A press while Foxglove held the keys, and whether it stepped (`Err`: why not).
    pub fn pressed(&mut self, result: &Result<(), String>) {
        self.presses = self.presses.saturating_add(1);
        match result {
            Ok(()) => self.stepped = self.stepped.saturating_add(1),
            Err(e) => self.last_refusal = Some(e.clone()),
        }
    }
}

// The shared row type of the Keyboard health view (#705).
pub use super::health::{HealthRow, Level};

static HEALTH: Mutex<ArtKeysHealth> = Mutex::new(ArtKeysHealth {
    supported: None,
    held: false,
    presses: 0,
    stepped: 0,
    last_refusal: None,
    last_hand_back: None,
});

/// Notes an event (the art service calls this as each happens).
pub fn note(f: impl FnOnce(&mut ArtKeysHealth)) {
    f(&mut HEALTH.lock().unwrap_or_else(|e| e.into_inner()));
}

/// The health view's rows now (registered with `health::register`).
pub fn rows_now() -> Vec<HealthRow> {
    rows(&now())
}

/// The art keys' health now.
pub fn now() -> ArtKeysHealth {
    HEALTH.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// The health view's rows: Held, Presses, Last hand-back.
pub fn rows(h: &ArtKeysHealth) -> Vec<HealthRow> {
    let row = |what: &str, level, detail: String| {
        HealthRow::new(SUBSYSTEM, format!("Art keys · {what}"), level, detail)
    };
    let held = match (h.supported, h.held) {
        (None, _) => row("Held", Level::Ok, "no keyboard connected".into()),
        (Some(false), _) => row(
            "Held",
            Level::Warn,
            "this keyboard's firmware has no art keys (HELLO bit 16)".into(),
        ),
        (Some(true), true) => row(
            "Held",
            Level::Ok,
            "Foxglove steps Rotation from them".into(),
        ),
        (Some(true), false) => row(
            "Held",
            Level::Ok,
            "the keyboard steps its own art (Foxglove paused)".into(),
        ),
    };
    let missed = h.presses - h.stepped.min(h.presses);
    let presses = if missed == 0 {
        row(
            "Presses",
            Level::Ok,
            format!("{} pressed, {} stepped", h.presses, h.stepped),
        )
    } else {
        row(
            "Presses",
            Level::Warn,
            format!(
                "{} pressed, {} stepped; the last that didn't: {}",
                h.presses,
                h.stepped,
                h.last_refusal.as_deref().unwrap_or("unknown")
            ),
        )
    };
    let hand_back = match &h.last_hand_back {
        None => row(
            "Last hand-back",
            Level::Ok,
            "none since Foxglove started".into(),
        ),
        Some(HandBack::Pause) => row("Last hand-back", Level::Ok, "on pause".into()),
        Some(HandBack::Quit) => row("Last hand-back", Level::Ok, "on quit".into()),
        Some(HandBack::Disconnected) => row(
            "Last hand-back",
            Level::Ok,
            "the keyboard took them back when it went".into(),
        ),
        Some(HandBack::Recovered) => row(
            "Last hand-back",
            Level::Warn,
            "the art worker restarted after a panic and took them again".into(),
        ),
        Some(HandBack::QuitFailed(e)) => row(
            "Last hand-back",
            Level::Fail,
            format!("not handed back on quit ({e}): the keys do nothing until a USB replug"),
        ),
    };
    vec![held, presses, hand_back]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn levels(h: &ArtKeysHealth) -> Vec<(String, Level)> {
        rows(h).into_iter().map(|r| (r.item, r.level)).collect()
    }

    /// The signal fires: each event the art service notes shows in the rows.
    #[test]
    fn each_event_shows_in_the_rows() {
        let mut h = ArtKeysHealth::default();
        assert!(rows(&h)
            .iter()
            .all(|r| r.level == Level::Ok && r.subsystem == SUBSYSTEM));
        assert_eq!(rows(&h)[0].detail, "no keyboard connected");

        h.connected(false);
        assert_eq!(levels(&h)[0].1, Level::Warn, "no bit 16");

        h.connected(true);
        h.told(true, None);
        assert!(h.held);
        assert_eq!(rows(&h)[0].detail, "Foxglove steps Rotation from them");

        h.pressed(&Ok(()));
        h.pressed(&Ok(()));
        assert_eq!(rows(&h)[1].detail, "2 pressed, 2 stepped");
        h.pressed(&Err("another art job is running".into()));
        let r = &rows(&h)[1];
        assert_eq!(r.level, Level::Warn);
        assert!(
            r.detail.contains("another art job is running"),
            "{}",
            r.detail
        );

        h.told(false, Some(HandBack::Pause));
        assert!(!h.held);
        assert_eq!(rows(&h)[2].detail, "on pause");
        h.told(true, None);
        assert_eq!(
            h.last_hand_back,
            Some(HandBack::Pause),
            "taking them keeps the last"
        );

        h.disconnected();
        assert_eq!(h.last_hand_back, Some(HandBack::Disconnected));
        assert_eq!(rows(&h)[0].detail, "no keyboard connected");
        h.disconnected();
        assert_eq!(
            h.last_hand_back,
            Some(HandBack::Disconnected),
            "nothing held, no change"
        );

        h.last_hand_back = Some(HandBack::Recovered);
        assert_eq!(levels(&h)[2].1, Level::Warn);
        h.told(
            false,
            Some(HandBack::QuitFailed("no reply from the keyboard".into())),
        );
        let r = &rows(&h)[2];
        assert_eq!(r.level, Level::Fail);
        assert!(r.detail.contains("no reply from the keyboard"));
        h.told(false, Some(HandBack::Quit));
        assert_eq!(rows(&h)[2].detail, "on quit");
    }
}
