//! One key layout per keyboard (#796; docs/design/device-focus-layouts-thumbnails-lights-declutter.md
//! section 2). Every page surface that draws keys (the Keyboard page drawing, the zone drawing,
//! Check zones and the dot test, the Scenes room) takes its keys from here, and the layer overlay
//! reads the same ZMK Studio physical layout, so they can't disagree.
//!
//! **The source, best first** ([`LayoutSource`]):
//! 1. The keyboard says: ZMK Studio's physical layouts, read with the keymap on connect (or from
//!    the keymap copy `keymap_cache` keeps) and handed over by `protocols::zmk`
//!    ([`from_keyboard`]). Later Vial's own definition.
//! 2. A definition the user loaded (VIA JSON, QMK `info.json`, KLE): B2 #797 and B3 #798 hand
//!    one over as a [`Supplied`] with [`LayoutSource::Definition`].
//! 3. The board profile's `[drawing]`.
//! 4. Guessed: a plain grid of the keymap's key count (`Drawing::plain`).
//!
//! **Key indices stay the profile's.** Zones, LED maps and the points "Check zones" saved are in
//! the profile drawing's key order and key units ([`boards::drawing`](super::boards::drawing)),
//! so #611 and the LED map data stay valid: a layout from the keyboard is put in that order (each
//! of its keys matched to the profile key in the same place) when the key counts agree, and
//! [`KeyLayout::point_from_profile`] moves a saved point onto it. When they don't agree the
//! keyboard wins and [`KeyLayout::mismatch`] says the profile's zones may not fit.
//!
//! Nothing here asks the keyboard; the layout is handed over when it is read (no polling).

use egui::{Pos2, Rect, Vec2};

use super::boards::{Board, Drawing};
use super::theme::Dot;

/// Where a keyboard's layout came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayoutSource {
    /// The keyboard said: ZMK Studio's physical layout (later Vial's own definition).
    Keyboard,
    /// A definition the user loaded (B2 #797, B3 #798 hand these over).
    #[cfg_attr(not(test), allow(dead_code))]
    Definition(DefinitionFormat),
    /// The board profile's `[drawing]`.
    Profile,
    /// No drawing anywhere: a plain grid of this many keys, in the keymap's order.
    Guessed(usize),
}

/// The file a loaded definition came from (B2 #797 builds `Via`, B3 #798 the others).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)]
pub enum DefinitionFormat {
    Via,
    QmkInfo,
    Kle,
}

impl LayoutSource {
    /// The chip on the drawing: "Layout from the keyboard".
    pub fn chip(self) -> String {
        match self {
            LayoutSource::Keyboard => "Layout from the keyboard".into(),
            LayoutSource::Definition(DefinitionFormat::Via) => {
                "Layout from its VIA definition".into()
            }
            LayoutSource::Definition(DefinitionFormat::QmkInfo) => {
                "Layout from its QMK info.json".into()
            }
            LayoutSource::Definition(DefinitionFormat::Kle) => "Layout from its KLE file".into(),
            LayoutSource::Profile => "Layout from Foxglove's board profile".into(),
            LayoutSource::Guessed(n) => format!("Shape guessed from {n} keys"),
        }
    }

    /// The chip's dot: green for a layout the keyboard or its definition gave, none for the
    /// profile, amber for a guess.
    pub fn dot(self) -> Option<Dot> {
        match self {
            LayoutSource::Keyboard | LayoutSource::Definition(_) => Some(Dot::Ok),
            LayoutSource::Profile => None,
            LayoutSource::Guessed(_) => Some(Dot::Warn),
        }
    }

    /// The muted line under a guessed drawing.
    pub fn note(self) -> Option<&'static str> {
        matches!(self, LayoutSource::Guessed(_))
            .then_some("Key order is right; their places aren't known.")
    }

    /// Lower is better: the ladder's step.
    fn rank(self) -> u8 {
        match self {
            LayoutSource::Keyboard => 1,
            LayoutSource::Definition(_) => 2,
            LayoutSource::Profile => 3,
            LayoutSource::Guessed(_) => 4,
        }
    }
}

