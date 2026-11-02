//! A light device's real lighting resolution (#611): the zones Foxglove can set one colour
//! each, which keys or areas each one lights, and how many lamps light nothing. It is often
//! lower than the LED count: the Eyelash Corne has 21 lamps per half and 6 zones (#608).
//!
//! Every source fills the same [`Zones`], so the page never asks where it came from:
//! - the device's own report (a LampArray's lamp count, a ZMK keyboard's LAMP_PARTS),
//! - a profile or catalog entry (a board profile's `[lighting] led_keys`, the lighting
//!   catalog's `zones`),
//! - the user's own check (the store's `[led_map]`, or a device the user added by hand).
//!
//! When more than one is known, [`Zones::pick`] takes the user's check over a profile over the
//! device's report (the user's ruling on #611), and keeps the report's count for one muted line.
//! The words follow docs/lighting-zones.md sections 1, 5 and 7.

/// Where a device's zones came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ZoneSource {
    /// The user's own check (Find my LEDs, the dot test, a device added by hand).
    Check,
    /// A board profile's map; `checked` once it was confirmed on a board.
    Profile { checked: bool },
    /// Foxglove's lighting catalog.
    Catalog,
    /// The device itself (a LampArray's descriptor, a keyboard's LAMP_PARTS).
    Reported,
}

/// One key a zone lights: its index on the device's drawing and, for a key in the drawing's
/// grid, its column and row. Keys outside the grid (thumbs, listed keys) have neither.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ZoneKey {
    pub index: u16,
    pub column: Option<u16>,
    pub row: Option<u16>,
}

/// One zone that lights something.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Zone {
    /// The part it is on (0 for a one-piece device, 0/1 for a split's halves).
    pub part: u8,
    /// The keys it lights; empty for an area or a lamp whose place isn't known by key.
    pub keys: Vec<ZoneKey>,
    /// What it lights when it isn't keys: "Front top fan".
    pub area: Option<String>,
}

/// A device's zones: the live ones, the lamps that light nothing, and where that came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Zones {
    /// The zones that light something, part by part.
    pub list: Vec<Zone>,
    /// Lamps that light nothing, per part (one entry per part).
    pub dead: Vec<u32>,
    pub source: ZoneSource,
    /// Lamps per part the device itself reports, when another source won and the counts
    /// differ: kept only for the source line.
    pub reported: Option<Vec<u32>>,
}

impl Zones {
    /// `n` lamps per part reported by the device, each a zone whose place isn't known.
    pub fn from_report(lamps: &[u32]) -> Zones {
        let list = lamps
            .iter()
            .enumerate()
            .flat_map(|(part, &n)| {
                (0..n).map(move |_| Zone {
                    part: part as u8,
                    ..Default::default()
                })
            })
            .collect();
        Zones {
            list,
            dead: vec![0; lamps.len()],
            source: ZoneSource::Reported,
            reported: None,
        }
    }

    /// The winner of the sources, in the user's order: their check, then a profile or catalog
    /// entry, then the device's own report. The report's lamp counts are kept on the winner
    /// when they differ from its own, for the source line ("the keyboard reports 21 lamps
    /// per half"). `None` when no source knows.
    pub fn pick(
        check: Option<Zones>,
        profile: Option<Zones>,
        device: Option<Zones>,
    ) -> Option<Zones> {
        let device_lamps = device.as_ref().map(Zones::lamps);
        let mut won = check.or(profile).or(device)?;
        if won.source != ZoneSource::Reported {
            won.reported = device_lamps.filter(|d| *d != won.lamps());
        }
        Some(won)
    }

    /// How many parts it has: at least 1.
    pub fn parts(&self) -> usize {
        let from_zones = self.list.iter().map(|z| usize::from(z.part) + 1).max();
        from_zones.unwrap_or(0).max(self.dead.len()).max(1)
    }

    /// Live zones per part.
    pub fn live(&self) -> Vec<u32> {
        let mut out = vec![0; self.parts()];
        for z in &self.list {
            out[usize::from(z.part)] += 1;
        }
        out
    }

    /// All lamps per part, live and dead.
    pub fn lamps(&self) -> Vec<u32> {
        let mut out = self.live();
        for (n, d) in out.iter_mut().zip(&self.dead) {
            *n += d;
        }
        out
    }

    /// Every zone is one key (a per-key keyboard).
    fn per_key(&self) -> bool {
        !self.list.is_empty() && self.list.iter().all(|z| z.keys.len() == 1)
    }

    /// Every zone is one lamp with no keys or area (a per-LED device, places unknown by key).
    fn per_lamp(&self) -> bool {
        !self.list.is_empty()
            && self
                .list
                .iter()
                .all(|z| z.keys.is_empty() && z.area.is_none())
    }

