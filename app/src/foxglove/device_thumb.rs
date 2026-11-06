//! Low-fi device thumbnails (#803; docs/design/device-focus-layouts-thumbnails-lights-declutter.md
//! section 3): a small drawing of a device from what it is and where its lights are, tinted with
//! the colours it shows now. D2 (#804) puts them on the Other devices rows, the battery list and
//! the focus switcher.
//!
//! **Shapes come from data, never from a product.** A keyboard (anything with a key layout) is
//! its own [`KeyLayout`], shrunk: both halves for a split. Anything else takes the first of, in
//! order: the catalog's `shape`, the `LampArrayKind` Windows reports, the catalog's `kind`, the
//! Bluetooth category; then its lamps where Windows places them, then "unknown" (a dashed
//! outline with a "?"). No device has a drawing of its own.
//!
//! **Tints** ([`Tint`]) are the colours the device shows now, through its calibration
//! ([`Tint::through`], `calibrate::on_screen`): dark when off, a grey outline and no tint when
//! Foxglove can't read or drive its lights.
//!
//! [`Thumb::marks`] is a pure function of the data, the box and the palette (the tests read it);
//! [`paint`] hands its marks to an egui painter. Nothing here asks a device anything.

// D2 (#804) places the thumbnails; until then only the tests and previews draw them.
#![cfg_attr(not(test), allow(dead_code))]

use egui::{Color32, Pos2, Rect, Stroke, Vec2};
use foxglove_devices::pc_lighting::{Control, DeviceKind, Shape as CatalogShape, ZoneRole};

use super::key_layout::KeyLayout;
use super::pc_lights::{LampLayout, Row};
use super::theme::Palette;

/// The size on an Other devices row, in points.
pub const ROW: Vec2 = Vec2::new(56.0, 38.0);
/// The size in the battery list.
pub const SMALL: Vec2 = Vec2::new(44.0, 30.0);

/// What a thumbnail draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    /// Its key layout, or a plain board when none is known.
    Keyboard,
    /// A keyboard in two parts: both, as in its layout.
    Split,
    /// An outline with a ring per fan zone and a bar per strip zone.
    Case,
    /// One ring.
    Cooler,
    /// A bar with a dot per LED (at most [`STRIP_DOTS`], sampled).
    Strip,
    Mouse,
    Headset,
    Controller,
    /// A dot per lamp where Windows places it, inside its bounding box.
    Lamps,
    /// A dashed outline with "?".
    Unknown,
}

impl Shape {
    pub const ALL: [Shape; 10] = [
        Shape::Keyboard,
        Shape::Split,
        Shape::Case,
        Shape::Cooler,
        Shape::Strip,
        Shape::Mouse,
        Shape::Headset,
        Shape::Controller,
        Shape::Lamps,
        Shape::Unknown,
    ];

    /// What it is, in a word or two (hover text, previews).
    pub fn label(self) -> &'static str {
        match self {
            Shape::Keyboard => "Keyboard",
            Shape::Split => "Split keyboard",
            Shape::Case => "Case",
            Shape::Cooler => "Cooler",
            Shape::Strip => "Light strip",
            Shape::Mouse => "Mouse",
            Shape::Headset => "Headset",
            Shape::Controller => "Controller",
            Shape::Lamps => "Lights",
            Shape::Unknown => "Unknown device",
        }
    }
}

/// The most dots a strip shows; more LEDs are sampled.
pub const STRIP_DOTS: usize = 12;
/// The most lamps a thumbnail places; more are sampled.
const MAX_LAMPS: usize = 256;

/// Windows' Bluetooth device category, for a device known only from Bluetooth (a battery row).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BtCategory {
    Keyboard,
    Mouse,
    Headset,
    Gamepad,
    Other,
}

/// `LampArrayKind`, as Windows numbers it.
mod lamp_kind {
    pub const KEYBOARD: u32 = 1;
    pub const MOUSE: u32 = 2;
    pub const GAME_CONTROLLER: u32 = 3;
    pub const SCENE: u32 = 5;
    pub const CHASSIS: u32 = 7;
    pub const HEADSET: u32 = 11;
}

