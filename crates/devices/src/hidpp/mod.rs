//! HID++ 2.0, the feature protocol Logitech mice, keyboards, headsets and receivers speak (#505).
//! A clean-room, read-mostly client: framing, the root feature lookup, the feature set, error
//! replies, and [`battery`] reads into the generic [`crate::battery::Battery`]. Written from the
//! protocol's public descriptions; the facts were cross-checked against libratbag (MIT) and
//! read in Solaar and Linux's hid-logitech-hidpp (GPL-2.0, learned from, nothing copied).
//!
//! It talks through a [`Transport`], so the tests run on a fake with no device. Only [`hid`]
//! (the `hid` feature, hidapi) opens one, for the app; [`probe`] finds the HID++ interfaces in an
//! enumeration and reads the batteries behind each. The public API only reads: every request it sends is a "get" function
//! (root GetFeature and GetProtocolVersion, feature-set GetCount and GetFeatureId, battery
//! status). Lighting and onboard-profile writes aren't part of it (#505's plan).
//!
//! # Frames
//! Every report is `[report id, device index, feature index, function << 4 | software id,
//! params...]`: short (report `0x10`, 7 bytes, 3 params) or long (`0x11`, 20 bytes, 16 params).
//! The device index is `0xFF` for a device on its own cable or Bluetooth link, and 1-6 for one
//! paired to a Unifying, Bolt or Lightspeed receiver. The software id (a nibble, never 0: 0
//! marks the device's own notifications) tells our replies from other apps' on the same
//! device, since every open handle sees every input report.
//!
//! # Errors
//! A failed request gets `[id, device, 0xFF, feature index, function|sw, code, ...]` (HID++ 2.0)
//! or `[id, device, 0x8F, sub id, address, code, ...]` (HID++ 1.0, also what a receiver or an
//! old device says to a 2.0 request): see [`Error20`] and [`Error10`].

pub mod battery;
#[cfg(feature = "hid")]
pub mod hid;
pub mod probe;

use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::DeviceError;

pub use battery::{BATTERY_STATUS, BATTERY_VOLTAGE, UNIFIED_BATTERY};

/// Short report: 7 bytes, 3 parameter bytes.
pub const REPORT_SHORT: u8 = 0x10;
/// Long report: 20 bytes, 16 parameter bytes.
pub const REPORT_LONG: u8 = 0x11;
pub const SHORT_LEN: usize = 7;
pub const LONG_LEN: usize = 20;
/// A device on its own cable or Bluetooth link (receivers number theirs 1-6).
pub const DEVICE_DIRECT: u8 = 0xFF;
/// The software id Foxglove sends: a nibble no other known HID++ client uses (Linux 0x1,
/// OpenRGB 0x7, libratbag 0x8, LGSTrayEx 0xA, Solaar 0xB, G HUB 0xD, device firmware 0xF), so
/// replies to other apps sharing the device are told apart from ours.
pub const SW_ID: u8 = 0x0C;
/// Error reply marker, HID++ 1.0 (in the sub-id byte).
pub const ERROR_10: u8 = 0x8F;
/// Error reply marker, HID++ 2.0 (in the feature-index byte).
pub const ERROR_20: u8 = 0xFF;

/// Root: always at index 0. GetFeature (function 0), GetProtocolVersion (function 1).
pub const ROOT: u16 = 0x0000;
/// Feature set: GetCount (function 0), GetFeatureId (function 1).
pub const FEATURE_SET: u16 = 0x0001;

/// The longest a request waits for its reply.
pub const REPLY_TIMEOUT: Duration = Duration::from_secs(2);
/// Other reports (other apps' replies, notifications) skipped while waiting, at most.
pub const MAX_SKIPPED: usize = 64;

/// Moves whole reports to and from one HID++ device. `read` blocks for at most `timeout` and
/// returns `None` when nothing came; it never spins.
pub trait Transport {
    fn write(&mut self, report: &[u8]) -> crate::Result<()>;
    fn read(&mut self, timeout: Duration) -> crate::Result<Option<Vec<u8>>>;
}

impl<T: Transport + ?Sized> Transport for &mut T {
    fn write(&mut self, report: &[u8]) -> crate::Result<()> {
        (**self).write(report)
    }

    fn read(&mut self, timeout: Duration) -> crate::Result<Option<Vec<u8>>> {
        (**self).read(timeout)
    }
}

/// HID++ 2.0 error codes, from a `0xFF` reply.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error20 {
    Unknown,
    InvalidArgument,
    OutOfRange,
    HardwareError,
    /// "Logitech internal" in older documents, "not allowed" in newer ones.
    NotAllowed,
    InvalidFeatureIndex,
    InvalidFunction,
    Busy,
    Unsupported,
    Other(u8),
}

