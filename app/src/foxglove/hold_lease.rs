//! The lease on what Foxglove holds on the keyboard (#847): the art keys (`art_service.rs`, D7 0D)
//! and the knob (`knob.rs`, D7 14), and #855's master volume next.
//!
//! Foxglove hands them back on a clean quit, but a crash or a kill never did, and the keyboard
//! kept sending its art keys to a PC that wasn't listening. Firmware with HELLO v2's
//! `HOLD_LEASE` (word 2's bit 34; `foxglove_link::protocol::hold_lease`) lets every held mode go
//! when a PC that set a lease goes silent:
//!
//! - **A keyboard connects** (each reconnect too): the app sets a lease of [`LEASE_S`]. The
//!   keyboard renews it on any report from the PC, so a busy link costs nothing more.
//! - **The keyboard asks** (D7 95 event 1): while a mode is held and the PC was quiet a third of
//!   the lease, and again at two thirds. The app answers with the lease again. So nothing here
//!   runs on a clock: the keyboard's one-shot ask is the only timer, it runs only while a mode is
//!   held, and at most one renewal goes per third of the lease (every 10 s on an idle link).
//! - **The lease lapsed** (D7 95 event 2; the app was stalled, or the PC slept through it): the
//!   keyboard let the modes go, and the art keys and the knob take them again.
//! - **Quit** sets no lease (D7 15 00), after the hand-backs.
//!
//! A killed app answers nothing: within [`LEASE_S`] the art keys step the board's art again and
//! the knob is the volume keys. Firmware without `HOLD_LEASE` is told nothing; its holds end with
//! the link, as before.
//!
//! Polling: none. Start events: a link's connect, the keyboard's ask or lapse, quit. Each sends
//! at most one report; the thread sleeps on its channel between them.

use std::sync::{mpsc, Arc, Mutex, Weak};

use foxglove_link::hello_v2::capabilities as caps2;
use foxglove_link::link::{Event, EventSubscription, Link};
use foxglove_link::protocol::hold_lease::{self as wire, Kind, Report};
use foxglove_link::Transport;

pub use super::health::{HealthRow, Level};

/// The lease, in seconds: the keyboard asks after 10 s and 20 s of quiet, and lets go at 30 s.
pub const LEASE_S: u8 = 30;

/// What can make the app tell the keyboard something.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trigger {
    /// A keyboard connected (or reconnected); `lease`: its firmware has `HOLD_LEASE`.
    Connected { lease: bool },
    /// A D7 95 from the keyboard.
    Report(Report),
}

/// What the app does about a trigger.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    Nothing,
    /// Send this report: the lease, set or renewed.
    Send(Vec<u8>),
    /// The keyboard let the modes go: the art keys and the knob take them again.
    Retake,
}

/// What `trigger` calls for.
pub fn decide(trigger: Trigger) -> Action {
    match trigger {
        Trigger::Connected { lease: true } => Action::Send(wire::set(LEASE_S)),
        // Firmware before #847: its holds end with the link only, as before.
        Trigger::Connected { lease: false } => Action::Nothing,
        Trigger::Report(r) => match r.kind {
            Kind::Ask => Action::Send(wire::set(LEASE_S)),
            Kind::Lapsed => Action::Retake,
            Kind::Reply => Action::Nothing,
        },
    }
}

/// The lease's health now: whether the keyboard takes one, the renewals and the lapses.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LeaseHealth {
    /// The connected keyboard takes a lease (word 2's bit 34); `None` with no keyboard.
    pub supported: Option<bool>,
    /// Asks the app answered since it started.
    pub renewals: u32,
    /// Lapses the keyboard told this app of (each one: the app didn't answer within the lease).
    pub lapses: u32,
    /// The modes the last lapse let go, in words.
    pub last_lapse: Option<String>,
}

static HEALTH: Mutex<LeaseHealth> = Mutex::new(LeaseHealth {
    supported: None,
    renewals: 0,
    lapses: 0,
    last_lapse: None,
});

fn note(f: impl FnOnce(&mut LeaseHealth)) {
    f(&mut HEALTH.lock().unwrap_or_else(|e| e.into_inner()));
}

/// The health view's subsystem.
pub const SUBSYSTEM: &str = "hold_lease";

/// The health view's row (registered with `health::register`).
pub fn rows_now() -> Vec<HealthRow> {
    rows(&HEALTH.lock().unwrap_or_else(|e| e.into_inner()).clone())
}

