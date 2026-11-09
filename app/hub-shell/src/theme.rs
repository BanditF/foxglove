//! The hub look (#384, epic #393), shared by Foxglove and Hideout: named colour tokens for a dark
//! and a light theme, the bundled fonts, and egui `Visuals` built from them. The accent comes
//! from the caller (Foxglove: its palette file's roles).

use egui::{Color32, CornerRadius, FontFamily, FontId, Stroke, TextStyle};
/// One theme's colour tokens.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    pub dark: bool,
    /// The window ground.
    pub ground: Color32,
    /// Cards and sections on the ground.
    pub surface: Color32,
    /// Rows and wells inside a card (striped grids, code).
    pub surface_raised: Color32,
    /// The deepest fill: sidebars, text fields.
    pub well: Color32,
    /// Card outlines and separators.
    pub line: Color32,
    /// Read-only status chips (#793): between `surface` and `control`, with no outline.
    pub chip_fill: Color32,
    /// The same chips on the status strip, which sits on `ground` (#800): visibly apart from
    /// it in both themes, with `text` and `muted` still reading on it.
    pub strip_chip_fill: Color32,
    /// Buttons and other controls.
    pub control: Color32,
    pub control_line: Color32,
    pub text: Color32,
    pub text_strong: Color32,
    pub muted: Color32,
    /// Bandit orange: the palette file's `accent` (dark) or `accent_light` (light).
    pub accent: Color32,
    pub accent_hover: Color32,
    /// Text on an accent fill.
    pub on_accent: Color32,
    /// A selected pill: an accent border on this fill, with `selected_text`.
    pub selected_fill: Color32,
    pub selected_text: Color32,
    pub ok: Color32,
    pub warn: Color32,
    pub danger: Color32,
}

const fn hex(rgb: u32) -> Color32 {
    Color32::from_rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
}

/// The dark theme around an accent and its hover colour (a palette's `accent` and `highlight`).
pub fn dark(accent: Color32, accent_hover: Color32) -> Palette {
    Palette {
        dark: true,
        ground: hex(0x0e1211),
        surface: hex(0x161c1a),
        surface_raised: hex(0x1d2422),
        well: hex(0x0a0d0c),
        line: hex(0x2a3330),
        chip_fill: hex(0x1c2321),
        strip_chip_fill: hex(0x1c2321),
        control: hex(0x232b28),
        control_line: hex(0x34403b),
        text: hex(0xd9e0dc),
        text_strong: hex(0xf2f5f3),
        muted: hex(0x8e9a94),
        accent,
        accent_hover,
        on_accent: hex(0x140d07),
        selected_fill: hex(0x3a2412),
        selected_text: hex(0xffcfa6),
        ok: hex(0x5fc48e),
        warn: hex(0xf0b53c),
        danger: hex(0xe5604f),
    }
}

/// The light theme around an accent (a palette's `accent_light`).
pub fn light(accent: Color32) -> Palette {
    Palette {
        dark: false,
        ground: hex(0xf1f4f2),
        surface: hex(0xffffff),
        surface_raised: hex(0xe8ede9),
        well: hex(0xf8faf9),
        line: hex(0xcfd8d3),
        chip_fill: hex(0xf0f3f1),
        strip_chip_fill: hex(0xdde4df),
        control: hex(0xe8ede9),
        control_line: hex(0xb9c4be),
        text: hex(0x26302c),
        text_strong: hex(0x0e1211),
        muted: hex(0x5a6660),
        accent,
        accent_hover: hex(0xd8701f),
        on_accent: hex(0xffffff),
        selected_fill: hex(0xfbe4d0),
        selected_text: hex(0x6e3105),
        ok: hex(0x237a4b),
        warn: hex(0x8f6100),
        danger: hex(0xb8352a),
    }
}

impl Palette {
    /// Every token by name, for the palette checks.
    pub fn tokens(&self) -> Vec<(&'static str, Color32)> {
        vec![
            ("ground", self.ground),
            ("surface", self.surface),
            ("surface_raised", self.surface_raised),
            ("well", self.well),
            ("line", self.line),
            ("chip_fill", self.chip_fill),
            ("strip_chip_fill", self.strip_chip_fill),
            ("control", self.control),
            ("control_line", self.control_line),
            ("text", self.text),
            ("text_strong", self.text_strong),
            ("muted", self.muted),
            ("accent", self.accent),
            ("accent_hover", self.accent_hover),
            ("on_accent", self.on_accent),
            ("selected_fill", self.selected_fill),
            ("selected_text", self.selected_text),
            ("ok", self.ok),
            ("warn", self.warn),
            ("danger", self.danger),
        ]
    }
}

// Fonts. All three are under the SIL Open Font License 1.1 (app/resources/fonts/*-OFL.txt).
const HUD_FONT: &[u8] = include_bytes!("../../resources/fonts/ChakraPetch-SemiBold.ttf");
const BODY_FONT: &[u8] = include_bytes!("../../resources/fonts/IBMPlexSans-Regular.ttf");
const MONO_FONT: &[u8] = include_bytes!("../../resources/fonts/IBMPlexMono-Medium.ttf");

pub const HUD: &str = "foxglove-hud";
pub const BODY: &str = "foxglove-body";
pub const MONO: &str = "foxglove-mono";

pub fn family(name: &str) -> FontFamily {
    FontFamily::Name(name.into())
}

/// Chakra Petch, the HUD display face: page titles and section headings.
pub fn hud(size: f32) -> FontId {
    FontId::new(size, family(HUD))
}

/// IBM Plex Sans, for body text and controls.
pub fn body(size: f32) -> FontId {
    FontId::new(size, family(BODY))
}

