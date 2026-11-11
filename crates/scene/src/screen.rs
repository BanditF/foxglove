//! Screen glow (#496): the lights take the colours at the edges of what's on screen, like a TV's
//! ambient light. Everything here is pure: the app hands in small pictures of each screen (its
//! capture downscales them) and gets colours per light back, so tests run on recorded frames.
//!
//! The approach follows Hyperion.ng's (MIT) grabber model, described here rather than copied:
//! each screen's picture is cut into **edge regions** (a strip along each side, split into
//! segments), each region is averaged into one colour, and every light takes the colour of the
//! region it is mapped to. **Black borders** (letterboxed films, 4:3 games) are found first and
//! skipped, so the regions lie on the picture itself. A **smoothing** stage eases each light
//! towards its new colour over a settling time.
//!
//! Where Foxglove differs: Hyperion maps regions to an LED strip laid out by hand (counts per
//! side, start corner). Here the room map already says where every light is, so a light takes
//! the nearest stretch of the nearest screen's edge ([`nearest_edge`]), with no layout to set.
//! And a **flash limit** sits after the smoothing: whatever the smoothing, a light never swings
//! between dark and bright more than [`MAX_SWINGS`] times a second (WCAG 2.3.1's three-flash
//! limit), so a strobing game or film becomes a steady pulse.

use std::collections::{BTreeMap, VecDeque};

use serde::{Deserialize, Serialize};

use crate::clock::SceneTime;
use crate::colour::Rgb;
use crate::effects::{Frame, FrameLight};
use crate::room::{DeviceKind, LightRole, RoomMap};
use crate::space::{Point, Rect};

/// The room id prefix of a monitor (`monitor:\\.\DISPLAY1`).
pub const MONITOR_PREFIX: &str = "monitor:";

/// The most screen updates a second that reach the lights.
pub const MAX_UPDATES_PER_S: u32 = 20;
/// The shortest gap between two updates, in milliseconds (at [`MAX_UPDATES_PER_S`]).
pub const MIN_UPDATE_MS: i64 = 1000 / MAX_UPDATES_PER_S as i64;
/// The most swings between dark and bright a light makes in any second.
pub const MAX_SWINGS: usize = 3;
/// A swing: the light's relative luminance turns and moves at least this far (WCAG's 10 %).
pub const SWING: f32 = 0.10;
/// How deep each edge strip reaches into the picture: a share of the screen's shorter side, as
/// Ambilight's and Hyperion's defaults do.
pub const DEPTH: f32 = 0.10;
/// A row or column at or below this (each sRGB channel, 0-255) is part of a black bar.
pub const BLACK: u8 = 16;
/// The edge segments along a screen's longer side; the shorter side gets as many per mm.
pub const SEGMENTS: usize = 16;
/// The longest step the smoothing takes in one update. After a still screen (the frame thread
/// slept, #1085) or a hold, the next picture eases in from where the lights are rather than
/// jumping there.
pub const MAX_STEP_MS: i64 = 2 * MIN_UPDATE_MS;
/// A full-screen game holds Screen glow: the lights fade to their own colours over this long.
pub const GAME_FADE_MS: i64 = 1000;
/// Capture fails: the lights hold their last colours this long (a short break, such as a mode
/// change or a UAC prompt, often ends meanwhile) ...
pub const FAIL_HOLD_MS: i64 = 2000;
/// ... then fade to their own colours over this long.
pub const FAIL_FADE_MS: i64 = 3000;

/// Why Screen glow hands the lights back (`docs/screen-ambient-ux.md`, "Gates the user sees").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HandBack {
    /// A full-screen game (or a presentation): fade at once over [`GAME_FADE_MS`].
    Game,
    /// Capture failed: hold [`FAIL_HOLD_MS`], then fade over [`FAIL_FADE_MS`].
    Failed,
}

impl HandBack {
    /// How long the colours hold before they fade, in milliseconds.
    pub fn hold_ms(self) -> i64 {
        match self {
            HandBack::Game => 0,
            HandBack::Failed => FAIL_HOLD_MS,
        }
    }

    /// How long the fade takes, in milliseconds.
    pub fn fade_ms(self) -> i64 {
        match self {
            HandBack::Game => GAME_FADE_MS,
            HandBack::Failed => FAIL_FADE_MS,
        }
    }
}

