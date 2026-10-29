//! Right-hand art: push a set through the left half to the right one, or pick a built-in set.
//! Blob format: art_relay.h ("NVA1" header), written by scripts/flipper_art.py.

use std::time::{Duration, Instant};

use anyhow::{bail, ensure, Result};

use crate::protocol::{self, art, left_state, right_state, ArtStatus, SlotInfo};
use crate::Transport;

pub const MAGIC: &[u8; 4] = b"NVA1";

/// Whether `blob` looks like an art set (magic only; the right half checks the rest).
pub fn is_blob(blob: &[u8]) -> bool {
    blob.starts_with(MAGIC)
}

fn command(t: &impl Transport, report: &[u8]) -> Result<ArtStatus> {
    let r = t.request(
        report,
        protocol::ART,
        report[1],
        Duration::from_millis(1000),
    )?;
    Ok(ArtStatus::parse(&r))
}

/// The relay status. The left answers from what it last heard and asks the right again, so a
/// second call shortly after returns the right half's fresh state.
pub fn status(t: &impl Transport) -> Result<ArtStatus> {
    command(t, &protocol::art_status())
}

/// Sends a blob to the left half and has it pushed to the right; returns once the right half
/// confirms or the relay fails.
pub fn push(
    t: &impl Transport,
    blob: &[u8],
    mut progress: impl FnMut(&ArtStatus),
) -> Result<ArtStatus> {
    push_cancellable(t, blob, &mut progress, || false)
}

/// Like [`push`], but stops sending when cancelled. There is no wire-level abort; an
/// unfinished receive expires on the keyboard, and a committed relay may still finish.
pub fn push_cancellable(
    t: &impl Transport,
    blob: &[u8],
    mut progress: impl FnMut(&ArtStatus),
    cancelled: impl Fn() -> bool,
) -> Result<ArtStatus> {
    let check = || {
        ensure!(!cancelled(), "push cancelled");
        Ok(())
    };
    check()?;
    ensure!(
        blob.len() <= u16::MAX as usize,
        "blob is {} bytes; the most is {}",
        blob.len(),
        u16::MAX
    );
    let st = command(
        t,
        &protocol::art_begin(blob.len() as u16, crc32fast::hash(blob)),
    )?;
    if st.state != left_state::RECEIVING {
        bail!("the left half refused the blob: {st}");
    }
    for (i, chunk) in blob.chunks(art::DATA_MAX).enumerate() {
        check()?;
        t.send(&protocol::art_data((i * art::DATA_MAX) as u16, chunk))?;
    }
    check()?;
    let mut st = command(t, &protocol::art_commit())?;
    let end = Instant::now() + Duration::from_secs(120);
    while st.state == left_state::PUSHING && Instant::now() < end {
        progress(&st);
        std::thread::sleep(Duration::from_millis(250));
        check()?;
        st = status(t)?;
    }
    if st.state != left_state::DONE {
        bail!("push failed: {st}");
    }
    Ok(st)
}

/// Before the one retry of a SELECT whose relay write failed: the left's request in the way (a
/// timed-out one, still out on the link) gets this long to end. Newer firmware waits for it
/// itself (#618); this covers a left without that.
pub const SELECT_RETRY_PAUSE: Duration = Duration::from_millis(500);

/// Shows built-in set `set` again (-1 = random). A relay write that fails ("Bluetooth write
/// failed": the split link was busy with other traffic) is tried once more, with a log line;
/// the error comes back only when that try fails too (#618).
pub fn select(t: &impl Transport, set: i8) -> Result<ArtStatus> {
    match select_once(t, set)? {
        Ok(st) => Ok(st),
        Err(st) if st.state == left_state::FAILED && st.error == protocol::relay_error::BLE => {
            note(format!(
                "Foxglove link: art SELECT {set}: the relay's write to the right failed ({st}); \
                 trying once more"
            ));
            std::thread::sleep(SELECT_RETRY_PAUSE);
            match select_once(t, set)? {
                Ok(st) => {
                    note(format!(
                        "Foxglove link: art SELECT {set}: the second try went through"
                    ));
                    Ok(st)
                }
                Err(st) => select_error(st),
            }
        }
        Err(st) => select_error(st),
    }
}