/// One key: where it is and how it's turned, in key units.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LayoutKey {
    /// The key unturned, placed so its centre is where the key's centre is (as the overlay's
    /// `Key` places it).
    pub rect: Rect,
    /// Degrees, clockwise around the rectangle's centre.
    pub r: f32,
    /// Its place in its source's own key order (ZMK Studio's key index, the keymap's order).
    pub index: usize,
}

impl LayoutKey {
    pub fn new(rect: Rect, r: f32, index: usize) -> LayoutKey {
        let r = if r.is_finite() { r } else { 0.0 };
        LayoutKey { rect, r, index }
    }

    /// Its four corners, turned.
    pub fn corners(&self) -> [Pos2; 4] {
        let c = self.rect.center();
        let rot = egui::emath::Rot2::from_angle(self.r.to_radians());
        let r = self.rect;
        [
            r.left_top(),
            r.right_top(),
            r.right_bottom(),
            r.left_bottom(),
        ]
        .map(|p| c + rot * (p - c))
    }

    /// Its outline as drawn, turned.
    pub fn bounds(&self) -> Rect {
        Rect::from_points(&self.corners())
    }

    /// Whether `p` (key units) is on the key, turned.
    pub fn contains(&self, p: Pos2) -> bool {
        let c = self.rect.center();
        let back = egui::emath::Rot2::from_angle(-self.r.to_radians());
        self.rect.contains(c + back * (p - c))
    }
}

/// A whole-board layout a source handed over: every layout it has (ZMK's physical layouts, VIA's
/// layout options), by name, and the one in use.
#[derive(Clone, Debug, PartialEq)]
pub struct Supplied {
    pub source: LayoutSource,
    pub layouts: Vec<(String, Vec<LayoutKey>)>,
    pub active: usize,
}

/// A keyboard's keys as every page draws them.
#[derive(Clone, Debug, PartialEq)]
pub struct KeyLayout {
    /// One list per part (the left half and the right, or the one piece), each from its part's
    /// top-left corner, in the profile's key order when it has one.
    pub parts: Vec<Vec<LayoutKey>>,
    /// Each part's top-left in the source's own units (the overlay's), zero for the profile.
    pub origins: Vec<Vec2>,
    /// One part's size in key units (the larger, on a split).
    pub size: Vec2,
    pub source: LayoutSource,
    /// The layout's name, when the source names it ("Eyelash 42").
    pub name: Option<String>,
    /// Every layout the source has, the active one among them; one or none means no choice.
    pub choices: Vec<String>,
    pub active: usize,
    /// The keyboard's layout has another key count than the profile's drawing: it is drawn in
    /// its own order, and zones from the profile may not match it.
    pub mismatch: bool,
    /// The profile's keys the parts were put in the order of, for moving saved points over.
    profile: Vec<Vec<Rect>>,
}

impl Default for KeyLayout {
    fn default() -> Self {
        from_drawing(&Drawing::default(), false, LayoutSource::Guessed(0))
    }
}

impl KeyLayout {
    /// Part `p`'s keys (an empty list past the last part).
    pub fn part(&self, p: usize) -> &[LayoutKey] {
        self.parts.get(p).map_or(&[], Vec::as_slice)
    }

    /// Part `p`'s keys unturned, for centres and hit tests that don't need the turn.
    pub fn rects(&self, p: usize) -> Vec<Rect> {
        self.part(p).iter().map(|k| k.rect).collect()
    }

    /// A point on part `p` of the profile's drawing (key units, as Check zones saves LED points)
    /// where it lands on this layout: by the profile key nearest it, which is the same key here.
    pub fn point_from_profile(&self, p: usize, at: [f32; 2]) -> [f32; 2] {
        moved(self.profile.get(p), &self.rects(p), at)
    }

