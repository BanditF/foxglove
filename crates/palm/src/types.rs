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

/// Serial takes precedence over path, so moving a serial device keeps its identity.
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

impl DeviceId {
    pub fn new(backend: BackendId, vid: u16, pid: u16, serial: Option<&str>, path: &str) -> Self {
        let identity = match serial.filter(|s| !s.is_empty()) {
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

    pub const LIGHTING: Self = Self(32);

    pub const SCREENS: Self = Self(64);

    pub const POWER: Self = Self(128);

    pub const SETTINGS: Self = Self(256);

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