/// A log line (the app's stderr goes to foxglove.log); tests read them back.
fn note(line: String) {
    #[cfg(test)]
    tests::NOTES.with(|n| n.borrow_mut().push(line.clone()));
    eprintln!("{line}");
}

/// One SELECT and the left's relay of it: Ok, or Err with the status it ended on.
fn select_once(t: &impl Transport, set: i8) -> Result<Result<ArtStatus, ArtStatus>> {
    let mut st = command(t, &protocol::art_select(set))?;
    let end = Instant::now() + Duration::from_secs(10);
    while st.state == left_state::PUSHING && Instant::now() < end {
        std::thread::sleep(Duration::from_millis(100));
        st = status(t)?;
    }
    Ok(match st.state {
        left_state::FAILED | left_state::PUSHING => Err(st),
        _ => Ok(st),
    })
}

fn select_error(st: ArtStatus) -> Result<ArtStatus> {
    if st.state == left_state::PUSHING {
        bail!("select did not finish in 10 s: {st}");
    }
    bail!("select failed: {st}")
}

/// Selects `set` and returns the right half's state once it plays it. The right acknowledges
/// SELECT and only then switches sets on its display queue, so the select's own reply still
/// names the old set (and for -1 not the one it drew). Each reply also has the left read the
/// right again (see [`status`]), so one status `settle` later names the set now on screen. The
/// select stands if that status goes unanswered: its own reply comes back instead.
pub fn select_settled(t: &impl Transport, set: i8, settle: Duration) -> Result<ArtStatus> {
    let selected = select(t, set)?;
    std::thread::sleep(settle);
    Ok(status(t).unwrap_or(selected))
}

/// Saves the pushed set now playing on the right into flash slot `slot`, under `name`, and
/// waits for the right half to finish writing (a few seconds at most).
pub fn save(t: &impl Transport, slot: u8, name: &str) -> Result<ArtStatus> {
    let st = command(t, &protocol::art_save(slot, name))?;
    finish_slot_op(t, st, "save")
}

/// Empties flash slot `slot`. If it was on screen, the right switches to a random built-in.
pub fn erase(t: &impl Transport, slot: u8) -> Result<ArtStatus> {
    let st = command(t, &protocol::art_erase(slot))?;
    finish_slot_op(t, st, "erase")
}

/// SAVE and ERASE run on the right after the left has relayed them: wait for the left's relay
/// to finish, then for the right to leave SAVING.
fn finish_slot_op(t: &impl Transport, mut st: ArtStatus, what: &str) -> Result<ArtStatus> {
    let end = Instant::now() + Duration::from_secs(20);
    while (st.state == left_state::PUSHING || st.right_state == right_state::SAVING)
        && Instant::now() < end
    {
        std::thread::sleep(Duration::from_millis(250));
        st = status(t)?;
    }
    match (st.state, st.right_state) {
        (left_state::FAILED, _) => bail!("{what} failed: {st}"),
        (_, right_state::SAVE_FAILED) => bail!("{what} failed on the right half: {st}"),
        (_, right_state::SAVING) => bail!("{what} did not finish in 20 s: {st}"),
        _ => Ok(st),
    }
}

/// What flash slot `slot` holds. The left asks the right, so this can take a moment.
pub fn slot(t: &impl Transport, slot: u8) -> Result<SlotInfo> {
    let r = t.request(
        &protocol::art_slot(slot),
        protocol::ART,
        art::SLOT,
        Duration::from_secs(3),
    )?;
    SlotInfo::parse(&r).map_err(|status| {
        anyhow::anyhow!(match status {
            1 => "the relay is busy".to_string(),
            2 => "the right half is not connected".to_string(),
            3 => "reading the right half failed".to_string(),
            4 => "the right half's firmware has no flash slots".to_string(),
            n => format!("slot read error {n}"),
        })
    })
}

