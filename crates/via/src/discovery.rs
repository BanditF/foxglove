//! Which Raw HID interfaces become VIA keyboards (#74), kept apart from hidapi so it can be
//! tested: QMK's Raw HID interface (usage page 0xFF60, usage 0x61) on a board with a loaded VIA
//! definition, or, in generic mode, on any board the catalog doesn't skip.

use foxglove_devices::{BackendId, DeviceId, DeviceInfo, DeviceKind};

use crate::catalog::Catalog;
use crate::definition::Definition;

pub const USAGE_PAGE: u16 = 0xFF60;
pub const USAGE: u16 = 0x61;

/// One HID interface as the OS lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Interface {
    pub vid: u16,
    pub pid: u16,
    pub usage_page: u16,
    pub usage: u16,
    pub serial: Option<String>,
    /// What names it without a trusted serial: where it's plugged in, else the HID path.
    pub stable_path: String,
}

impl Interface {
    pub fn is_raw_hid(&self) -> bool {
        (self.usage_page, self.usage) == (USAGE_PAGE, USAGE)
    }
}

/// What the backend looks for.
pub struct Rules<'a> {
    pub backend: BackendId,
    pub definitions: &'a [Definition],
    pub catalog: &'a Catalog,
    /// Also boards without a loaded definition.
    pub generic: bool,
}

impl Rules<'_> {
    pub fn definition(&self, vid: u16, pid: u16) -> Option<&Definition> {
        self.definitions
            .iter()
            .find(|d| (d.vendor_id, d.product_id) == (vid, pid))
    }

    /// Whether an interface is one to open, and under what name.
    pub fn accepts(&self, i: &Interface) -> Option<String> {
        if !i.is_raw_hid() || self.catalog.skipped(i.vid, i.pid).is_some() {
            return None;
        }
        match self.definition(i.vid, i.pid) {
            Some(def) => Some(def.name.clone()),
            None if self.generic => Some(self.catalog.name(i.vid, i.pid)),
            None => None,
        }
    }

    /// The keyboards among the interfaces, one per board. Two identical boards with the same
    /// serial are told apart by their paths (#292).
    pub fn plan(&self, interfaces: &[Interface]) -> Vec<DeviceInfo> {
        let mut seen: Vec<(DeviceId, String)> = Vec::new();
        let mut names = Vec::new();
        for i in interfaces {
            let Some(name) = self.accepts(i) else {
                continue;
            };
            seen.push((
                DeviceId::new(
                    self.backend.clone(),
                    i.vid,
                    i.pid,
                    i.serial.as_deref(),
                    &i.stable_path,
                ),
                i.stable_path.clone(),
            ));
            names.push(name);
        }
        foxglove_devices::untrust_shared_serials(&mut seen);
        let mut found: Vec<DeviceInfo> = Vec::new();
        for ((id, _), name) in seen.into_iter().zip(names) {
            if found.iter().all(|f| f.id != id) {
                found.push(DeviceInfo {
                    id,
                    backend: self.backend.clone(),
                    name,
                    kind: DeviceKind::Keyboard,
                    firmware: None,
                });
            }
        }
        found
    }

    /// Whether an interface is the one a discovered keyboard names: by serial, or by where it's
    /// plugged in when its serial was shared (#292).
    pub fn is(&self, i: &Interface, info: &DeviceInfo) -> bool {
        self.accepts(i).is_some()
            && (DeviceId::new(
                self.backend.clone(),
                i.vid,
                i.pid,
                i.serial.as_deref(),
                &i.stable_path,
            ) == info.id
                || DeviceId::new(self.backend.clone(), i.vid, i.pid, None, &i.stable_path)
                    == info.id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::definition::SAMPLE;

    fn iface(vid: u16, pid: u16, page: u16, path: &str) -> Interface {
        Interface {
            vid,
            pid,
            usage_page: page,
            usage: if page == USAGE_PAGE { USAGE } else { 6 },
            serial: None,
            stable_path: path.into(),
        }
    }

    fn present() -> Vec<Interface> {
        vec![
            iface(0x3434, 0x0250, USAGE_PAGE, "usb:k5"),
            iface(0x3434, 0x0250, 0x0001, "usb:k5"), // its keyboard interface
            iface(0xFEED, 0x6060, USAGE_PAGE, "usb:diy"),
            iface(0x1D50, 0x615E, USAGE_PAGE, "usb:corne"), // ZMK: the host link
            iface(0x046D, 0xC52B, 0xFF00, "usb:mouse"),
        ]
    }

    #[test]
    fn with_definitions_only_defined_boards_are_found() {
        let defs = [Definition::parse(SAMPLE).unwrap()];
        let catalog = Catalog::bundled();
        let rules = Rules {
            backend: BackendId("via".into()),
            definitions: &defs,
            catalog: &catalog,
            generic: false,
        };
        let found = rules.plan(&present());
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "Sample K5-like", "the definition's name");
        assert_eq!((found[0].id.vid, found[0].id.pid), (0x3434, 0x0250));
        assert!(rules.is(&present()[0], &found[0]));
        assert!(
            !rules.is(&present()[1], &found[0]),
            "not its keyboard interface"
        );
    }

    #[test]
    fn generic_finds_every_qmk_raw_hid_board_but_the_skipped_ones() {
        let catalog = Catalog::bundled();
        let rules = Rules {
            backend: BackendId("via".into()),
            definitions: &[],
            catalog: &catalog,
            generic: true,
        };
        let names: Vec<String> = rules.plan(&present()).into_iter().map(|d| d.name).collect();
        assert_eq!(
            names,
            vec![
                "Keychron K5 Pro (ANSI, RGB)".to_string(),
                "QMK keyboard feed:6060".to_string()
            ],
            "ZMK's host-link interface and non-Raw-HID interfaces are left alone"
        );
    }

    #[test]
    fn identical_boards_with_a_shared_serial_are_told_apart_by_port() {
        let catalog = Catalog::bundled();
        let rules = Rules {
            backend: BackendId("via".into()),
            definitions: &[],
            catalog: &catalog,
            generic: true,
        };
        let mut a = iface(0xFEED, 0x6060, USAGE_PAGE, "usb:port1");
        a.serial = Some("vial:f64c2b3c".into());
        let mut b = a.clone();
        b.stable_path = "usb:port2".into();
        let found = rules.plan(&[a.clone(), b.clone()]);
        assert_eq!(found.len(), 2);
        assert_ne!(found[0].id, found[1].id);
        assert!(rules.is(&a, &found[0]) && rules.is(&b, &found[1]));
        assert!(!rules.is(&a, &found[1]));
    }
}