/// The colours a device shows now.
#[derive(Clone, Debug, PartialEq)]
pub enum Tint {
    /// Lit: its colours, one per zone or lamp in its own order, already through its calibration.
    /// Black is a zone that is off. Spread over the drawing's parts when the counts differ.
    Lit(Vec<Color32>),
    /// Every light off: drawn dark.
    Off,
    /// Foxglove can't read or drive its lights: a grey outline, no tint.
    Unknown,
}

impl Tint {
    /// Colours Foxglove holds for device `key` (by zone id, `None` for an unset zone), through
    /// the device's calibration as the screen shows it. Unset zones are drawn off; all of them
    /// off (or none known) is [`Tint::Off`].
    pub fn through(key: &str, zones: &[(Option<&str>, Option<[u8; 3]>)]) -> Tint {
        let lit: Vec<Color32> = zones
            .iter()
            .map(|(zone, c)| {
                let [r, g, b] = c.map_or([0; 3], |c| super::calibrate::on_screen(key, *zone, c));
                Color32::from_rgb(r, g, b)
            })
            .collect();
        Tint::lit(lit)
    }

    /// `colours` as a tint: [`Tint::Off`] when none is lit.
    pub fn lit(colours: Vec<Color32>) -> Tint {
        if colours.iter().any(|c| !is_black(*c)) {
            Tint::Lit(colours)
        } else {
            Tint::Off
        }
    }

    /// A Lights page row's tint: unknown when Foxglove can't change its lights (recognised, no
    /// driver), or when it holds no colour for any of its zones; else its zones' colours.
    pub fn of_row(row: &Row) -> Tint {
        if row.control == Control::NotYet || row.zones.iter().all(|(_, c)| c.is_none()) {
            return Tint::Unknown;
        }
        let zones: Vec<(Option<&str>, Option<[u8; 3]>)> = row
            .zones
            .iter()
            .enumerate()
            .map(|(i, (_, c))| (row.zone_ids.get(i).map(String::as_str), *c))
            .collect();
        Tint::through(&row.key, &zones)
    }

    /// Part `i` of `n`'s colour: `None` when unknown, black when off.
    fn colour(&self, i: usize, n: usize) -> Option<Color32> {
        match self {
            Tint::Lit(cs) if !cs.is_empty() => {
                Some(cs[(i * cs.len() / n.max(1)).min(cs.len() - 1)])
            }
            Tint::Lit(_) | Tint::Off => Some(Color32::BLACK),
            Tint::Unknown => None,
        }
    }

    /// One colour for a one-light drawing (a cooler's ring): the brightest of its colours.
    fn one(&self) -> Option<Color32> {
        match self {
            Tint::Lit(cs) => Some(
                cs.iter()
                    .copied()
                    .max_by_key(|c| u16::from(c.r()) + u16::from(c.g()) + u16::from(c.b()))
                    .unwrap_or(Color32::BLACK),
            ),
            Tint::Off => Some(Color32::BLACK),
            Tint::Unknown => None,
        }
    }
}

fn is_black(c: Color32) -> bool {
    c.r() == 0 && c.g() == 0 && c.b() == 0
}

/// A zone of a device's front, in millimetres from its top-left corner.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Zone {
    pub at: Option<[f32; 2]>,
    pub ambient: bool,
}

/// Everything a thumbnail is drawn from.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Thumb {
    /// The keyboard's layout: a device with one is a keyboard.
    pub keys: Option<KeyLayout>,
    /// The catalog's `shape`.
    pub catalog_shape: Option<CatalogShape>,
    /// The `LampArrayKind` Windows reports.
    pub lamp_kind: Option<u32>,
    /// The catalog's `kind`.
    pub catalog_kind: Option<DeviceKind>,
    pub bt: Option<BtCategory>,
    /// The front's size in millimetres (the catalog's `size`, or the LampArray's bounding box).
    pub size_mm: Option<[f32; 2]>,
    /// Its zones, in zone order.
    pub zones: Vec<Zone>,
    /// Each lamp where Windows places it (millimetres in the bounding box); empty when Windows
    /// gives no positions.
    pub lamps: Vec<[f32; 2]>,
    pub tint: Option<Tint>,
}

impl Thumb {
    /// A keyboard: its own layout, from `key_layout::current()` or `key_layout::of(board)`.
    pub fn keyboard(layout: KeyLayout, tint: Tint) -> Thumb {
        Thumb {
            keys: Some(layout),
            tint: Some(tint),
            ..Default::default()
        }
    }

