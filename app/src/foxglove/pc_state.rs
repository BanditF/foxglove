//! The PC's state for the keyboard (#609, #604): awake with a lease, or away.
//!
//! ZMK idles a keyboard 30 s after its last key whatever the power source, so a plugged-in
//! keyboard went dark while the PC was in use (mouse only, reading), and stayed dark after the PC
//! woke: Modern Standby never suspends the keyboard's USB, so the keyboard can't tell. Foxglove
//! can, from the same Windows events as `away.rs` (#410), and tells the left half (D7 11
//! PC_STATE, HELLO bit 26; firmware widgets/pc_state.h), which tells the right:
//!
//! - **The display goes off** (the PC asleep, user decision 1 on #609): away. The keyboard idles
//!   at once.
//! - **The display comes on, or the session unlocks** (#604's "back"): awake with the wake flag.
//!   The keyboard wakes at once, as a key would, and holds a half on its own USB power out of
//!   idle for [`LEASE_S`].
//! - **A lock alone** changes nothing (decision 1): the keyboard stays lit.
//! - **A keyboard connects** (HELLO with bit 26, each reconnect too): awake without the wake
//!   (or away with the display off), so a keyboard that rebooted or replugged has the state.
//! - **The lease:** the keyboard asks for a renewal a minute and again 20 s before the lease ends
//!   (an unsolicited D7 91 with event 1). The app answers with a new lease while the display is
//!   on, and with nothing while it is off. So nothing here runs on a clock: the keyboard's
//!   one-shot ask is the only timer, it fires only while the lease runs on USB power, and a
//!   closed or crashed app simply lets the lease lapse. The keyboard then stays lit 10 minutes
//!   after the last key and idles (decision 2). On battery the keyboard holds nothing and asks
//!   nothing: only the edges above are sent.
//!
//! Polling: none. Start events: a lock/display edge, a link's connect, the keyboard's ask. Stop:
//! each sends at most one report; the thread sleeps on its channel between them.

use std::sync::{mpsc, Arc, Weak};

use foxglove_link::link::{Event, EventSubscription, Link};
use foxglove_link::protocol::{self, capabilities};
use foxglove_link::Transport;

use super::away::{self, Change};

/// The lease the app gives the keyboard, in seconds. The keyboard asks for a renewal a minute
/// before it ends, so a running app renews about every 9 minutes, and a gone one is noticed
/// within 10.
pub const LEASE_S: u16 = 600;

/// What can make the app tell the keyboard something.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trigger {
    /// A keyboard with PC_STATE connected (or reconnected).
    Connected,
    /// A lock or display edge (`away.rs`).
    Change(Change),
    /// The keyboard's ask: its lease ends soon.
    LeaseEnding,
}

/// What the app tells the keyboard.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Message {
    Away,
    Awake { wake: bool },
}

impl Message {
    pub fn report(self) -> Vec<u8> {
        match self {
            Message::Away => protocol::pc_away(),
            Message::Awake { wake } => protocol::pc_awake(LEASE_S, wake),
        }
    }

    fn label(self) -> &'static str {
        match self {
            Message::Away => "away",
            Message::Awake { wake: true } => "awake, wake",
            Message::Awake { wake: false } => "awake",
        }
    }
}

impl Trigger {
    fn label(self) -> &'static str {
        match self {
            Trigger::Connected => "keyboard connected",
            Trigger::Change(Change::DisplayOff) => "display off",
            Trigger::Change(Change::DisplayOn) => "display on",
            Trigger::Change(Change::Locked) => "session locked",
            Trigger::Change(Change::Unlocked) => "session unlocked",
            Trigger::LeaseEnding => "lease renewal",
        }
    }
}

/// What to tell the keyboard on `trigger`, with the display off or on now. `None`: nothing.
pub fn decide(trigger: Trigger, display_off: bool) -> Option<Message> {
    match trigger {
        Trigger::Change(Change::DisplayOff) => Some(Message::Away),
        Trigger::Change(Change::DisplayOn) => Some(Message::Awake { wake: true }),
        // An unlock is #604's "back" too; with the display still off the PC isn't back yet.
        Trigger::Change(Change::Unlocked) => {
            (!display_off).then_some(Message::Awake { wake: true })
        }
        // Decision 1 on #609: a lock alone keeps the keyboard lit.
        Trigger::Change(Change::Locked) => None,
        Trigger::Connected => Some(if display_off {
            Message::Away
        } else {
            Message::Awake { wake: false }
        }),
        // Only an awake PC renews; asleep, the lease lapses (it is away anyway).
        Trigger::LeaseEnding => (!display_off).then_some(Message::Awake { wake: false }),
    }
}

