//! Health of the Dynamic Lighting (LampArray) devices (#691), as plain data for the shared
//! health view (`health.read`, `foxglove-lab health`): one [`LampHealth`] per device the Lights
//! worker found, and [`rows`] turning it into view-model rows. Nothing here draws or asks
//! anything: it reads the worker's snapshot, which the worker updates on its own events (a
//! scan, a write, Windows' `AvailabilityChanged`). No polling.
//!
//! The rows are the Keyboard health view's shared type (`foxglove_link::health::HealthRow`, #705),
//! registered with `health::register` as "dynamic_lighting".

use foxglove_devices::pc_lighting::{Control, KeyboardLampArray, LampAccess};

use super::pc_lights::Snapshot;

/// The subsystem these rows belong to in the health view.
pub const SUBSYSTEM: &str = "lights.dynamic_lighting";

/// How the last write to a device went.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Apply {
    Ok,
    /// With the reason Windows (or the driver) gave.
    Failed(String),
}

/// One Dynamic Lighting device's health.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LampHealth {
    /// The Lights page key (`vvvv:pppp`).
    pub key: String,
    pub name: String,
    /// Lamps read from the device; 0 until its attributes are read.
    pub lamps: usize,
    /// Foxglove holds a handle on it and drives it.
    pub controllable: bool,
    /// What Windows lets Foxglove do with it now (`IsEnabled`, `IsAvailable`); `None` while
    /// it isn't controllable.
    pub access: Option<LampAccess>,
    /// Foxglove runs with package identity, so Windows' "Background light control" can pick
    /// it (#179); without it Foxglove drives the device only while its window is in front.
    pub identity: bool,
    /// The last colours Foxglove applied to it; `None` before the first.
    pub last_apply: Option<Apply>,
}

// The shared row type of the Keyboard health view (#705).
pub use super::health::{HealthRow, Level};

/// The health view's rows now, every Dynamic Lighting device's (registered with
/// `health::register`).
pub fn rows_now() -> Vec<HealthRow> {
    let mut out: Vec<HealthRow> = super::pc_lights::lamp_health()
        .iter()
        .flat_map(rows)
        .collect();
    let keyboards = super::pc_lights::snapshot().keyboard_lamp_arrays;
    let identity = super::scene_lamps::identity();
    out.extend(keyboard_rows(&keyboards, identity));
    out.extend(identity_row(
        super::identity::ENABLED,
        identity,
        super::identity::package_here(),
    ));
    if identity && !keyboards.is_empty() {
        out.extend(hold_rows(super::lamp_hold::now()));
    }
    out
}

/// The package identity row (#179): OK with identity; WARN when this copy should have it and
/// doesn't (the package is next to the exe, so it was installed or registered for this
/// folder), naming the likely cause; none for an ordinary build without the package.
pub fn identity_row(enabled: bool, has: bool, package_here: bool) -> Option<HealthRow> {
    let row =
        |level, detail: &str| HealthRow::new(SUBSYSTEM, "Dynamic Lighting identity", level, detail);
    match (has, enabled, package_here) {
        (true, _, _) => Some(row(
            Level::Ok,
            "Foxglove runs with package identity: Windows lists it under Settings > \
             Personalization > Dynamic Lighting > Background light control",
        )),
        (false, true, true) => Some(row(
            Level::Warn,
            "This copy is built for package identity and its package is next to it, but it runs \
             without: not registered for this folder yet, or its certificate isn't trusted \
             (scripts\\dev-identity.ps1 plan). Dynamic Lighting devices follow Foxglove only \
             while its window is in front",
        )),
        (false, false, true) => Some(row(
            Level::Warn,
            "The identity package is next to this exe, but this build has no identity manifest: \
             rebuild with FOXGLOVE_SPARSE_IDENTITY=1 (and FOXGLOVE_IDENTITY_PUBLISHER for a dev \
             certificate) or [identity] sparse = true in tools\\local.toml, or Windows' background light control skips Foxglove",
        )),
        (false, _, false) => None,
    }
}

/// With identity, Foxglove's hold on the keyboard's own LampArrays, per transport (#179): WARN
/// when Windows gives one to another controller (Foxglove ranks below it for that device).
pub fn hold_rows(holds: super::lamp_hold::Holds) -> Vec<HealthRow> {
    [("USB", holds.usb), ("Bluetooth", holds.ble)]
        .into_iter()
        .filter_map(|(via, held)| {
            let item = format!("Keyboard LampArray over {via}");
            Some(match held? {
                true => HealthRow::new(
                    SUBSYSTEM,
                    item,
                    Level::Ok,
                    "Foxglove holds it as the background light controller",
                ),
                false => HealthRow::new(
                    SUBSYSTEM,
                    item,
                    Level::Warn,
                    format!(
                        "Another controller has it: in Settings > Personalization > Dynamic \
                         Lighting, drag Foxglove to the top of Background light control, and \
                         check the keyboard's {via} device page. Foxglove never changes that setting"
                    ),
                ),
            })
        })
        .collect()
}