impl Error20 {
    pub fn from_code(code: u8) -> Self {
        match code {
            0x01 => Self::Unknown,
            0x02 => Self::InvalidArgument,
            0x03 => Self::OutOfRange,
            0x04 => Self::HardwareError,
            0x05 => Self::NotAllowed,
            0x06 => Self::InvalidFeatureIndex,
            0x07 => Self::InvalidFunction,
            0x08 => Self::Busy,
            0x09 => Self::Unsupported,
            other => Self::Other(other),
        }
    }
}

/// HID++ 1.0 error codes, from a `0x8F` reply.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error10 {
    /// The device speaks only HID++ 1.0 (its answer to GetProtocolVersion).
    InvalidSubId,
    InvalidAddress,
    InvalidValue,
    ConnectFail,
    TooManyDevices,
    AlreadyExists,
    Busy,
    /// Nothing paired at that receiver slot.
    UnknownDevice,
    /// The paired device is off or asleep.
    ResourceError,
    RequestUnavailable,
    InvalidParamValue,
    WrongPinCode,
    Other(u8),
}

impl Error10 {
    pub fn from_code(code: u8) -> Self {
        match code {
            0x01 => Self::InvalidSubId,
            0x02 => Self::InvalidAddress,
            0x03 => Self::InvalidValue,
            0x04 => Self::ConnectFail,
            0x05 => Self::TooManyDevices,
            0x06 => Self::AlreadyExists,
            0x07 => Self::Busy,
            0x08 => Self::UnknownDevice,
            0x09 => Self::ResourceError,
            0x0A => Self::RequestUnavailable,
            0x0B => Self::InvalidParamValue,
            0x0C => Self::WrongPinCode,
            other => Self::Other(other),
        }
    }
}

/// A failed HID++ request.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum HidppError {
    #[error("HID++ 2.0 error {0:?}")]
    Feature(Error20),
    #[error("HID++ 1.0 error {0:?}")]
    Protocol10(Error10),
    #[error("feature {0:#06x} not on this device")]
    NoFeature(u16),
    #[error("no reply in time")]
    Timeout,
    #[error("bad reply: {0}")]
    Malformed(&'static str),
    #[error(transparent)]
    Transport(#[from] DeviceError),
}

impl From<HidppError> for DeviceError {
    fn from(e: HidppError) -> Self {
        match e {
            HidppError::Transport(e) => e,
            HidppError::Timeout => DeviceError::Timeout,
            HidppError::NoFeature(_)
            | HidppError::Feature(Error20::Unsupported)
            | HidppError::Protocol10(Error10::InvalidSubId) => DeviceError::Unsupported,
            HidppError::Feature(Error20::Busy) | HidppError::Protocol10(Error10::Busy) => {
                DeviceError::Busy("HID++ device busy".into())
            }
            HidppError::Protocol10(Error10::ResourceError | Error10::UnknownDevice) => {
                DeviceError::Disconnected
            }
            other => DeviceError::Backend(other.to_string()),
        }
    }
}

pub type Result<T> = std::result::Result<T, HidppError>;

/// Which report lengths the device's HID interface takes. Bluetooth links and some mice take
/// only long ones.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reports {
    ShortAndLong,
    LongOnly,
}

/// Builds one request report: short when the parameters fit and the device takes short ones.
pub fn encode(
    reports: Reports,
    device: u8,
    feature: u8,
    function: u8,
    sw_id: u8,
    params: &[u8],
) -> Vec<u8> {
    let short = params.len() <= SHORT_LEN - 4 && reports == Reports::ShortAndLong;
    let (id, len) = if short {
        (REPORT_SHORT, SHORT_LEN)
    } else {
        (REPORT_LONG, LONG_LEN)
    };
    let mut report = vec![0; len];
    report[0] = id;
    report[1] = device;
    report[2] = feature;
    report[3] = (function << 4) | (sw_id & 0x0F);
    let n = params.len().min(len - 4);
    report[4..4 + n].copy_from_slice(&params[..n]);
    report
}

/// What one input report means for a request sent as `(device, feature, function|sw)`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Answer {
    /// Its reply: the parameter bytes (3 or 16).
    Reply(Vec<u8>),
    Failed(HidppError),
    /// Something else: another app's reply, a notification, another report id.
    Other,
}

