//! Which controls each lighting effect uses (#670), as data, for every class of effect the app
//! drives: a keyboard's own underglow effects (ZMK's, by number), Windows Dynamic Lighting
//! (LampArray) devices, and the PC's other lights (built-in drivers). Systems, not products:
//! the pages ask this table what an effect takes and show only those controls, and an effect
//! that makes its own colours (a rainbow) gets a rainbow preview where the colour picker was.
//!
//! Firmware quirks live here as data, not as code in the pages. A keyboard that says which of
//! its effects use the chosen colour (RGB_STATE byte 29, HELLO bit 29, #626: Swirl as a band of
//! hues around the colour) overrides the underglow rows by number ([`underglow_as`]); without
//! that report the table is the only source.
//!
//! What the pages show is the keyboard's report of what it is showing (RGB_STATE, D7 83), never
//! the setting Foxglove saved: [`Showing::new`] is that rule.

use egui::{Color32, Rect};
use foxglove_link::protocol::RgbState;

/// The controls an effect uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Controls {
    pub hue: bool,
    pub sat: bool,
    pub brightness: bool,
    pub speed: bool,
}

impl Controls {
    /// One colour, steady: hue, saturation and brightness.
    pub const COLOUR: Controls = Controls {
        hue: true,
        sat: true,
        brightness: true,
        speed: false,
    };
    /// One colour, moving: all four.
    pub const COLOUR_MOVING: Controls = Controls {
        speed: true,
        ..Controls::COLOUR
    };
    /// Its own colours (a rainbow), moving: brightness and speed.
    pub const OWN_COLOURS: Controls = Controls {
        hue: false,
        sat: false,
        brightness: true,
        speed: true,
    };

    /// It shows a colour the user picks (hue or saturation).
    pub const fn colour(self) -> bool {
        self.hue || self.sat
    }
}

/// What kind of device runs the effect.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EffectClass {
    /// A keyboard's own underglow effects (ZMK's `&rgb_ug`), by number.
    Underglow,
    /// Windows Dynamic Lighting (LampArray): Foxglove sets each lamp's colour.
    LampArray,
    /// The PC's other lights through a built-in driver (the case, a cooler).
    PcLights,
}

/// One effect, as data.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EffectSpec {
    pub class: EffectClass,
    /// The id the device takes (`foxglove_devices::Effect::id`): ZMK's number as text.
    pub id: &'static str,
    pub name: &'static str,
    pub controls: Controls,
    /// What its name doesn't say, shown after it.
    pub note: Option<&'static str>,
    /// It moves a band of hues around the chosen colour ([`BAND`] either side): the preview is
    /// that band, not one colour.
    pub band: bool,
}

impl EffectSpec {
    /// It shows the chosen colour (hue and saturation).
    pub const fn uses_colour(&self) -> bool {
        self.controls.colour()
    }

    /// It makes its own colours: the pages show a rainbow in place of the colour picker.
    pub const fn rainbow(&self) -> bool {
        !self.uses_colour()
    }

    /// The preview of it lit with this colour: the band, the colour, or a rainbow.
    pub const fn preview(&self, hue: u16, sat: u8, brightness: u8) -> Preview {
        if self.rainbow() {
            Preview::Rainbow
        } else if self.band {
            Preview::Band(hue, sat, brightness)
        } else {
            Preview::Colour(hue, sat, brightness)
        }
    }

    /// Its name with its note: "Swirl (rainbow; colour not used yet)".
    pub fn label(&self) -> String {
        match self.note {
            Some(note) => format!("{} ({note})", self.name),
            None => self.name.to_string(),
        }
    }
}

const RAINBOW: &str = "rainbow; colour not used yet";

/// Degrees of hue either side of the chosen colour that a band effect moves through: the
/// firmware's default (`CONFIG_FOXGLOVE_COLOUR_SWIRL_BAND`, #626).
pub const BAND: u16 = 30;

