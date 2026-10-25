//! The app's look (#384, epic #393): named colour tokens for a dark and a light theme, the
//! bundled fonts, and egui `Visuals` built from them. Pages take their colours and fonts from
//! here ([`palette`], [`hud`], [`body`], [`mono`]) instead of writing hex values of their own.
//!
//! The layer overlay keeps its own look: its key labels use `FontFamily::Proportional`, which
//! stays egui's default font. Only the text styles (`Body`, `Heading`, ...) move to the new fonts.

use egui::{Color32, CornerRadius, FontFamily, FontId, RichText, Stroke, TextStyle};
use serde::{Deserialize, Serialize};

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
    /// Buttons and other controls.
    pub control: Color32,
    pub control_line: Color32,
    pub text: Color32,
    pub text_strong: Color32,
    pub muted: Color32,
    /// Bandit orange.
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

/// Dark graphite with Bandit orange, as drawn on the approved canvas.
pub const DARK: Palette = Palette {
    dark: true,
    ground: hex(0x0e1211),
    surface: hex(0x161c1a),
    surface_raised: hex(0x1d2422),
    well: hex(0x0a0d0c),
    line: hex(0x2a3330),
    control: hex(0x232b28),
    control_line: hex(0x34403b),
    text: hex(0xd9e0dc),
    text_strong: hex(0xf2f5f3),
    muted: hex(0x8e9a94),
    accent: hex(0xf08a3c),
    accent_hover: hex(0xffb070),
    on_accent: hex(0x140d07),
    selected_fill: hex(0x3a2412),
    selected_text: hex(0xffcfa6),
    ok: hex(0x5fc48e),
    warn: hex(0xf0b53c),
    danger: hex(0xe5604f),
};

/// The same tokens on a light ground. The orange is darker so text in it still reads.
pub const LIGHT: Palette = Palette {
    dark: false,
    ground: hex(0xf1f4f2),
    surface: hex(0xffffff),
    surface_raised: hex(0xe8ede9),
    well: hex(0xf8faf9),
    line: hex(0xcfd8d3),
    control: hex(0xe8ede9),
    control_line: hex(0xb9c4be),
    text: hex(0x26302c),
    text_strong: hex(0x0e1211),
    muted: hex(0x5a6660),
    accent: hex(0xb4540f),
    accent_hover: hex(0xd8701f),
    on_accent: hex(0xffffff),
    selected_fill: hex(0xfbe4d0),
    selected_text: hex(0x6e3105),
    ok: hex(0x237a4b),
    warn: hex(0x8f6100),
    danger: hex(0xb8352a),
};

