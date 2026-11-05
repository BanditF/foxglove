//! The zone drawing (#611, docs/lighting-zones.md sections 2 and 3): **Show zones** after a
//! device's summary line opens it in place, **Hide zones** closes it. Closed by default and
//! remembered per device for the session only (egui's memory, never saved). It adds no page and
//! no dial.
//!
//! - A keyboard: its profile's drawing, each zone's keys in one of [`TINTS`] with a numbered dot
//!   on the zone's top key, both halves of a split ("Right half (mirrors the left)"), and a
//!   zone list for 2 to 12 zones. A per-key board shows no numbers and no list.
//! - Areas, not keys (a case): the zone names as a row of tinted tiles in zone order.
//! - A muted footer: the lamps that light nothing, how per-key looks land on zones, and "Not
//!   right? **Check zones…**" (with **Forget my check** while the user's own check wins).
//!
//! The hide rules: a device of one zone gets no link and no drawing; nothing here is greyed.
//! Everything is drawn from the zones record ([`Zones`]); no board or device is named in code.

use egui::{Color32, Rect, RichText, Sense, Stroke, Vec2};
use foxglove_devices::pc_lighting::{zones as words, LightCaps, Zone, ZoneKind, ZoneSource, Zones};

use super::led_map_ui::Geometry;
use super::light_zones::Line;
use super::parts::Parts;
use super::theme;

/// The zones' tints, in zone order (the 8th zone takes the first again).
pub const TINTS: [Color32; 7] = [
    Color32::from_rgb(0xf0, 0x8a, 0x3c),
    Color32::from_rgb(0x7f, 0xb0, 0xe0),
    Color32::from_rgb(0x5f, 0xc4, 0x8e),
    Color32::from_rgb(0xf0, 0xd2, 0x3c),
    Color32::from_rgb(0xe0, 0x7f, 0xa8),
    Color32::from_rgb(0xa9, 0x8b, 0xe0),
    Color32::from_rgb(0x4f, 0xc8, 0xc8),
];

/// The zone list shows for this many zones (per part) at most; more is a per-key device.
pub const MAX_LISTED: usize = 12;

/// The drawing's height, in points.
const HEIGHT: f32 = 170.0;

/// "Show zones" / "Hide zones".
pub const SHOW: &str = "Show zones";
pub const HIDE: &str = "Hide zones";
/// The probe's one name (section 4).
pub const CHECK: &str = "Check zones…";
pub const FORGET: &str = "Forget my check";

/// What the drawing asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ask {
    /// "Check zones…".
    Check,
    /// "Forget my check": back to the profile or the device's report.
    Forget,
}

fn open_id(device: &str) -> egui::Id {
    egui::Id::new(("zones-open", device))
}