    /// A Lights page row: its catalog shape and kind, its zones, and for a LampArray the kind and
    /// lamps Windows reports (`pc_lights::lamp_layouts()`), tinted as [`Tint::of_row`].
    pub fn of_row(row: &Row, lamps: Option<&LampLayout>) -> Thumb {
        let positioned = lamps.filter(|l| l.lamps.iter().any(|p| p[0] != 0.0 || p[1] != 0.0));
        let size_mm = row
            .size
            .map(|[w, h]| [f32::from(w), f32::from(h)])
            .or_else(|| {
                lamps
                    .map(|l| l.size_mm)
                    .filter(|s| s[0] > 0.0 && s[1] > 0.0)
            });
        Thumb {
            keys: None,
            catalog_shape: row.shape,
            lamp_kind: lamps.map(|l| l.kind),
            catalog_kind: Some(row.kind),
            bt: None,
            size_mm,
            zones: row
                .zone_positions
                .iter()
                .enumerate()
                .map(|(i, at)| Zone {
                    at: at.map(|[x, y]| [f32::from(x), f32::from(y)]),
                    ambient: row.zone_roles.get(i) == Some(&ZoneRole::Ambient),
                })
                .collect(),
            lamps: positioned.map(|l| l.lamps.clone()).unwrap_or_default(),
            tint: Some(Tint::of_row(row)),
        }
    }

    /// A device known only from Bluetooth (a battery row): its category, lights unknown.
    pub fn bluetooth(category: BtCategory) -> Thumb {
        Thumb {
            bt: Some(category),
            tint: Some(Tint::Unknown),
            ..Default::default()
        }
    }

    fn tint(&self) -> &Tint {
        self.tint.as_ref().unwrap_or(&Tint::Unknown)
    }

    /// The shape, picked from the data in order: a key layout, the catalog's `shape`, the
    /// LampArray kind, the catalog's `kind`, the Bluetooth category, lamp positions, unknown.
    pub fn shape(&self) -> Shape {
        if let Some(k) = &self.keys {
            return if k.parts.iter().filter(|p| !p.is_empty()).count() > 1 {
                Shape::Split
            } else {
                Shape::Keyboard
            };
        }
        let from_catalog = |s: CatalogShape| match s {
            CatalogShape::Keyboard => Shape::Keyboard,
            CatalogShape::Case => Shape::Case,
            CatalogShape::Cooler => Shape::Cooler,
            CatalogShape::Strip => Shape::Strip,
            CatalogShape::Mouse => Shape::Mouse,
            CatalogShape::Headset => Shape::Headset,
            CatalogShape::Controller => Shape::Controller,
            CatalogShape::Lamps => Shape::Lamps,
        };
        let from_lamp_kind = |k: u32| match k {
            lamp_kind::KEYBOARD => Some(Shape::Keyboard),
            lamp_kind::MOUSE => Some(Shape::Mouse),
            lamp_kind::GAME_CONTROLLER => Some(Shape::Controller),
            lamp_kind::CHASSIS => Some(Shape::Case),
            lamp_kind::HEADSET => Some(Shape::Headset),
            lamp_kind::SCENE if in_a_row(&self.lamps) => Some(Shape::Strip),
            _ => None,
        };
        let from_kind = |k: DeviceKind| match k {
            DeviceKind::Case => Some(Shape::Case),
            DeviceKind::Cooler => Some(Shape::Cooler),
            DeviceKind::Strip => Some(Shape::Strip),
            DeviceKind::Peripheral | DeviceKind::Other => None,
        };
        let from_bt = |c: BtCategory| match c {
            BtCategory::Keyboard => Some(Shape::Keyboard),
            BtCategory::Mouse => Some(Shape::Mouse),
            BtCategory::Headset => Some(Shape::Headset),
            BtCategory::Gamepad => Some(Shape::Controller),
            BtCategory::Other => None,
        };
        self.catalog_shape
            .map(from_catalog)
            .or_else(|| self.lamp_kind.and_then(from_lamp_kind))
            .or_else(|| self.catalog_kind.and_then(from_kind))
            .or_else(|| self.bt.and_then(from_bt))
            .or_else(|| (!self.lamps.is_empty()).then_some(Shape::Lamps))
            .unwrap_or(Shape::Unknown)
    }

