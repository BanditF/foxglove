//! The keyboard-first pass (docs/resilience.md, #25): art pushes cut short, corrupted or caught by a
//! split-link drop, and panel frames shown, flooded and released, as the PC side would do them when
//! the app dies or misbehaves.
use anyhow::{bail, ensure, Result};
use digitalis::{
    protocol::{self, left_state, ArtStatus},
    Transport,
};
use std::time::{Duration, Instant};

/// How a push goes wrong on purpose.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cut {
    /// The whole push.
    None,
    /// BEGIN and a few DATA, then nothing: the app died while sending over USB.
    Data,
    /// Everything up to COMMIT, then nothing: the app died while the left pushes to the right.
    Commit,
}

/// A deterministic test frame for the left panel: a checkerboard of `pattern + 2` pixel squares,
/// so frames with different patterns differ everywhere. `stride` bytes a row, MSB first, 1 = ink.
pub fn test_frame(width: usize, height: usize, pattern: u8) -> Vec<u8> {
    let (stride, square) = (width.div_ceil(8), usize::from(pattern) + 2);
    let mut frame = vec![0u8; stride * height];
    for y in 0..height {
        for x in 0..width {
            if (x / square + y / square) % 2 == 0 {
                frame[y * stride + x / 8] |= 0x80 >> (x % 8);
            }
        }
    }
    frame
}

/// A frame as a binary PBM (P4), the format the panel_dump step reads.
pub fn pbm(width: usize, height: usize, frame: &[u8]) -> Vec<u8> {
    let mut out = format!("P4\n{width} {height}\n").into_bytes();
    out.extend_from_slice(frame);
    out
}

/// A blob whose header the right half must reject (its width byte is wrong), with a CRC that
/// matches, so the left lets it through.
pub fn bad_header(blob: &[u8]) -> Vec<u8> {
    let mut b = blob.to_vec();
    if b.len() > 4 {
        b[4] = b[4].wrapping_add(1);
    }
    b
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

/// Sends `blob` as the PC would, stopping where `cut` says. `wrong_crc` announces a CRC that won't
/// match. `drop_link` drops the split link (D7 0B GUARD_TEST, dev builds) once the left is pushing
/// to the right. Returns what happened, for the report.
pub fn push(
    t: &impl Transport,
    blob: &[u8],
    wrong_crc: bool,
    cut: Cut,
    drop_link: bool,
) -> Result<String> {
    ensure!(blob.len() <= u16::MAX as usize, "blob too big");
    let crc = crc32fast::hash(blob) ^ u32::from(wrong_crc);
    let st = command(t, &protocol::art_begin(blob.len() as u16, crc))?;
    ensure!(
        st.state == left_state::RECEIVING,
        "the left half refused BEGIN: {st}"
    );
    let chunks: Vec<&[u8]> = blob.chunks(protocol::art::DATA_MAX).collect();
    let send = if cut == Cut::Data {
        5.min(chunks.len())
    } else {
        chunks.len()
    };
    for (i, chunk) in chunks.iter().take(send).enumerate() {
        t.send(&protocol::art_data(
            (i * protocol::art::DATA_MAX) as u16,
            chunk,
        ))?;
    }
    if cut == Cut::Data {
        return Ok(format!(
            "BEGIN and {send} of {} DATA, then nothing",
            chunks.len()
        ));
    }
    let st = command(t, &protocol::art_commit())?;
    if cut == Cut::Commit || !drop_link {
        return Ok(format!("all {} DATA and COMMIT (left: {st})", chunks.len()));
    }
    let end = Instant::now() + Duration::from_secs(3);
    let mut st = st;
    while st.state != left_state::PUSHING && Instant::now() < end {
        std::thread::sleep(Duration::from_millis(20));
        st = command(t, &protocol::art_status())?;
    }
    ensure!(
        st.state == left_state::PUSHING,
        "never saw the left pushing to the right: {st}"
    );
    let drops = || -> Result<u8> {
        let h = t.request(
            &protocol::hello(),
            protocol::PANEL,
            protocol::panel::HELLO,
            Duration::from_millis(500),
        )?;
        Ok(h[31])
    };
    let before = drops()?;
    let r = t.request(
        &[protocol::SETTINGS, 0x0B],
        protocol::SETTINGS,
        0x0B,
        Duration::from_secs(2),
    )?;
    match r[2] {
        0 => {
            std::thread::sleep(Duration::from_millis(300));
            let after = drops().map_or("no HELLO reply".into(), |d| d.to_string());
            Ok(format!(
                "split link dropped mid-relay ({} of {} bytes pushed; guard drops {before} -> {after})",
                st.pushed, st.length
            ))
        }
        4 => bail!("GUARD_TEST not built in (GA build)"),
        s => bail!("GUARD_TEST answered status {s}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_by_pattern_differ_and_fill_the_stride() {
        let (a, b) = (test_frame(64, 52, 0), test_frame(64, 52, 1));
        assert_eq!(a.len(), 8 * 52);
        assert_ne!(a, b);
        assert_eq!(a[0], 0b1100_1100); // 2-pixel squares, ink first
        assert!(pbm(64, 52, &a).starts_with(b"P4\n64 52\n"));
    }

    #[test]
    fn bad_header_changes_the_width_byte_only() {
        let blob = b"NVA1\x40\x44\x04\x00rest".to_vec();
        let bad = bad_header(&blob);
        assert_eq!(bad[4], 0x41);
        assert_eq!((&bad[..4], &bad[5..]), (&blob[..4], &blob[5..]));
    }
}
