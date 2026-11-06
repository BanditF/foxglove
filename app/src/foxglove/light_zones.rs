//! The keyboard's real lighting resolution (#611, #608): its zones from the three sources, in
//! the user's order, and the Keyboard card's one line about them.
//!
//! - **Your check:** the store's `[led_map]` points (Find my LEDs, the dot test), each point
//!   taken as the profile's group it sits on, else the key under it.
//! - **The board's profile:** `[lighting] led_keys` (a key, a group of keys, or `[]` for a
//!   pixel that lights nothing), while scenes use it (shipped or checked).
//! - **The keyboard's report:** LAMP_PARTS (HELLO bit 22): how many lamps each part has, not
//!   which keys they light.
//!
//! The record and its words are `foxglove_devices::pc_lighting::zones`, which the case and
//! LampArray rows use too. Everything here is data: a board's zones come from its profile, no
//! board is named in code. Built when the Lights page draws, from caches; nothing is asked.

use foxglove_devices::pc_lighting::{
    zones, Color, Zone, ZoneHealth, ZoneKey, ZoneKind, ZoneSource, Zones,
};
use foxglove_link::lamps::LampParts;

use super::boards::{Board, Drawing, Lamp, LED_MAP_CHECKED};

/// The summary line's tooltip, first line (docs/lighting-zones.md section 7).
pub const TIP: &str =
    "How finely Foxglove can set this device's colours. Effects use one colour per zone.";

/// A point of the user's check this close to a profile group's centre (in key units) is that
/// group.
const NEAR: f32 = 0.5;

/// The Keyboard card's lighting line: the summary ("6 lighting zones per half, by column"),
/// its tooltip, and what the zone drawing shows under it (docs/lighting-zones.md sections 1
/// and 2).
#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    pub summary: String,
    /// The Keyboard card's chip (#794 K4/K5): "6 zones per half", "Zones unknown"; its hover is
    /// [`Line::chip_hover`].
    pub chip: String,
    /// The summary's tooltip: what it means (and, while the zones are unknown, what the
    /// keyboard reports).
    pub details: Vec<String>,
    /// What the hide rules go by.
    pub kind: ZoneKind,
    /// The zones the drawing shows; `None` while they're unknown.
    pub zones: Option<Zones>,
    /// The drawing's muted source line: "From the board's profile · not checked on yours yet".
    pub source: Option<String>,
    /// "15 more lamps per half light nothing; Foxglove leaves them out."
    pub dead: Option<String>,
    /// The board's drawing, one part (the second is its mirror image).
    pub drawing: Drawing,
}

/// A key of the drawing as a zone key: its column and row when the drawing is a grid and the
/// key is in it (thumbs and listed keys have neither).
fn key_of(drawing: &Drawing, index: u8) -> ZoneKey {
    let (columns, rows, _) = drawing.grid();
    let i = usize::from(index);
    let gridded = drawing.keys.is_empty() && i < columns * rows;
    ZoneKey {
        index: u16::from(index),
        column: gridded.then(|| (i / rows) as u16),
        row: gridded.then(|| (i % rows) as u16),
    }
}

fn zone(drawing: &Drawing, part: usize, keys: &[u8]) -> Zone {
    Zone {
        part: part as u8,
        keys: keys.iter().map(|&k| key_of(drawing, k)).collect(),
        area: None,
    }
}

/// The profile's map, part by part. `None` unless scenes use it (shipped or checked) and it
/// covers every part.
pub fn from_profile(board: &Board, drawing: &Drawing) -> Option<Zones> {
    let l = board.lighting.as_ref()?;
    if !l.led_map_usable() || l.led_keys.is_empty() {
        return None;
    }
    let parts = l.lamp_parts(board.halves)?;
    let mut out = Zones {
        list: Vec::new(),
        dead: vec![0; parts.len()],
        source: ZoneSource::Profile {
            checked: l.led_map == LED_MAP_CHECKED,
        },
        reported: None,
    };
    for p in 0..parts.len() {
        for lamp in l.part_keys(&parts, p)? {
            if lamp.dead() {
                out.dead[p] += 1;
            } else {
                out.list.push(zone(drawing, p, lamp.keys()));
            }
        }
    }
    Some(out)
}