/// Tells `keyboard` what `trigger` calls for, with one log line; what it sent.
pub fn tell(keyboard: &dyn Transport, trigger: Trigger, display_off: bool) -> Option<Message> {
    let message = decide(trigger, display_off)?;
    match keyboard.send(&message.report()) {
        Ok(()) => eprintln!(
            "Foxglove: PC state to the keyboard: {} ({})",
            message.label(),
            trigger.label()
        ),
        Err(e) => eprintln!(
            "Foxglove: PC state to the keyboard: {} ({}) failed: {e}",
            message.label(),
            trigger.label()
        ),
    }
    Some(message)
}

/// Whether the keyboard on `link` said it takes PC_STATE (HELLO bit 26).
fn takes_pc_state(link: &Link) -> bool {
    link.state()
        .connected
        .and_then(|c| c.hello)
        .is_some_and(|h| h.capabilities & capabilities::PC_STATE != 0)
}

enum Work {
    Link(Arc<Link>),
    Connected(Weak<Link>),
    LeaseEnding(Weak<Link>),
    Change(Change),
}

/// Starts following the PC's state for every shared keyboard link. `FOXGLOVE_DISABLE=pc-state`
/// leaves it out (the keyboard then goes by its own rules: lit 10 minutes after the last key on
/// USB power).
pub fn start() {
    if super::repaint_trace::disabled("pc-state") {
        return;
    }
    let (tx, rx) = mpsc::channel::<Work>();
    // Both listeners run with their lists locked: they only enqueue.
    let changes = away::subscribe_changes({
        let tx = tx.clone();
        move |change| {
            let _ = tx.send(Work::Change(change));
        }
    });
    let links = super::link::subscribe_links({
        let tx = tx.clone();
        move |link| {
            let _ = tx.send(Work::Link(link));
        }
    });
    super::services::spawn_named("pc-state", move || {
        let _alive = (changes, links);
        let mut seen: Vec<(Weak<Link>, EventSubscription, EventSubscription)> = Vec::new();
        for work in rx {
            seen.retain(|(l, _, _)| l.strong_count() > 0);
            match work {
                Work::Link(link) => {
                    let weak = Arc::downgrade(&link);
                    if seen.iter().any(|(l, _, _)| Weak::ptr_eq(l, &weak)) {
                        continue;
                    }
                    // On the link's thread: only enqueue. A connect with bit 26, once per
                    // connection (State events come on connects and disconnects only).
                    let events = link.subscribe_events({
                        let tx = tx.clone();
                        let weak = weak.clone();
                        move |event| {
                            if let Event::State(state) = event {
                                let takes = state
                                    .connected
                                    .and_then(|c| c.hello)
                                    .is_some_and(|h| h.capabilities & capabilities::PC_STATE != 0);
                                if takes {
                                    let _ = tx.send(Work::Connected(weak.clone()));
                                }
                            }
                        }
                    });
                    let asks = link.subscribe_reports({
                        let tx = tx.clone();
                        let weak = weak.clone();
                        move |_, report| {
                            if protocol::is_pc_state_event(report) {
                                let _ = tx.send(Work::LeaseEnding(weak.clone()));
                            }
                        }
                    });
                    seen.push((weak, events, asks));
                }
                Work::Connected(link) => {
                    if let Some(link) = link.upgrade() {
                        tell(&*link, Trigger::Connected, away::display_off());
                    }
                }
                Work::LeaseEnding(link) => {
                    if let Some(link) = link.upgrade() {
                        tell(&*link, Trigger::LeaseEnding, away::display_off());
                    }
                }
                Work::Change(change) => {
                    for (link, _, _) in &seen {
                        if let Some(link) = link.upgrade().filter(|l| takes_pc_state(l)) {
                            tell(&*link, Trigger::Change(change), away::display_off());
                        }
                    }
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
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
            anyhow::bail!("PC_STATE never waits for a reply")
        }
    }

    impl FakeKeyboard {
        fn take(&self) -> Vec<Vec<u8>> {
            std::mem::take(&mut *self.sent.lock().unwrap())
        }
    }

    const AWAY: [u8; 6] = [0xD7, 0x11, 0, 0, 0, 0];
    const BACK: [u8; 6] = [0xD7, 0x11, 1, 1, 0x58, 0x02]; // awake, wake, 600 s
    const RENEW: [u8; 6] = [0xD7, 0x11, 1, 0, 0x58, 0x02]; // awake, 600 s

    #[test]
    fn the_display_is_the_pc_sleeping_and_a_lock_alone_keeps_it_lit() {
        let kb = FakeKeyboard::default();
        // Decision 1 on #609: the display off is away, a lock alone is nothing.
        tell(&kb, Trigger::Change(Change::Locked), false);
        assert!(kb.take().is_empty(), "a lock alone sends nothing");
        tell(&kb, Trigger::Change(Change::DisplayOff), true);
        assert_eq!(kb.take(), [AWAY]);
        // #604: the display back on wakes the keyboard at once.
        tell(&kb, Trigger::Change(Change::DisplayOn), false);
        assert_eq!(kb.take(), [BACK]);
        // An unlock is "back" too, but not while the display is still off.
        tell(&kb, Trigger::Change(Change::Unlocked), true);
        assert!(kb.take().is_empty());
        tell(&kb, Trigger::Change(Change::Unlocked), false);
        assert_eq!(kb.take(), [BACK]);
    }

    #[test]
    fn a_connect_gives_the_state_and_the_keyboards_ask_renews_only_while_awake() {
        let kb = FakeKeyboard::default();
        tell(&kb, Trigger::Connected, false);
        assert_eq!(kb.take(), [RENEW], "a connect doesn't wake, it holds");
        tell(&kb, Trigger::Connected, true);
        assert_eq!(kb.take(), [AWAY]);
        tell(&kb, Trigger::LeaseEnding, false);
        assert_eq!(kb.take(), [RENEW]);
        tell(&kb, Trigger::LeaseEnding, true);
        assert!(kb.take().is_empty(), "asleep, the lease lapses");
    }

    /// The keyboard's side, as widgets/pc_state.c runs it: a lease from each awake, asks a
    /// minute and 20 s before its end, and only while it has USB power.
    struct FirmwareLease {
        usb: bool,
        lease_end: Option<u64>,
        asks: u8,
    }

    impl FirmwareLease {
        fn take(&mut self, now: u64, report: &[u8]) {
            match report[2] {
                1 => {
                    let lease = u64::from(u16::from_le_bytes([report[4], report[5]]));
                    self.lease_end = Some(now + lease);
                    self.asks = 0;
                }
                0 => self.lease_end = None,
                _ => {}
            }
        }

        /// Whether the keyboard asks at second `now`.
        fn asks_at(&mut self, now: u64) -> bool {
            let Some(end) = self.lease_end.filter(|&end| self.usb && now < end) else {
                return false;
            };
            let due = match self.asks {
                0 => end - 60,
                1 => end - 20,
                _ => return false,
            };
            if now == due {
                self.asks += 1;
                return true;
            }
            false
        }
    }

    /// #609's acceptance count over a 30-minute run: one message per display change, one renewal
    /// per lease period while awake on USB power, none while away, none on battery beyond the
    /// changes.
    #[test]
    fn a_30_minute_run_sends_one_per_change_and_one_renewal_per_lease() {
        fn deliver(fw: &mut FirmwareLease, now: u64, kb: &FakeKeyboard) {
            for r in kb.take() {
                fw.take(now, &r);
            }
        }
        fn run(usb: bool, display_off_from: u64, display_on_at: u64) -> (usize, usize, usize) {
            let kb = FakeKeyboard::default();
            let mut fw = FirmwareLease {
                usb,
                lease_end: None,
                asks: 0,
            };
            let (mut changes, mut renewals, mut away_sends) = (0, 0, 0);
            let mut display_off = false;
            tell(&kb, Trigger::Connected, false);
            deliver(&mut fw, 0, &kb);
            for now in 1..=30 * 60 {
                if now == display_off_from {
                    display_off = true;
                    changes +=
                        usize::from(tell(&kb, Trigger::Change(Change::DisplayOff), true).is_some());
                }
                if now == display_on_at {
                    display_off = false;
                    changes +=
                        usize::from(tell(&kb, Trigger::Change(Change::DisplayOn), false).is_some());
                }
                if fw.asks_at(now) {
                    let sent = tell(&kb, Trigger::LeaseEnding, display_off).is_some();
                    renewals += usize::from(sent);
                    away_sends += usize::from(sent && display_off);
                }
                deliver(&mut fw, now, &kb);
            }
            (changes, renewals, away_sends)
        }
        // Awake on USB all along: a renewal at 9, 18 and 27 minutes, nothing else.
        assert_eq!(run(true, u64::MAX, u64::MAX), (0, 3, 0));
        // The display off from minute 5 to 25: away, then back; the asks the lease brings while
        // away get no answer, and the lease from the back is renewed at minute 34 (past the run).
        assert_eq!(run(true, 5 * 60, 25 * 60), (2, 0, 0));
        // On battery: the keyboard asks nothing, so only the changes go.
        assert_eq!(run(false, 5 * 60, 25 * 60), (2, 0, 0));
        assert_eq!(run(false, u64::MAX, u64::MAX), (0, 0, 0));
    }
}