/// ZMK's underglow effects by number: 0 solid, 1 breathe, 2 spectrum, 3 swirl. Solid stands
/// still (no speed); Spectrum and Swirl cycle the hue themselves and ignore the colour (#616).
/// Stock ZMK, and Foxglove firmware before #626 (it doesn't report byte 29).
pub const UNDERGLOW: [EffectSpec; 4] = [
    EffectSpec {
        class: EffectClass::Underglow,
        id: "0",
        name: "Solid",
        controls: Controls::COLOUR,
        note: None,
        band: false,
    },
    EffectSpec {
        class: EffectClass::Underglow,
        id: "1",
        name: "Breathe",
        controls: Controls::COLOUR_MOVING,
        note: None,
        band: false,
    },
    EffectSpec {
        class: EffectClass::Underglow,
        id: "2",
        name: "Spectrum",
        controls: Controls::OWN_COLOURS,
        note: Some(RAINBOW),
        band: false,
    },
    EffectSpec {
        class: EffectClass::Underglow,
        id: "3",
        name: "Swirl",
        controls: Controls::OWN_COLOURS,
        note: Some(RAINBOW),
        band: false,
    },
];

/// ZMK's underglow effects as a keyboard draws them when it says they use the chosen colour
/// (RGB_STATE byte 29, #626): Swirl moves a band of hues around the colour, so it takes hue,
/// saturation, brightness and speed. Spectrum's row is the same, for firmware that says so of
/// it; Foxglove's firmware keeps Spectrum the rainbow. Solid and Breathe are as in
/// [`UNDERGLOW`].
pub const UNDERGLOW_IN_COLOUR: [EffectSpec; 4] = [
    UNDERGLOW[0],
    UNDERGLOW[1],
    EffectSpec {
        class: EffectClass::Underglow,
        id: "2",
        name: "Spectrum",
        controls: Controls::COLOUR_MOVING,
        note: None,
        band: true,
    },
    EffectSpec {
        class: EffectClass::Underglow,
        id: "3",
        name: "Swirl",
        controls: Controls::COLOUR_MOVING,
        note: None,
        band: true,
    },
];

/// Windows Dynamic Lighting has no effects of its own: Foxglove sets each lamp's colour.
pub const LAMP_ARRAY: [EffectSpec; 1] = [EffectSpec {
    class: EffectClass::LampArray,
    id: "static",
    name: "Colour",
    controls: Controls::COLOUR,
    note: None,
    band: false,
}];

/// The PC's other lights through a built-in driver: one colour per zone, steady.
pub const PC_LIGHTS: [EffectSpec; 1] = [EffectSpec {
    class: EffectClass::PcLights,
    id: "static",
    name: "Colour",
    controls: Controls::COLOUR,
    note: None,
    band: false,
}];

/// A class's effects, in the order the pages list them.
pub fn effects(class: EffectClass) -> &'static [EffectSpec] {
    match class {
        EffectClass::Underglow => &UNDERGLOW,
        EffectClass::LampArray => &LAMP_ARRAY,
        EffectClass::PcLights => &PC_LIGHTS,
    }
}

/// Effect `id` of `class`, if the table has it.
pub fn spec(class: EffectClass, id: &str) -> Option<&'static EffectSpec> {
    effects(class).iter().find(|e| e.id == id)
}

/// Underglow effect number `n`, as stock ZMK draws it.
pub fn underglow(n: u8) -> Option<&'static EffectSpec> {
    UNDERGLOW.get(usize::from(n))
}

/// Underglow effect number `n` as the keyboard reports drawing it: `colour_effects` is
/// [`RgbState::colour_effects`] (one bit per effect number), `None` for firmware that doesn't
/// say, which gets [`UNDERGLOW`].
pub fn underglow_as(n: u8, colour_effects: Option<u8>) -> Option<&'static EffectSpec> {
    let in_colour = colour_effects.is_some_and(|m| n < 8 && m & (1 << n) != 0);
    if in_colour {
        UNDERGLOW_IN_COLOUR.get(usize::from(n))
    } else {
        underglow(n)
    }
}

/// Underglow effect number `n` as the keyboard in `state` draws it ([`underglow_as`]).
pub fn underglow_in(state: Option<&RgbState>, n: u8) -> Option<&'static EffectSpec> {
    underglow_as(n, state.and_then(|s| s.colour_effects))
}