    /// How many lights the drawing tints: its lamps, else its zones, else its colours.
    fn lights(&self) -> usize {
        let colours = match self.tint() {
            Tint::Lit(cs) => cs.len(),
            _ => 0,
        };
        [self.lamps.len(), self.zones.len(), colours]
            .into_iter()
            .find(|n| *n > 0)
            .unwrap_or(0)
    }

    /// The thumbnail in `rect` as marks to paint, in `p`'s colours.
    pub fn marks(&self, rect: Rect, p: &Palette) -> Vec<Mark> {
        let inner = rect.shrink(2.0);
        let s = Style::new(self.tint(), p);
        let mut out = Vec::new();
        match self.shape() {
            Shape::Keyboard | Shape::Split => self.keyboard_marks(inner, &s, &mut out),
            Shape::Case => self.case_marks(inner, &s, &mut out),
            Shape::Cooler => {
                let r = inner.width().min(inner.height()) * 0.38;
                let c = inner.center();
                out.push(Mark::Circle {
                    centre: c,
                    radius: r * 1.2,
                    fill: s.body_fill,
                    stroke: s.body_stroke,
                });
                out.push(Mark::Circle {
                    centre: c,
                    radius: r * 0.8,
                    fill: Color32::TRANSPARENT,
                    stroke: s.ring(self.tint().one(), r * 0.3),
                });
            }
            Shape::Strip => self.strip_marks(inner, &s, &mut out),
            Shape::Mouse | Shape::Headset | Shape::Controller => {
                self.glyph_marks(self.shape(), inner, &s, &mut out)
            }
            Shape::Lamps => {
                let size = self.lamp_box();
                let area = fit(inner, size);
                out.push(Mark::Rect {
                    rect: area,
                    rounding: 2.0,
                    fill: Color32::TRANSPARENT,
                    stroke: s.body_stroke,
                });
                self.lamp_dots(area, size, &s, &mut out);
            }
            Shape::Unknown => {
                let area = Rect::from_center_size(
                    inner.center(),
                    Vec2::new(inner.width().min(inner.height() * 1.3), inner.height()),
                );
                out.push(Mark::Dashed {
                    rect: area,
                    color: p.muted,
                });
                out.push(Mark::Text {
                    at: area.center(),
                    text: "?",
                    size: area.height() * 0.5,
                    color: p.muted,
                });
            }
        }
        out
    }

    /// The keyboard's own keys, shrunk to fit (both halves side by side on a split); a plain
    /// board when no layout is known; its lamps as keys for a LampArray keyboard with positions.
    fn keyboard_marks(&self, inner: Rect, s: &Style, out: &mut Vec<Mark>) {
        let tint = self.tint();
        let parts: Vec<Vec<[Pos2; 4]>> = match &self.keys {
            Some(k) if k.parts.iter().any(|p| !p.is_empty()) => k
                .parts
                .iter()
                .filter(|p| !p.is_empty())
                .map(|p| {
                    p.iter()
                        .map(|key| {
                            let small = super::key_layout::LayoutKey::new(
                                key.rect.shrink(0.08),
                                key.r,
                                key.index,
                            );
                            small.corners()
                        })
                        .collect()
                })
                .collect(),
            _ if !self.lamps.is_empty() => {
                let size = self.lamp_box();
                let area = fit(inner, size);
                out.push(Mark::Rect {
                    rect: area,
                    rounding: 2.0,
                    fill: s.body_fill,
                    stroke: s.body_stroke,
                });
                let n = self.lamps.len().min(MAX_LAMPS);
                let side = (area.width() / 24.0).clamp(1.5, 3.0);
                for i in 0..n {
                    let at = to_box(area, size, self.lamps[i * self.lamps.len() / n]);
                    let (fill, stroke) = s.part(tint.colour(i, n));
                    out.push(Mark::Rect {
                        rect: Rect::from_center_size(at, Vec2::splat(side)),
                        rounding: 0.5,
                        fill,
                        stroke,
                    });
                }
                return;
            }
            _ => vec![plain_board()],
        };
        // Each part from its own top-left, side by side with a gap of 0.6 keys.
        const GAP: f32 = 0.6;
        let mut placed: Vec<Vec<[Pos2; 4]>> = Vec::new();
        let mut x = 0.0;
        let mut height: f32 = 0.0;
        for part in &parts {
            let outline = part
                .iter()
                .flatten()
                .fold(Rect::NOTHING, |a, p| a.union(Rect::from_pos(*p)));
            let shift = Vec2::new(x, 0.0) - outline.min.to_vec2();
            placed.push(part.iter().map(|q| q.map(|p| p + shift)).collect());
            x += outline.width() + GAP;
            height = height.max(outline.height());
        }
        let size = Vec2::new((x - GAP).max(1.0), height.max(1.0));
        let area = fit(inner, size);
        let scale = area.width() / size.x;
        let n: usize = placed.iter().map(Vec::len).sum();
        let mut i = 0;
        for part in placed {
            for quad in part {
                let points = quad.map(|p| area.min + p.to_vec2() * scale).to_vec();
                let (fill, stroke) = s.part(tint.colour(i, n));
                out.push(Mark::Poly {
                    points,
                    fill,
                    stroke,
                });
                i += 1;
            }
        }
    }

