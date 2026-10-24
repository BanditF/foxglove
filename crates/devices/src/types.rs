use serde::{Deserialize, Serialize};
use std::fmt;

/// The namespace identifying a registered backend.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct BackendId(pub String);

impl fmt::Display for BackendId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// A trusted serial takes precedence over path, so moving such a device keeps its identity
/// ([`trusted_serial`], [`untrust_shared_serials`]).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DeviceId {
    pub backend: BackendId,
    pub vid: u16,
    pub pid: u16,
    pub identity: DeviceIdentity,
}

/// The serial number or path used to identify a device.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DeviceIdentity {
    Serial(String),
    Path(String),
}

/// Whether a USB serial can tell boards apart (#292). QMK sends none unless configured, and
/// some firmware sends the same placeholder on every unit: too short, one character repeated,
/// a counting run, or a placeholder word aren't trusted.
pub fn trusted_serial(serial: &str) -> bool {
    let s = serial.trim();
    let lower = s.to_ascii_lowercase();
    let first = s.chars().next();
    let counting = "0123456789abcdefghijklmnopqrstuvwxyz";
    s.len() >= 4
        && !s.chars().all(|c| Some(c) == first)
        && !counting.contains(&lower)
        && !["none", "null", "n/a", "na", "default", "serial", "serialnumber", "unknown", "123456789", "0123456789"].contains(&lower.as_str())
}

/// After discovery: devices of one backend with the same vid:pid and serial on different
/// paths can't be told apart by it (#292), so they're named by their paths instead.
/// `found` is each device's id and path.
pub fn untrust_shared_serials(found: &mut [(DeviceId, String)]) {
    let shared: Vec<bool> = found
        .iter()
        .map(|(id, path)| {
            matches!(id.identity, DeviceIdentity::Serial(_))
                && found.iter().any(|(other, other_path)| other == id && other_path != path)
        })
        .collect();
    for ((id, path), shared) in found.iter_mut().zip(shared) {
        if shared {
            id.identity = DeviceIdentity::Path(path.clone());
        }
    }
}

impl DeviceId {
    pub fn new(backend: BackendId, vid: u16, pid: u16, serial: Option<&str>, path: &str) -> Self {
        let identity = match serial.filter(|s| trusted_serial(s)) {
            Some(s) => DeviceIdentity::Serial(s.into()),
            None => DeviceIdentity::Path(path.into()),
        };
        Self {
            backend,
            vid,
            pid,
            identity,
        }
    }
}

// Hex-encode strings to keep dots and separators out of fact namespace components.
fn hex(s: &str) -> String {
    s.as_bytes().iter().map(|b| format!("{b:02x}")).collect()
}

impl fmt::Display for DeviceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (tag, value) = match &self.identity {
            DeviceIdentity::Serial(s) => ("s", s),
            DeviceIdentity::Path(s) => ("p", s),
        };
        write!(
            f,
            "{}-{:04x}:{:04x}-{tag}{}",
            hex(&self.backend.0),
            self.vid,
            self.pid,
            hex(value)
        )
    }
}

/// The kind of peripheral represented by a device.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DeviceKind {
    Keyboard,
    KeyboardHalf,
    Mouse,
    Strip,
    Other,
}

/// Discovery metadata for a device.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceInfo {
    pub id: DeviceId,
    pub backend: BackendId,
    pub name: String,
    pub kind: DeviceKind,
    pub firmware: Option<String>,
}

/// Flags describing the capabilities a device exposes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Caps(u16);

impl Caps {
    pub const KEYMAP: Self = Self(1);

    pub const KEYMAP_WRITE: Self = Self(2);

    pub const LAYERS: Self = Self(4);

    pub const KEY_EVENTS: Self = Self(8);

    pub const BEHAVIORS: Self = Self(16);

    /// RGB out: the device takes colors or effects ([`crate::Lighting`]).
    pub const LIGHTING: Self = Self(32);

    pub const SCREENS: Self = Self(64);

    pub const POWER: Self = Self(128);

    pub const SETTINGS: Self = Self(256);

    /// Color in: the device or vendor software supplies colors ([`crate::ColorSource`]).
    pub const COLOR_IN: Self = Self(512);

    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    pub const fn bits(self) -> u16 {
        self.0
    }
}

impl std::ops::BitOr for Caps {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

impl std::ops::BitOrAssign for Caps {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

#[cfg(test)]
mod serial_tests {
    use super::*;

    fn id(serial: Option<&str>, path: &str) -> DeviceId {
        DeviceId::new(BackendId("via".into()), 0x3434, 0x0250, serial, path)
    }

    #[test]
    fn only_a_telling_serial_is_trusted() {
        for good in ["A1B2C3D4", "6E8F2A001", "K5PRO-00412"] {
            assert!(trusted_serial(good), "{good}");
        }
        for bad in ["", "   ", "1", "abc", "0000", "FFFFFFFF", "123456", "ABCDEF", "None", "serial", "SerialNumber", "0123456789"] {
            assert!(!trusted_serial(bad), "{bad:?}");
        }
        assert_eq!(id(Some("0000"), "p1").identity, DeviceIdentity::Path("p1".into()));
        assert_eq!(id(Some("A1B2C3D4"), "p1").identity, DeviceIdentity::Serial("A1B2C3D4".into()));
    }

    #[test]
    fn a_serial_two_boards_share_isnt_trusted() {
        let mut found = vec![
            (id(Some("A1B2C3D4"), "p1"), "p1".to_string()),
            (id(Some("A1B2C3D4"), "p2"), "p2".to_string()),
            (id(Some("ZZ998877"), "p3"), "p3".to_string()),
        ];
        untrust_shared_serials(&mut found);
        assert_eq!(found[0].0.identity, DeviceIdentity::Path("p1".into()));
        assert_eq!(found[1].0.identity, DeviceIdentity::Path("p2".into()));
        assert_eq!(found[2].0.identity, DeviceIdentity::Serial("ZZ998877".into()), "its own serial stays");
        // One board seen twice on the same path keeps its serial.
        let mut once = vec![(id(Some("A1B2C3D4"), "p1"), "p1".to_string()), (id(Some("A1B2C3D4"), "p1"), "p1".to_string())];
        untrust_shared_serials(&mut once);
        assert!(matches!(once[0].0.identity, DeviceIdentity::Serial(_)));
    }
}