/// The "Held modes · Lease" row: OK, or a warning once a lapse reached this app (it was stalled
/// past the lease while it held the keys or the knob).
pub fn rows(h: &LeaseHealth) -> Vec<HealthRow> {
    let (level, detail) = match h.supported {
        None => (Level::Ok, "no keyboard connected".to_string()),
        Some(false) => (
            Level::Ok,
            "this keyboard's firmware has no hold lease (word 2's bit 34): its art keys and knob \
             return to the board only when the link drops"
                .to_string(),
        ),
        Some(true) if h.lapses == 0 => (
            Level::Ok,
            format!(
                "{LEASE_S} s lease: the keyboard takes its art keys and knob back {LEASE_S} s \
                 after Foxglove goes quiet; {} renewal(s) answered",
                h.renewals
            ),
        ),
        Some(true) => (
            Level::Warn,
            format!(
                "the lease lapsed {} time(s) while Foxglove ran (it didn't answer within \
                 {LEASE_S} s); last: {}; taken back",
                h.lapses,
                h.last_lapse.as_deref().unwrap_or("nothing held")
            ),
        ),
    };
    vec![HealthRow::new(
        SUBSYSTEM,
        "Held modes · Lease",
        level,
        detail,
    )]
}

fn held_words(held: u8) -> String {
    let mut parts = Vec::new();
    if held & wire::HELD_ART_KEYS != 0 {
        parts.push("art keys");
    }
    if held & wire::HELD_KNOB != 0 {
        parts.push("knob");
    }
    if parts.is_empty() {
        "nothing held".to_string()
    } else {
        parts.join(" and ")
    }
}

/// Does what `trigger` calls for on `keyboard`; what it decided. `retake` runs the art keys' and
/// the knob's take-back (the service passes theirs, the tests their own).
pub fn act(keyboard: &dyn Transport, trigger: Trigger, retake: impl FnOnce()) -> Action {
    let action = decide(trigger);
    match &action {
        Action::Nothing => {}
        Action::Send(report) => {
            if let Err(e) = keyboard.send(report) {
                eprintln!("Foxglove: hold lease: can't tell the keyboard: {e}");
            } else if matches!(trigger, Trigger::Report(_)) {
                note(|h| h.renewals += 1);
            }
        }
        Action::Retake => {
            let held = match trigger {
                Trigger::Report(r) => r.held,
                Trigger::Connected { .. } => 0,
            };
            let words = held_words(held);
            eprintln!("Foxglove: hold lease: the keyboard let go of the {words}; taking back");
            note(|h| {
                h.lapses += 1;
                h.last_lapse = Some(words);
            });
            retake();
        }
    }
    action
}

/// Whether the keyboard on `link` takes a lease: HELLO v2's word 2, bit 34.
fn takes_lease(link: &Link) -> bool {
    let Some(hello) = super::link::ask_hello(link) else {
        return false;
    };
    foxglove_link::hello_v2::read(link, &hello).word2() & caps2::HOLD_LEASE != 0
}

enum Work {
    Link(Arc<Link>),
    Connected(Weak<Link>),
    Disconnected(Weak<Link>),
    Report(Weak<Link>, Report),
}

static WORK: Mutex<Option<mpsc::Sender<Work>>> = Mutex::new(None);
/// The links Foxglove set a lease on, for [`stop`] to clear it.
static LEASED: Mutex<Vec<Weak<Link>>> = Mutex::new(Vec::new());

fn leased() -> std::sync::MutexGuard<'static, Vec<Weak<Link>>> {
    LEASED.lock().unwrap_or_else(|e| e.into_inner())
}

/// Foxglove quits (after the art keys and the knob were handed back): no lease, so a later app
/// before #847 on the same link isn't timed out.
pub fn stop() {
    for link in std::mem::take(&mut *leased()) {
        if let Some(link) = link.upgrade() {
            let _ = link.send(&wire::set(0));
        }
    }
}

/// Starts the lease for every shared keyboard link. `FOXGLOVE_DISABLE=hold-lease` leaves it out
/// (the keyboard then keeps the art keys and the knob held until the link drops, as before #847).
pub fn start() {
    if super::repaint_trace::disabled("hold-lease") {
        return;
    }
    let (tx, rx) = mpsc::channel::<Work>();
    *WORK.lock().unwrap_or_else(|e| e.into_inner()) = Some(tx.clone());
    let links = super::link::subscribe_links({
        let tx = tx.clone();
        move |link| {
            let _ = tx.send(Work::Link(link));
        }
    });
    super::services::spawn_named("hold-lease", move || {
        let _alive = links;
        run(rx, tx);
    });
}