    /// A case's front: its outline, its interior tinted by an ambient zone, a ring per point
    /// zone and a bar for a point zone at its top or bottom edge (a strip along it). No zones:
    /// its lamps where Windows places them, else three fan rings.
    fn case_marks(&self, inner: Rect, s: &Style, out: &mut Vec<Mark>) {
        let size = self
            .size_mm
            .map_or(Vec2::new(205.0, 475.0), |[w, h]| Vec2::new(w, h));
        let area = fit(inner, size);
        let tint = self.tint();
        let n = self.zones.len();
        let ambient = self
            .zones
            .iter()
            .enumerate()
            .filter(|(_, z)| z.ambient)
            .find_map(|(i, _)| tint.colour(i, n))
            .filter(|c| !is_black(*c));
        out.push(Mark::Rect {
            rect: area,
            rounding: 2.0,
            fill: ambient.map_or(s.body_fill, |c| c.gamma_multiply(0.35)),
            stroke: s.body_stroke,
        });
        if n == 0 && !self.lamps.is_empty() {
            self.lamp_dots(area, self.lamp_box(), s, out);
            return;
        }
        let points: Vec<usize> = (0..n).filter(|i| !self.zones[*i].ambient).collect();
        let rings = points.len().max(if n == 0 { 3 } else { 0 });
        let radius = (area.width() * 0.3).min(area.height() / (2.4 * rings.max(1) as f32));
        for k in 0..rings {
            let i = points.get(k).copied();
            let colour = match i {
                Some(i) => tint.colour(i, n),
                None => tint.colour(k, rings),
            };
            // Where the catalog places it, else spread top to bottom.
            let at = i.and_then(|i| self.zones[i].at).map_or_else(
                || {
                    Pos2::new(
                        area.center().x,
                        area.top() + area.height() * (k as f32 + 0.5) / rings as f32,
                    )
                },
                |at| to_box(area, size, at),
            );
            let edge = area.height() * 0.08;
            if i.is_some() && (at.y < area.top() + edge || at.y > area.bottom() - edge) {
                let bar = Rect::from_center_size(
                    Pos2::new(
                        area.center().x,
                        at.y.clamp(area.top() + 2.0, area.bottom() - 2.0),
                    ),
                    Vec2::new(area.width() * 0.7, 1.6),
                );
                let (fill, stroke) = s.part(colour);
                out.push(Mark::Rect {
                    rect: bar,
                    rounding: 0.8,
                    fill,
                    stroke,
                });
            } else {
                out.push(Mark::Circle {
                    centre: at,
                    radius,
                    fill: Color32::TRANSPARENT,
                    stroke: s.ring(colour, (radius * 0.35).clamp(0.8, 2.0)),
                });
            }
        }
    }