/// The fade's curve: `p` (0..=1, the share of the fade's time gone) to the share of the way
/// gone. It eases in and out (smoothstep), so the lights neither jump off nor land with a stop.
pub fn fade_curve(p: f32) -> f32 {
    let p = if p.is_finite() {
        p.clamp(0.0, 1.0)
    } else {
        1.0
    };
    p * p * (3.0 - 2.0 * p)
}

/// How quickly a light follows the screen: Quick follows every cut, Calm drifts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Smoothing {
    Quick,
    #[default]
    Even,
    Calm,
}

impl Smoothing {
    pub const ALL: [Smoothing; 3] = [Smoothing::Quick, Smoothing::Even, Smoothing::Calm];

    /// About how long a light takes to settle on a new colour, in milliseconds.
    pub fn settle_ms(self) -> f32 {
        match self {
            Smoothing::Quick => 100.0,
            Smoothing::Even => 300.0,
            Smoothing::Calm => 1000.0,
        }
    }

    /// The easing's time constant: three of them settle 95 % of the way.
    fn tau_ms(self) -> f32 {
        self.settle_ms() / 3.0
    }

    /// The default for a new Screen glow: Calm when Windows' animation effects are off.
    pub fn default_for(animations: bool) -> Smoothing {
        if animations {
            Smoothing::Even
        } else {
            Smoothing::Calm
        }
    }
}

/// One screen's picture, downscaled: sRGB pixels in rows, top to bottom.
#[derive(Clone, Debug, PartialEq)]
pub struct ScreenImage {
    /// Windows' device name (`\\.\DISPLAY1`), as the room's monitors are named.
    pub monitor: String,
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<[u8; 3]>,
}

impl ScreenImage {
    /// A picture of one colour.
    pub fn filled(monitor: &str, width: usize, height: usize, c: [u8; 3]) -> ScreenImage {
        ScreenImage {
            monitor: monitor.into(),
            width,
            height,
            pixels: vec![c; width * height],
        }
    }

    fn at(&self, x: usize, y: usize) -> [u8; 3] {
        self.pixels[y * self.width + x]
    }

    fn valid(&self) -> bool {
        self.width > 0 && self.height > 0 && self.pixels.len() >= self.width * self.height
    }

    /// Paints a rectangle (`x0..x1`, `y0..y1`), for recorded test frames.
    pub fn paint(&mut self, (x0, y0, x1, y1): (usize, usize, usize, usize), c: [u8; 3]) {
        for y in y0.min(self.height)..y1.min(self.height) {
            for x in x0.min(self.width)..x1.min(self.width) {
                self.pixels[y * self.width + x] = c;
            }
        }
    }
}

/// The picture's part of a screen, without black bars: `(x0, y0, x1, y1)`, ends exclusive.
/// Bars are found from each side inwards; a screen that is black all over has none (the whole
/// screen is the picture, and the lights go dark with it).
pub fn picture(img: &ScreenImage) -> (usize, usize, usize, usize) {
    let (w, h) = (img.width, img.height);
    if !img.valid() {
        return (0, 0, w, h);
    }
    let dark = |c: [u8; 3]| c.iter().all(|&v| v <= BLACK);
    let row = |y: usize| (0..w).all(|x| dark(img.at(x, y)));
    let col = |x: usize, y0: usize, y1: usize| (y0..y1).all(|y| dark(img.at(x, y)));
    let mut y0 = 0;
    while y0 < h && row(y0) {
        y0 += 1;
    }
    if y0 == h {
        return (0, 0, w, h);
    }
    let mut y1 = h;
    while y1 > y0 && row(y1 - 1) {
        y1 -= 1;
    }
    let mut x0 = 0;
    while x0 < w && col(x0, y0, y1) {
        x0 += 1;
    }
    let mut x1 = w;
    while x1 > x0 && col(x1 - 1, y0, y1) {
        x1 -= 1;
    }
    (x0, y0, x1, y1)
}

/// A side of a screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Edge {
    Top,
    Right,
    Bottom,
    Left,
}

/// One screen's edge colours: each side's segments, left to right (top, bottom) or top to bottom
/// (left, right).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Edges {
    /// Windows' device name (`\\.\DISPLAY1`).
    pub monitor: String,
    pub top: Vec<Rgb>,
    pub right: Vec<Rgb>,
    pub bottom: Vec<Rgb>,
    pub left: Vec<Rgb>,
}