#[cfg(test)]
thread_local! {
    /// The devices whose drawing the previews and tests show open.
    pub static PREVIEW_OPEN: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Whether `device`'s drawing is open (this session only). With egui's "everything is visible"
/// (the tests that look for every sentence, #794) it is open.
pub fn is_open(ctx: &egui::Context, device: &str) -> bool {
    if ctx.memory(|m| m.everything_is_visible()) {
        return true;
    }
    #[cfg(test)]
    if PREVIEW_OPEN.with_borrow(|open| open.iter().any(|d| d == device)) {
        return true;
    }
    ctx.data(|d| d.get_temp::<bool>(open_id(device)))
        .unwrap_or(false)
}

/// Opens or closes `device`'s drawing (the previews and tests open it).
pub fn set_open(ctx: &egui::Context, device: &str, open: bool) {
    ctx.data_mut(|d| d.insert_temp(open_id(device), open));
}

/// Whether a device of `kind` has a drawing: zones and per-key devices; one zone, on/off
/// lights and unknown zones don't (section 3).
pub fn drawable(kind: ZoneKind) -> bool {
    matches!(kind, ZoneKind::Zones | ZoneKind::PerLed)
}

/// The "Show zones" / "Hide zones" link after a summary; nothing for a device without a
/// drawing. Whether the drawing is open now.
pub fn toggle(ui: &mut egui::Ui, device: &str, kind: ZoneKind) -> bool {
    if !drawable(kind) {
        return false;
    }
    let open = is_open(ui.ctx(), device);
    if ui.link(if open { HIDE } else { SHOW }).clicked() {
        set_open(ui.ctx(), device, !open);
        return !open;
    }
    open
}

/// A zone in a few words, from its data alone: "column 6 + 1 thumb key", "row 2", "4 keys",
/// "Front top fan", "between keys" (a lamp the user placed under no key).
pub fn describe(zone: &Zone) -> String {
    if let Some(area) = &zone.area {
        return area.clone();
    }
    let plural =
        |n: usize, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
    let grid: Vec<_> = zone.keys.iter().filter(|k| k.column.is_some()).collect();
    let others = zone.keys.len() - grid.len();
    let mut out = Vec::new();
    if let Some(first) = grid.first() {
        if grid.len() > 1 && grid.iter().all(|k| k.column == first.column) {
            out.push(format!("column {}", first.column.unwrap_or(0) + 1));
        } else if grid.len() > 1 && grid.iter().all(|k| k.row == first.row) {
            out.push(format!("row {}", first.row.unwrap_or(0) + 1));
        } else {
            out.push(plural(grid.len(), "key", "keys"));
        }
    }
    if others > 0 {
        out.push(if grid.is_empty() {
            plural(others, "key", "keys")
        } else {
            plural(others, "thumb key", "thumb keys")
        });
    }
    if out.is_empty() {
        return "between keys".into();
    }
    out.join(" + ")
}

/// A zone's tint: by its place on its part, so mirrored halves match.
pub fn tint(i: usize) -> Color32 {
    TINTS[i % TINTS.len()]
}

/// The zones on part `p`.
fn on_part(z: &Zones, p: usize) -> Vec<&Zone> {
    z.list
        .iter()
        .filter(|zone| usize::from(zone.part) == p)
        .collect()
}

/// Every part lights the same keys in the same order (a split board's halves, as profiles
/// write them).
pub fn mirrored(z: &Zones) -> bool {
    let parts = z.parts();
    parts > 1 && {
        let keys = |p| -> Vec<Vec<u16>> {
            on_part(z, p)
                .iter()
                .map(|zone| zone.keys.iter().map(|k| k.index).collect())
                .collect()
        };
        let first = keys(0);
        (1..parts).all(|p| keys(p) == first) && z.dead.iter().all(|d| Some(d) == z.dead.first())
    }
}

/// The drawing's title on the Keyboard card.
pub const TITLE: &str = "Zones";

/// The keyboard's drawing under its summary line: a header (its title, with how per-key looks
/// land on zones on hover, and the source as a chip, #794 K7/K9), board, zone list and footer.
/// `check` says Check zones… can run on the connected keyboard.
pub fn keyboard(ui: &mut egui::Ui, line: &Line, check: bool) -> Option<Ask> {
    let z = line.zones.as_ref()?;
    ui.horizontal_wrapped(|ui| {
        let title = ui.label(theme::caps_label(ui, TITLE));
        if line.kind == ZoneKind::Zones {
            title.on_hover_text(words::ZONE_LOOKS);
        }
        if let Some(source) = &line.source {
            theme::hint_chip(ui, source, None, None);
        }
    });
    ui.add_space(4.0);
    let per_key = line.kind == ZoneKind::PerLed;
    board(ui, line, z, per_key);
    if !per_key {
        list(ui, z);
    }
    footer(ui, z, line.dead.as_deref(), line.kind, false, Some(check))
}

/// The board, top-down: each zone's keys tinted, a numbered dot on its top key (none on a
/// per-key board), keys that light nothing in outline, and the parts named on a split.
fn board(ui: &mut egui::Ui, line: &Line, z: &Zones, per_key: bool) {
    let p = theme::palette(ui);
    let halves = if z.parts() > 1 { 2u8 } else { 1 };
    let size = Vec2::new(ui.available_width().min(720.0), HEIGHT);
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 6.0, p.well);
    let mut within = rect.shrink(12.0);
    if halves == 2 {
        within.min.y += 16.0;
    }
    let g = Geometry::new(&line.drawing, halves, within);
    let parts = Parts::halves(halves);
    let same = mirrored(z);
    for a in 0..usize::from(halves) {
        let keys = g.keys(&line.drawing, a);
        let zones = on_part(z, a);
        let mut owner: Vec<Option<usize>> = vec![None; keys.len()];
        for (i, zone) in zones.iter().enumerate() {
            for k in &zone.keys {
                if let Some(o) = owner.get_mut(usize::from(k.index)) {
                    *o = Some(i);
                }
            }
        }
        let on_screen = |k: &Rect| {
            Rect::from_min_max(
                g.screen(a, [k.min.x, k.min.y]),
                g.screen(a, [k.max.x, k.max.y]),
            )
            .shrink(g.scale * 0.06)
        };
        for (k, owner) in keys.iter().zip(&owner) {
            let r = on_screen(k);
            match owner {
                Some(i) => {
                    let t = if per_key { p.accent } else { tint(*i) };
                    painter.rect(
                        r,
                        3.0,
                        t.gamma_multiply(0.45),
                        Stroke::new(1.0, t),
                        egui::StrokeKind::Inside,
                    );
                }
                None => {
                    painter.rect_stroke(r, 3.0, Stroke::new(1.0, p.line), egui::StrokeKind::Inside);
                }
            }
        }
        if !per_key {
            for (i, zone) in zones.iter().enumerate() {
                let top = zone
                    .keys
                    .iter()
                    .filter_map(|k| keys.get(usize::from(k.index)))
                    .min_by(|x, y| {
                        x.min
                            .y
                            .total_cmp(&y.min.y)
                            .then(x.min.x.total_cmp(&y.min.x))
                    });
                let Some(top) = top else { continue };
                let c = on_screen(top).center();
                painter.circle_filled(c, 8.0, tint(i));
                painter.text(
                    c,
                    egui::Align2::CENTER_CENTER,
                    (i + 1).to_string(),
                    egui::FontId::proportional(11.0),
                    Color32::from_rgb(0x1a, 0x1a, 0x1a),
                );
            }
        }
        if halves == 2 {
            let name = if a == 1 && same {
                format!(
                    "{} (mirrors the {})",
                    parts.name(1),
                    parts.short(0).to_lowercase()
                )
            } else {
                parts.name(a)
            };
            painter.text(
                g.rect(a).center_top() - Vec2::new(0.0, 10.0),
                egui::Align2::CENTER_BOTTOM,
                name,
                egui::FontId::proportional(12.0),
                p.muted,
            );
        }
    }
}