/// IBM Plex Mono, for small labels, versions and numbers.
pub fn mono(size: f32) -> FontId {
    FontId::new(size, family(MONO))
}

/// Adds the three fonts as their own families. Each falls back to egui's fonts (and the
/// Phosphor icons, when they're already in `Proportional`) for glyphs it lacks.
pub fn add_fonts(fonts: &mut egui::FontDefinitions) {
    let proportional = fonts
        .families
        .get(&FontFamily::Proportional)
        .cloned()
        .unwrap_or_default();
    let monospace = fonts
        .families
        .get(&FontFamily::Monospace)
        .cloned()
        .unwrap_or_default();
    for (name, bytes, fallback) in [
        (HUD, HUD_FONT, &proportional),
        (BODY, BODY_FONT, &proportional),
        (MONO, MONO_FONT, &monospace),
    ] {
        fonts.font_data.insert(
            name.to_owned(),
            std::sync::Arc::new(egui::FontData::from_static(bytes)),
        );
        let mut list = vec![name.to_owned()];
        list.extend(fallback.iter().cloned());
        if !list.iter().any(|f| f == "phosphor") && fonts.font_data.contains_key("phosphor") {
            list.push("phosphor".to_owned());
        }
        fonts.families.insert(family(name), list);
    }
}

/// egui `Visuals` for a palette: cards on graphite, orange for focus, selection and sliders.
pub fn visuals(p: &Palette) -> egui::Visuals {
    let mut v = if p.dark {
        egui::Visuals::dark()
    } else {
        egui::Visuals::light()
    };
    let r6 = CornerRadius::same(6);
    let r8 = CornerRadius::same(8);

    v.weak_text_color = Some(p.muted);
    v.hyperlink_color = p.accent;
    v.faint_bg_color = p.surface_raised;
    v.extreme_bg_color = p.well;
    v.text_edit_bg_color = Some(p.well);
    v.code_bg_color = p.surface_raised;
    v.warn_fg_color = p.warn;
    v.error_fg_color = p.danger;
    v.window_fill = p.ground;
    v.panel_fill = p.ground;
    v.window_stroke = Stroke::new(1.0, p.line);
    v.window_corner_radius = r8;
    v.menu_corner_radius = r8;
    let shadow = Color32::from_black_alpha(if p.dark { 140 } else { 40 });
    v.window_shadow = egui::Shadow {
        offset: [0, 8],
        blur: 24,
        spread: 0,
        color: shadow,
    };
    v.popup_shadow = egui::Shadow {
        offset: [0, 4],
        blur: 12,
        spread: 0,
        color: shadow,
    };
    v.selection.bg_fill = p.accent;
    v.selection.stroke = Stroke::new(1.0, p.on_accent);
    v.text_cursor.stroke = Stroke::new(2.0, p.accent);
    v.slider_trailing_fill = true;

    let w = &mut v.widgets;
    w.noninteractive.bg_fill = p.surface;
    w.noninteractive.weak_bg_fill = p.surface;
    w.noninteractive.bg_stroke = Stroke::new(1.0, p.line);
    w.noninteractive.fg_stroke = Stroke::new(1.0, p.text);
    w.noninteractive.corner_radius = r6;

    w.inactive.bg_fill = p.control;
    w.inactive.weak_bg_fill = p.control;
    w.inactive.bg_stroke = Stroke::new(1.0, p.control_line);
    w.inactive.fg_stroke = Stroke::new(1.0, p.text);
    w.inactive.corner_radius = r6;

    w.hovered.bg_fill = p.control;
    w.hovered.weak_bg_fill = p.control;
    w.hovered.bg_stroke = Stroke::new(1.0, p.accent);
    w.hovered.fg_stroke = Stroke::new(1.5, p.text_strong);
    w.hovered.corner_radius = r6;

    // Pressed and keyboard focus: a 2 px orange ring, the focus ring (#384).
    w.active.bg_fill = p.accent;
    w.active.weak_bg_fill = p.selected_fill;
    w.active.bg_stroke = Stroke::new(2.0, p.accent);
    w.active.fg_stroke = Stroke::new(2.0, p.text_strong);
    w.active.corner_radius = r6;

    w.open.bg_fill = p.control;
    w.open.weak_bg_fill = p.control;
    w.open.bg_stroke = Stroke::new(1.0, p.accent);
    w.open.fg_stroke = Stroke::new(1.0, p.text_strong);
    w.open.corner_radius = r6;
    v
}

/// Text styles on the bundled fonts, and a little more room in controls.
pub fn style_text(style: &mut egui::Style) {
    style.text_styles = [
        (TextStyle::Small, body(11.0)),
        (TextStyle::Body, body(13.5)),
        (TextStyle::Button, body(13.5)),
        (TextStyle::Monospace, mono(12.5)),
        (TextStyle::Heading, hud(19.0)),
    ]
    .into();
    style.spacing.button_padding = egui::vec2(8.0, 3.0);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fonts_get_their_own_families_and_leave_the_overlay_font_alone() {
        let mut fonts = egui::FontDefinitions::default();
        let before = fonts.families[&FontFamily::Proportional].clone();
        add_fonts(&mut fonts);
        assert_eq!(fonts.families[&FontFamily::Proportional], before);
        for name in [HUD, BODY, MONO] {
            assert_eq!(fonts.families[&family(name)][0], name);
            assert!(fonts.font_data.contains_key(name));
        }
    }

    #[test]
    fn the_accent_is_the_callers() {
        let orange = Color32::from_rgb(0xf2, 0x84, 0x2c);
        assert_eq!(dark(orange, orange).accent, orange);
        assert_eq!(light(orange).accent, orange);
        assert!(dark(orange, orange).dark && !light(orange).dark);
    }
}