    /// The other way: a point on this layout's part `p` on the profile's drawing.
    pub fn point_to_profile(&self, p: usize, at: [f32; 2]) -> [f32; 2] {
        match self.profile.get(p) {
            Some(profile) => moved(Some(&self.rects(p)), profile, at),
            None => at,
        }
    }
}

/// `at` near a key of `from`, moved with that key onto the same key of `to`.
fn moved(from: Option<&Vec<Rect>>, to: &[Rect], at: [f32; 2]) -> [f32; 2] {
    let Some(from) = from.filter(|f| f.len() == to.len() && !f.is_empty()) else {
        return at;
    };
    let p = Pos2::new(at[0], at[1]);
    let nearest = (0..from.len())
        .min_by(|&a, &b| {
            from[a]
                .center()
                .distance_sq(p)
                .total_cmp(&from[b].center().distance_sq(p))
        })
        .unwrap_or(0);
    let q = to[nearest].center() + (p - from[nearest].center());
    [q.x, q.y]
}

/// The profile's (or the guessed grid's) drawing as a layout: the same keys in the same places as
/// `Drawing::keys` gives them, the right half its mirror image.
pub fn from_drawing(drawing: &Drawing, split: bool, source: LayoutSource) -> KeyLayout {
    let parts: Vec<Vec<LayoutKey>> = (0..if split { 2 } else { 1 })
        .map(|p| {
            drawing
                .keys(p == 1)
                .into_iter()
                .enumerate()
                .map(|(i, rect)| LayoutKey::new(rect, 0.0, i))
                .collect()
        })
        .collect();
    let (w, h) = drawing.size();
    KeyLayout {
        origins: vec![Vec2::ZERO; parts.len()],
        parts,
        size: Vec2::new(w, h),
        source,
        name: None,
        choices: Vec::new(),
        active: 0,
        mismatch: false,
        profile: Vec::new(),
    }
}

/// The ladder: the best layout a source handed over, else the profile's drawing, else a grid
/// guessed from `keymap_keys` (0 before a keymap was read).
pub fn resolve(board: Option<&Board>, supplied: &[Supplied], keymap_keys: usize) -> KeyLayout {
    let split = super::parts::Parts::of(board).is_split();
    let profile = board.and_then(|b| b.drawing.as_ref());
    let mut best: Vec<&Supplied> = supplied.iter().collect();
    best.sort_by_key(|s| s.source.rank());
    if let Some(l) = best
        .into_iter()
        .find_map(|s| from_supplied(s, split, profile))
    {
        return l;
    }
    let drawing = super::boards::plain_or_profile(board, keymap_keys);
    let source = if profile.is_some() {
        LayoutSource::Profile
    } else {
        let parts = if split { 2 } else { 1 };
        LayoutSource::Guessed(drawing.keys(false).len() * parts)
    };
    from_drawing(&drawing, split, source)
}

/// A handed-over layout as parts: a split's halves found in it (the widest gap between its keys),
/// each moved to its own corner and put in the profile's key order when the counts agree.
fn from_supplied(s: &Supplied, split: bool, profile: Option<&Drawing>) -> Option<KeyLayout> {
    let (name, keys) = s.layouts.get(s.active)?;
    if keys.is_empty() || keys.len() > MAX_KEYS {
        return None;
    }
    let mut parts = if split && keys.len() > 1 {
        halves(keys)
    } else {
        vec![keys.clone()]
    };
    let mut origins = Vec::new();
    let mut size = Vec2::ZERO;
    for part in &mut parts {
        let outline = part.iter().fold(Rect::NOTHING, |a, k| a.union(k.bounds()));
        let corner = outline.min.to_vec2();
        for k in part.iter_mut() {
            k.rect = k.rect.translate(-corner);
        }
        origins.push(corner);
        size = size.max(outline.size());
    }
    let mut profile_keys = Vec::new();
    let mut mismatch = false;
    if let Some(d) = profile {
        let wanted: Vec<Vec<Rect>> = (0..parts.len()).map(|p| d.keys(split && p == 1)).collect();
        if parts.iter().zip(&wanted).all(|(g, w)| g.len() == w.len()) {
            for (g, w) in parts.iter_mut().zip(&wanted) {
                *g = in_order_of(g, w);
            }
            profile_keys = wanted;
        } else {
            mismatch = true;
        }
    }
    Some(KeyLayout {
        parts,
        origins,
        size,
        source: s.source,
        name: (!name.is_empty()).then(|| name.clone()),
        choices: s.layouts.iter().map(|(n, _)| n.clone()).collect(),
        active: s.active,
        mismatch,
        profile: profile_keys,
    })
}