/// Matches an input report against the request it may answer.
pub fn answer(report: &[u8], device: u8, feature: u8, fn_sw: u8) -> Answer {
    let len = match report.first() {
        Some(&REPORT_SHORT) => SHORT_LEN,
        Some(&REPORT_LONG) => LONG_LEN,
        _ => return Answer::Other,
    };
    if report.len() < len || report[1] != device {
        return Answer::Other;
    }
    let (sub, addr) = (report[2], report[3]);
    if (sub == ERROR_20 || sub == ERROR_10) && addr == feature && report[4] == fn_sw {
        let code = report[5];
        return Answer::Failed(if sub == ERROR_20 {
            HidppError::Feature(Error20::from_code(code))
        } else {
            HidppError::Protocol10(Error10::from_code(code))
        });
    }
    if sub == feature && addr == fn_sw {
        return Answer::Reply(report[4..len].to_vec());
    }
    Answer::Other
}

/// A feature's place on one device, from root GetFeature.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FeatureInfo {
    pub index: u8,
    /// Type flags: bit 7 obsolete, 6 hidden, 5 engineering-only.
    pub flags: u8,
    pub version: u8,
}

/// One HID++ 2.0 device (behind a receiver slot, or direct).
pub struct Hidpp<T: Transport> {
    transport: T,
    device: u8,
    reports: Reports,
    sw_id: u8,
    timeout: Duration,
    features: HashMap<u16, Option<FeatureInfo>>,
}

impl<T: Transport> Hidpp<T> {
    /// `device`: [`DEVICE_DIRECT`], or the receiver slot (1-6).
    pub fn new(transport: T, device: u8) -> Self {
        Self {
            transport,
            device,
            reports: Reports::ShortAndLong,
            sw_id: SW_ID,
            timeout: REPLY_TIMEOUT,
            features: HashMap::new(),
        }
    }

    /// For an interface that takes only long reports (Bluetooth, some mice).
    pub fn long_only(mut self) -> Self {
        self.reports = Reports::LongOnly;
        self
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn device_index(&self) -> u8 {
        self.device
    }

    pub fn into_transport(self) -> T {
        self.transport
    }

    /// Sends one request and waits, bounded, for its reply's parameters. Kept in the crate so
    /// the public API stays read-only.
    pub(crate) fn request(&mut self, feature: u8, function: u8, params: &[u8]) -> Result<Vec<u8>> {
        let report = encode(
            self.reports,
            self.device,
            feature,
            function,
            self.sw_id,
            params,
        );
        let fn_sw = report[3];
        self.transport.write(&report)?;
        let deadline = Instant::now() + self.timeout;
        for _ in 0..=MAX_SKIPPED {
            let left = deadline.saturating_duration_since(Instant::now());
            let Some(input) = self.transport.read(left)? else {
                return Err(HidppError::Timeout);
            };
            match answer(&input, self.device, feature, fn_sw) {
                Answer::Reply(params) => return Ok(params),
                Answer::Failed(e) => return Err(e),
                Answer::Other if left.is_zero() => return Err(HidppError::Timeout),
                Answer::Other => {}
            }
        }
        Err(HidppError::Timeout)
    }

    /// Root GetProtocolVersion: `(major, minor)`, `(1, 0)` for a HID++ 1.0 device.
    pub fn protocol_version(&mut self) -> Result<(u8, u8)> {
        const PING: u8 = 0x5A;
        match self.request(0, 1, &[0, 0, PING]) {
            Ok(p) if p[2] == PING => Ok((p[0], p[1])),
            Ok(_) => Err(HidppError::Malformed("ping byte not echoed")),
            Err(HidppError::Protocol10(Error10::InvalidSubId)) => Ok((1, 0)),
            Err(e) => Err(e),
        }
    }

    /// Root GetFeature: where `id` sits on this device, `None` when it has no such feature.
    /// Remembered, so each feature is asked once per device.
    pub fn feature(&mut self, id: u16) -> Result<Option<FeatureInfo>> {
        if id == ROOT {
            return Ok(Some(FeatureInfo {
                index: 0,
                flags: 0,
                version: 0,
            }));
        }
        if let Some(known) = self.features.get(&id) {
            return Ok(*known);
        }
        let p = self.request(0, 0, &id.to_be_bytes())?;
        let info = (p[0] != 0).then_some(FeatureInfo {
            index: p[0],
            flags: p[1],
            version: p[2],
        });
        self.features.insert(id, info);
        Ok(info)
    }

    /// The feature's index, or [`HidppError::NoFeature`].
    pub fn index(&mut self, id: u16) -> Result<u8> {
        self.feature(id)?
            .map(|f| f.index)
            .ok_or(HidppError::NoFeature(id))
    }

    /// Every feature, from the feature set (0x0001): `(index, id, flags)`, root first.
    pub fn features(&mut self) -> Result<Vec<(u8, u16, u8)>> {
        let set = self.index(FEATURE_SET)?;
        let count = self.request(set, 0, &[])?[0];
        let mut all = Vec::with_capacity(usize::from(count) + 1);
        for index in 0..=count {
            let p = self.request(set, 1, &[index])?;
            let id = u16::from_be_bytes([p[0], p[1]]);
            all.push((index, id, p[2]));
            if index != 0 {
                // Newer devices add the version in a fourth byte (long replies only).
                self.features.entry(id).or_insert(Some(FeatureInfo {
                    index,
                    flags: p[2],
                    version: p.get(3).copied().unwrap_or(0),
                }));
            }
        }
        Ok(all)
    }
}

/// A scripted HID++ device, for tests here and in the app (the `mock` feature).
#[cfg(any(test, feature = "mock"))]
#[doc(hidden)]
pub mod fake {
    use std::collections::VecDeque;
    use std::time::Duration;

