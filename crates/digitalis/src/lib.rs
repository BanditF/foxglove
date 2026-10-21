//! Digitalis: Foxglove's link to the keyboard.
//!
//! The protocol is the firmware's host link (boards/shields/nice_view_custom/widgets/host_link.h):
//! 32-byte Raw HID reports, byte 0 picks the protocol (0xD5 host panel, 0xD6 art relay), byte 1 the
//! command, and replies come back with byte 1 = command | 0x80.
//!
//! - [`protocol`]: report builders and reply parsers, no I/O
//! - [`Transport`]: what the senders need from a connection; [`hid`] has the hidapi one
//! - [`panel`]: the 1-bit panel image and sending it
//! - [`art`]: pushing art sets to the right half and picking built-in ones

pub mod art;
#[cfg(feature = "hid")]
pub mod hid;
pub mod panel;
pub mod protocol;

use std::time::Duration;

use anyhow::Result;

/// A connection to the keyboard's Raw HID interface.
pub trait Transport {
    /// Sends one report (at most [`protocol::REPORT`] bytes; the rest is zero).
    fn send(&self, report: &[u8]) -> Result<()>;

    /// Sends `report` and waits for the reply to (`proto`, `cmd`), skipping other reports.
    fn request(&self, report: &[u8], proto: u8, cmd: u8, timeout: Duration) -> Result<[u8; protocol::REPORT]>;
}