impl Palette {
    /// Every token by name, for checks and previews.
    #[allow(dead_code)] // used by the tests and the pages that follow (#385 on)
    pub fn tokens(&self) -> Vec<(&'static str, Color32)> {
        vec![
            ("ground", self.ground),
            ("surface", self.surface),
            ("surface_raised", self.surface_raised),
            ("well", self.well),
            ("line", self.line),
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

/// The palette of the theme `ui` is drawn in.
pub fn palette(ui: &egui::Ui) -> &'static Palette {
    if ui.visuals().dark_mode {
        &DARK
    } else {
        &LIGHT
    }
}

// Fonts. All three are under the SIL Open Font License 1.1 (app/resources/fonts/*-OFL.txt).
const HUD_FONT: &[u8] = include_bytes!("../../resources/fonts/ChakraPetch-SemiBold.ttf");
const BODY_FONT: &[u8] = include_bytes!("../../resources/fonts/IBMPlexSans-Regular.ttf");
const MONO_FONT: &[u8] = include_bytes!("../../resources/fonts/IBMPlexMono-Medium.ttf");

const HUD: &str = "foxglove-hud";
const BODY: &str = "foxglove-body";
const MONO: &str = "foxglove-mono";

fn family(name: &str) -> FontFamily {
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
    let proportional = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    let monospace = fonts.families.get(&FontFamily::Monospace).cloned().unwrap_or_default();
    for (name, bytes, fallback) in [
        (HUD, HUD_FONT, &proportional),
        (BODY, BODY_FONT, &proportional),
        (MONO, MONO_FONT, &monospace),
    ] {
        fonts.font_data.insert(name.to_owned(), std::sync::Arc::new(egui::FontData::from_static(bytes)));
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
    let mut v = if p.dark { egui::Visuals::dark() } else { egui::Visuals::light() };
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
    v.window_shadow = egui::Shadow { offset: [0, 8], blur: 24, spread: 0, color: shadow };
    v.popup_shadow = egui::Shadow { offset: [0, 4], blur: 12, spread: 0, color: shadow };
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
fn style_text(style: &mut egui::Style) {
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

/// Which theme the windows use; the Theme section picks it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Choice {
    #[default]
    Dark,
    Light,
    /// Follow Windows' app mode.
    System,
}

impl Choice {
    fn preference(self) -> egui::ThemePreference {
        match self {
            Choice::Dark => egui::ThemePreference::Dark,
            Choice::Light => egui::ThemePreference::Light,
            Choice::System => egui::ThemePreference::System,
        }
    }
}

/// The store's `[ui]` section.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct UiSettings {
    theme: Choice,
}

/// Sets both themes' styles on `ctx` and picks `choice`. Call after `set_fonts` with
/// [`add_fonts`] applied.
pub fn apply_choice(ctx: &egui::Context, choice: Choice) {
    ctx.set_visuals_of(egui::Theme::Dark, visuals(&DARK));
    ctx.set_visuals_of(egui::Theme::Light, visuals(&LIGHT));
    ctx.all_styles_mut(style_text);
    ctx.set_theme(choice.preference());
}

/// Startup: the saved theme choice (dark when none is saved).
pub fn apply(ctx: &egui::Context) {
    let saved = super::store::try_load::<UiSettings>("ui").unwrap_or_default();
    apply_choice(ctx, saved.theme);
}

/// A section or card heading in the HUD face.
pub fn heading(ui: &egui::Ui, text: &str) -> RichText {
    RichText::new(text).font(hud(15.0)).color(palette(ui).text_strong)
}

/// A small upper-case label (`EFFECT`, `LIBRARY`) in the mono face.
#[allow(dead_code)] // for the pages that follow (#385 on)
pub fn caps_label(ui: &egui::Ui, text: &str) -> RichText {
    RichText::new(text.to_uppercase()).font(mono(11.0)).color(palette(ui).muted)
}

/// A card: a surface with a line around it.
pub fn card(ui: &egui::Ui) -> egui::Frame {
    let p = palette(ui);
    egui::Frame::new()
        .fill(p.surface)
        .stroke(Stroke::new(1.0, p.line))
        .corner_radius(CornerRadius::same(8))
}

/// A pill: when selected, an orange border on a dark-orange fill.
pub fn pill(ui: &mut egui::Ui, selected: bool, text: &str) -> egui::Response {
    let pill = pill_button(ui, selected, text);
    ui.add(pill)
}

/// The pill as a widget, for `add_enabled` (a disabled pill in a wrapping row, #388).
pub fn pill_button(ui: &egui::Ui, selected: bool, text: &str) -> egui::Button<'static> {
    let p = palette(ui);
    let (fill, line, color) = if selected {
        (p.selected_fill, p.accent, p.selected_text)
    } else {
        (p.control, p.control_line, p.text)
    };
    egui::Button::new(RichText::new(text.to_owned()).color(color))
        .fill(fill)
        .stroke(Stroke::new(1.0, line))
        .corner_radius(CornerRadius::same(99))
}

/// The main action of a card: an orange button.
#[allow(dead_code)] // for the pages that follow (#385 on)
pub fn primary_button(ui: &egui::Ui, text: &str) -> egui::Button<'static> {
    let p = palette(ui);
    egui::Button::new(RichText::new(text.to_owned()).color(p.on_accent)).fill(p.accent).stroke(Stroke::new(1.0, p.accent))
}

/// The window theme picker, saved in the store's `[ui]` section.
pub fn picker(ui: &mut egui::Ui) {
    let current = match ui.ctx().options(|o| o.theme_preference) {
        egui::ThemePreference::Dark => Choice::Dark,
        egui::ThemePreference::Light => Choice::Light,
        egui::ThemePreference::System => Choice::System,
    };
    ui.horizontal_wrapped(|ui| {
        ui.label("Window theme");
        for (choice, label) in [(Choice::Dark, "Dark"), (Choice::Light, "Light"), (Choice::System, "System")] {
            if pill(ui, current == choice, label).clicked() && current != choice {
                ui.ctx().set_theme(choice.preference());
                if let Err(e) = super::store::save("ui", &UiSettings { theme: choice }) {
                    eprintln!("Foxglove: can't save the theme: {e}");
                }
            }
        }
    });
    ui.add_space(4.0);
}

/// The font credit for the about line.
pub fn credits(ui: &mut egui::Ui) {
    ui.label(
        RichText::new("Fonts: Chakra Petch, IBM Plex Sans and IBM Plex Mono, under the SIL Open Font License 1.1")
            .small()
            .weak(),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_token_is_defined_in_both_themes() {
        let (dark, light) = (DARK.tokens(), LIGHT.tokens());
        let names = |t: &[(&'static str, Color32)]| t.iter().map(|(n, _)| *n).collect::<Vec<_>>();
        assert_eq!(names(&dark), names(&light));
        // Every field of the palette is listed once.
        let mut unique = names(&dark);
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), dark.len());
        // The palette is `dark` plus 4-byte colours; tokens() must list each colour.
        assert_eq!(dark.len(), (std::mem::size_of::<Palette>() - 1) / 4, "tokens() misses a field");
        for (name, color) in dark.iter().chain(&light) {
            assert_eq!(color.a(), 255, "{name} must be opaque");
        }
        const { assert!(DARK.dark && !LIGHT.dark) };
    }

    #[test]
    fn approved_colours() {
        assert_eq!(DARK.ground, Color32::from_rgb(0x0e, 0x12, 0x11));
        assert_eq!(DARK.surface, Color32::from_rgb(0x16, 0x1c, 0x1a));
        assert_eq!(DARK.surface_raised, Color32::from_rgb(0x1d, 0x24, 0x22));
        assert_eq!(DARK.line, Color32::from_rgb(0x2a, 0x33, 0x30));
        assert_eq!(DARK.accent, Color32::from_rgb(0xf0, 0x8a, 0x3c));
        assert_eq!(DARK.muted, Color32::from_rgb(0x8e, 0x9a, 0x94));
    }

    /// WCAG relative luminance contrast.
    fn contrast(a: Color32, b: Color32) -> f32 {
        let l = |c: Color32| {
            let ch = |v: u8| {
                let v = v as f32 / 255.0;
                if v <= 0.03928 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
            };
            0.2126 * ch(c.r()) + 0.7152 * ch(c.g()) + 0.0722 * ch(c.b())
        };
        let (x, y) = (l(a), l(b));
        (x.max(y) + 0.05) / (x.min(y) + 0.05)
    }

    #[test]
    fn text_reads_on_every_surface() {
        for p in [&DARK, &LIGHT] {
            for bg in [p.ground, p.surface, p.surface_raised, p.control] {
                for (name, fg) in [("text", p.text), ("muted", p.muted)] {
                    let c = contrast(fg, bg);
                    assert!(c >= 4.5, "{name} on {bg:?} (dark {}): {c:.2}", p.dark);
                }
            }
            assert!(contrast(p.accent, p.surface) >= 4.5, "links (dark {})", p.dark);
            assert!(contrast(p.on_accent, p.accent) >= 4.5, "text on orange (dark {})", p.dark);
            assert!(contrast(p.selected_text, p.selected_fill) >= 4.5, "pills (dark {})", p.dark);
        }
    }

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
    fn the_choice_is_stored_by_name() {
        let text = toml::to_string(&UiSettings { theme: Choice::System }).unwrap();
        assert_eq!(text.trim(), "theme = \"system\"");
        let back: UiSettings = toml::from_str("").unwrap();
        assert_eq!(back.theme, Choice::Dark);
    }

    #[test]
    fn styles_use_the_new_fonts() {
        let ctx = egui::Context::default();
        apply_choice(&ctx, Choice::Light);
        assert_eq!(ctx.options(|o| o.theme_preference), egui::ThemePreference::Light);
        let style = ctx.style_of(egui::Theme::Dark);
        assert_eq!(style.text_styles[&TextStyle::Heading].family, family(HUD));
        assert_eq!(style.visuals.selection.bg_fill, DARK.accent);
        assert_eq!(ctx.style_of(egui::Theme::Light).visuals.window_fill, LIGHT.ground);
    }
}