/// One row per keyboard that is also a LampArray (#754): Windows lists it under Dynamic
/// Lighting (over USB, Bluetooth or both), and Foxglove drives its lights over the keyboard's
/// own link (the Keyboard card), so it has no Dynamic Lighting device rows of its own. While
/// Windows Dynamic Lighting holds the keyboard's lights, Foxglove draws through it (#749 b):
/// only while its window is in front, unless `identity` (package identity, #179) says
/// Windows' background order decides. The wording follows `lights_owner`'s notice.
pub fn keyboard_rows(keyboards: &[KeyboardLampArray], identity: bool) -> Vec<HealthRow> {
    let mut names: Vec<&str> = Vec::new();
    for k in keyboards {
        if !names.contains(&k.name.as_str()) {
            names.push(&k.name);
        }
    }
    names
        .into_iter()
        .map(|name| {
            let of = || keyboards.iter().filter(|k| k.name == name);
            let via = match (of().any(|k| !k.bluetooth), of().any(|k| k.bluetooth)) {
                (true, true) => "over USB and Bluetooth",
                (false, true) => "over Bluetooth",
                _ => "over USB",
            };
            HealthRow::new(
                SUBSYSTEM,
                format!("{name} (keyboard) · Found"),
                Level::Ok,
                format!(
                    "Windows lists it {via}; Foxglove drives its lights over the keyboard's \
                     own link (the Keyboard card). While Windows Dynamic Lighting holds the \
                     keyboard's lights, {}",
                    if identity {
                        super::lights_owner::THROUGH_IDENTITY
                    } else {
                        super::lights_owner::THROUGH
                    }
                ),
            )
        })
        .collect()
}

/// Every Dynamic Lighting device in a Lights snapshot, with the last apply per key from
/// `applies`.
pub fn from_snapshot(
    s: &Snapshot,
    applies: &std::collections::BTreeMap<String, Apply>,
    identity: bool,
) -> Vec<LampHealth> {
    s.devices
        .iter()
        .filter(|r| r.control == Control::LampArray)
        .map(|r| LampHealth {
            key: r.key.clone(),
            name: r.name.clone(),
            lamps: r.zones.len(),
            controllable: r.lamp_access.is_some(),
            access: r.lamp_access,
            identity,
            last_apply: applies.get(&r.key).cloned(),
        })
        .collect()
}