/// The user's own check (the store's `[led_map]` points): each lamp they saw is the profile's
/// group it sits on (the dot test saves group centres), else the key under it, else a lamp
/// with no key (between keys: underglow). One they couldn't see lights nothing. `None` without
/// a saved check.
pub fn from_check(board: &Board, drawing: &Drawing) -> Option<Zones> {
    let l = board.lighting.as_ref()?;
    if l.led_points.is_empty() {
        return None;
    }
    let parts = l.lamp_parts(board.halves)?;
    let mut out = Zones {
        list: Vec::new(),
        dead: vec![0; parts.len()],
        source: ZoneSource::Check,
        reported: None,
    };
    for p in 0..parts.len() {
        // The second part is the first's mirror image, as the wizard draws it.
        let keys = drawing.keys(p == 1);
        let groups: &[Lamp] = l.part_keys(&parts, p).unwrap_or(&[]);
        for i in parts.range(p) {
            let Some([x, y]) = l.led_points.get(i).copied().flatten() else {
                out.dead[p] += 1;
                continue;
            };
            let at = egui::pos2(x, y);
            let group = groups
                .iter()
                .filter_map(|g| Some((g, g.center(&keys)?.distance(at))))
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .filter(|(_, d)| *d <= NEAR)
                .map(|(g, _)| g.keys().to_vec());
            let lit = group
                .or_else(|| {
                    let k = keys.iter().position(|r| r.contains(at))?;
                    Some(vec![u8::try_from(k).ok()?])
                })
                .unwrap_or_default();
            out.list.push(zone(drawing, p, &lit));
        }
    }
    Some(out)
}

/// The keyboard's zones: the user's check, else the profile's map, else the keyboard's own
/// lamp counts (`firmware`, LAMP_PARTS). `None` when none of them knows.
pub fn keyboard(
    board: Option<&Board>,
    drawing: &Drawing,
    firmware: Option<&LampParts>,
) -> Option<Zones> {
    let device = firmware.map(|p| {
        let counts: Vec<u32> = p.counts().map(|n| n as u32).collect();
        Zones::from_report(&counts)
    });
    Zones::pick(
        board.and_then(|b| from_check(b, drawing)),
        board.and_then(|b| from_profile(b, drawing)),
        device,
    )
}

/// "21 lamps per half", "61 lamps".
fn lamps_text(counts: &[u32]) -> String {
    let n: u32 = counts.iter().sum();
    let (n, each) = match counts {
        [a, b] if a == b => (*a, " per half"),
        _ => (n, ""),
    };
    format!("{n} {}{each}", if n == 1 { "lamp" } else { "lamps" })
}

/// The Keyboard card's line for `zones`, on `drawing`. Lamp counts alone (the keyboard's
/// report) say how many lamps there are, not which keys each lights, so the zones are unknown
/// then.
pub fn line(found: Option<&Zones>, drawing: &Drawing) -> Line {
    let mut out = Line {
        summary: "Lighting zones unknown".to_string(),
        chip: "Zones unknown".to_string(),
        details: vec![TIP.to_string()],
        kind: ZoneKind::Unknown,
        zones: None,
        source: None,
        dead: None,
        drawing: drawing.clone(),
    };
    match found {
        Some(z) if z.source != ZoneSource::Reported => {
            if let Some(summary) = zones::summary(Color::PerZone, 0, Some(z), true) {
                out.summary = summary;
            }
            out.kind = zones::kind(Color::PerZone, 0, Some(z), true);
            // A few zones in the Scenes page's words; per key or one zone, the summary.
            out.chip = match out.kind {
                ZoneKind::Zones => zones::scene_words(Color::PerZone, 0, Some(z), true),
                _ => None,
            }
            .unwrap_or_else(|| out.summary.clone());
            out.source = Some(zones::source_line(z, true));
            out.dead = zones::dead_line(z);
            out.zones = Some(z.clone());
        }
        Some(z) => out.details.push(format!(
            "The keyboard reports {}; which keys each one lights isn't known yet.",
            lamps_text(&z.lamps())
        )),
        None => {}
    }
    out
}

