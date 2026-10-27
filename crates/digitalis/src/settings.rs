//! Keyboard settings that live on the keyboard (0xD7), so they hold without the app.

use std::time::Duration;

use anyhow::Result;

use crate::protocol::{self, BatteryDim, BatteryDimConfig, InvokeStatus, RgbState, RgbSync};
use crate::Transport;

/// Reads "right half's underglow follows the left" (`set` = `None`) or sets it.
pub fn rgb_sync(t: &impl Transport, set: Option<bool>) -> Result<RgbSync> {
    let r = t.request(
        &protocol::rgb_sync(set),
        protocol::SETTINGS,
        protocol::settings::RGB_SYNC,
        Duration::from_millis(500),
    )?;
    Ok(RgbSync::parse(&r))
}

/// Reads the underglow's levels by power state and both halves' state (`set` = `None`), or sets
/// the levels: the left saves them and passes them on to the right.
pub fn battery_dim(t: &impl Transport, set: Option<BatteryDimConfig>) -> Result<BatteryDim> {
    let r = t.request(
        &protocol::battery_dim(set),
        protocol::SETTINGS,
        protocol::settings::BATTERY_DIM,
        Duration::from_millis(500),
    )?;
    Ok(BatteryDim::parse(&r))
}

/// Presses behavior `local_id` (ZMK Studio's id) with these parameters as a key would,
/// releasing it after `hold_ms` (0 = a tap).
pub fn invoke(
    t: &impl Transport,
    local_id: u16,
    param1: u32,
    param2: u32,
    hold_ms: u16,
) -> Result<InvokeStatus> {
    let wait =
        Duration::from_millis(1500 + u64::from(hold_ms.min(protocol::settings::MAX_HOLD_MS)));
    let r = t.request(
        &protocol::invoke(local_id, param1, param2, hold_ms),
        protocol::SETTINGS,
        protocol::settings::INVOKE,
        wait,
    )?;
    Ok(InvokeStatus::parse(&r))
}

/// Both halves' underglow. The right half's comes from its last relayed status, and each call
/// asks for a fresh one, so calling about once a second keeps it current.
pub fn rgb_state(t: &impl Transport) -> Result<RgbState> {
    let r = t.request(
        &protocol::rgb_state(),
        protocol::SETTINGS,
        protocol::settings::RGB_STATE,
        Duration::from_millis(500),
    )?;
    Ok(RgbState::parse(&r))
}

/// Reboots `half` (0 left, 1 right; the right goes over the relay) into its UF2 bootloader:
/// ARM, then TRIGGER with the nonce ARM returned. Only for the flasher, after the user has
/// confirmed the flash; firmware without HELLO capability bit 9 answers "not built in".
pub fn enter_bootloader(t: &impl Transport, half: u8) -> Result<()> {
    anyhow::ensure!(half <= 1, "half must be 0 (left) or 1 (right)");
    let status = |r: &[u8; protocol::REPORT]| match r[2] {
        0 => Ok(()),
        2 => anyhow::bail!("no relay link to the right half"),
        3 => anyhow::bail!("the right half did not answer the relay in time"),
        4 => anyhow::bail!("bootloader command not built into this firmware"),
        5 => anyhow::bail!("bootloader command rejected as malformed"),
        6 => anyhow::bail!("bootloader nonce wrong or expired"),
        s => anyhow::bail!("bootloader command failed with status {s}"),
    };
    let wait = Duration::from_millis(1500);
    let armed = t.request(
        &protocol::bootloader_arm(),
        protocol::SETTINGS,
        protocol::settings::BOOTLOADER,
        wait,
    )?;
    status(&armed)?;
    let nonce = u32::from_le_bytes([armed[3], armed[4], armed[5], armed[6]]);
    // The right needs the relay round trip; the left may reboot before its reply goes out.
    let wait = Duration::from_secs(4);
    match t.request(
        &protocol::bootloader_trigger(half, nonce),
        protocol::SETTINGS,
        protocol::settings::BOOTLOADER,
        wait,
    ) {
        Ok(r) => status(&r),
        Err(_) if half == 0 => Ok(()), // the caller confirms by the bootloader drive appearing
        Err(e) => Err(e),
    }
}