/// The zone list, one entry per zone ("1 column 6 + 1 thumb key"), for 2 to 12 zones. Mirrored
/// parts list once; others list each part under its name.
fn list(ui: &mut egui::Ui, z: &Zones) {
    let parts = z.parts();
    let shown = if mirrored(z) { 1 } else { parts };
    let names = Parts::halves(parts.min(255) as u8);
    for part in 0..shown {
        let zones = on_part(z, part);
        if !(2..=MAX_LISTED).contains(&zones.len()) {
            continue;
        }
        ui.add_space(4.0);
        if shown > 1 {
            ui.label(theme::caps_label(ui, &names.name(part)));
        }
        entries(ui, zones.iter().map(|zone| describe(zone)), true);
    }
}

/// Tinted entries in zone order, numbered or not.
fn entries(ui: &mut egui::Ui, names: impl Iterator<Item = String>, numbered: bool) {
    let p = theme::palette(ui);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(12.0, 4.0);
        for (i, name) in names.enumerate() {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 5.0;
                let (r, _) = ui.allocate_exact_size(egui::vec2(16.0, 16.0), Sense::hover());
                ui.painter().circle_filled(r.center(), 7.0, tint(i));
                if numbered {
                    ui.painter().text(
                        r.center(),
                        egui::Align2::CENTER_CENTER,
                        (i + 1).to_string(),
                        egui::FontId::proportional(10.0),
                        Color32::from_rgb(0x1a, 0x1a, 0x1a),
                    );
                }
                ui.label(RichText::new(name).size(12.5).color(p.text));
            });
        }
    });
}