/// How [`wait_relay_ready`] ended.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RelayWait {
    /// The relay to the right half is up (link 2): the status that said so.
    Ready(ArtStatus),
    /// The deadline passed first; the last status heard, if any.
    TimedOut(Option<ArtStatus>),
}

/// Waits until the left's relay to the right half is up (art STATUS link 2), for at most
/// `timeout`. It asks STATUS once (that is host traffic, which the firmware needs before it sends
/// events, and it gives the state now), then sleeps on the link's events until the left sends
/// the relay-ready event (D6 85 with link 2, firmware with HELLO bits 3 and 20) or the deadline.
/// A reconnect asks STATUS again. No timer besides the deadline (#594).
pub fn wait_relay_ready(link: &crate::link::Link, timeout: Duration) -> Result<RelayWait> {
    use crate::link::Event;
    use std::sync::mpsc;

    let end = Instant::now() + timeout;
    // Subscribed before asking, so an event between the reply and the wait isn't missed.
    let (tx, rx) = mpsc::channel();
    let _events = link.subscribe_events(move |e| {
        let _ = tx.send(e);
    });
    let mut generation = link.state().generation;
    let mut last = None;
    let mut ask = true;
    loop {
        if std::mem::take(&mut ask) {
            if let Ok(s) = status(link) {
                if s.link == 2 {
                    return Ok(RelayWait::Ready(s));
                }
                last = Some(s);
            }
        }
        let left = end.saturating_duration_since(Instant::now());
        match rx.recv_timeout(left) {
            Ok(Event::ArtStatus { report, .. }) => {
                let s = ArtStatus::parse(&report);
                if s.link == 2 {
                    return Ok(RelayWait::Ready(s));
                }
                last = Some(s);
            }
            Ok(Event::State(s)) => {
                // A new connection: the firmware needs host traffic again before it sends events.
                if s.connected.is_some() && s.generation != generation {
                    generation = s.generation;
                    ask = true;
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => return Ok(RelayWait::TimedOut(last)),
            Err(mpsc::RecvTimeoutError::Disconnected) => bail!("link stopped"),
        }
    }
}

/// Plays flash slot `slot` (SELECT set_count + slot).
pub fn select_slot(t: &impl Transport, set_count: u8, slot: u8) -> Result<ArtStatus> {
    let set = i8::try_from(u16::from(set_count) + u16::from(slot))
        .map_err(|_| anyhow::anyhow!("slot {slot} is out of SELECT's range"))?;
    select(t, set)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::panel::tests::Mock;

    fn reply(cmd: u8, state: u8) -> [u8; protocol::REPORT] {
        let mut r = [0u8; protocol::REPORT];
        r[0] = protocol::ART;
        r[1] = 0x80 | cmd;
        r[2] = state;
        r
    }

    #[test]
    fn push_sends_begin_data_commit() {
        let t = Mock::default();
        t.replies.borrow_mut().extend([
            reply(art::BEGIN, left_state::RECEIVING),
            reply(art::COMMIT, left_state::PUSHING),
            reply(art::STATUS, left_state::DONE),
        ]);
        let blob: Vec<u8> = b"NVA1"
            .iter()
            .copied()
            .chain((0..56).map(|i| i as u8))
            .collect(); // 60 bytes
        let mut seen = 0;
        push(&t, &blob, |_| seen += 1).unwrap();
        assert_eq!(seen, 1);

        let sent = t.sent.borrow();
        assert_eq!(sent[0], protocol::art_begin(60, crc32fast::hash(&blob)));
        // 27 + 27 + 6 bytes
        assert_eq!(&sent[1][..5], &[0xD6, 0x02, 0, 0, 27]);
        assert_eq!(&sent[2][..5], &[0xD6, 0x02, 27, 0, 27]);
        assert_eq!(&sent[3][..5], &[0xD6, 0x02, 54, 0, 6]);
        assert_eq!(&sent[3][5..], &blob[54..]);
        assert_eq!(sent[4], protocol::art_commit());
        assert_eq!(sent[5], protocol::art_status());
    }

    #[test]
    fn push_stops_when_the_left_refuses() {
        let t = Mock::default();
        t.replies
            .borrow_mut()
            .push_back(reply(art::BEGIN, left_state::IDLE));
        assert!(push(&t, b"NVA1", |_| {}).is_err());
        assert_eq!(t.sent.borrow().len(), 1);
    }

    #[test]
    fn cancellation_stops_before_begin_or_before_commit() {
        use std::cell::Cell;

        let t = Mock::default();
        assert!(push_cancellable(&t, b"NVA1", |_| {}, || true).is_err());
        assert!(t.sent.borrow().is_empty());

        t.replies
            .borrow_mut()
            .push_back(reply(art::BEGIN, left_state::RECEIVING));
        let checks = Cell::new(0);
        let result = push_cancellable(
            &t,
            b"NVA1",
            |_| {},
            || {
                checks.set(checks.get() + 1);
                checks.get() == 3
            },
        );
        assert!(result.unwrap_err().to_string().contains("cancelled"));
        let sent = t.sent.borrow();
        assert_eq!(sent.len(), 2);
        assert_eq!(sent[0][1], art::BEGIN);
        assert_eq!(sent[1][1], art::DATA);
    }

    #[test]
    fn cancellation_stops_relay_polling() {
        let t = Mock::default();
        t.replies.borrow_mut().extend([
            reply(art::BEGIN, left_state::RECEIVING),
            reply(art::COMMIT, left_state::PUSHING),
        ]);
        let cancelled = std::cell::Cell::new(false);
        assert!(
            push_cancellable(&t, b"NVA1", |_| cancelled.set(true), || cancelled.get()).is_err()
        );
        assert_eq!(t.sent.borrow().last().unwrap()[1], art::COMMIT);
    }

    /// The select's reply still names the old set; the status after it names the new one.
    #[test]
    fn a_settled_select_reads_the_set_now_on_screen() {
        let with_set = |cmd, set: i8| {
            let mut r = reply(cmd, left_state::DONE);
            r[2 + 13] = set as u8;
            r
        };
        let t = Mock::default();
        t.replies
            .borrow_mut()
            .extend([with_set(art::SELECT, 1), with_set(art::STATUS, 4)]);
        let st = select_settled(&t, 4, Duration::ZERO).unwrap();
        assert_eq!(st.right_set, 4);
        let sent = t.sent.borrow();
        assert_eq!(sent.len(), 2);
        assert_eq!(sent[1], protocol::art_status());
        drop(sent);
        // No answer to the status: the select still succeeded.
        t.replies.borrow_mut().push_back(with_set(art::SELECT, 2));
        assert_eq!(select_settled(&t, 5, Duration::ZERO).unwrap().right_set, 2);
    }

    thread_local! {
        pub(super) static NOTES: std::cell::RefCell<Vec<String>> = const {
            std::cell::RefCell::new(Vec::new())
        };
    }

    fn notes() -> Vec<String> {
        NOTES.with(|n| n.take())
    }

    fn failed(cmd: u8, error: u8) -> [u8; protocol::REPORT] {
        let mut r = reply(cmd, left_state::FAILED);
        r[3] = error;
        r
    }

    /// #618: one "Bluetooth write failed" is tried once more, logged, and the user sees success.
    #[test]
    fn a_failed_relay_write_is_tried_once_more_and_logged() {
        notes();
        let t = Mock::default();
        t.replies.borrow_mut().extend([
            failed(art::SELECT, protocol::relay_error::BLE),
            reply(art::SELECT, left_state::DONE),
        ]);
        assert!(select(&t, 3).is_ok());
        let sent = t.sent.borrow();
        assert_eq!(sent.len(), 2, "{sent:?}");
        assert!(sent.iter().all(|r| *r == protocol::art_select(3)));
        let notes = notes();
        assert_eq!(notes.len(), 2, "{notes:?}");
        assert!(notes[0].contains("art SELECT 3") && notes[0].contains("trying once more"));
        assert!(notes[0].contains("Bluetooth write failed"));
        assert!(notes[1].contains("second try went through"));
    }

    /// The user's error comes only after the retry failed too; other errors aren't retried.
    #[test]
    fn the_error_shows_only_after_the_retry_fails() {
        notes();
        let t = Mock::default();
        t.replies.borrow_mut().extend([
            failed(art::SELECT, protocol::relay_error::BLE),
            failed(art::SELECT, protocol::relay_error::BLE),
        ]);
        let e = select(&t, 2).unwrap_err().to_string();
        assert!(e.contains("Bluetooth write failed"), "{e}");
        assert_eq!(t.sent.borrow().len(), 2);
        assert_eq!(notes().len(), 1);

        // "right half not connected" (1) isn't transient: no second try.
        let t = Mock::default();
        t.replies.borrow_mut().push_back(failed(art::SELECT, 1));
        assert!(select(&t, 2).is_err());
        assert_eq!(t.sent.borrow().len(), 1);
        assert!(notes().is_empty());
    }

    #[test]
    fn select_reports_failure() {
        let t = Mock::default();
        t.replies
            .borrow_mut()
            .push_back(reply(art::SELECT, left_state::DONE));
        assert!(select(&t, 3).is_ok());
        t.replies
            .borrow_mut()
            .push_back(reply(art::SELECT, left_state::FAILED));
        assert!(select(&t, 3).is_err());
    }

    /// A left half in memory for [`wait_relay_ready`]: reads block on a condvar (no polling),
    /// STATUS answers with this connection's link byte, and each STATUS can be followed by the
    /// unsolicited D6 85 reports in `events`, or by the connection dropping.
    mod relay {
        use super::super::*;
        use crate::link::{Connected, Link, RawHid, Report};
        use std::collections::VecDeque;
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        use std::sync::{Arc, Condvar, Mutex};

        #[derive(Default)]
        struct Left {
            inbox: Mutex<VecDeque<Report>>,
            ready: Condvar,
            stopped: AtomicBool,
            dead: AtomicBool,
            link: u8,
            events: Vec<u8>,
            drop_after_status: bool,
            statuses: Arc<AtomicUsize>,
        }

        fn status_report(link: u8, cmd: u8) -> Report {
            let mut r = [0u8; protocol::REPORT];
            r[0] = protocol::ART;
            r[1] = 0x80 | cmd;
            r[2 + 6] = link;
            r
        }

        impl Left {
            fn push(&self, r: Report) {
                self.inbox.lock().unwrap().push_back(r);
                self.ready.notify_all();
            }
        }

        impl RawHid for Left {
            fn read(&self, buf: &mut Report) -> Result<usize> {
                let mut inbox = self.inbox.lock().unwrap();
                loop {
                    if self.stopped.load(Ordering::Relaxed) {
                        return Ok(0);
                    }
                    if let Some(r) = inbox.pop_front() {
                        *buf = r;
                        return Ok(protocol::REPORT);
                    }
                    if self.dead.load(Ordering::Relaxed) {
                        anyhow::bail!("device gone");
                    }
                    inbox = self.ready.wait(inbox).unwrap();
                }
            }
            fn interrupt(&self) {
                let _inbox = self.inbox.lock().unwrap();
                self.stopped.store(true, Ordering::Relaxed);
                self.ready.notify_all();
            }
            fn write(&self, out: &[u8]) -> Result<()> {
                if out[..2] == [protocol::ART, art::STATUS] {
                    self.statuses.fetch_add(1, Ordering::Relaxed);
                    self.push(status_report(self.link, art::STATUS));
                    for &link in &self.events {
                        self.push(status_report(link, art::STATUS));
                    }
                    if self.drop_after_status {
                        let _inbox = self.inbox.lock().unwrap();
                        self.dead.store(true, Ordering::Relaxed);
                        self.ready.notify_all();
                    }
                }
                Ok(())
            }
        }

        /// A link whose n-th connection is `connections[n]`.
        fn start(connections: Vec<Left>) -> (Link, Arc<AtomicUsize>) {
            let statuses = Arc::new(AtomicUsize::new(0));
            let mut connections: VecDeque<Left> = connections
                .into_iter()
                .map(|c| Left {
                    statuses: statuses.clone(),
                    ..c
                })
                .collect();
            let link = Link::start(Box::new(move || {
                let left = connections
                    .pop_front()
                    .ok_or_else(|| anyhow::anyhow!("no keyboard"))?;
                let connected = Connected {
                    product: "Fake left".into(),
                    hello: None,
                    bus: Default::default(),
                };
                Ok((Box::new(left) as Box<dyn RawHid>, connected))
            }));
            link.wait_connected(Duration::from_secs(2)).unwrap();
            (link, statuses)
        }

        #[test]
        fn a_relay_already_up_needs_no_event() {
            let (link, statuses) = start(vec![Left {
                link: 2,
                ..Default::default()
            }]);
            let got = wait_relay_ready(&link, Duration::from_secs(5)).unwrap();
            assert!(matches!(got, RelayWait::Ready(s) if s.link == 2));
            assert_eq!(statuses.load(Ordering::Relaxed), 1);
        }

        #[test]
        fn the_relay_ready_event_ends_the_wait() {
            // Split only (1), then the relay is lost (0) and comes up (2) as events.
            let (link, statuses) = start(vec![Left {
                link: 1,
                events: vec![0, 2],
                ..Default::default()
            }]);
            let got = wait_relay_ready(&link, Duration::from_secs(5)).unwrap();
            assert!(matches!(got, RelayWait::Ready(s) if s.link == 2));
            assert_eq!(
                statuses.load(Ordering::Relaxed),
                1,
                "asked once, then events"
            );
        }

        #[test]
        fn no_event_times_out_with_the_last_status() {
            let (link, statuses) = start(vec![Left {
                link: 1,
                ..Default::default()
            }]);
            let started = Instant::now();
            let got = wait_relay_ready(&link, Duration::from_millis(150)).unwrap();
            assert!(matches!(got, RelayWait::TimedOut(Some(s)) if s.link == 1));
            assert!(started.elapsed() >= Duration::from_millis(150));
            assert_eq!(
                statuses.load(Ordering::Relaxed),
                1,
                "no re-asking on a timer"
            );
        }

        #[test]
        fn a_reconnect_asks_again() {
            // The left drops after answering; the next connection has the relay up.
            let (link, statuses) = start(vec![
                Left {
                    link: 1,
                    drop_after_status: true,
                    ..Default::default()
                },
                Left {
                    link: 2,
                    ..Default::default()
                },
            ]);
            let got = wait_relay_ready(&link, Duration::from_secs(5)).unwrap();
            assert!(matches!(got, RelayWait::Ready(s) if s.link == 2));
            assert_eq!(statuses.load(Ordering::Relaxed), 2);
        }
    }

    /// #618 with the app's own traffic: a left half in memory that runs the relay as the firmware
    /// does (one state for every caller: a SELECT is PUSHING until the relay ends, then DONE or
    /// FAILED), whose first relay write fails as it did beside a timed-out request. Other threads
    /// read STATUS and RGB_STATE through the same link the whole time, as the app's background
    /// does. The user's SELECT still succeeds (once retried) and nobody gets another's reply.
    mod contention {
        use super::super::*;
        use super::notes;
        use crate::link::{Connected, Link, RawHid, Report};
        use std::collections::VecDeque;
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        use std::sync::{Arc, Condvar, Mutex};

        #[derive(Default)]
        struct Relay {
            state: u8,
            error: u8,
            set: i8,
            selects: usize,
            /// STATUS reads the running SELECT stays PUSHING for.
            pushing_for: usize,
        }

        #[derive(Default)]
        struct Left {
            inbox: Mutex<VecDeque<Report>>,
            ready: Condvar,
            stopped: AtomicBool,
            relay: Mutex<Relay>,
            /// SELECTs whose relay write fails ("Bluetooth write failed").
            failing: usize,
            selects: Arc<AtomicUsize>,
            rgb_reads: Arc<AtomicUsize>,
        }

        impl Left {
            fn push(&self, r: Report) {
                self.inbox.lock().unwrap().push_back(r);
                self.ready.notify_all();
            }
        }

        fn art_reply(cmd: u8, relay: &Relay) -> Report {
            let mut r = [0u8; protocol::REPORT];
            r[0] = protocol::ART;
            r[1] = 0x80 | cmd;
            r[2] = relay.state;
            r[3] = relay.error;
            r[2 + 6] = 2; // relay up
            r[2 + 13] = relay.set as u8;
            r
        }

        impl RawHid for Left {
            fn read(&self, buf: &mut Report) -> Result<usize> {
                let mut inbox = self.inbox.lock().unwrap();
                loop {
                    if self.stopped.load(Ordering::Relaxed) {
                        return Ok(0);
                    }
                    if let Some(r) = inbox.pop_front() {
                        *buf = r;
                        return Ok(protocol::REPORT);
                    }
                    inbox = self.ready.wait(inbox).unwrap();
                }
            }
            fn interrupt(&self) {
                let _inbox = self.inbox.lock().unwrap();
                self.stopped.store(true, Ordering::Relaxed);
                self.ready.notify_all();
            }
            fn write(&self, out: &[u8]) -> Result<()> {
                match (out[0], out[1]) {
                    (protocol::ART, art::SELECT) => {
                        let mut relay = self.relay.lock().unwrap();
                        relay.selects += 1;
                        self.selects.fetch_add(1, Ordering::Relaxed);
                        relay.state = left_state::PUSHING;
                        relay.error = 0;
                        relay.pushing_for = 2;
                        relay.set = out[2] as i8;
                        let r = art_reply(art::SELECT, &relay);
                        drop(relay);
                        self.push(r);
                    }
                    (protocol::ART, art::STATUS) => {
                        let mut relay = self.relay.lock().unwrap();
                        if relay.state == left_state::PUSHING {
                            if relay.pushing_for > 0 {
                                relay.pushing_for -= 1;
                            } else if relay.selects <= self.failing {
                                relay.state = left_state::FAILED;
                                relay.error = protocol::relay_error::BLE;
                            } else {
                                relay.state = left_state::DONE;
                            }
                        }
                        let r = art_reply(art::STATUS, &relay);
                        drop(relay);
                        self.push(r);
                    }
                    (protocol::SETTINGS, protocol::settings::RGB_STATE) => {
                        self.rgb_reads.fetch_add(1, Ordering::Relaxed);
                        let mut r = [0u8; protocol::REPORT];
                        r[0] = protocol::SETTINGS;
                        r[1] = 0x80 | protocol::settings::RGB_STATE;
                        r[2] = 0x5A; // marks it as the RGB reply
                        self.push(r);
                    }
                    _ => {}
                }
                Ok(())
            }
        }

        type Started = (Arc<Link>, Arc<AtomicUsize>, Arc<AtomicUsize>);

        fn start(failing: usize) -> Started {
            let selects = Arc::new(AtomicUsize::new(0));
            let rgb_reads = Arc::new(AtomicUsize::new(0));
            let mut left = Some(Left {
                failing,
                selects: selects.clone(),
                rgb_reads: rgb_reads.clone(),
                ..Default::default()
            });
            let link = Link::start(Box::new(move || {
                let left = left.take().ok_or_else(|| anyhow::anyhow!("no keyboard"))?;
                let connected = Connected {
                    product: "Fake left".into(),
                    hello: None,
                    bus: Default::default(),
                };
                Ok((Box::new(left) as Box<dyn RawHid>, connected))
            }));
            link.wait_connected(Duration::from_secs(2)).unwrap();
            (Arc::new(link), selects, rgb_reads)
        }

        /// The app's background: STATUS and RGB_STATE reads until `stop`, each answered by its
        /// own kind of reply. Returns how many each thread made.
        fn background(
            link: &Arc<Link>,
            stop: &Arc<AtomicBool>,
        ) -> Vec<std::thread::JoinHandle<usize>> {
            let status_reads = {
                let (link, stop) = (link.clone(), stop.clone());
                std::thread::spawn(move || {
                    let mut n = 0;
                    while !stop.load(Ordering::Relaxed) {
                        let st = status(&*link).unwrap();
                        assert_eq!(st.link, 2);
                        n += 1;
                    }
                    n
                })
            };
            let rgb = {
                let (link, stop) = (link.clone(), stop.clone());
                std::thread::spawn(move || {
                    let mut n = 0;
                    while !stop.load(Ordering::Relaxed) {
                        let r = link
                            .request(
                                &[protocol::SETTINGS, protocol::settings::RGB_STATE],
                                protocol::SETTINGS,
                                protocol::settings::RGB_STATE,
                                Duration::from_secs(1),
                            )
                            .unwrap();
                        assert_eq!(r[2], 0x5A, "an art reply reached the RGB read");
                        n += 1;
                    }
                    n
                })
            };
            vec![status_reads, rgb]
        }

        #[test]
        fn a_user_select_amid_app_traffic_succeeds_after_one_retry() {
            notes();
            let (link, selects, rgb_reads) = start(1);
            let stop = Arc::new(AtomicBool::new(false));
            let threads = background(&link, &stop);
            let st = select(&*link, 4);
            stop.store(true, Ordering::Relaxed);
            let counts: Vec<usize> = threads.into_iter().map(|t| t.join().unwrap()).collect();
            let st = st.unwrap();
            assert_eq!(st.state, left_state::DONE);
            assert_eq!(st.right_set, 4);
            assert_eq!(selects.load(Ordering::Relaxed), 2, "one SELECT, one retry");
            assert!(counts.iter().all(|&n| n > 0), "background ran: {counts:?}");
            assert!(rgb_reads.load(Ordering::Relaxed) > 0);
            let notes = notes();
            assert_eq!(notes.len(), 2, "{notes:?}");
            assert!(notes[0].contains("trying once more"));
        }

        #[test]
        fn a_user_select_amid_app_traffic_needs_no_retry_when_the_relay_is_fine() {
            notes();
            let (link, selects, _) = start(0);
            let stop = Arc::new(AtomicBool::new(false));
            let threads = background(&link, &stop);
            let st = select(&*link, 1);
            stop.store(true, Ordering::Relaxed);
            for t in threads {
                t.join().unwrap();
            }
            assert_eq!(st.unwrap().right_set, 1);
            assert_eq!(selects.load(Ordering::Relaxed), 1);
            assert!(notes().is_empty());
        }
    }

    #[test]
    fn recognises_blobs() {
        assert!(is_blob(b"NVA1\x8c\x44"));
        assert!(!is_blob(b"PNG"));
    }
}