/// The look Foxglove saved for the keyboard (what the Lights page or the followed accent last
/// set), for the "the keyboard shows X, saved Y" note. Only the fields that were set.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Saved {
    pub effect: Option<u8>,
    pub hue: Option<u16>,
    pub sat: Option<u8>,
}

/// What a keyboard's lights show, for the preview and the controls.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Showing {
    /// The effect on show; `None` before the first report or for one the table doesn't have.
    pub effect: Option<&'static EffectSpec>,
    /// The controls to show: the effect's, or all four for an unknown effect.
    pub controls: Controls,
    pub preview: Preview,
}

/// The preview where the colour picker would be.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Preview {
    /// One colour: hue 0-359, saturation and brightness 0-100.
    Colour(u16, u8, u8),
    /// A band of hues around this colour ([`BAND`] either side), moving (#626).
    Band(u16, u8, u8),
    /// The effect makes its own colours.
    Rainbow,
    /// The LEDs are dark.
    Dark,
    /// Nothing reported yet.
    Unknown,
}

impl Showing {
    /// From the keyboard's last report (its left half: the one the PC talks to). The report
    /// wins: `saved` never changes what is shown, only whether [`Self::differs`] says so.
    pub fn new(state: Option<&RgbState>) -> Showing {
        let Some(left) = state.map(|s| s.left) else {
            return Showing {
                effect: None,
                controls: Controls::COLOUR_MOVING,
                preview: Preview::Unknown,
            };
        };
        let effect = underglow_in(state, left.effect);
        let controls = effect.map_or(Controls::COLOUR_MOVING, |e| e.controls);
        let preview = if !left.on {
            Preview::Dark
        } else {
            effect.map_or(Preview::Colour(left.hue, left.sat, left.brightness), |e| {
                e.preview(left.hue, left.sat, left.brightness)
            })
        };
        Showing {
            effect,
            controls,
            preview,
        }
    }

    /// The saved look differs from what the keyboard shows (a key combo, another tool, a
    /// re-link): its words, such as "Solid, amber", or `None` when they agree or nothing was
    /// saved. Only the fields the shown effect uses count: a colour saved under a rainbow
    /// isn't a difference anyone can see.
    pub fn differs(&self, state: Option<&RgbState>, saved: Option<&Saved>) -> Option<String> {
        let (left, saved) = (state?.left, saved?);
        let shown = self.effect?;
        let effect_differs = saved.effect.is_some_and(|e| e != left.effect);
        let colour_differs = shown.uses_colour()
            && (saved.hue.is_some_and(|h| hue_apart(h, left.hue) > 3)
                || saved.sat.is_some_and(|s| s.abs_diff(left.sat) > 3));
        if !effect_differs && !colour_differs {
            return None;
        }
        let saved_effect = saved
            .effect
            .and_then(|e| underglow_as(e, state.and_then(|s| s.colour_effects)));
        let mut words = saved_effect.map_or(shown.name, |e| e.name).to_string();
        // The saved colour, when the saved effect shows one.
        if saved_effect.unwrap_or(shown).uses_colour() {
            if let Some(h) = saved.hue {
                words.push_str(&format!(", {}", colour_name(h, saved.sat.unwrap_or(100))));
            }
        }
        Some(words)
    }
}

fn hue_apart(a: u16, b: u16) -> u16 {
    let d = (i32::from(a) - i32::from(b)).rem_euclid(360) as u16;
    d.min(360 - d)
}

/// A colour's plain name from its hue and saturation: "white", "amber", "blue".
pub fn colour_name(hue: u16, sat: u8) -> &'static str {
    if sat < 15 {
        return "white";
    }
    match hue % 360 {
        0..=14 | 345..=359 => "red",
        15..=34 => "orange",
        35..=54 => "amber",
        55..=69 => "yellow",
        70..=164 => "green",
        165..=194 => "cyan",
        195..=254 => "blue",
        255..=284 => "purple",
        _ => "pink",
    }
}