impl Edges {
    /// The edge regions of `img`, on its picture (black bars skipped).
    pub fn of(img: &ScreenImage) -> Edges {
        let mut e = Edges {
            monitor: img.monitor.clone(),
            ..Edges::default()
        };
        if !img.valid() {
            return e;
        }
        let (x0, y0, x1, y1) = picture(img);
        let (pw, ph) = (x1 - x0, y1 - y0);
        let depth = ((pw.min(ph) as f32 * DEPTH).round() as usize).clamp(1, pw.min(ph).max(1));
        let (across, down) = if pw >= ph {
            let d = ((SEGMENTS as f32 * ph as f32 / pw as f32).round() as usize).max(2);
            (SEGMENTS, d)
        } else {
            let a = ((SEGMENTS as f32 * pw as f32 / ph as f32).round() as usize).max(2);
            (a, SEGMENTS)
        };
        let average = |rx0: usize, ry0: usize, rx1: usize, ry1: usize| -> Rgb {
            let mut sum = [0.0f32; 3];
            let mut n = 0u32;
            for y in ry0..ry1.max(ry0 + 1).min(y1) {
                for x in rx0..rx1.max(rx0 + 1).min(x1) {
                    for (s, v) in sum.iter_mut().zip(Rgb(img.at(x, y)).linear()) {
                        *s += v;
                    }
                    n += 1;
                }
            }
            if n == 0 {
                return Rgb::BLACK;
            }
            Rgb::from_linear(sum.map(|s| s / n as f32))
        };
        let split = |from: usize, len: usize, k: usize, i: usize| {
            (from + len * i / k, from + len * (i + 1) / k)
        };
        for i in 0..across {
            let (a, b) = split(x0, pw, across, i);
            e.top.push(average(a, y0, b, y0 + depth));
            e.bottom.push(average(a, y1 - depth, b, y1));
        }
        for i in 0..down {
            let (a, b) = split(y0, ph, down, i);
            e.left.push(average(x0, a, x0 + depth, b));
            e.right.push(average(x1 - depth, a, x1, b));
        }
        e
    }

    fn side(&self, edge: Edge) -> &[Rgb] {
        match edge {
            Edge::Top => &self.top,
            Edge::Right => &self.right,
            Edge::Bottom => &self.bottom,
            Edge::Left => &self.left,
        }
    }

    /// The colour `along` (0..=1) one side, blended between its two nearest segments in linear
    /// light; black for a side with none.
    pub fn at(&self, edge: Edge, along: f32) -> Rgb {
        let s = self.side(edge);
        match s {
            [] => Rgb::BLACK,
            [one] => *one,
            _ => {
                let along = if along.is_finite() {
                    along.clamp(0.0, 1.0)
                } else {
                    0.5
                };
                // Segment centres sit at (i + 0.5) / n.
                let at = (along * s.len() as f32 - 0.5).clamp(0.0, (s.len() - 1) as f32);
                let i = at.floor() as usize;
                let j = (i + 1).min(s.len() - 1);
                let f = at - i as f32;
                let (a, b) = (s[i].linear(), s[j].linear());
                Rgb::from_linear([0, 1, 2].map(|k| a[k] + (b[k] - a[k]) * f))
            }
        }
    }
}

/// Where a light takes its colour: a stretch of one screen's edge.
#[derive(Clone, Debug, PartialEq)]
pub struct EdgeSpot {
    /// The screen's room id (`monitor:\\.\DISPLAY1`).
    pub monitor: String,
    pub edge: Edge,
    /// How far along the side, 0..=1 (left to right, or top to bottom).
    pub along: f32,
    /// That point on the edge, in room millimetres (for the map's line).
    pub at: Point,
}

/// The stretch of the nearest screen's edge to `p`, in the room's millimetres: a keyboard below
/// a screen follows its bottom edge, a case beside it that side, top to bottom. A device far
/// from every screen still follows the nearest one. `None` with no screen in the room.
pub fn nearest_edge(room: &RoomMap, p: Point) -> Option<EdgeSpot> {
    room.devices
        .iter()
        .filter(|d| d.kind == DeviceKind::Monitor)
        .filter(|d| d.rect.w > 0.0 && d.rect.h > 0.0)
        .map(|d| {
            let (edge, at, dist) = edge_of(&d.rect, p);
            let along = match edge {
                Edge::Top | Edge::Bottom => (at.x - d.rect.x) / d.rect.w,
                Edge::Left | Edge::Right => (at.y - d.rect.y) / d.rect.h,
            };
            (
                dist,
                EdgeSpot {
                    monitor: d.id.clone(),
                    edge,
                    along: along.clamp(0.0, 1.0),
                    at,
                },
            )
        })
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, s)| s)
}