impl Line {
    /// The chip's hover: the summary, then what it means (every word of the old line, #794).
    pub fn chip_hover(&self) -> String {
        std::iter::once(self.summary.as_str())
            .chain(self.details.iter().map(String::as_str))
            .collect::<Vec<_>>()
            .join(
                "
",
            )
    }
}

/// The connected keyboard's line, from the caches: its profile (with the user's check applied),
/// its drawing and the lamp counts its firmware reported when it connected. `None` with no
/// keyboard, or for one Foxglove knows no lights of (no `[lighting]`, no report).
pub fn current() -> Option<Line> {
    super::devices::keyboard_id()?;
    let board = super::boards::current();
    let firmware = super::devices::firmware_lamp_parts();
    if board.as_ref().and_then(|b| b.lighting.as_ref()).is_none() && firmware.is_none() {
        return None;
    }
    let drawing = super::boards::drawing(board.as_ref());
    Some(line(
        keyboard(board.as_ref(), &drawing, firmware.as_ref()).as_ref(),
        &drawing,
    ))
}

/// The Scenes page's words for the connected keyboard (#611): "6 zones per half", "a colour
/// per key"; `None` while its zones are unknown.
pub fn scene_words() -> Option<String> {
    let line = current()?;
    zones::scene_words(Color::PerZone, 0, line.zones.as_ref(), true)
}