/// Paints a rainbow across `rect` (left to right, red back to red), with rounded ends when
/// `round`.
pub fn paint_rainbow(painter: &egui::Painter, rect: Rect, round: bool) {
    const STOPS: usize = 24;
    let mut mesh = egui::Mesh::default();
    for i in 0..=STOPS {
        let t = i as f32 / STOPS as f32;
        let c: Color32 = egui::ecolor::HsvaGamma {
            h: t,
            s: 0.85,
            v: 1.0,
            a: 1.0,
        }
        .into();
        let x = rect.left() + t * rect.width();
        mesh.colored_vertex(egui::pos2(x, rect.top()), c);
        mesh.colored_vertex(egui::pos2(x, rect.bottom()), c);
        if i > 0 {
            let n = (i as u32) * 2;
            mesh.add_triangle(n - 2, n - 1, n);
            mesh.add_triangle(n - 1, n, n + 1);
        }
    }
    painter.add(mesh);
    if round {
        // Rounded corners: the page's background over the corners would need its colour, so
        // a thin outline in the card's line colour softens them instead.
        let line = Color32::from_black_alpha(60);
        painter.rect_stroke(
            rect,
            3.0,
            egui::Stroke::new(1.0, line),
            egui::StrokeKind::Inside,
        );
    }
}

/// A rainbow preview of `size`, allocated in `ui`.
pub fn rainbow(ui: &mut egui::Ui, size: egui::Vec2) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::hover());
    paint_rainbow(ui.painter(), rect, true);
    response
}

/// The band's colours across `rect`: hue `hue - BAND` on the left to `hue + BAND` on the right
/// through the colour itself in the middle, at the given saturation and brightness (0-100),
/// as the keyboard's colour Swirl moves them (#626). `shade` maps an HSB colour to the screen
/// (the pages' `shown_color`, so the band and the swatches agree).
pub fn paint_band(
    painter: &egui::Painter,
    rect: Rect,
    (hue, sat, brightness): (u16, u8, u8),
    shade: impl Fn(u16, u8, u8) -> Color32,
) {
    const STOPS: i32 = 12;
    let mut mesh = egui::Mesh::default();
    for i in 0..=STOPS {
        let t = i as f32 / STOPS as f32;
        let offset = (2 * i - STOPS) * i32::from(BAND) / STOPS;
        let h = (i32::from(hue) + offset).rem_euclid(360) as u16;
        let c = shade(h, sat, brightness);
        let x = rect.left() + t * rect.width();
        mesh.colored_vertex(egui::pos2(x, rect.top()), c);
        mesh.colored_vertex(egui::pos2(x, rect.bottom()), c);
        if i > 0 {
            let n = (i as u32) * 2;
            mesh.add_triangle(n - 2, n - 1, n);
            mesh.add_triangle(n - 1, n, n + 1);
        }
    }
    painter.add(mesh);
}