/// The side of `r` nearest to `p`, the point on it, and how far `p` is from the screen (0 when
/// it is over it).
fn edge_of(r: &Rect, p: Point) -> (Edge, Point, f32) {
    let q = Point::new(p.x.clamp(r.x, r.right()), p.y.clamp(r.y, r.bottom()));
    let (dx, dy) = ((p.x - q.x).abs(), (p.y - q.y).abs());
    if dx > 0.0 || dy > 0.0 {
        // Outside: the side it is beyond; past a corner, the side it is further beyond.
        let edge = if dy >= dx {
            if p.y < r.y {
                Edge::Top
            } else {
                Edge::Bottom
            }
        } else if p.x < r.x {
            Edge::Left
        } else {
            Edge::Right
        };
        return (edge, q, dx.hypot(dy));
    }
    // Over the screen: the side it is nearest.
    let sides = [
        (Edge::Top, p.y - r.y, Point::new(p.x, r.y)),
        (Edge::Bottom, r.bottom() - p.y, Point::new(p.x, r.bottom())),
        (Edge::Left, p.x - r.x, Point::new(r.x, p.y)),
        (Edge::Right, r.right() - p.x, Point::new(r.right(), p.y)),
    ];
    let (edge, _, at) = sides
        .into_iter()
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .unwrap_or((Edge::Bottom, 0.0, q));
    (edge, at, 0.0)
}

/// The screen colour for a light at `p`: its edge spot's colour, when that screen was read.
pub fn colour_at(room: &RoomMap, edges: &[Edges], p: Point) -> Option<Rgb> {
    let spot = nearest_edge(room, p)?;
    let name = spot.monitor.strip_prefix(MONITOR_PREFIX)?;
    let e = edges.iter().find(|e| e.monitor == name)?;
    Some(e.at(spot.edge, spot.along))
}

/// Relative luminance (WCAG), 0..=1.
pub fn luminance(c: Rgb) -> f32 {
    let [r, g, b] = c.linear();
    0.2126 * r + 0.7152 * g + 0.0722 * b
}

/// The flash limit for one light (WCAG 2.3.1): a turn in brightness of at least [`SWING`] is
/// one swing, and at most [`MAX_SWINGS`] happen in any second. A turn that would be one more
/// is held back: the light stays where it is until the oldest swing is a second old.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FlashLimit {
    /// The way the light last moved: +1 brighter, -1 darker, 0 not yet.
    dir: i8,
    /// The brightest (or darkest) it reached going that way.
    extreme: f32,
    /// When each recent swing happened (ms).
    swings: VecDeque<i64>,
    /// What it shows now.
    shown: Option<Rgb>,
}

impl FlashLimit {
    /// The colour to show at `t_ms` for a wanted colour.
    pub fn apply(&mut self, want: Rgb, t_ms: i64) -> Rgb {
        while self.swings.front().is_some_and(|&s| t_ms - s >= 1000) {
            self.swings.pop_front();
        }
        let Some(shown) = self.shown else {
            self.shown = Some(want);
            self.extreme = luminance(want);
            return want;
        };
        let y = luminance(want);
        let delta = y - self.extreme;
        let way: i8 = if delta > 0.0 { 1 } else { -1 };
        if self.dir == 0 || way == self.dir {
            // The same way (or the first move): no swing; follow it.
            if self.dir == 0 && delta.abs() >= SWING {
                self.dir = way;
            }
            if self.dir == way && (way > 0 && y > self.extreme || way < 0 && y < self.extreme) {
                self.extreme = y;
            }
            self.shown = Some(want);
            return want;
        }
        if delta.abs() < SWING {
            // A small turn: not a swing, and the extreme stays.
            self.shown = Some(want);
            return want;
        }
        if self.swings.len() >= MAX_SWINGS {
            return shown;
        }
        self.swings.push_back(t_ms);
        self.dir = way;
        self.extreme = y;
        self.shown = Some(want);
        want
    }

    /// Swings in the second before the last update.
    pub fn recent(&self) -> usize {
        self.swings.len()
    }
}

/// One light's eased colour, in linear light.
#[derive(Clone, Debug, Default, PartialEq)]
struct Light {
    linear: Option<[f32; 3]>,
    /// The screen's colour it eases towards (none while it fades, or with its screen unread).
    want: Option<[f32; 3]>,
    /// The flash limit kept it from its colour in the last update.
    held_back: bool,
    limit: FlashLimit,
}

/// A light: (device, light) as the room map names them.
pub type LightKey = (String, String);