/// SET_FRAME (#272): colours for lamps `start..` (0-20 left, 21-41 right), drawn at once on the
/// left and within 100 ms on the right. Chunks go without a reply; the last one shows the frame
/// and waits for the keyboard's answer, so a caller that sends its next frame after this returns
/// keeps one frame in flight. Lamps not sent keep their colour. The keyboard's hand-back event
/// ([`protocol::is_frame_event`]) reaches [`crate::link::Link::subscribe_reports`] observers.
pub fn set_frame(
    t: &impl Transport,
    start: u8,
    colours: &[[u8; 3]],
) -> Result<protocol::FrameReply> {
    anyhow::ensure!(
        usize::from(start) + colours.len() <= 2 * protocol::frame::HALF_LAMPS,
        "lamps {start}..{} are past the keyboard's 42",
        usize::from(start) + colours.len()
    );
    let reports = protocol::frame(start, colours);
    let (last, chunks) = reports
        .split_last()
        .ok_or_else(|| anyhow::anyhow!("a frame needs at least one lamp"))?;
    for r in chunks {
        t.send(r)?;
    }
    let r = t.request(
        last,
        protocol::SETTINGS,
        protocol::settings::FRAME,
        Duration::from_millis(500),
    )?;
    Ok(protocol::FrameReply::parse(&r))
}

/// Hands both halves back to their own effect (SET_FRAME RELEASE).
pub fn release_frame(t: &impl Transport) -> Result<protocol::FrameReply> {
    let r = t.request(
        &protocol::frame_release(),
        protocol::SETTINGS,
        protocol::settings::FRAME,
        Duration::from_millis(500),
    )?;
    Ok(protocol::FrameReply::parse(&r))
}

/// Reads the active Bluetooth profile and selected typing endpoint.
pub fn conn_state(t: &impl Transport) -> Result<protocol::ConnState> {
    let r = t.request(
        &protocol::conn_state(),
        protocol::SETTINGS,
        protocol::settings::CONN_STATE,
        Duration::from_millis(500),
    )?;
    Ok(protocol::ConnState::parse(&r))
}

/// How long the right half's build may take. It is read over the Bluetooth relay, which queues
/// it behind whatever the relay is already doing (art slot reads take up to 3 s each), so the
/// 4 s that suits the left timed out in the app while the lab, on an idle relay, got through.
const RIGHT_BUILD_WAIT: Duration = Duration::from_secs(10);
/// Replies 1 (busy) and 3 (relay error) come at once and usually clear; ask again this many times.
const BUILD_RETRIES: u32 = 2;

/// One half's firmware build (0 = left, 1 = right). The right's comes over the relay, so this
/// can take several seconds.
pub fn build_info(t: &impl Transport, half: u8) -> Result<protocol::BuildInfo> {
    build_info_after(t, half, Duration::from_millis(750))
}