fn run(rx: mpsc::Receiver<Work>, tx: mpsc::Sender<Work>) {
    let mut seen: Vec<(Weak<Link>, EventSubscription, EventSubscription)> = Vec::new();
    for work in rx {
        seen.retain(|(l, _, _)| l.strong_count() > 0);
        match work {
            Work::Link(link) => {
                let weak = Arc::downgrade(&link);
                if seen.iter().any(|(l, _, _)| Weak::ptr_eq(l, &weak)) {
                    continue;
                }
                // On the link's thread: only enqueue. The link's state now comes first.
                let events = link.subscribe_events({
                    let tx = tx.clone();
                    let weak = weak.clone();
                    move |event| {
                        if let Event::State(state) = event {
                            let _ = tx.send(if state.connected.is_some() {
                                Work::Connected(weak.clone())
                            } else {
                                Work::Disconnected(weak.clone())
                            });
                        }
                    }
                });
                let reports = link.subscribe_reports({
                    let tx = tx.clone();
                    let weak = weak.clone();
                    move |_, report| {
                        if let Some(r) = wire::parse(report) {
                            let _ = tx.send(Work::Report(weak.clone(), r));
                        }
                    }
                });
                seen.push((weak, events, reports));
            }
            Work::Connected(weak) => {
                let Some(link) = weak.upgrade() else {
                    continue;
                };
                let lease = takes_lease(&link);
                note(|h| h.supported = Some(lease));
                if act(&*link, Trigger::Connected { lease }, || {}) != Action::Nothing {
                    let mut l = leased();
                    if !l.iter().any(|w| Weak::ptr_eq(w, &weak)) {
                        l.push(weak.clone());
                    }
                }
            }
            Work::Disconnected(weak) => {
                // The keyboard drops the lease with the link; the next connect sets it again.
                leased().retain(|w| !Weak::ptr_eq(w, &weak));
                note(|h| h.supported = None);
            }
            Work::Report(weak, r) => {
                let Some(link) = weak.upgrade() else {
                    continue;
                };
                act(&*link, Trigger::Report(r), || {
                    super::art_service::holds_lapsed();
                    super::knob::holds_lapsed(weak.clone());
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use foxglove_link::protocol;
    use std::cell::Cell;
    use std::time::Duration;

    /// A keyboard that keeps what it was sent.
    #[derive(Default)]
    struct FakeKeyboard {
        sent: Mutex<Vec<Vec<u8>>>,
    }

    impl Transport for FakeKeyboard {
        fn send(&self, report: &[u8]) -> anyhow::Result<()> {
            self.sent.lock().unwrap().push(report.to_vec());
            Ok(())
        }

        fn request(
            &self,
            _: &[u8],
            _: u8,
            _: u8,
            _: Duration,
        ) -> anyhow::Result<[u8; protocol::REPORT]> {
            anyhow::bail!("the lease never waits for a reply")
        }
    }

    impl FakeKeyboard {
        fn take(&self) -> Vec<Vec<u8>> {
            std::mem::take(&mut *self.sent.lock().unwrap())
        }
    }

    const SET: [u8; 3] = [0xD7, 0x15, 30];

    fn report(kind: Kind, held: u8) -> Report {
        Report {
            kind,
            status: 0,
            lease_s: LEASE_S,
            left_s: 0,
            held,
            lapses: 0,
        }
    }

    #[test]
    fn a_connect_sets_the_lease_only_on_firmware_that_takes_one() {
        let kb = FakeKeyboard::default();
        act(&kb, Trigger::Connected { lease: true }, || {
            panic!("no retake")
        });
        assert_eq!(kb.take(), [SET]);
        act(&kb, Trigger::Connected { lease: false }, || {
            panic!("no retake")
        });
        assert!(kb.take().is_empty(), "firmware before #847 is told nothing");
    }

    #[test]
    fn the_keyboards_ask_renews_and_a_lapse_takes_the_modes_back() {
        let kb = FakeKeyboard::default();
        let retaken = Cell::new(0);
        act(&kb, Trigger::Report(report(Kind::Ask, 3)), || {
            panic!("no retake")
        });
        assert_eq!(kb.take(), [SET], "the ask is answered with the lease");
        act(&kb, Trigger::Report(report(Kind::Reply, 3)), || {
            panic!("no retake")
        });
        assert!(kb.take().is_empty(), "a reply is only an echo");
        act(&kb, Trigger::Report(report(Kind::Lapsed, 3)), || {
            retaken.set(retaken.get() + 1)
        });
        assert_eq!(retaken.get(), 1);
        assert!(
            kb.take().is_empty(),
            "the services take the modes back themselves"
        );
    }

    #[test]
    fn the_reports_are_the_firmwares() {
        // The bytes tests/zmk-sim/hold-lease pins: an ask is renewed, a lapse retaken.
        let ask = protocol::hold_lease::parse(&{
            let mut r = [0u8; protocol::REPORT];
            r[..7].copy_from_slice(&[0xD7, 0x95, 1, 0, 0x1E, 0x14, 3]);
            r
        })
        .unwrap();
        assert_eq!(decide(Trigger::Report(ask)), Action::Send(SET.to_vec()));
        let lapsed = protocol::hold_lease::parse(&{
            let mut r = [0u8; protocol::REPORT];
            r[..8].copy_from_slice(&[0xD7, 0x95, 2, 0, 0x1E, 0, 3, 1]);
            r
        })
        .unwrap();
        assert_eq!(decide(Trigger::Report(lapsed)), Action::Retake);
        assert_eq!(held_words(lapsed.held), "art keys and knob");
    }

    /// The keyboard's side, as widgets/host_link.c runs it: any report from the PC renews; while
    /// a mode is held it asks at a third and two thirds of the lease's quiet and lets go at the
    /// full lease.
    struct FirmwareLease {
        lease_s: u64,
        heard: u64,
        asks: u8,
        held: bool,
        lapses: u32,
    }

    impl FirmwareLease {
        fn heard(&mut self, now: u64, report: &[u8]) {
            self.heard = now;
            self.asks = 0;
            if report[..2] == [0xD7, 0x15] && report[2] != wire::READ {
                self.lease_s = u64::from(report[2]);
            }
        }

        /// What the keyboard sends at second `now`.
        fn tick(&mut self, now: u64) -> Option<Kind> {
            if self.lease_s == 0 || !self.held {
                return None;
            }
            let quiet = now - self.heard;
            if quiet >= self.lease_s {
                self.held = false;
                self.lapses += 1;
                return Some(Kind::Lapsed);
            }
            if self.asks < 2 && quiet == (u64::from(self.asks) + 1) * self.lease_s / 3 {
                self.asks += 1;
                return Some(Kind::Ask);
            }
            None
        }
    }

    /// #847's acceptance over a 10-minute run: a live app keeps its modes with one renewal per
    /// third of the lease of quiet and nothing else; a killed one loses them within the lease;
    /// nothing held is nothing sent either way.
    #[test]
    fn a_live_app_keeps_its_modes_and_a_killed_one_loses_them_within_the_lease() {
        /// (renewals sent, lapses, the second the modes went)
        fn run(held: bool, killed_at: u64) -> (usize, u32, Option<u64>) {
            let kb = FakeKeyboard::default();
            let mut fw = FirmwareLease {
                lease_s: 0,
                heard: 0,
                asks: 0,
                held,
                lapses: 0,
            };
            act(&kb, Trigger::Connected { lease: true }, || {});
            for r in kb.take() {
                fw.heard(0, &r);
            }
            let (mut renewals, mut gone) = (0, None);
            for now in 1..=600 {
                let Some(kind) = fw.tick(now) else {
                    continue;
                };
                if kind == Kind::Lapsed {
                    gone = gone.or(Some(now));
                }
                if now >= killed_at {
                    continue; // a killed app hears nothing
                }
                act(&kb, Trigger::Report(report(kind, 3)), || fw.held = true);
                for r in kb.take() {
                    renewals += 1;
                    fw.heard(now, &r);
                }
            }
            (renewals, fw.lapses, gone)
        }
        // Alive all along, holding: a renewal every 10 s (the first ask of each quiet stretch),
        // no lapse.
        assert_eq!(run(true, u64::MAX), (60, 0, None));
        // Killed at minute 5: the last renewal at 290 s, asks at 300 and 310 go unanswered, and
        // the keyboard lets go at 320 s, within the lease.
        assert_eq!(run(true, 300), (29, 1, Some(320)));
        // Nothing held: the keyboard asks nothing, so the app sends nothing past the connect.
        assert_eq!(run(false, u64::MAX), (0, 0, None));
    }

    #[test]
    fn the_health_row_says_how_the_lease_runs() {
        let row = |h: &LeaseHealth| rows(h).remove(0);
        assert_eq!(row(&LeaseHealth::default()).level, Level::Ok);
        let old = row(&LeaseHealth {
            supported: Some(false),
            ..Default::default()
        });
        assert!(
            old.detail.contains("only when the link drops"),
            "{}",
            old.detail
        );
        let ok = row(&LeaseHealth {
            supported: Some(true),
            renewals: 4,
            ..Default::default()
        });
        assert_eq!(ok.level, Level::Ok);
        assert!(ok.detail.contains("4 renewal(s)"), "{}", ok.detail);
        let lapsed = row(&LeaseHealth {
            supported: Some(true),
            lapses: 1,
            last_lapse: Some("art keys and knob".into()),
            ..Default::default()
        });
        assert_eq!(lapsed.level, Level::Warn);
        assert!(
            lapsed.detail.contains("art keys and knob"),
            "{}",
            lapsed.detail
        );
    }
}
