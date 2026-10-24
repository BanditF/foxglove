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