/// The preview of `preview` in `rect`: a band, a colour, a rainbow, or dark (nothing known
/// reads dark too). `shade` as for [`paint_band`].
pub fn paint_preview(
    painter: &egui::Painter,
    rect: Rect,
    preview: Preview,
    shade: impl Fn(u16, u8, u8) -> Color32,
) {
    match preview {
        Preview::Rainbow => paint_rainbow(painter, rect, true),
        Preview::Band(h, s, b) => paint_band(painter, rect, (h, s, b), shade),
        Preview::Colour(h, s, b) => {
            painter.rect_filled(rect, 3.0, shade(h, s, b));
        }
        Preview::Dark | Preview::Unknown => {
            painter.rect_filled(rect, 3.0, Color32::from_gray(20));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use foxglove_link::protocol::RgbHalf;

    fn state(on: bool, effect: u8, hue: u16, sat: u8) -> RgbState {
        let h = RgbHalf {
            on,
            hue,
            sat,
            brightness: 42,
            effect,
            speed: 3,
        };
        RgbState {
            left: h,
            right: Some((h, 0)),
            colour_effects: None,
        }
    }

    /// The same, from firmware that draws Swirl in the colour (#626: byte 29 = 0x0B).
    fn colour_state(on: bool, effect: u8, hue: u16, sat: u8) -> RgbState {
        RgbState {
            colour_effects: Some(0x0B),
            ..state(on, effect, hue, sat)
        }
    }

    /// Every class has a table, every row is found by its class and id, and ids are unique
    /// within a class.
    #[test]
    fn every_effect_class_has_its_controls_as_data() {
        for class in [
            EffectClass::Underglow,
            EffectClass::LampArray,
            EffectClass::PcLights,
        ] {
            let rows = effects(class);
            assert!(!rows.is_empty(), "{class:?}");
            for row in rows {
                assert_eq!(row.class, class);
                assert_eq!(spec(class, row.id), Some(row));
                assert!(row.controls.brightness, "{} keeps brightness", row.name);
            }
        }
        assert_eq!(spec(EffectClass::LampArray, "swirl"), None);
        assert!(spec(EffectClass::LampArray, "static")
            .unwrap()
            .uses_colour());
        assert!(
            !spec(EffectClass::PcLights, "static")
                .unwrap()
                .controls
                .speed
        );
    }

    /// ZMK's effects by number: Solid is one still colour, Breathe a moving one, Spectrum and
    /// Swirl make their own colours (a rainbow: no hue or saturation, brightness and speed
    /// stay).
    #[test]
    fn the_underglow_table_says_what_each_effect_uses() {
        let names: Vec<_> = UNDERGLOW.iter().map(|e| e.name).collect();
        assert_eq!(names, ["Solid", "Breathe", "Spectrum", "Swirl"]);
        for (n, e) in (0u8..).zip(&UNDERGLOW) {
            assert_eq!(e.id, n.to_string(), "the id is ZMK's number");
            assert_eq!(underglow(n), Some(e));
        }
        assert_eq!(underglow(0).unwrap().controls, Controls::COLOUR);
        assert_eq!(underglow(1).unwrap().controls, Controls::COLOUR_MOVING);
        for n in [2, 3] {
            let e = underglow(n).unwrap();
            assert!(e.rainbow());
            assert!(!e.controls.hue && !e.controls.sat);
            assert!(e.controls.brightness && e.controls.speed);
        }
        assert_eq!(underglow(4), None);
    }

    /// The preview follows the keyboard's report: a rainbow for Swirl, its colour for Solid,
    /// dark when the LEDs are dark, and all controls for an effect the table doesn't know.
    #[test]
    fn the_preview_follows_the_report() {
        let swirl = Showing::new(Some(&state(true, 3, 39, 70)));
        assert_eq!(swirl.preview, Preview::Rainbow);
        assert_eq!(swirl.controls, Controls::OWN_COLOURS);
        let solid = Showing::new(Some(&state(true, 0, 39, 70)));
        assert_eq!(solid.preview, Preview::Colour(39, 70, 42));
        assert!(!solid.controls.speed);
        assert_eq!(
            Showing::new(Some(&state(false, 3, 39, 70))).preview,
            Preview::Dark
        );
        let odd = Showing::new(Some(&state(true, 9, 39, 70)));
        assert_eq!(odd.effect, None);
        assert_eq!(odd.controls, Controls::COLOUR_MOVING);
        assert_eq!(Showing::new(None).preview, Preview::Unknown);
    }

    /// #670: the board shows a rainbow Swirl while Foxglove saved Solid gold. The shown state
    /// wins: the preview is the rainbow and the controls are Swirl's; the saved look is only
    /// named. A key combo or a re-link that changes the board changes the preview the same way.
    #[test]
    fn the_shown_state_wins_over_the_saved_setting() {
        let saved = Saved {
            effect: Some(0),
            hue: Some(39),
            sat: Some(70),
        };
        let board = state(true, 3, 39, 70);
        let s = Showing::new(Some(&board));
        assert_eq!(s.preview, Preview::Rainbow, "not the saved gold");
        assert_eq!(s.effect.unwrap().name, "Swirl");
        assert_eq!(
            s.differs(Some(&board), Some(&saved)).as_deref(),
            Some("Solid, amber")
        );
        // The keyboard switched to Solid red on a key: the preview is red, and the saved
        // amber is named.
        let red = state(true, 0, 0, 100);
        let s = Showing::new(Some(&red));
        assert_eq!(s.preview, Preview::Colour(0, 100, 42));
        assert_eq!(
            s.differs(Some(&red), Some(&saved)).as_deref(),
            Some("Solid, amber")
        );
        // They agree (within the keyboard's rounding): no note.
        let same = state(true, 0, 41, 68);
        assert_eq!(
            Showing::new(Some(&same)).differs(Some(&same), Some(&saved)),
            None
        );
        // A colour saved under a rainbow isn't a visible difference.
        let rainbow_saved = Saved {
            effect: Some(3),
            hue: Some(200),
            sat: Some(90),
        };
        assert_eq!(
            s_for(&board).differs(Some(&board), Some(&rainbow_saved)),
            None
        );
        // Nothing saved, or nothing reported: no note.
        assert_eq!(s_for(&board).differs(Some(&board), None), None);
        assert_eq!(Showing::new(None).differs(None, Some(&saved)), None);
    }

    fn s_for(s: &RgbState) -> Showing {
        Showing::new(Some(s))
    }

    /// #626: firmware that says it draws Swirl in the chosen colour gets Swirl with all four
    /// controls and a band preview of that colour; Spectrum stays the rainbow, Solid and
    /// Breathe as before. Firmware that doesn't say keeps the rainbow Swirl.
    #[test]
    fn a_colour_swirl_uses_the_colour_when_the_keyboard_says_so() {
        let swirl = underglow_as(3, Some(0x0B)).unwrap();
        assert_eq!(swirl.name, "Swirl");
        assert_eq!(swirl.controls, Controls::COLOUR_MOVING);
        assert!(swirl.uses_colour() && swirl.band && swirl.note.is_none());
        assert_eq!(swirl.label(), "Swirl");
        assert!(
            underglow_as(2, Some(0x0B)).unwrap().rainbow(),
            "Spectrum: no bit 2"
        );
        assert_eq!(underglow_as(0, Some(0x0B)), underglow(0));
        assert_eq!(underglow_as(1, Some(0x0B)), underglow(1));
        assert!(
            underglow_as(3, None).unwrap().rainbow(),
            "firmware before #626"
        );
        assert!(
            underglow_as(3, Some(0x03)).unwrap().rainbow(),
            "a report without bit 3"
        );
        assert_eq!(underglow_as(9, Some(0xFF)), None);

        let s = Showing::new(Some(&colour_state(true, 3, 39, 70)));
        assert_eq!(
            s.preview,
            Preview::Band(39, 70, 42),
            "the chosen colour's band"
        );
        assert_eq!(s.controls, Controls::COLOUR_MOVING);
        let old = Showing::new(Some(&state(true, 3, 39, 70)));
        assert_eq!(old.preview, Preview::Rainbow);
        let spectrum = Showing::new(Some(&colour_state(true, 2, 39, 70)));
        assert_eq!(spectrum.preview, Preview::Rainbow);
        assert_eq!(
            Showing::new(Some(&colour_state(false, 3, 39, 70))).preview,
            Preview::Dark
        );

        // A colour saved under the colour Swirl is a difference anyone can see.
        let board = colour_state(true, 3, 39, 70);
        let saved = Saved {
            effect: Some(3),
            hue: Some(200),
            sat: Some(90),
        };
        assert_eq!(
            s_for(&board).differs(Some(&board), Some(&saved)).as_deref(),
            Some("Swirl, blue")
        );
    }

    /// The band runs from BAND below the colour to BAND above it, through the colour.
    #[test]
    fn the_band_preview_spans_the_band_around_the_colour() {
        let ctx = egui::Context::default();
        let seen = std::cell::RefCell::new(Vec::new());
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
            let rect = Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(120.0, 10.0));
            paint_band(ui.painter(), rect, (10, 70, 42), |h, _, _| {
                seen.borrow_mut().push(h);
                Color32::from_gray(h as u8)
            });
        });
        let seen = seen.into_inner();
        assert_eq!(seen.first(), Some(&340), "BAND below, across 0");
        assert_eq!(seen.last(), Some(&40));
        assert!(seen.contains(&10), "through the colour");
        assert!(
            seen.iter().all(|&h| hue_apart(h, 10) <= BAND),
            "never outside the band"
        );
    }

    #[test]
    fn colours_have_plain_names() {
        assert_eq!(colour_name(39, 70), "amber");
        assert_eq!(colour_name(26, 76), "orange");
        assert_eq!(colour_name(210, 50), "blue");
        assert_eq!(colour_name(0, 5), "white");
        assert_eq!(colour_name(355, 100), "red");
    }
}
