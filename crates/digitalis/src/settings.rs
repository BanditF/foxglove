//! Keyboard settings that live on the keyboard (0xD7), so they hold without the app.

use std::time::Duration;

use anyhow::Result;

use crate::protocol::{self, RgbSync};
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