    /// A bar with a dot per LED: its lamps (or zones, or colours), at most [`STRIP_DOTS`].
    fn strip_marks(&self, inner: Rect, s: &Style, out: &mut Vec<Mark>) {
        let bar = Rect::from_center_size(
            inner.center(),
            Vec2::new(inner.width(), (inner.height() * 0.28).clamp(5.0, 9.0)),
        );
        out.push(Mark::Rect {
            rect: bar,
            rounding: bar.height() / 2.0,
            fill: s.body_fill,
            stroke: s.body_stroke,
        });
        let lights = self.lights();
        let dots = if lights == 0 {
            8
        } else {
            lights.min(STRIP_DOTS)
        };
        let step = (bar.width() - bar.height()) / dots as f32;
        let r = (bar.height() * 0.3).min(step * 0.4);
        for k in 0..dots {
            let at = Pos2::new(
                bar.left() + bar.height() / 2.0 + step * (k as f32 + 0.5),
                bar.center().y,
            );
            // Sampled: dot k shows light k * lights / dots.
            let (fill, stroke) = s.part(self.tint().colour(k, dots));
            out.push(Mark::Circle {
                centre: at,
                radius: r,
                fill,
                stroke,
            });
        }
    }

    /// A mouse, a headset or a controller: a class glyph with a tint dot per lamp, where Windows
    /// places them when it does.
    fn glyph_marks(&self, shape: Shape, inner: Rect, s: &Style, out: &mut Vec<Mark>) {
        let c = inner.center();
        let h = inner.height();
        let line = s.body_stroke;
        // Where the dots go when Windows gives no positions.
        let spots: Vec<Pos2> = match shape {
            Shape::Mouse => {
                let body = Rect::from_center_size(c, Vec2::new(h * 0.62, h));
                out.push(Mark::Rect {
                    rect: body,
                    rounding: body.width() / 2.0,
                    fill: s.body_fill,
                    stroke: line,
                });
                // The buttons' seam.
                out.push(Mark::Line {
                    points: vec![
                        Pos2::new(c.x, body.top() + 1.0),
                        Pos2::new(c.x, body.top() + h * 0.36),
                    ],
                    stroke: line,
                });
                if !self.lamps.is_empty() {
                    self.lamp_dots(body.shrink(2.0), self.lamp_box(), s, out);
                    return;
                }
                vec![
                    Pos2::new(c.x, body.top() + h * 0.2),
                    Pos2::new(c.x, body.top() + h * 0.68),
                ]
            }
            Shape::Headset => {
                let w = h * 1.05;
                let band: Vec<Pos2> = (0..=16)
                    .map(|i| {
                        let a = std::f32::consts::PI * (1.0 + i as f32 / 16.0);
                        Pos2::new(c.x + a.cos() * w * 0.4, c.y + 2.0 + a.sin() * h * 0.42)
                    })
                    .collect();
                out.push(Mark::Line {
                    points: band,
                    stroke: Stroke::new(line.width.max(1.0) * 1.6, line.color),
                });
                let cup = Vec2::new(w * 0.22, h * 0.42);
                let cups = [
                    Pos2::new(c.x - w * 0.4, c.y + h * 0.18),
                    Pos2::new(c.x + w * 0.4, c.y + h * 0.18),
                ];
                for at in cups {
                    out.push(Mark::Rect {
                        rect: Rect::from_center_size(at, cup),
                        rounding: cup.x / 2.0,
                        fill: s.body_fill,
                        stroke: line,
                    });
                }
                cups.to_vec()
            }
            _ => {
                let w = (h * 1.5).min(inner.width());
                let body = Rect::from_center_size(c, Vec2::new(w * 0.7, h * 0.5));
                for x in [c.x - w * 0.3, c.x + w * 0.3] {
                    out.push(Mark::Circle {
                        centre: Pos2::new(x, c.y + h * 0.12),
                        radius: h * 0.26,
                        fill: s.body_fill,
                        stroke: line,
                    });
                }
                // Filled, the body hides the grips' inner outlines; unknown, it is an outline too.
                let stroke = if s.body_fill == Color32::TRANSPARENT {
                    line
                } else {
                    Stroke::NONE
                };
                out.push(Mark::Rect {
                    rect: body,
                    rounding: body.height() * 0.45,
                    fill: s.body_fill,
                    stroke,
                });
                vec![
                    Pos2::new(c.x - w * 0.22, c.y),
                    Pos2::new(c.x, c.y - h * 0.08),
                    Pos2::new(c.x + w * 0.22, c.y),
                ]
            }
        };
        if !self.lamps.is_empty() {
            let around = Rect::from_points(&spots).expand(h * 0.12);
            self.lamp_dots(around, self.lamp_box(), s, out);
            return;
        }
        let n = self.lights().max(1);
        let r = (h * 0.08).max(1.4);
        for k in 0..n.min(spots.len() * 4) {
            // Lamps past the spots share them, a little apart.
            let spot = spots[k % spots.len()];
            let at = spot + Vec2::new(0.0, (k / spots.len()) as f32 * r * 2.4);
            let (fill, stroke) = s.part(self.tint().colour(k, n));
            out.push(Mark::Circle {
                centre: at,
                radius: r,
                fill,
                stroke,
            });
        }
    }