/// The health view's rows for one device: found, controllable, Windows' setting, last apply.
pub fn rows(h: &LampHealth) -> Vec<HealthRow> {
    let row = |what: &str, level, detail: String| {
        HealthRow::new(SUBSYSTEM, format!("{} · {what}", h.name), level, detail)
    };
    let mut out = vec![if h.lamps > 0 {
        row(
            "Found",
            Level::Ok,
            format!("Windows lists it, {} lamps", h.lamps),
        )
    } else {
        row(
            "Found",
            Level::Warn,
            "Windows lists it; its lamps aren't read yet".into(),
        )
    }];
    out.push(if h.controllable {
        row("Control", Level::Ok, "Foxglove drives it".into())
    } else {
        row(
            "Control",
            Level::Warn,
            "Foxglove has no handle on it (no Dynamic Lighting device for it right now)".into(),
        )
    });
    if let Some(a) = h.access {
        let background = if h.identity {
            "background light control possible"
        } else {
            "front only: no package identity, so no background light control"
        };
        out.push(match (a.enabled, a.available) {
            (false, _) => row(
                "Windows",
                Level::Warn,
                "Turned off in Settings > Personalization > Dynamic Lighting".into(),
            ),
            (true, false) if h.identity => row(
                "Windows",
                Level::Warn,
                "Another app has it now (Dynamic Lighting's app order)".into(),
            ),
            (true, false) => row(
                "Windows",
                Level::Warn,
                format!("Windows has it while Foxglove's window isn't in front ({background})"),
            ),
            (true, true) => row(
                "Windows",
                Level::Ok,
                format!("Foxglove may drive it now ({background})"),
            ),
        });
    }
    out.push(match &h.last_apply {
        None => row("Last apply", Level::Ok, "Nothing applied yet".into()),
        Some(Apply::Ok) => row("Last apply", Level::Ok, "Colours applied".into()),
        Some(Apply::Failed(why)) => row("Last apply", Level::Fail, format!("Failed: {why}")),
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mouse() -> LampHealth {
        LampHealth {
            key: "046d:c332".into(),
            name: "G502 Proteus Spectrum".into(),
            lamps: 2,
            controllable: true,
            access: Some(LampAccess::default()),
            identity: false,
            last_apply: Some(Apply::Ok),
        }
    }

    fn levels(h: &LampHealth) -> Vec<(String, Level)> {
        rows(h).into_iter().map(|r| (r.item, r.level)).collect()
    }

    /// #754: a keyboard that is also a LampArray is named in the Dynamic Lighting rows, once
    /// for USB and Bluetooth, as the keyboard's.
    #[test]
    fn a_keyboards_own_lamp_array_has_one_row_labelled_as_the_keyboard() {
        let corne = |key: &str, bluetooth| KeyboardLampArray {
            key: key.into(),
            name: "Eyelash Corne".into(),
            vid: 0x1d50,
            pid: 0x615e,
            bluetooth,
        };
        let r = keyboard_rows(
            &[corne("1d50:615e:1E89", false), corne("1d50:615e", true)],
            false,
        );
        assert_eq!(r.len(), 1, "{r:?}");
        assert_eq!(r[0].item, "Eyelash Corne (keyboard) · Found");
        assert_eq!(
            (r[0].level, r[0].subsystem.as_str()),
            (Level::Ok, SUBSYSTEM)
        );
        assert!(
            r[0].detail
                .starts_with("Windows lists it over USB and Bluetooth; "),
            "{r:?}"
        );
        assert!(r[0].detail.contains("the Keyboard card"), "{r:?}");
        // #749 b: the notice's wording, not the old "not through Dynamic Lighting".
        assert!(
            r[0].detail.ends_with(
                "While Windows Dynamic Lighting holds the keyboard's lights, Foxglove draws \
                 through it while its window is in front"
            ),
            "{r:?}"
        );
        assert!(!r[0].detail.contains("not through"), "{r:?}");
        assert!(!r[0].detail.contains("  "), "no stray spaces: {r:?}");
        let usb = keyboard_rows(&[corne("1d50:615e:1E89", false)], true);
        assert!(
            usb[0].detail.starts_with("Windows lists it over USB;"),
            "{usb:?}"
        );
        assert!(
            usb[0].detail.ends_with("Foxglove draws through it"),
            "package identity: no front-window clause: {usb:?}"
        );
        assert!(keyboard_rows(&[], false).is_empty());
    }

    #[test]
    fn a_driven_device_is_all_ok_and_says_front_only_without_identity() {
        let h = mouse();
        assert!(levels(&h).iter().all(|(_, l)| *l == Level::Ok));
        let r = rows(&h);
        assert_eq!(r.len(), 4);
        assert!(r.iter().all(|r| r.subsystem == SUBSYSTEM));
        assert!(r[2].detail.contains("front only"), "{:?}", r[2]);
        assert_eq!(r[0].item, "G502 Proteus Spectrum · Found");
    }

    #[test]
    fn each_problem_has_its_row_and_level() {
        let mut h = mouse();
        h.last_apply = Some(Apply::Failed("device gone".into()));
        let r = rows(&h);
        assert_eq!(
            (r[3].level, r[3].detail.as_str()),
            (Level::Fail, "Failed: device gone")
        );
        h.access = Some(LampAccess {
            enabled: false,
            available: true,
        });
        assert!(rows(&h)[2].detail.contains("Turned off"));
        h.access = Some(LampAccess {
            enabled: true,
            available: false,
        });
        h.identity = true;
        assert!(rows(&h)[2].detail.contains("Another app"));
        assert_eq!(rows(&h)[2].level, Level::Warn);
        // Not controllable: no Windows row, a warning instead.
        h.controllable = false;
        h.access = None;
        h.lamps = 0;
        let l = levels(&h);
        assert_eq!(l.len(), 3);
        assert_eq!(l[0].1, Level::Warn);
        assert_eq!(l[1].1, Level::Warn);
    }

    /// #179: the identity row says OK with identity, warns when this copy should have it (the
    /// package is next to it) and names why, and stays away from ordinary builds.
    #[test]
    fn the_identity_row_catches_a_lost_identity() {
        let ok = identity_row(true, true, true).unwrap();
        assert_eq!(ok.level, Level::Ok);
        assert!(ok.detail.contains("Background light control"));
        let missing = identity_row(true, false, true).unwrap();
        assert_eq!(missing.level, Level::Warn);
        assert!(
            missing.detail.contains("not registered"),
            "{}",
            missing.detail
        );
        let rebuilt = identity_row(false, false, true).unwrap();
        assert_eq!(rebuilt.level, Level::Warn);
        assert!(
            rebuilt.detail.contains("FOXGLOVE_SPARSE_IDENTITY=1"),
            "{}",
            rebuilt.detail
        );
        assert_eq!(identity_row(false, false, false), None, "an ordinary build");
        assert_eq!(
            identity_row(true, false, false),
            None,
            "a lab or worktree copy"
        );
    }

    #[test]
    fn the_hold_rows_warn_per_transport_another_controller_has() {
        use crate::foxglove::lamp_hold::Holds;
        let r = hold_rows(Holds {
            usb: Some(true),
            ble: Some(false),
        });
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].level, Level::Ok);
        assert_eq!(r[1].level, Level::Warn);
        assert!(
            r[1].detail.contains("Bluetooth device page"),
            "{}",
            r[1].detail
        );
        assert!(
            hold_rows(Holds::default()).is_empty(),
            "nothing held: no rows"
        );
    }
}