    /// "by column" when every zone's grid keys share one column (keys outside the grid, such
    /// as thumbs, allowed), "by row" likewise; worked out from the keys, never typed into a
    /// profile. `None` for fewer than two zones, areas, or zones that cross.
    pub fn arrangement(&self) -> Option<&'static str> {
        if self.list.len() < 2 {
            return None;
        }
        let shared = |pick: fn(&ZoneKey) -> Option<u16>| {
            self.list.iter().all(|z| {
                let mut on = z.keys.iter().filter_map(pick);
                match on.next() {
                    Some(first) => on.all(|c| c == first),
                    None => false,
                }
            })
        };
        if shared(|k| k.column) {
            Some("by column")
        } else if shared(|k| k.row) {
            Some("by row")
        } else {
            None
        }
    }
}

/// "per half" for two parts, "per part" for more.
fn per_part(parts: usize) -> &'static str {
    if parts == 2 {
        "per half"
    } else {
        "per part"
    }
}

/// The same count on every part (and more than one part).
fn mirrored(counts: &[u32]) -> Option<u32> {
    let first = *counts.first()?;
    (counts.len() > 1 && counts.iter().all(|&n| n == first)).then_some(first)
}

fn plural(n: u32, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// The summary line (docs/lighting-zones.md section 1): "6 lighting zones per half, by
/// column", "5 lighting zones", "1 zone (whole device)", "Per-key lighting, 61 keys",
/// "Per-LED lighting, 24 lamps", "On/off light", "Lighting zones unknown". Dead lamps never
/// count. `keys` says the device is a keyboard. `None` for a device with no lights.
pub fn summary(
    color: super::Color,
    count: u32,
    zones: Option<&Zones>,
    keys: bool,
) -> Option<String> {
    use super::Color;
    let per_led = |n: u32| {
        if keys {
            format!("Per-key lighting, {}", plural(n, "key", "keys"))
        } else {
            format!("Per-LED lighting, {}", plural(n, "lamp", "lamps"))
        }
    };
    if color == Color::None && zones.is_none() {
        return None;
    }
    if color == Color::OnOff {
        return Some("On/off light".into());
    }
    let Some(z) = zones.filter(|z| !z.list.is_empty()) else {
        return Some(match color {
            Color::Single => "1 zone (whole device)".into(),
            Color::PerZone if count > 0 => plural(count, "lighting zone", "lighting zones"),
            Color::PerLed if count > 0 => per_led(count),
            _ => "Lighting zones unknown".into(),
        });
    };
    let total = z.list.len() as u32;
    if total == 1 {
        return Some("1 zone (whole device)".into());
    }
    if (keys && z.per_key()) || z.per_lamp() {
        return Some(per_led(total));
    }
    let live = z.live();
    let mut text = match (mirrored(&live), live.len()) {
        (Some(n), parts) => format!(
            "{} {}",
            plural(n, "lighting zone", "lighting zones"),
            per_part(parts)
        ),
        (None, 1) => plural(total, "lighting zone", "lighting zones"),
        (None, _) => {
            let counts: Vec<String> = live.iter().map(u32::to_string).collect();
            format!("{} lighting zones", counts.join(" + "))
        }
    };
    if let Some(how) = z.arrangement() {
        text.push_str(", ");
        text.push_str(how);
    }
    Some(text)
}

/// The zone drawing's muted footer line about lamps that light nothing: "15 more lamps per
/// half light nothing; Foxglove leaves them out." `None` when every lamp lights something.
pub fn dead_line(zones: &Zones) -> Option<String> {
    let total: u32 = zones.dead.iter().sum();
    if total == 0 {
        return None;
    }
    let (n, each) = match mirrored(&zones.dead) {
        Some(n) => (n, format!(" {}", per_part(zones.dead.len()))),
        None => (total, String::new()),
    };
    let (lamps, light) = if n == 1 {
        ("lamp", "lights")
    } else {
        ("lamps", "light")
    };
    Some(format!(
        "{n} more {lamps}{each} {light} nothing; Foxglove leaves them out."
    ))
}

/// Where the zones came from, in the words of docs/lighting-zones.md section 5: "From your
/// check", "From the board's profile · not checked on yours yet", "Reported by the device";
/// with the device's own different count appended ("· the keyboard reports 21 lamps per
/// half; 15 of them light nothing here"). Never a warning.
pub fn source_line(zones: &Zones, keys: bool) -> String {
    let mut text = match zones.source {
        ZoneSource::Check => "From your check".to_string(),
        ZoneSource::Profile { checked: true } => "From the board's profile".to_string(),
        ZoneSource::Profile { checked: false } => {
            "From the board's profile · not checked on yours yet".to_string()
        }
        ZoneSource::Catalog => "From Foxglove's device list".to_string(),
        ZoneSource::Reported => "Reported by the device".to_string(),
    };
    if let Some(reported) = &zones.reported {
        let what = if keys { "the keyboard" } else { "the device" };
        let live = zones.live();
        let dark: Vec<u32> = reported
            .iter()
            .enumerate()
            .map(|(i, &n)| n.saturating_sub(live.get(i).copied().unwrap_or(0)))
            .collect();
        let (count, each) = match mirrored(reported) {
            Some(n) => (n, format!(" {}", per_part(reported.len()))),
            None => (reported.iter().sum(), String::new()),
        };
        text.push_str(&format!(
            " · {what} reports {}{each}",
            plural(count, "lamp", "lamps")
        ));
        // Per part when the count is, else in all.
        let unlit = match mirrored(&dark) {
            Some(n) if !each.is_empty() => n,
            _ => dark.iter().sum(),
        };
        if unlit > 0 && unlit < count {
            text.push_str(&format!("; {unlit} of them light nothing here"));
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::super::Color;
    use super::*;

    /// A key in the grid's column `c` (6 columns of 3 rows, then 3 thumbs, as the Corne's
    /// drawing numbers them).
    fn grid(c: u16, r: u16) -> ZoneKey {
        ZoneKey {
            index: c * 3 + r,
            column: Some(c),
            row: Some(r),
        }
    }

    fn thumb(t: u16) -> ZoneKey {
        ZoneKey {
            index: 18 + t,
            column: None,
            row: None,
        }
    }

    /// The Eyelash Corne's profile map (#608): 6 column groups per half (inner three with a
    /// thumb), 15 lamps per half dead, both halves alike.
    fn corne(source: ZoneSource) -> Zones {
        let mut list = Vec::new();
        for part in 0..2u8 {
            for (i, c) in (0..6u16).rev().enumerate() {
                let mut keys: Vec<ZoneKey> = (0..3).map(|r| grid(c, r)).collect();
                if i < 3 {
                    keys.push(thumb(2 - i as u16));
                }
                list.push(Zone {
                    part,
                    keys,
                    area: None,
                });
            }
        }
        Zones {
            list,
            dead: vec![15, 15],
            source,
            reported: None,
        }
    }

    fn case() -> Zones {
        Zones {
            list: [
                "Diamond display",
                "Interior light",
                "Front bottom fan",
                "Front middle fan",
                "Front top fan",
            ]
            .iter()
            .map(|n| Zone {
                area: Some((*n).into()),
                ..Default::default()
            })
            .collect(),
            dead: vec![0],
            source: ZoneSource::Catalog,
            reported: None,
        }
    }

    #[test]
    fn the_corne_says_six_zones_per_half_by_column() {
        let z = corne(ZoneSource::Profile { checked: false });
        assert_eq!(z.live(), [6, 6]);
        assert_eq!(z.lamps(), [21, 21]);
        assert_eq!(z.arrangement(), Some("by column"));
        assert_eq!(
            summary(Color::PerZone, 0, Some(&z), true).unwrap(),
            "6 lighting zones per half, by column"
        );
        assert_eq!(
            dead_line(&z).unwrap(),
            "15 more lamps per half light nothing; Foxglove leaves them out."
        );
        assert_eq!(
            source_line(&z, true),
            "From the board's profile · not checked on yours yet"
        );
    }

    #[test]
    fn a_case_says_its_zone_count_and_no_arrangement() {
        let z = case();
        assert_eq!(z.arrangement(), None);
        assert_eq!(
            summary(Color::PerZone, 5, Some(&z), false).unwrap(),
            "5 lighting zones"
        );
        assert_eq!(dead_line(&z), None);
        assert_eq!(source_line(&z, false), "From Foxglove's device list");
    }

    #[test]
    fn each_kind_has_its_line() {
        assert_eq!(summary(Color::None, 0, None, false), None);
        assert_eq!(
            summary(Color::OnOff, 0, None, false).unwrap(),
            "On/off light"
        );
        assert_eq!(
            summary(Color::Single, 0, None, false).unwrap(),
            "1 zone (whole device)"
        );
        assert_eq!(
            summary(Color::PerZone, 0, None, false).unwrap(),
            "Lighting zones unknown"
        );
        assert_eq!(
            summary(Color::PerLed, 0, None, true).unwrap(),
            "Lighting zones unknown"
        );
        assert_eq!(
            summary(Color::PerLed, 24, None, false).unwrap(),
            "Per-LED lighting, 24 lamps"
        );
        // A LampArray's report: one zone per lamp, places not known by key.
        let bar = Zones::from_report(&[24]);
        assert_eq!(
            summary(Color::PerLed, 24, Some(&bar), false).unwrap(),
            "Per-LED lighting, 24 lamps"
        );
        assert_eq!(source_line(&bar, false), "Reported by the device");
        // A per-key board: one key per zone.
        let per_key = Zones {
            list: (0..61)
                .map(|i| Zone {
                    keys: vec![grid(i % 12, i / 12)],
                    ..Default::default()
                })
                .collect(),
            dead: vec![0],
            source: ZoneSource::Profile { checked: true },
            reported: None,
        };
        assert_eq!(
            summary(Color::PerZone, 0, Some(&per_key), true).unwrap(),
            "Per-key lighting, 61 keys"
        );
        // One zone is the whole device, whatever the colour kind says.
        let one = Zones::from_report(&[1]);
        assert_eq!(
            summary(Color::PerLed, 1, Some(&one), false).unwrap(),
            "1 zone (whole device)"
        );
    }

    #[test]
    fn halves_that_differ_say_both_counts_and_one_piece_never_says_half() {
        let mut z = corne(ZoneSource::Check);
        z.list.retain(|zone| {
            !(zone.part == 1 && zone.keys.len() == 3 && zone.keys[0].column == Some(0))
        });
        z.dead = vec![15, 16];
        assert_eq!(z.live(), [6, 5]);
        assert_eq!(
            summary(Color::PerZone, 0, Some(&z), true).unwrap(),
            "6 + 5 lighting zones, by column"
        );
        assert_eq!(
            dead_line(&z).unwrap(),
            "31 more lamps light nothing; Foxglove leaves them out."
        );
        let mut one = corne(ZoneSource::Check);
        one.list.retain(|zone| zone.part == 0);
        one.dead = vec![1];
        assert_eq!(
            summary(Color::PerZone, 0, Some(&one), true).unwrap(),
            "6 lighting zones, by column"
        );
        assert_eq!(
            dead_line(&one).unwrap(),
            "1 more lamp lights nothing; Foxglove leaves them out."
        );
    }

    #[test]
    fn rows_and_crossing_zones() {
        let rows = Zones {
            list: (0..3)
                .map(|r| Zone {
                    keys: (0..4).map(|c| grid(c, r)).collect(),
                    ..Default::default()
                })
                .collect(),
            dead: vec![0],
            source: ZoneSource::Check,
            reported: None,
        };
        assert_eq!(rows.arrangement(), Some("by row"));
        let crossing = Zones {
            list: vec![
                Zone {
                    keys: vec![grid(0, 0), grid(1, 1)],
                    ..Default::default()
                },
                Zone {
                    keys: vec![grid(2, 0)],
                    ..Default::default()
                },
            ],
            ..rows.clone()
        };
        assert_eq!(crossing.arrangement(), None);
        // A zone of thumbs only has no column: nothing is claimed.
        let thumbs = Zones {
            list: vec![
                Zone {
                    keys: vec![thumb(0)],
                    ..Default::default()
                },
                Zone {
                    keys: vec![grid(1, 0), grid(1, 1)],
                    ..Default::default()
                },
            ],
            ..rows
        };
        assert_eq!(thumbs.arrangement(), None);
    }

    #[test]
    fn check_beats_profile_beats_the_devices_report() {
        let device = Zones::from_report(&[21, 21]);
        let profile = corne(ZoneSource::Profile { checked: false });
        let mut check = corne(ZoneSource::Check);
        check.list.truncate(5);
        check.dead = vec![16, 21];

        let won = Zones::pick(
            Some(check.clone()),
            Some(profile.clone()),
            Some(device.clone()),
        )
        .unwrap();
        assert_eq!(won.source, ZoneSource::Check);
        assert_eq!(won.list.len(), 5);

        let won = Zones::pick(None, Some(profile.clone()), Some(device.clone())).unwrap();
        assert_eq!(won.source, ZoneSource::Profile { checked: false });
        // The report has the same 21 lamps per half as the profile's map: nothing to add.
        assert_eq!(won.reported, None);

        let won = Zones::pick(None, None, Some(device.clone())).unwrap();
        assert_eq!(won.source, ZoneSource::Reported);
        assert_eq!(
            summary(Color::PerLed, 42, Some(&won), true).unwrap(),
            "Per-key lighting, 42 keys"
        );

        assert_eq!(Zones::pick(None, None, None), None);

        // A profile listing only its live pixels, against a report of the whole strip.
        let mut short = corne(ZoneSource::Check);
        short.dead = vec![0, 0];
        let won = Zones::pick(Some(short), Some(profile), Some(device)).unwrap();
        assert_eq!(won.reported, Some(vec![21, 21]));
        assert_eq!(
            source_line(&won, true),
            "From your check · the keyboard reports 21 lamps per half; 15 of them light nothing here"
        );
    }
}