/// A PC device's drawing under its summary line: the source line, its zones as tinted tiles
/// in zone order (named by the device, `names`, where the zones have no area of their own; for
/// up to 12 zones, numbered only for a zone device), and the footer.
pub fn device(ui: &mut egui::Ui, caps: &LightCaps, names: &[String]) {
    let p = theme::palette(ui);
    let kind = caps.kind(false);
    let Some(z) = caps.zones.as_ref() else {
        return;
    };
    ui.add(
        egui::Label::new(
            RichText::new(words::source_line(z, false))
                .color(p.muted)
                .size(12.5),
        )
        .wrap(),
    );
    if (2..=MAX_LISTED).contains(&z.list.len()) {
        ui.add_space(2.0);
        let tiles = z.list.iter().enumerate().map(|(i, zone)| {
            zone.area
                .clone()
                .or_else(|| names.get(i).cloned())
                .unwrap_or_else(|| format!("Zone {}", i + 1))
        });
        entries(ui, tiles, kind == ZoneKind::Zones);
    }
    footer(ui, z, words::dead_line(z).as_deref(), kind, true, None);
}

/// The muted footer, at most three lines: lamps that light nothing, how per-key looks land on
/// zones (zone devices only, with `looks`; the keyboard has it on its title's hover), and "Not
/// right? Check zones…" (`check`: `None` where the probe isn't offered, else whether it can run
/// now) with "Forget my check" while the user's own check wins.
fn footer(
    ui: &mut egui::Ui,
    z: &Zones,
    dead: Option<&str>,
    kind: ZoneKind,
    looks: bool,
    check: Option<bool>,
) -> Option<Ask> {
    let p = theme::palette(ui);
    let mut out = None;
    ui.add_space(4.0);
    let muted = |ui: &mut egui::Ui, text: &str| {
        ui.add(egui::Label::new(RichText::new(text).color(p.muted).size(12.5)).wrap());
    };
    if let Some(dead) = dead {
        muted(ui, dead);
    }
    if looks && kind == ZoneKind::Zones {
        muted(ui, words::ZONE_LOOKS);
    }
    if let Some(can) = check {
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("Not right?").color(p.muted).size(12.5));
            if check_link(ui, can).clicked() {
                out = Some(Ask::Check);
            }
            if z.source == ZoneSource::Check
                && ui
                    .link(FORGET)
                    .on_hover_text("Go back to the board's profile, or what the keyboard reports")
                    .clicked()
            {
                out = Some(Ask::Forget);
            }
        });
    }
    out
}

/// "Check zones…", enabled when it can run now.
pub fn check_link(ui: &mut egui::Ui, can: bool) -> egui::Response {
    let hint = if can {
        "Light one lamp at a time and click the keys it lights"
    } else {
        "Needs keyboard firmware that lights one lamp at a time, and a board profile"
    };
    ui.add_enabled(can, egui::Link::new(CHECK))
        .on_hover_text(hint)
        .on_disabled_hover_text(hint)
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use foxglove_devices::pc_lighting::ZoneKey;

    /// Every text a frame drew, a line each.
    pub fn collect(shape: &egui::Shape, out: &mut String) {
        match shape {
            egui::Shape::Text(t) => {
                out.push_str(t.galley.text());
                out.push('\n');
            }
            egui::Shape::Vec(v) => v.iter().for_each(|s| collect(s, out)),
            _ => {}
        }
    }

    fn key(
        index: u16,
        column: Option<u16>,
        row: Option<u16>,
    ) -> foxglove_devices::pc_lighting::ZoneKey {
        ZoneKey { index, column, row }
    }

    #[test]
    fn zones_are_described_from_their_keys() {
        let zone = |keys: Vec<ZoneKey>| Zone {
            keys,
            ..Default::default()
        };
        assert_eq!(
            describe(&zone(vec![
                key(15, Some(5), Some(0)),
                key(16, Some(5), Some(1)),
                key(17, Some(5), Some(2)),
                key(20, None, None)
            ])),
            "column 6 + 1 thumb key"
        );
        assert_eq!(
            describe(&zone(vec![
                key(0, Some(0), Some(1)),
                key(3, Some(1), Some(1))
            ])),
            "row 2"
        );
        assert_eq!(
            describe(&zone(vec![
                key(0, Some(0), Some(0)),
                key(4, Some(1), Some(1))
            ])),
            "2 keys"
        );
        assert_eq!(
            describe(&zone(vec![key(0, None, None), key(1, None, None)])),
            "2 keys"
        );
        assert_eq!(describe(&zone(vec![])), "between keys");
        assert_eq!(
            describe(&Zone {
                area: Some("Front top fan".into()),
                ..Default::default()
            }),
            "Front top fan"
        );
    }
}