/// Keys handed over for a whole board, at most (like `boards::MAX_KEYS`, a wall of keys is a bad
/// read, not a board).
const MAX_KEYS: usize = 256;

/// A split's keys in two: left of the widest gap between key centres, and right of it, each in
/// its source order.
fn halves(keys: &[LayoutKey]) -> Vec<Vec<LayoutKey>> {
    let mut xs: Vec<f32> = keys.iter().map(|k| k.rect.center().x).collect();
    xs.sort_by(f32::total_cmp);
    let cut = xs
        .windows(2)
        .max_by(|a, b| (a[1] - a[0]).total_cmp(&(b[1] - b[0])))
        .map_or(xs[0], |w| (w[0] + w[1]) / 2.0);
    let (left, right): (Vec<LayoutKey>, Vec<LayoutKey>) =
        keys.iter().partition(|k| k.rect.center().x <= cut);
    vec![left, right]
}

/// `keys` in the order of `wanted` (as many): each wanted key takes the key in the same place,
/// both shapes scaled to the same box, nearest pairs first.
fn in_order_of(keys: &[LayoutKey], wanted: &[Rect]) -> Vec<LayoutKey> {
    let unit = |points: Vec<Pos2>| {
        let outline = Rect::from_points(&points);
        let span = outline.size().max(Vec2::splat(f32::EPSILON));
        points
            .into_iter()
            .map(|p| ((p - outline.min) / span).to_pos2())
            .collect::<Vec<Pos2>>()
    };
    let have = unit(keys.iter().map(|k| k.rect.center()).collect());
    let want = unit(wanted.iter().map(Rect::center).collect());
    let mut pairs: Vec<(f32, usize, usize)> = (0..have.len())
        .flat_map(|i| {
            let have = &have;
            want.iter()
                .enumerate()
                .map(move |(j, w)| (have[i].distance_sq(*w), i, j))
        })
        .collect();
    pairs.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut out: Vec<Option<LayoutKey>> = vec![None; wanted.len()];
    let mut used = vec![false; keys.len()];
    for (_, i, j) in pairs {
        if !used[i] && out[j].is_none() {
            used[i] = true;
            out[j] = Some(keys[i]);
        }
    }
    out.into_iter().flatten().collect()
}

/// What the sources handed over, by keyboard (vid, pid), and the keyboard connected now.
#[derive(Default)]
struct Store {
    keyboard: Option<(u16, u16)>,
    supplied: Vec<((u16, u16), Supplied)>,
}

#[cfg(not(test))]
static STORE: std::sync::Mutex<Store> = std::sync::Mutex::new(Store {
    keyboard: None,
    supplied: Vec::new(),
});

#[cfg(not(test))]
fn with_store<R>(f: impl FnOnce(&mut Store) -> R) -> R {
    f(&mut STORE.lock().unwrap_or_else(|e| e.into_inner()))
}

// Tests run side by side: each keeps its own handed-over layouts.
#[cfg(test)]
thread_local! {
    static STORE: std::cell::RefCell<Store> = std::cell::RefCell::new(Store::default());
}

#[cfg(test)]
fn with_store<R>(f: impl FnOnce(&mut Store) -> R) -> R {
    STORE.with_borrow_mut(f)
}