    use super::Transport;

    /// A scripted device: each request must be the next expected one, and queues its replies.
    #[derive(Default)]
    pub struct Fake {
        script: VecDeque<(Vec<u8>, Vec<Vec<u8>>)>,
        inbox: VecDeque<Vec<u8>>,
        pub sent: Vec<Vec<u8>>,
    }

    /// Parses `"10 FF 00 0C"` into bytes, padded with zeros to its report's length.
    pub fn frame(hex: &str) -> Vec<u8> {
        let mut bytes: Vec<u8> = hex
            .split_whitespace()
            .map(|b| u8::from_str_radix(b, 16).unwrap())
            .collect();
        let len = if bytes[0] == super::REPORT_SHORT {
            super::SHORT_LEN
        } else {
            super::LONG_LEN
        };
        bytes.resize(len, 0);
        bytes
    }

    impl Fake {
        pub fn expect(mut self, request: &str, replies: &[&str]) -> Self {
            self.script
                .push_back((frame(request), replies.iter().map(|r| frame(r)).collect()));
            self
        }

        pub fn done(&self) -> bool {
            self.script.is_empty()
        }
    }

    impl Transport for Fake {
        fn write(&mut self, report: &[u8]) -> crate::Result<()> {
            let (want, replies) = self.script.pop_front().expect("unexpected request");
            assert_eq!(report, want.as_slice(), "request");
            self.sent.push(report.to_vec());
            self.inbox.extend(replies);
            Ok(())
        }