/// Screen glow handing the lights back: their colours hold, then fade to their own.
#[derive(Clone, Debug, PartialEq)]
struct Fade {
    /// When the colours start to move (ms).
    start: i64,
    /// How long they take (ms).
    ms: i64,
    /// Each light's own colour, in linear light. A light without one keeps its colour.
    to: BTreeMap<LightKey, [f32; 3]>,
    /// Each point light's colour as the fade began (taken at its first update).
    from: BTreeMap<LightKey, [f32; 3]>,
    /// Each area light's colour as it showed when the hand-back began (after its own easing),
    /// so it fades on from there rather than jumping to its neighbours' peak.
    areas: BTreeMap<LightKey, [f32; 3]>,
}

impl Fade {
    /// How far along it is at `t`: `None` while the colours still hold.
    fn at(&self, t: SceneTime) -> Option<f32> {
        (t.ms() >= self.start)
            .then(|| fade_curve((t.ms() - self.start) as f32 / self.ms.max(1) as f32))
    }

    fn end(&self) -> i64 {
        self.start + self.ms
    }
}

/// A running Screen glow: each light's eased, flash-limited colour from the screens' edges.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Glow {
    pub smoothing: Smoothing,
    lights: BTreeMap<LightKey, Light>,
    last: Option<SceneTime>,
    fade: Option<Fade>,
}

impl Glow {
    pub fn new(smoothing: Smoothing) -> Glow {
        Glow {
            smoothing,
            ..Glow::default()
        }
    }

    /// Whether an update at `t` is due: at most [`MAX_UPDATES_PER_S`] a second.
    pub fn due(&self, t: SceneTime) -> bool {
        self.last
            .is_none_or(|l| t.ms() - l.ms() >= MIN_UPDATE_MS * 9 / 10 || t < l)
    }

    /// Every light's colour at `t` from the latest `edges`: each point light eased towards its
    /// nearest edge's colour, then flash-limited; ambient lights as the scene's other effects
    /// make them (the peak of their device's points). A light whose screen wasn't read keeps
    /// its colour (black at first).
    ///
    /// While the lights are handed back ([`Glow::hand_back`]) the screen is ignored: each light
    /// holds, then fades to its own colour.
    pub fn render(&mut self, room: &RoomMap, edges: &[Edges], t: SceneTime) -> Frame {
        // A step after a long gap (a still screen, a hold) is one ordinary step.
        let dt = self.last.map_or(f32::INFINITY, |l| {
            (t.ms() - l.ms()).clamp(0, MAX_STEP_MS) as f32
        });
        self.last = Some(t);
        // The share of the way to go this step (all of it the first time).
        let k = if dt.is_finite() {
            1.0 - (-dt / self.smoothing.tau_ms()).exp()
        } else {
            1.0
        };
        let mut frame = Frame {
            t,
            lights: Vec::new(),
        };
        let fading = self.fade.as_ref().map(|f| f.at(t));
        // The point lights in the room now: any other light's state goes (a device unplugged,
        // or a light that became an area light), so it can't keep the glow unsettled.
        let mut present = std::collections::BTreeSet::new();
        for d in &room.devices {
            for l in &d.lights {
                let key = (d.id.clone(), l.id.clone());
                let colour = match l.role {
                    LightRole::Ambient => Rgb::BLACK,
                    LightRole::Point => {
                        present.insert(key.clone());
                        let state = self.lights.entry(key.clone()).or_default();
                        let mut now = match (&mut self.fade, fading) {
                            // Handing back: hold, then fade to the light's own colour. A light
                            // found meanwhile starts at its own colour (black when unknown).
                            (Some(fade), Some(along)) => {
                                state.want = None;
                                let held = state
                                    .linear
                                    .or_else(|| fade.to.get(&key).copied())
                                    .unwrap_or([0.0; 3]);
                                match along {
                                    None => held,
                                    Some(e) => {
                                        let from = *fade.from.entry(key.clone()).or_insert(held);
                                        match fade.to.get(&key) {
                                            Some(to) => mix(from, *to, e),
                                            None => from,
                                        }
                                    }
                                }
                            }
                            _ => {
                                let target = colour_at(room, edges, l.at).map(Rgb::linear);
                                state.want = target;
                                match (state.linear, target) {
                                    (Some(c), Some(want)) => mix(c, want, k),
                                    (None, Some(want)) => want,
                                    (Some(c), None) => c,
                                    (None, None) => [0.0; 3],
                                }
                            }
                        };
                        // Close enough to show the screen's own bytes: there.
                        if let Some(want) = state.want {
                            if Rgb::from_linear(now) == Rgb::from_linear(want) {
                                now = want;
                            }
                        }
                        state.linear = Some(now);
                        let wanted = Rgb::from_linear(now);
                        let shown = state.limit.apply(wanted, t.ms());
                        state.held_back = shown != wanted;
                        shown
                    }
                };
                frame.lights.push(FrameLight {
                    device: d.id.clone(),
                    light: l.id.clone(),
                    colour,
                });
            }
        }
        self.lights.retain(|k, _| present.contains(k));
        crate::ambient::group(&mut frame, room);
        // Handing back, an area light holds the colour it showed, then fades to its own (or
        // keeps it, unknown).
        if let (Some(fade), Some(along)) = (&self.fade, fading) {
            let e = along.unwrap_or(0.0);
            let places = room.devices.iter().flat_map(|d| d.lights.iter());
            for (l, place) in frame.lights.iter_mut().zip(places) {
                if place.role != LightRole::Ambient {
                    continue;
                }
                let key = (l.device.clone(), l.light.clone());
                let from = fade
                    .areas
                    .get(&key)
                    .copied()
                    .unwrap_or_else(|| l.colour.linear());
                let to = fade.to.get(&key).copied().unwrap_or(from);
                l.colour = Rgb::from_linear(mix(from, to, e));
            }
        }
        frame
    }