/// A keyboard connected (its board profile was just looked up): its handed-over layouts, if
/// any, apply from now on.
pub fn connected(vid: u16, pid: u16) {
    with_store(|s| s.keyboard = Some((vid, pid)));
}

/// A source read keyboard (vid, pid)'s layout: it replaces what that source said before, and the
/// Scenes room is built again on it. Called where the keymap is read, never on a timer.
pub fn from_keyboard(vid: u16, pid: u16, layout: Supplied) {
    let changed = with_store(|s| {
        s.keyboard = Some((vid, pid));
        let rank = layout.source.rank();
        let old = s
            .supplied
            .iter()
            .position(|(id, l)| *id == (vid, pid) && l.source.rank() == rank);
        match old {
            Some(i) if s.supplied[i].1 == layout => false,
            Some(i) => {
                s.supplied[i].1 = layout;
                true
            }
            None => {
                s.supplied.push(((vid, pid), layout));
                true
            }
        }
    });
    if changed {
        super::scene_run::devices_changed();
    }
}

/// What the sources handed over for the keyboard connected now.
fn supplied_now() -> Vec<Supplied> {
    with_store(|s| {
        s.supplied
            .iter()
            .filter(|(id, _)| Some(*id) == s.keyboard)
            .map(|(_, l)| l.clone())
            .collect()
    })
}

/// The connected keyboard's layout, from the caches; asks the keyboard nothing.
pub fn current() -> KeyLayout {
    resolve(
        super::boards::current().as_ref(),
        &supplied_now(),
        super::boards::keymap_key_count(),
    )
}

/// `board`'s layout: the connected keyboard's when it is that board, else its profile's.
pub fn of(board: Option<&Board>) -> KeyLayout {
    let connected = super::boards::current().map(|b| b.id);
    let supplied = if board.map(|b| &b.id) == connected.as_ref() {
        supplied_now()
    } else {
        Vec::new()
    };
    resolve(board, &supplied, super::boards::keymap_key_count())
}

/// The source chip: A1's flat status chip (#793: no outline, a muted fill), its dot green for
/// the keyboard or a definition, none for the profile, amber for a guess; its hover the layout's
/// name and where it came from.
pub fn source_chip(ui: &mut egui::Ui, layout: &KeyLayout) -> egui::Response {
    super::theme::hint_chip(
        ui,
        &layout.source.chip(),
        Some(&hover(layout)),
        layout.source.dot(),
    )
}

/// The chip's hover: where the layout came from, in a sentence.
pub fn hover(layout: &KeyLayout) -> String {
    let mut s = match layout.source {
        LayoutSource::Keyboard => {
            "Drawn from the physical layout the keyboard reports (ZMK Studio)".to_string()
        }
        LayoutSource::Definition(_) => "Drawn from the definition you loaded".to_string(),
        LayoutSource::Profile => {
            "Drawn from Foxglove's board profile for this keyboard".to_string()
        }
        LayoutSource::Guessed(n) => {
            format!("No layout known for this keyboard: a plain grid of its {n} keys")
        }
    };
    if let Some(name) = &layout.name {
        s.push_str(&format!(": \u{201c}{name}\u{201d}"));
        if layout.choices.len() > 1 {
            s.push_str(&format!(
                " ({} of {})",
                layout.active + 1,
                layout.choices.len()
            ));
        }
    }
    s.push('.');
    if layout.mismatch {
        s.push_str(" Its key count differs from the board profile's, so zones from the profile may not match this layout.");
    }
    s
}

/// What keyboard (vid, pid) handed over, for the tests of other modules.
#[cfg(test)]
pub fn set_for_test(vid: u16, pid: u16, layout: Option<Supplied>) {
    with_store(|s| {
        s.keyboard = Some((vid, pid));
        s.supplied.clear();
        if let Some(l) = layout {
            s.supplied.push(((vid, pid), l));
        }
    });
}

#[cfg(test)]
#[path = "key_layout_tests.rs"]
pub(crate) mod tests;
