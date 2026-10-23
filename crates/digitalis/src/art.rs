//! Right-hand art: push a set through the left half to the right one, or pick a built-in set.
//! Blob format: art_relay.h ("NVA1" header), written by scripts/flipper_art.py.

use std::time::{Duration, Instant};

use anyhow::{bail, ensure, Result};

use crate::protocol::{self, art, left_state, ArtStatus};
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

/// Shows built-in set `set` again (-1 = random).
pub fn select(t: &impl Transport, set: i8) -> Result<ArtStatus> {
    let mut st = command(t, &protocol::art_select(set))?;
    let end = Instant::now() + Duration::from_secs(10);
    while st.state == left_state::PUSHING && Instant::now() < end {
        std::thread::sleep(Duration::from_millis(100));
        st = status(t)?;
    }
    match st.state {
        left_state::FAILED => bail!("select failed: {st}"),
        left_state::PUSHING => bail!("select did not finish in 10 s: {st}"),
        _ => Ok(st),
    }
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

        t.replies.borrow_mut().push_back(reply(art::BEGIN, left_state::RECEIVING));
        let checks = Cell::new(0);
        let result = push_cancellable(&t, b"NVA1", |_| {}, || {
            checks.set(checks.get() + 1);
            checks.get() == 3
        });
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
        assert!(push_cancellable(&t, b"NVA1", |_| cancelled.set(true), || cancelled.get()).is_err());
        assert_eq!(t.sent.borrow().last().unwrap()[1], art::COMMIT);
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

    #[test]
    fn recognises_blobs() {
        assert!(is_blob(b"NVA1\x8c\x44"));
        assert!(!is_blob(b"PNG"));
    }
}