    /// A dot per lamp at its place in the bounding box (`size`, millimetres), drawn in `area`.
    fn lamp_dots(&self, area: Rect, size: Vec2, s: &Style, out: &mut Vec<Mark>) {
        let n = self.lamps.len().min(MAX_LAMPS);
        let r = (area.width().min(area.height()) / 14.0).clamp(0.8, 2.2);
        for i in 0..n {
            let lamp = i * self.lamps.len() / n;
            let (fill, stroke) = s.part(self.tint().colour(lamp, self.lamps.len()));
            out.push(Mark::Circle {
                centre: to_box(area, size, self.lamps[lamp]),
                radius: r,
                fill,
                stroke,
            });
        }
    }

    /// The lamps' bounding box: Windows' when it gives one, else their extent.
    fn lamp_box(&self) -> Vec2 {
        if let Some([w, h]) = self.size_mm.filter(|s| s[0] > 0.0 && s[1] > 0.0) {
            return Vec2::new(w, h);
        }
        let max = self
            .lamps
            .iter()
            .fold(Vec2::splat(1.0), |m, p| m.max(Vec2::new(p[0], p[1])));
        max * 1.1
    }
}

/// Lamps in one row: at least two, and all of them close to one line across the widest way.
fn in_a_row(lamps: &[[f32; 2]]) -> bool {
    if lamps.len() < 2 {
        return false;
    }
    let outline = Rect::from_points(
        &lamps
            .iter()
            .map(|p| Pos2::new(p[0], p[1]))
            .collect::<Vec<_>>(),
    );
    let (long, short) = if outline.width() >= outline.height() {
        (outline.width(), outline.height())
    } else {
        (outline.height(), outline.width())
    };
    long > 0.0 && short <= long * 0.15
}

/// A plain board for a keyboard whose layout isn't known: four rows of twelve keys.
fn plain_board() -> Vec<[Pos2; 4]> {
    (0..4)
        .flat_map(|row| {
            (0..12).map(move |col| {
                let r = Rect::from_min_size(Pos2::new(col as f32, row as f32), Vec2::splat(1.0))
                    .shrink(0.08);
                [
                    r.left_top(),
                    r.right_top(),
                    r.right_bottom(),
                    r.left_bottom(),
                ]
            })
        })
        .collect()
}

/// The largest rectangle of `size`'s aspect inside `r`, centred.
fn fit(r: Rect, size: Vec2) -> Rect {
    let size = size.max(Vec2::splat(f32::EPSILON));
    let scale = (r.width() / size.x).min(r.height() / size.y);
    Rect::from_center_size(r.center(), size * scale)
}

/// `at` (millimetres in a box of `size`) in `area`, kept inside it.
fn to_box(area: Rect, size: Vec2, at: [f32; 2]) -> Pos2 {
    let size = size.max(Vec2::splat(f32::EPSILON));
    let p = area.min
        + Vec2::new(
            at[0] / size.x * area.width(),
            at[1] / size.y * area.height(),
        );
    p.clamp(area.min, area.max)
}

/// The colours a tint state draws its body and parts in.
struct Style {
    body_fill: Color32,
    body_stroke: Stroke,
    off: Color32,
    line: Stroke,
    unknown: Stroke,
}

impl Style {
    fn new(tint: &Tint, p: &Palette) -> Style {
        let unknown = Stroke::new(1.0, p.muted);
        let known = !matches!(tint, Tint::Unknown);
        Style {
            body_fill: if known { p.well } else { Color32::TRANSPARENT },
            body_stroke: if known {
                Stroke::new(1.0, p.control_line)
            } else {
                unknown
            },
            off: if p.dark {
                Color32::from_rgb(0x22, 0x24, 0x28)
            } else {
                Color32::from_rgb(0x3a, 0x3c, 0x40)
            },
            line: Stroke::new(0.5, p.line),
            unknown,
        }
    }