fn build_info_after(
    t: &impl Transport,
    half: u8,
    retry_wait: Duration,
) -> Result<protocol::BuildInfo> {
    let wait = if half == 0 {
        Duration::from_secs(4)
    } else {
        RIGHT_BUILD_WAIT
    };
    let mut attempt = 0;
    loop {
        let r = t.request(
            &protocol::build_info(half),
            protocol::SETTINGS,
            protocol::settings::BUILD_INFO,
            wait,
        )?;
        match protocol::BuildInfo::parse(&r) {
            Ok(b) => return Ok(b),
            Err(1 | 3) if attempt < BUILD_RETRIES => {
                attempt += 1;
                std::thread::sleep(retry_wait); // the relay is busy: a bounded back-off, not a poll
            }
            Err(status) => {
                anyhow::bail!(match status {
                    1 => "busy".to_string(),
                    2 => "the right half is not connected".to_string(),
                    3 => "relay error".to_string(),
                    4 => "the right half's firmware doesn't report its build".to_string(),
                    n => format!("error {n}"),
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Connection;
    impl Transport for Connection {
        fn send(&self, _: &[u8]) -> Result<()> {
            panic!("connection state uses request")
        }
        fn request(
            &self,
            report: &[u8],
            proto: u8,
            cmd: u8,
            timeout: Duration,
        ) -> Result<[u8; protocol::REPORT]> {
            assert_eq!(report, [0xd7, 4]);
            assert_eq!((proto, cmd), (0xd7, 4));
            assert_eq!(timeout, Duration::from_millis(500));
            let mut r = [0; protocol::REPORT];
            r[..8].copy_from_slice(&[0xd7, 0x84, 2, 1, 1, 0, 1, 4]);
            Ok(r)
        }
    }

    /// Records FRAME reports; answers the one that asks with an ok reply.
    #[derive(Default)]
    struct Frames(std::cell::RefCell<Vec<(Vec<u8>, bool)>>);
    impl Transport for Frames {
        fn send(&self, report: &[u8]) -> Result<()> {
            self.0.borrow_mut().push((report.to_vec(), false));
            Ok(())
        }
        fn request(
            &self,
            report: &[u8],
            proto: u8,
            cmd: u8,
            _: Duration,
        ) -> Result<[u8; protocol::REPORT]> {
            assert_eq!((proto, cmd), (0xd7, 0x0c));
            self.0.borrow_mut().push((report.to_vec(), true));
            let owner = if report[2] & protocol::frame::RELEASE != 0 {
                0
            } else {
                3
            };
            let mut r = [0; protocol::REPORT];
            r[..6].copy_from_slice(&[0xd7, 0x8c, 0, owner, 30, 1]);
            Ok(r)
        }
    }

    #[test]
    fn a_frame_sends_its_chunks_and_waits_for_the_last() {
        let t = Frames::default();
        assert!(set_frame(&t, 0, &[[255, 128, 0]; 42]).unwrap().held());
        let asked: Vec<(u8, bool)> = t.0.borrow().iter().map(|(r, a)| (r[3], *a)).collect();
        assert_eq!(
            asked,
            [(0, false), (9, false), (18, false), (27, false), (36, true)]
        );
        assert!(set_frame(&t, 0, &[]).is_err());
        assert!(set_frame(&t, 40, &[[1, 2, 3]; 3]).is_err(), "past lamp 41");
        assert_eq!(t.0.borrow().len(), 5, "nothing sent for a bad frame");
        assert!(!release_frame(&t).unwrap().held());
    }

    #[test]
    fn connection_query_uses_settings_transport() {
        let state = conn_state(&Connection).unwrap();
        assert_eq!(state.active_profile, 2);
        assert!(state.connected && state.bonded && state.usb_ready);
        assert_eq!((state.endpoint, state.profile_count), (0, 4));
    }

    /// Scripted BOOTLOADER replies; records requests.
    struct Boot(
        std::cell::RefCell<Vec<Vec<u8>>>,
        Vec<[u8; protocol::REPORT]>,
    );
    impl Transport for Boot {
        fn send(&self, _: &[u8]) -> Result<()> {
            panic!("bootloader uses request")
        }
        fn request(
            &self,
            report: &[u8],
            proto: u8,
            cmd: u8,
            _: Duration,
        ) -> Result<[u8; protocol::REPORT]> {
            assert_eq!((proto, cmd), (0xd7, 8));
            let mut sent = self.0.borrow_mut();
            sent.push(report.to_vec());
            self.1
                .get(sent.len() - 1)
                .copied()
                .ok_or_else(|| anyhow::anyhow!("timeout"))
        }
    }
    fn boot_reply(bytes: &[u8]) -> [u8; protocol::REPORT] {
        let mut r = [0; protocol::REPORT];
        r[..bytes.len()].copy_from_slice(bytes);
        r
    }

    #[test]
    fn bootloader_arms_then_triggers_with_the_nonce() {
        let t = Boot(
            Default::default(),
            vec![
                boot_reply(&[0xd7, 0x88, 0, 0x78, 0x56, 0x34, 0x12]),
                boot_reply(&[0xd7, 0x88, 0, 1]),
            ],
        );
        enter_bootloader(&t, 1).unwrap();
        assert_eq!(
            *t.0.borrow(),
            [
                vec![0xd7, 8, 0],
                vec![0xd7, 8, 1, 1, 0x78, 0x56, 0x34, 0x12]
            ]
        );
    }

    #[test]
    fn bootloader_errors_and_left_reboot_before_reply() {
        let unsupported = Boot(Default::default(), vec![boot_reply(&[0xd7, 0x88, 4])]);
        assert!(enter_bootloader(&unsupported, 0)
            .unwrap_err()
            .to_string()
            .contains("not built"));
        assert_eq!(
            unsupported.0.borrow().len(),
            1,
            "no TRIGGER without a nonce"
        );
        // The left can reboot before replying to TRIGGER: fine, the drive appearing confirms it.
        let left = Boot(
            Default::default(),
            vec![boot_reply(&[0xd7, 0x88, 0, 1, 2, 3, 4])],
        );
        enter_bootloader(&left, 0).unwrap();
        let right = Boot(
            Default::default(),
            vec![boot_reply(&[0xd7, 0x88, 0, 1, 2, 3, 4])],
        );
        assert!(enter_bootloader(&right, 1).is_err());
        assert!(enter_bootloader(&right, 2).is_err());
    }

    /// Scripted BUILD_INFO replies; records the timeout of each request.
    struct Build(
        std::cell::RefCell<Vec<Duration>>,
        Vec<[u8; protocol::REPORT]>,
    );
    impl Transport for Build {
        fn send(&self, _: &[u8]) -> Result<()> {
            panic!("build info uses request")
        }
        fn request(
            &self,
            _: &[u8],
            proto: u8,
            cmd: u8,
            timeout: Duration,
        ) -> Result<[u8; protocol::REPORT]> {
            assert_eq!((proto, cmd), (0xd7, 9));
            let mut seen = self.0.borrow_mut();
            seen.push(timeout);
            self.1
                .get(seen.len() - 1)
                .copied()
                .ok_or_else(|| anyhow::anyhow!("no reply from the keyboard"))
        }
    }

    #[test]
    fn right_build_waits_longer_than_the_left_and_retries_a_busy_relay() {
        let ok = boot_reply(&[0xd7, 0x89, 0, 1, 0, 1, 0, 0, b'a', b'b', b'c']);
        let busy = boot_reply(&[0xd7, 0x89, 1, 1]);
        let t = Build(
            Default::default(),
            vec![busy, boot_reply(&[0xd7, 0x89, 3, 1]), ok],
        );
        let b = build_info_after(&t, 1, Duration::ZERO).unwrap();
        assert_eq!((b.version.minor, b.sha.as_str()), (1, "abc"));
        assert_eq!(*t.0.borrow(), [RIGHT_BUILD_WAIT; 3]);

        let left = Build(Default::default(), vec![ok]);
        build_info_after(&left, 0, Duration::ZERO).unwrap();
        assert_eq!(*left.0.borrow(), [Duration::from_secs(4)]);
    }

    #[test]
    fn build_errors_give_up_after_the_retries_or_at_once() {
        let busy = boot_reply(&[0xd7, 0x89, 1, 1]);
        let t = Build(Default::default(), vec![busy; 5]);
        assert_eq!(
            build_info_after(&t, 1, Duration::ZERO)
                .unwrap_err()
                .to_string(),
            "busy"
        );
        assert_eq!(t.0.borrow().len(), 3, "one try and two retries");
        let gone = Build(Default::default(), vec![boot_reply(&[0xd7, 0x89, 2, 1])]);
        assert!(build_info_after(&gone, 1, Duration::ZERO)
            .unwrap_err()
            .to_string()
            .contains("not connected"));
        assert_eq!(gone.0.borrow().len(), 1, "not connected is final");
    }
}