        fn read(&mut self, _timeout: Duration) -> crate::Result<Option<Vec<u8>>> {
            Ok(self.inbox.pop_front())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fake::{frame, Fake};
    use super::*;

    #[test]
    fn frames_short_and_long() {
        let short = encode(Reports::ShortAndLong, 0xFF, 0x00, 0, SW_ID, &[0x10, 0x04]);
        assert_eq!(short, frame("10 FF 00 0C 10 04 00"));
        let long = encode(Reports::LongOnly, 0x01, 0x00, 0, SW_ID, &[0x10, 0x04]);
        assert_eq!(long, frame("11 01 00 0C 10 04"));
        assert_eq!(long.len(), LONG_LEN);
        // Four or more parameters need a long report.
        let four = encode(Reports::ShortAndLong, 0xFF, 0x05, 3, SW_ID, &[1, 2, 3, 4]);
        assert_eq!(four, frame("11 FF 05 3C 01 02 03 04"));
    }

    #[test]
    fn root_get_feature_finds_and_caches_an_index() {
        // GetFeature(0x1004) -> index 0x06, flags 0, version 1; a long reply to a short request.
        let fake = Fake::default().expect("10 FF 00 0C 10 04 00", &["11 FF 00 0C 06 00 01"]);
        let mut dev = Hidpp::new(fake, DEVICE_DIRECT);
        let want = FeatureInfo {
            index: 6,
            flags: 0,
            version: 1,
        };
        assert_eq!(dev.feature(0x1004), Ok(Some(want)));
        // Cached: no second request (the fake would panic on one).
        assert_eq!(dev.feature(0x1004), Ok(Some(want)));
        assert_eq!(dev.feature(ROOT).unwrap().unwrap().index, 0);
        assert!(dev.into_transport().done());
    }

    #[test]
    fn index_zero_means_absent() {
        let fake = Fake::default().expect("10 02 00 0C 80 70 00", &["10 02 00 0C 00 00 00"]);
        let mut dev = Hidpp::new(fake, 2);
        assert_eq!(dev.feature(0x8070), Ok(None));
        assert_eq!(dev.index(0x8070), Err(HidppError::NoFeature(0x8070)));
    }

    #[test]
    fn error_replies_20_and_10() {
        // HID++ 2.0: invalid function on feature 3.
        let fake = Fake::default().expect("10 FF 03 7C", &["11 FF FF 03 7C 07"]);
        let mut dev = Hidpp::new(fake, DEVICE_DIRECT);
        assert_eq!(
            dev.request(3, 7, &[]),
            Err(HidppError::Feature(Error20::InvalidFunction))
        );
        // HID++ 1.0 from a receiver: the slot-3 device is asleep.
        let fake = Fake::default().expect("10 03 00 0C 00 01 00", &["10 03 8F 00 0C 09 00"]);
        let mut dev = Hidpp::new(fake, 3);
        let e = dev.feature(0x0001).unwrap_err();
        assert_eq!(e, HidppError::Protocol10(Error10::ResourceError));
        assert_eq!(DeviceError::from(e), DeviceError::Disconnected);
        assert_eq!(
            DeviceError::from(HidppError::Feature(Error20::Busy)),
            DeviceError::Busy("HID++ device busy".into())
        );
    }

    #[test]
    fn protocol_version_and_hidpp10_devices() {
        let fake = Fake::default().expect("10 FF 00 1C 00 00 5A", &["10 FF 00 1C 04 05 5A"]);
        assert_eq!(
            Hidpp::new(fake, DEVICE_DIRECT).protocol_version(),
            Ok((4, 5))
        );
        let fake = Fake::default().expect("10 01 00 1C 00 00 5A", &["10 01 8F 00 1C 01 00"]);
        assert_eq!(Hidpp::new(fake, 1).protocol_version(), Ok((1, 0)));
        let fake = Fake::default().expect("10 01 00 1C 00 00 5A", &["10 01 00 1C 04 05 00"]);
        assert!(matches!(
            Hidpp::new(fake, 1).protocol_version(),
            Err(HidppError::Malformed(_))
        ));
    }

    #[test]
    fn skips_other_apps_replies_and_notifications() {
        let fake = Fake::default().expect(
            "10 FF 00 0C 10 00 00",
            &[
                // Another app's reply to the same request (software id 3).
                "11 FF 00 03 02 00 00",
                // A device notification (software id 0) on feature 2.
                "11 FF 02 00 55",
                // Another device behind the same receiver, and a non-HID++ report.
                "11 01 00 0C 09",
                "20 FF 00 0A",
                "11 FF 00 0C 02 00 00",
            ],
        );
        let mut dev = Hidpp::new(fake, DEVICE_DIRECT);
        assert_eq!(dev.index(0x1000), Ok(2));
    }

    #[test]
    fn no_reply_is_a_bounded_timeout() {
        let fake = Fake::default().expect("10 FF 00 0C 10 00 00", &["11 FF 00 03 02"]);
        let mut dev = Hidpp::new(fake, DEVICE_DIRECT).with_timeout(Duration::from_millis(5));
        assert_eq!(dev.feature(0x1000), Err(HidppError::Timeout));
        assert_eq!(DeviceError::from(HidppError::Timeout), DeviceError::Timeout);
    }

    #[test]
    fn long_only_devices_get_long_requests() {
        let fake = Fake::default().expect("11 FF 00 0C 10 01", &["11 FF 00 0C 04 00 00"]);
        let mut dev = Hidpp::new(fake, DEVICE_DIRECT).long_only();
        assert_eq!(dev.index(0x1001), Ok(4));
    }

    #[test]
    fn feature_set_lists_every_feature() {
        let fake = Fake::default()
            .expect("10 FF 00 0C 00 01 00", &["11 FF 00 0C 01 00 00"])
            .expect("10 FF 01 0C", &["11 FF 01 0C 03"])
            .expect("10 FF 01 1C 00", &["11 FF 01 1C 00 00 00"])
            .expect("10 FF 01 1C 01", &["11 FF 01 1C 00 01 00"])
            .expect("10 FF 01 1C 02", &["11 FF 01 1C 10 04 00"])
            .expect("10 FF 01 1C 03", &["11 FF 01 1C 80 71 20"]);
        let mut dev = Hidpp::new(fake, DEVICE_DIRECT);
        let all = dev.features().unwrap();
        assert_eq!(
            all,
            vec![
                (0, ROOT, 0),
                (1, FEATURE_SET, 0),
                (2, UNIFIED_BATTERY, 0),
                (3, 0x8071, 0x20)
            ]
        );
        // Learnt from the list: no GetFeature request for it.
        assert_eq!(dev.index(UNIFIED_BATTERY), Ok(2));
        assert!(dev.into_transport().done());
    }
}
