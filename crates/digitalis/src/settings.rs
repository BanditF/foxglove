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
}
