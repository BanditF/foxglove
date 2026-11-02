//! Health of the Dynamic Lighting (LampArray) devices (#691), as plain data for the shared
//! health view (`health.read`, `foxglove-lab health`): one [`LampHealth`] per device the Lights
//! worker found, and [`rows`] turning it into view-model rows. Nothing here draws or asks
//! anything: it reads the worker's snapshot, which the worker updates on its own events (a
//! scan, a write, Windows' `AvailabilityChanged`). No polling.

// The shared health view (worktrees\health: `health.read`, the app's health page,
// `foxglove-lab health`) picks these up; until it lands only the tests read them.
#![allow(dead_code)]

use foxglove_devices::pc_lighting::{Control, LampAccess};

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

/// A row's state in the health view.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Ok,
    /// Works as Windows allows, but not everything the user may expect.
    Warn,
    Fail,
}

/// One view-model row for the health view.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HealthRow {
    pub subsystem: &'static str,
    /// The device's name and what the row checks: "G502 Proteus Spectrum · Windows".
    pub item: String,
    pub level: Level,
    pub detail: String,
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
    let row = |what: &str, level, detail: String| HealthRow {
        subsystem: SUBSYSTEM,
        item: format!("{} · {what}", h.name),
        level,
        detail,
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
}