    /// A lit part's fill and outline: its colour; dark when off (black); a grey outline and no
    /// fill when unknown.
    fn part(&self, colour: Option<Color32>) -> (Color32, Stroke) {
        match colour {
            Some(c) if !is_black(c) => (c, Stroke::NONE),
            Some(_) => (self.off, self.line),
            None => (Color32::TRANSPARENT, self.unknown),
        }
    }

    /// A ring's stroke, `width` wide: its colour, dark when off, grey when unknown.
    fn ring(&self, colour: Option<Color32>, width: f32) -> Stroke {
        match colour {
            Some(c) if !is_black(c) => Stroke::new(width, c),
            Some(_) => Stroke::new(width, self.off),
            None => self.unknown,
        }
    }
}

/// One thing to paint.
#[derive(Clone, Debug, PartialEq)]
pub enum Mark {
    /// A turned key.
    Poly {
        points: Vec<Pos2>,
        fill: Color32,
        stroke: Stroke,
    },
    Rect {
        rect: Rect,
        rounding: f32,
        fill: Color32,
        stroke: Stroke,
    },
    Circle {
        centre: Pos2,
        radius: f32,
        fill: Color32,
        stroke: Stroke,
    },
    Line {
        points: Vec<Pos2>,
        stroke: Stroke,
    },
    /// The unknown shape's outline.
    Dashed {
        rect: Rect,
        color: Color32,
    },
    Text {
        at: Pos2,
        text: &'static str,
        size: f32,
        color: Color32,
    },
}

impl Mark {
    /// Its colour as drawn: the fill, else the outline's.
    pub fn colour(&self) -> Color32 {
        match self {
            Mark::Poly { fill, stroke, .. }
            | Mark::Rect { fill, stroke, .. }
            | Mark::Circle { fill, stroke, .. } => {
                if *fill != Color32::TRANSPARENT {
                    *fill
                } else {
                    stroke.color
                }
            }
            Mark::Line { stroke, .. } => stroke.color,
            Mark::Dashed { color, .. } | Mark::Text { color, .. } => *color,
        }
    }
}

/// Paints `thumb` in `rect` with `painter`, in `p`'s colours.
pub fn paint(painter: &egui::Painter, rect: Rect, thumb: &Thumb, p: &Palette) {
    use egui::epaint::{PathShape, Shape as E, StrokeKind};
    for mark in thumb.marks(rect, p) {
        match mark {
            Mark::Poly {
                points,
                fill,
                stroke,
            } => {
                painter.add(E::Path(PathShape::convex_polygon(points, fill, stroke)));
            }
            Mark::Rect {
                rect,
                rounding,
                fill,
                stroke,
            } => {
                painter.rect(rect, rounding, fill, stroke, StrokeKind::Inside);
            }
            Mark::Circle {
                centre,
                radius,
                fill,
                stroke,
            } => {
                painter.circle(centre, radius, fill, stroke);
            }
            Mark::Line { points, stroke } => {
                painter.add(E::line(points, stroke));
            }
            Mark::Dashed { rect, color } => {
                let corners = [
                    rect.left_top(),
                    rect.right_top(),
                    rect.right_bottom(),
                    rect.left_bottom(),
                    rect.left_top(),
                ];
                painter.extend(E::dashed_line(&corners, Stroke::new(1.0, color), 3.0, 2.0));
            }
            Mark::Text {
                at,
                text,
                size,
                color,
            } => {
                painter.text(
                    at,
                    egui::Align2::CENTER_CENTER,
                    text,
                    egui::FontId::proportional(size),
                    color,
                );
            }
        }
    }
}

/// [`paint`] as a widget of `size`, with the shape's name on hover.
pub fn show(ui: &mut egui::Ui, size: Vec2, thumb: &Thumb) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::hover());
    if ui.is_rect_visible(rect) {
        paint(ui.painter(), rect, thumb, super::theme::palette(ui));
    }
    response.on_hover_text(thumb.shape().label())
}

#[cfg(test)]
#[path = "device_thumb_tests.rs"]
pub(crate) mod tests;