    /// Hands the lights back from `t` (a full-screen game, capture failing): each light holds
    /// its colour for `why`'s hold, then fades to `own` (its own colour: what it shows once the
    /// scene lets it go) over `why`'s fade, flash-limited as ever. A light missing from `own`
    /// (its own colour isn't known, such as Windows' own lights) keeps its colour. `areas` is
    /// each area light's colour as it shows now (the scene eases area lights after this), so
    /// they fade on from there. A second call while handing back changes nothing.
    pub fn hand_back(
        &mut self,
        why: HandBack,
        own: BTreeMap<LightKey, Rgb>,
        areas: BTreeMap<LightKey, Rgb>,
        t: SceneTime,
    ) {
        if self.fade.is_some() {
            return;
        }
        let linear =
            |m: BTreeMap<LightKey, Rgb>| m.into_iter().map(|(k, c)| (k, c.linear())).collect();
        self.fade = Some(Fade {
            start: t.ms() + why.hold_ms(),
            ms: why.fade_ms(),
            to: linear(own),
            from: BTreeMap::new(),
            areas: linear(areas),
        });
    }

    /// The hold ended: the lights ease back to the screen from wherever the fade left them, at
    /// the smoothing's pace (the first step no bigger than [`MAX_STEP_MS`]'s).
    pub fn resume(&mut self) {
        self.fade = None;
    }

    /// The lights are being handed back ([`Glow::hand_back`]).
    pub fn handing_back(&self) -> bool {
        self.fade.is_some()
    }

    /// When the colours start to fade, while they still hold.
    pub fn fade_starts(&self, t: SceneTime) -> Option<SceneTime> {
        self.fade
            .as_ref()
            .filter(|f| t.ms() < f.start)
            .map(|f| SceneTime(f.start))
    }

    /// The fade has reached every light's own colour by `t` (the flash limit held none back in
    /// the last update).
    pub fn handed_back(&self, t: SceneTime) -> bool {
        self.fade.as_ref().is_some_and(|f| t.ms() >= f.end())
            && self.lights.values().all(|l| !l.held_back)
    }

    /// Every light showed its screen colour in the last update (or has none to go to) and the
    /// flash limit held none back: more updates of the same pictures change nothing. Not while
    /// handing back.
    pub fn settled(&self) -> bool {
        self.fade.is_none()
            && self
                .lights
                .values()
                .all(|l| !l.held_back && l.want.is_none_or(|w| l.linear.is_some_and(|c| c == w)))
    }

    /// Each point light's colour as it shows now (after the flash limit).
    pub fn shown(&self) -> BTreeMap<LightKey, Rgb> {
        self.lights
            .iter()
            .filter_map(|(k, l)| Some((k.clone(), l.limit.shown?)))
            .collect()
    }
}

/// `a` moved the share `k` of the way to `b`, in linear light.
fn mix(a: [f32; 3], b: [f32; 3], k: f32) -> [f32; 3] {
    [0, 1, 2].map(|i| a[i] + (b[i] - a[i]) * k)
}

#[cfg(test)]
mod tests;
