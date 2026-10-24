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
    let r = t.request(&protocol::art_slot(slot), protocol::ART, art::SLOT, Duration::from_secs(3))?;
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