/// The keyboard's lighting-zones health row (#611): the zones and lamps found, the source, and
/// a warning when the firmware's report (LAMP_PARTS) and the profile (or the user's check)
/// disagree. Logged once when a keyboard connects (`devices::learn_lamp_parts`); the health
/// view takes the same row. A PC lighting device's row is `zones::health` on its caps.
/// Named by its profile, or "Keyboard".
pub fn keyboard_health(
    board: Option<&Board>,
    drawing: &Drawing,
    firmware: Option<&LampParts>,
) -> ZoneHealth {
    let name = board.map_or("Keyboard", |b| b.name.as_str());
    zones::health(name, keyboard(board, drawing, firmware).as_ref(), true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn corne() -> Board {
        Board::parse(include_str!("../../resources/boards/eyelash-corne.toml")).unwrap()
    }

    fn drawing(b: &Board) -> Drawing {
        b.drawing.clone().unwrap_or_default()
    }

    /// #608: the Corne's profile is 6 column groups per half (the inner three with a thumb),
    /// and 15 lamps per half that light nothing.
    #[test]
    fn the_corne_profile_is_six_zones_per_half_by_column() {
        let b = corne();
        let d = drawing(&b);
        let z = from_profile(&b, &d).unwrap();
        assert_eq!(z.live(), [6, 6]);
        assert_eq!(z.dead, [15, 15]);
        assert_eq!(z.source, ZoneSource::Profile { checked: false });
        // Pixel 0: the inner column (T G B: keys 15-17, column 5) and Space (thumb 20).
        assert_eq!(z.list[0].keys.len(), 4);
        assert!(z.list[0].keys[..3].iter().all(|k| k.column == Some(5)));
        assert_eq!(z.list[0].keys[3].column, None);
        let l = line(Some(&z), &d);
        assert_eq!(l.summary, "6 lighting zones per half, by column");
        assert_eq!(l.details, [TIP]);
        assert_eq!(l.kind, ZoneKind::Zones);
        assert_eq!(
            l.source.as_deref(),
            Some("From the board's profile · not checked on yours yet")
        );
        assert_eq!(
            l.dead.as_deref(),
            Some("15 more lamps per half light nothing; Foxglove leaves them out.")
        );
    }

    /// The firmware's LAMP_PARTS says 21 per half, as the profile's strip: the profile wins
    /// with nothing to add. A keyboard known only by its report has unknown zones.
    #[test]
    fn the_profile_beats_the_keyboards_report() {
        let b = corne();
        let d = drawing(&b);
        let report = LampParts::new(&[21, 21]).unwrap();
        let z = keyboard(Some(&b), &d, Some(&report)).unwrap();
        assert_eq!(z.source, ZoneSource::Profile { checked: false });
        assert_eq!(z.reported, None);
        let only = keyboard(None, &d, Some(&report)).unwrap();
        assert_eq!(only.source, ZoneSource::Reported);
        let l = line(Some(&only), &d);
        assert_eq!(l.summary, "Lighting zones unknown");
        assert_eq!((l.kind, l.zones.is_none()), (ZoneKind::Unknown, true));
        assert_eq!(
            l.details[1],
            "The keyboard reports 21 lamps per half; which keys each one lights isn't known yet."
        );
        assert_eq!(keyboard(None, &d, None), None);
        assert_eq!(line(None, &d).summary, "Lighting zones unknown");
        // A provisional map isn't used by scenes: not a source.
        let mut provisional = corne();
        provisional.lighting.as_mut().unwrap().led_map = "provisional".into();
        assert_eq!(from_profile(&provisional, &d), None);
    }

    /// The dot test saves each live lamp's centre (#608): read back, each point is its group
    /// again, the unseen pixels light nothing, and the line says it's the user's check.
    #[test]
    fn the_users_check_wins_and_points_snap_to_their_groups() {
        let mut b = corne();
        let d = drawing(&b);
        let l = b.lighting.as_mut().unwrap();
        let parts = l.lamp_parts(2).unwrap();
        let mut points = vec![None; parts.total()];
        for p in 0..2 {
            let keys = d.keys(p == 1);
            for (i, lamp) in l.part_keys(&parts, p).unwrap().iter().enumerate() {
                points[parts.range(p).start + i] = lamp.center(&keys).map(|c| [c.x, c.y]);
            }
        }
        l.led_points = points;
        l.led_map = LED_MAP_CHECKED.into();
        let report = LampParts::new(&[21, 21]).unwrap();
        let z = keyboard(Some(&b), &d, Some(&report)).unwrap();
        assert_eq!(z.source, ZoneSource::Check);
        assert_eq!(z.live(), [6, 6]);
        assert_eq!(
            z.list[0].keys.len(),
            4,
            "the whole group, not the key under its centre"
        );
        let l = line(Some(&z), &d);
        assert_eq!(l.summary, "6 lighting zones per half, by column");
        assert_eq!(l.source.as_deref(), Some("From your check"));
    }

    /// The health signal fires (#611): the Corne's firmware agreeing with its profile is OK;
    /// firmware reporting another strip length than the profile warns, naming both.
    #[test]
    fn keyboard_health_warns_when_firmware_and_profile_disagree() {
        use foxglove_devices::pc_lighting::HealthLevel;
        let b = corne();
        let d = drawing(&b);
        let same = LampParts::new(&[21, 21]).unwrap();
        let ok = keyboard_health(Some(&b), &d, Some(&same));
        assert_eq!(
            (ok.device.as_str(), ok.zones, ok.lamps, ok.source, ok.level),
            ("Eyelash Corne", 12, 42, "profile", HealthLevel::Ok)
        );
        let other = LampParts::new(&[7, 7]).unwrap();
        let warn = keyboard_health(Some(&b), &d, Some(&other));
        assert_eq!(warn.level, HealthLevel::Warn);
        assert_eq!(
            warn.note.as_deref(),
            Some("firmware reports 7 + 7 lamps; the profile has 21 + 21")
        );
        let fw = keyboard_health(None, &d, Some(&same));
        assert_eq!((fw.device.as_str(), fw.source), ("Keyboard", "firmware"));
        assert_eq!(keyboard_health(None, &d, None).level, HealthLevel::Unknown);
    }

    /// A per-key board (the second desk's one-piece Plank 3): one key per LED, never "half".
    #[test]
    fn a_per_key_one_piece_board_says_per_key() {
        let b = Board::parse(crate::foxglove::fixtures::second_desk::PLANK3).unwrap();
        let d = drawing(&b);
        let z = keyboard(Some(&b), &d, None).unwrap();
        let l = line(Some(&z), &d);
        assert_eq!(l.summary, "Per-key lighting, 61 keys");
        assert_eq!(l.kind, ZoneKind::PerLed);
        assert!(!l.source.unwrap().contains("half"));
    }
}
