//! Keyboard settings that live on the keyboard (0xD7), so they hold without the app.

use std::time::Duration;

use anyhow::Result;

use crate::protocol::{self, InvokeStatus, RgbState, RgbSync};
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

/// Presses behavior `local_id` (ZMK Studio's id) with these parameters as a key would,
/// releasing it after `hold_ms` (0 = a tap).
pub fn invoke(
    t: &impl Transport,
    local_id: u16,
    param1: u32,
    param2: u32,
    hold_ms: u16,
) -> Result<InvokeStatus> {
    let wait = Duration::from_millis(1500 + u64::from(hold_ms.min(protocol::settings::MAX_HOLD_MS)));
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
    let armed = t.request(&protocol::bootloader_arm(), protocol::SETTINGS, protocol::settings::BOOTLOADER, wait)?;
    status(&armed)?;
    let nonce = u32::from_le_bytes([armed[3], armed[4], armed[5], armed[6]]);
    // The right needs the relay round trip; the left may reboot before its reply goes out.
    let wait = Duration::from_secs(4);
    match t.request(&protocol::bootloader_trigger(half, nonce), protocol::SETTINGS, protocol::settings::BOOTLOADER, wait) {
        Ok(r) => status(&r),
        Err(_) if half == 0 => Ok(()), // the caller confirms by the bootloader drive appearing
        Err(e) => Err(e),
    }
}

/// Reads the active Bluetooth profile and selected typing endpoint.
pub fn conn_state(t: &impl Transport) -> Result<protocol::ConnState> {
    let r = t.request(
        &protocol::conn_state(), protocol::SETTINGS, protocol::settings::CONN_STATE,
        Duration::from_millis(500),
    )?;
    Ok(protocol::ConnState::parse(&r))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Connection;
    impl Transport for Connection {
        fn send(&self, _: &[u8]) -> Result<()> { panic!("connection state uses request") }
        fn request(&self, report: &[u8], proto: u8, cmd: u8, timeout: Duration)
            -> Result<[u8; protocol::REPORT]> {
            assert_eq!(report, [0xd7, 4]);
            assert_eq!((proto, cmd), (0xd7, 4));
            assert_eq!(timeout, Duration::from_millis(500));
            let mut r = [0; protocol::REPORT];
            r[..8].copy_from_slice(&[0xd7, 0x84, 2, 1, 1, 0, 1, 4]);
            Ok(r)
        }
    }

    #[test]
    fn connection_query_uses_settings_transport() {
        let state = conn_state(&Connection).unwrap();
        assert_eq!(state.active_profile, 2);
        assert!(state.connected && state.bonded && state.usb_ready);
        assert_eq!((state.endpoint, state.profile_count), (0, 4));
    }

    /// Scripted BOOTLOADER replies; records requests.
    struct Boot(std::cell::RefCell<Vec<Vec<u8>>>, Vec<[u8; protocol::REPORT]>);
    impl Transport for Boot {
        fn send(&self, _: &[u8]) -> Result<()> { panic!("bootloader uses request") }
        fn request(&self, report: &[u8], proto: u8, cmd: u8, _: Duration)
            -> Result<[u8; protocol::REPORT]> {
            assert_eq!((proto, cmd), (0xd7, 8));
            let mut sent = self.0.borrow_mut();
            sent.push(report.to_vec());
            self.1.get(sent.len() - 1).copied().ok_or_else(|| anyhow::anyhow!("timeout"))
        }
    }
    fn boot_reply(bytes: &[u8]) -> [u8; protocol::REPORT] {
        let mut r = [0; protocol::REPORT];
        r[..bytes.len()].copy_from_slice(bytes);
        r
    }

    #[test]
    fn bootloader_arms_then_triggers_with_the_nonce() {
        let t = Boot(Default::default(), vec![
            boot_reply(&[0xd7, 0x88, 0, 0x78, 0x56, 0x34, 0x12]),
            boot_reply(&[0xd7, 0x88, 0, 1]),
        ]);
        enter_bootloader(&t, 1).unwrap();
        assert_eq!(*t.0.borrow(), [vec![0xd7, 8, 0], vec![0xd7, 8, 1, 1, 0x78, 0x56, 0x34, 0x12]]);
    }

    #[test]
    fn bootloader_errors_and_left_reboot_before_reply() {
        let unsupported = Boot(Default::default(), vec![boot_reply(&[0xd7, 0x88, 4])]);
        assert!(enter_bootloader(&unsupported, 0).unwrap_err().to_string().contains("not built"));
        assert_eq!(unsupported.0.borrow().len(), 1, "no TRIGGER without a nonce");
        // The left can reboot before replying to TRIGGER: fine, the drive appearing confirms it.
        let left = Boot(Default::default(), vec![boot_reply(&[0xd7, 0x88, 0, 1, 2, 3, 4])]);
        enter_bootloader(&left, 0).unwrap();
        let right = Boot(Default::default(), vec![boot_reply(&[0xd7, 0x88, 0, 1, 2, 3, 4])]);
        assert!(enter_bootloader(&right, 1).is_err());
        assert!(enter_bootloader(&right, 2).is_err());
    }
}
