//! The app's look (#384, epic #393): named colour tokens for a dark and a light theme, the
//! bundled fonts, and egui `Visuals` built from them. Pages take their colours and fonts from
//! here ([`palette`], [`hud`], [`body`], [`mono`]) instead of writing hex values of their own.
//!
//! The layer overlay keeps its own look: its key labels use `FontFamily::Proportional`, which
//! stays egui's default font. Only the text styles (`Body`, `Heading`, ...) move to the new fonts.

use std::sync::LazyLock;

use egui::{Color32, CornerRadius, FontFamily, FontId, RichText, Stroke, TextStyle};
use foxglove_scene::Rgb;
use serde::{Deserialize, Serialize};

use super::palette_file::{self, Roles};

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

fn role(c: Rgb) -> Color32 {
    let [r, g, b] = c.0;
    Color32::from_rgb(r, g, b)
}

/// Dark graphite with Bandit orange, as drawn on the approved canvas. The orange is the bundled
/// palette's (`resources/palettes/bandit.json`, docs/palette.md).
pub static DARK: LazyLock<Palette> = LazyLock::new(|| dark(&palette_file::bandit().roles));

/// The same tokens on a light ground. The orange is darker so text in it still reads.
pub static LIGHT: LazyLock<Palette> = LazyLock::new(|| light(&palette_file::bandit().roles));

/// The dark theme around a palette's roles: `accent` and `highlight` (hover).
pub fn dark(roles: &Roles) -> Palette {
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
        accent: role(roles.accent),
        accent_hover: role(roles.highlight),
        on_accent: hex(0x140d07),
        selected_fill: hex(0x3a2412),
        selected_text: hex(0xffcfa6),
        ok: hex(0x5fc48e),
        warn: hex(0xf0b53c),
        danger: hex(0xe5604f),
    }
}

/// The light theme around a palette's roles: `accent_light`.
pub fn light(roles: &Roles) -> Palette {
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
        accent: role(roles.accent_light),
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
    #[cfg(test)]
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
    RichText::new(text)
        .font(hud(15.0))
        .color(palette(ui).text_strong)
}

/// A small upper-case label (`EFFECT`, `LIBRARY`) in the mono face.
pub fn caps_label(ui: &egui::Ui, text: &str) -> RichText {
    RichText::new(text.to_uppercase())
        .font(mono(11.0))
        .color(palette(ui).muted)
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
pub fn primary_button(ui: &egui::Ui, text: &str) -> egui::Button<'static> {
    let p = palette(ui);
    egui::Button::new(RichText::new(text.to_owned()).color(p.on_accent))
        .fill(p.accent)
        .stroke(Stroke::new(1.0, p.accent))
}

// Disclosure widgets (#793; docs/design/device-focus-layouts-thumbnails-lights-declutter.md,
// section 4): a state becomes a flat chip, an explanation becomes hover text, and amber stays
// on screen as one short bar. Outlines are only for clickable things.

/// The coloured state in a chip's leading dot: ok green, muted grey or warn amber.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dot {
    Ok,
    Muted,
    Warn,
}

impl Dot {
    fn color(self, p: &Palette) -> Color32 {
        match self {
            Dot::Ok => p.ok,
            Dot::Muted => p.muted,
            Dot::Warn => p.warn,
        }
    }
}

/// How loud an [`info_bar`] is (WinUI's `InfoBar` severities).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Info,
    Warn,
    // WinUI's third severity. Phase A has no red state (#795 drew none), so only the previews
    // and tests use it for now.
    #[cfg_attr(not(test), allow(dead_code))]
    Danger,
}

impl Level {
    fn color(self, p: &Palette) -> Color32 {
        match self {
            Level::Info => p.muted,
            Level::Warn => p.warn,
            Level::Danger => p.danger,
        }
    }
}

/// The longest short line an [`info_bar`] takes (rule 5: 60 characters or less).
pub const BAR_MAX_CHARS: usize = 60;

/// An info bar's fill: its level's colour mixed into `surface`, opaque.
pub fn bar_fill(p: &Palette, level: Level) -> Color32 {
    let amount = match (level, p.dark) {
        (Level::Info, _) => 0.10,
        (_, true) => 0.11,
        (_, false) => 0.12,
    };
    let tint = level.color(p);
    let mix = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * amount).round() as u8;
    let s = p.surface;
    Color32::from_rgb(
        mix(s.r(), tint.r()),
        mix(s.g(), tint.g()),
        mix(s.b(), tint.b()),
    )
}

const CHIP_RADIUS: u8 = 6;
const CHIP_DOT: f32 = 3.5;
/// The room the dot takes before the text.
const CHIP_DOT_ROOM: f32 = 12.0;

/// A chip's size and fill. Every chip of one place is one height: [`ChipStyle::CARD`] in the
/// cards, [`ChipStyle::STRIP`] on the status strip, whose part chips (`shell::chip`) take the
/// same padding and text size, so the strip's row is level whatever its chips are.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChipStyle {
    /// Around the text.
    pub pad: egui::Vec2,
    /// The text's size in the body face.
    pub text: f32,
    strip: bool,
}

impl ChipStyle {
    /// The chips in a card, on `surface`.
    pub const CARD: ChipStyle = ChipStyle {
        pad: egui::vec2(8.0, 3.0),
        text: 12.5,
        strip: false,
    };
    /// The status strip's chips, on `ground` (#800).
    pub const STRIP: ChipStyle = ChipStyle {
        pad: egui::vec2(10.0, 6.0),
        text: 13.0,
        strip: true,
    };

    /// The flat fill on what the chip sits on.
    pub fn fill(self, p: &Palette) -> Color32 {
        if self.strip {
            p.strip_chip_fill
        } else {
            p.chip_fill
        }
    }
}

/// Places a chip and paints its face: the muted fill, the leading dot and the text, cut to one
/// row at the room left. Read-only chips stop here, so this function never outlines anything
/// (the flat-chip rule; a source test checks it). Whether the row cut the text.
fn chip_face(
    ui: &mut egui::Ui,
    style: ChipStyle,
    text: &str,
    dot: Option<Dot>,
    sense: egui::Sense,
) -> (egui::Rect, egui::Response, bool) {
    let p = palette(ui);
    let pad = style.pad;
    let dot_room = if dot.is_some() { CHIP_DOT_ROOM } else { 0.0 };
    let mut job =
        egui::text::LayoutJob::simple_singleline(text.to_owned(), body(style.text), p.text);
    job.wrap = egui::text::TextWrapping {
        max_width: (ui.available_width() - pad.x * 2.0 - dot_room).max(40.0),
        max_rows: 1,
        break_anywhere: true,
        overflow_character: Some('…'),
    };
    let galley = ui.ctx().fonts_mut(|f| f.layout_job(job));
    let elided = galley.elided;
    let size = egui::vec2(dot_room + galley.size().x, galley.size().y) + pad * 2.0;
    let (rect, response) = ui.allocate_exact_size(size, sense);
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        painter.rect_filled(rect, CornerRadius::same(CHIP_RADIUS), style.fill(p));
        if let Some(dot) = dot {
            let center = egui::pos2(rect.left() + pad.x + CHIP_DOT, rect.center().y);
            painter.circle_filled(center, CHIP_DOT, dot.color(p));
        }
        painter.galley(rect.min + pad + egui::vec2(dot_room, 0.0), galley, p.text);
    }
    (rect, response, elided)
}

/// A chip's hover: `hover`, with the whole text first when the row cut it.
fn chip_hover(text: &str, hover: Option<&str>, elided: bool) -> Option<String> {
    match (hover, elided) {
        (Some(h), true) => Some(format!("{text}\n{h}")),
        (Some(h), false) => Some(h.to_owned()),
        (None, true) => Some(text.to_owned()),
        (None, false) => None,
    }
}

/// A read-only status chip (rule 2): 1 to 4 words of state on a muted fill with no outline,
/// and a leading `dot` when the state has a colour. The text stays `text`, not coloured. The
/// sentence that explains it is `hover`, word for word; a chip never gets an ⓘ.
pub fn hint_chip(
    ui: &mut egui::Ui,
    text: &str,
    hover: Option<&str>,
    dot: Option<Dot>,
) -> egui::Response {
    hint_chip_in(ui, ChipStyle::CARD, text, hover, dot)
}

/// [`hint_chip`] in `style`: [`ChipStyle::STRIP`] on the status strip.
pub fn hint_chip_in(
    ui: &mut egui::Ui,
    style: ChipStyle,
    text: &str,
    hover: Option<&str>,
    dot: Option<Dot>,
) -> egui::Response {
    let (_, response, elided) = chip_face(ui, style, text, dot, egui::Sense::hover());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Label, true, text));
    match chip_hover(text, hover, elided) {
        Some(h) => response.on_hover_text(h),
        None => response,
    }
}

/// A chip that opens something when clicked (such as the status strip's focus switcher): the
/// [`hint_chip`] face plus the outline every clickable thing has, the accent on hover and the
/// focus ring on keyboard focus. Enter and Space click it, as they click a button.
// No card has a clickable chip yet; the strip's switcher uses `click_chip_in`.
#[cfg_attr(not(test), allow(dead_code))]
pub fn click_chip(
    ui: &mut egui::Ui,
    text: &str,
    hover: Option<&str>,
    dot: Option<Dot>,
) -> egui::Response {
    click_chip_in(ui, ChipStyle::CARD, text, hover, dot)
}

/// [`click_chip`] in `style`: [`ChipStyle::STRIP`] for the status strip's switcher (#800).
pub fn click_chip_in(
    ui: &mut egui::Ui,
    style: ChipStyle,
    text: &str,
    hover: Option<&str>,
    dot: Option<Dot>,
) -> egui::Response {
    let (rect, response, elided) = chip_face(ui, style, text, dot, egui::Sense::click());
    let line = if response.hovered() || response.has_focus() {
        ui.style().interact(&response).bg_stroke
    } else {
        Stroke::new(1.0, palette(ui).control_line)
    };
    ui.painter().rect_stroke(
        rect,
        CornerRadius::same(CHIP_RADIUS),
        line,
        egui::StrokeKind::Inside,
    );
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, text));
    match chip_hover(text, hover, elided) {
        Some(h) => response.on_hover_text(h),
        None => response,
    }
}

#[cfg(test)]
thread_local! {
    /// The previews show the ⓘ with this hover text as if Enter had been pressed on it.
    pub static PREVIEW_PINNED: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}

/// The ⓘ after a control or in an info bar (rule 3), a hint that there is more. Hovering it
/// shows `hover`, exactly. It takes keyboard focus, and Enter or Space (or a click) shows the
/// text until pressed again, Escape, or the focus moves on. Nothing ever highlights it on its
/// own: no first-run glow, no "new" mark (decision 6).
pub fn info_dot(ui: &mut egui::Ui, hover: &str) -> egui::Response {
    let p = palette(ui);
    let (rect, response) = ui.allocate_exact_size(egui::vec2(16.0, 16.0), egui::Sense::click());
    let pinned_id = response.id.with("foxglove-info-pinned");
    let mut pinned = ui.ctx().data(|d| d.get_temp::<bool>(pinned_id)) == Some(true);
    if response.clicked() {
        pinned = !pinned;
    } else if !response.has_focus() && !response.hovered() {
        pinned = false;
    }
    if response.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        pinned = false;
    }
    #[cfg(test)]
    if PREVIEW_PINNED.with_borrow(|t| t.as_deref() == Some(hover)) {
        pinned = true;
        response.request_focus();
    }
    ui.ctx().data_mut(|d| d.insert_temp(pinned_id, pinned));
    if ui.is_rect_visible(rect) {
        let lit = response.hovered() || response.has_focus() || pinned;
        let color = if lit { p.accent } else { p.muted };
        let painter = ui.painter();
        let center = rect.center();
        painter.circle_stroke(center, 6.0, Stroke::new(1.2, color));
        painter.text(center, egui::Align2::CENTER_CENTER, "i", body(10.5), color);
        if response.has_focus() {
            // The focus ring every control has (#384).
            painter.circle_stroke(center, 8.0, Stroke::new(2.0, p.accent));
        }
    }
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, hover));
    if pinned {
        response.show_tooltip_text(hover);
        response
    } else {
        response.on_hover_text(hover)
    }
}

/// A control and its ⓘ (rule 3): `hover` goes on the control too, so the ⓘ only says there is
/// more. Call it in the control's row, right after adding the control; returns its response.
pub fn with_info(ui: &mut egui::Ui, control: egui::Response, hover: &str) -> egui::Response {
    let control = control.on_hover_text(hover);
    info_dot(ui, hover);
    control
}

/// What an [`info_bar`] did this frame.
pub struct BarResponse {
    /// The whole bar. No page reads it yet (#795 needed only the action); the tests measure it.
    #[cfg_attr(not(test), allow(dead_code))]
    pub response: egui::Response,
    /// Its action button, when it has one.
    pub action: Option<egui::Response>,
}

impl BarResponse {
    /// Whether the action button was clicked.
    pub fn clicked(&self) -> bool {
        self.action.as_ref().is_some_and(|a| a.clicked())
    }
}

/// A one-line bar (WinUI's `InfoBar`, rule 5): the level's fill and no outline, a dot in the
/// level's colour, the `short` line that names the problem (60 characters or less, hover =
/// `full`), an ⓘ with the `full` sentence, and the `action` button. It is never collapsed: on a
/// narrow card it wraps.
pub fn info_bar(
    ui: &mut egui::Ui,
    level: Level,
    short: &str,
    full: &str,
    action: Option<&str>,
) -> BarResponse {
    debug_assert!(
        short.chars().count() <= BAR_MAX_CHARS,
        "an info bar's line is {BAR_MAX_CHARS} characters or less: {short:?}"
    );
    let p = palette(ui);
    let mut button = None;
    let inner = egui::Frame::new()
        .fill(bar_fill(p, level))
        .corner_radius(CornerRadius::same(6))
        .inner_margin(egui::Margin::symmetric(10, 6))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                let (dot, _) = ui.allocate_exact_size(egui::vec2(8.0, 18.0), egui::Sense::hover());
                ui.painter()
                    .circle_filled(dot.center(), 4.0, level.color(p));
                ui.add(egui::Label::new(RichText::new(short).color(p.text_strong)).wrap())
                    .on_hover_text(full);
                info_dot(ui, full);
                if let Some(label) = action {
                    button = Some(ui.button(label));
                }
            });
        });
    BarResponse {
        response: inner.response,
        action: button,
    }
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
        for (choice, label) in [
            (Choice::Dark, "Dark"),
            (Choice::Light, "Light"),
            (Choice::System, "System"),
        ] {
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

/// Headless frames for the widget tests: no window, the app's fonts and theme.
#[cfg(test)]
pub mod headless {
    use egui::epaint::Shape;
    use egui::{Event, Key, Pos2, RawInput, Rect};

    pub fn ctx(choice: super::Choice) -> egui::Context {
        let ctx = egui::Context::default();
        let mut fonts = egui::FontDefinitions::default();
        super::add_fonts(&mut fonts);
        ctx.set_fonts(fonts);
        super::apply_choice(&ctx, choice);
        ctx
    }

    /// A press of `key`, then its release.
    pub fn tap(key: Key) -> Vec<Event> {
        [true, false]
            .map(|pressed| Event::Key {
                key,
                physical_key: None,
                pressed,
                repeat: false,
                modifiers: Default::default(),
            })
            .to_vec()
    }

    pub fn hover(pos: Pos2) -> Vec<Event> {
        vec![Event::PointerMoved(pos)]
    }

    /// The pointer pressed and released at `pos`.
    pub fn click(pos: Pos2) -> Vec<Event> {
        let button = |pressed| Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        vec![Event::PointerMoved(pos), button(true), button(false)]
    }

    /// One frame at second `time` with `events` (time moves on a second a frame, past the
    /// tooltip delay); the frame's shapes, flattened.
    pub fn frame(
        ctx: &egui::Context,
        time: f64,
        events: Vec<Event>,
        draw: impl FnMut(&mut egui::Ui),
    ) -> Vec<Shape> {
        let raw = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(480.0, 320.0))),
            time: Some(time),
            predicted_dt: 1.0,
            focused: true,
            events,
            ..Default::default()
        };
        let mut out = ctx.run_ui(raw, draw);
        out.textures_delta.clear();
        let mut shapes = Vec::new();
        fn flat(s: Shape, out: &mut Vec<Shape>) {
            match s {
                Shape::Vec(v) => v.into_iter().for_each(|s| flat(s, out)),
                s => out.push(s),
            }
        }
        for c in out.shapes {
            flat(c.shape, &mut shapes);
        }
        shapes
    }

    /// Every line of text the frame painted, whole (a wrapped tooltip is one text).
    pub fn texts(shapes: &[Shape]) -> Vec<String> {
        shapes
            .iter()
            .filter_map(|s| match s {
                Shape::Text(t) => Some(t.galley.text().to_owned()),
                _ => None,
            })
            .collect()
    }
}

#[cfg(test)]
mod widget_tests {
    use super::headless::{self, click, frame, hover, tap, texts};
    use super::*;
    use egui::epaint::Shape;
    use egui::Key;

    const SOURCE: &str = include_str!("theme.rs");

    /// The body of `fn name` in this file, up to its closing brace at the start of a line (with
    /// either line ending: a Windows checkout has CRLF).
    fn body(name: &str) -> &'static str {
        let start = SOURCE
            .find(&format!("fn {name}("))
            .unwrap_or_else(|| panic!("fn {name} in theme.rs"));
        let rest = &SOURCE[start..];
        let end = ["\n}\n", "\n}\r\n"]
            .iter()
            .filter_map(|e| rest.find(e))
            .min()
            .expect("the function ends");
        &rest[..end]
    }

    /// The flat-chip rule (rule 2): nothing a read-only chip runs outlines it. An outline is
    /// only for clickable things, so it lives in `click_chip_in` alone.
    #[test]
    fn read_only_chips_and_bars_have_no_stroke_in_their_source() {
        for name in [
            "hint_chip",
            "hint_chip_in",
            "chip_face",
            "chip_hover",
            "info_bar",
            "bar_fill",
        ] {
            let code = body(name).to_lowercase();
            assert!(!code.contains("stroke"), "{name} outlines something");
        }
        assert!(body("click_chip_in").contains("rect_stroke"));
        // Amber is never collapsed (rule 5).
        assert!(!body("info_bar").to_lowercase().contains("collaps"));
    }

    /// The rectangles painted with `fill`, and whether each has an outline.
    fn outlined(shapes: &[Shape], fill: Color32) -> Vec<bool> {
        shapes
            .iter()
            .filter_map(|s| match s {
                Shape::Rect(r) if r.fill == fill => {
                    Some(r.stroke.width > 0.0 && r.stroke.color.a() > 0)
                }
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_status_chip_is_flat_and_a_clickable_chip_is_outlined() {
        for choice in [Choice::Dark, Choice::Light] {
            let ctx = headless::ctx(choice);
            let p = if choice == Choice::Dark {
                *DARK
            } else {
                *LIGHT
            };
            let draw = |ui: &mut egui::Ui| {
                hint_chip(
                    ui,
                    "6 zones per half",
                    Some("6 lighting zones per half"),
                    None,
                );
                hint_chip(ui, "calibrated", None, Some(Dot::Ok));
                hint_chip(ui, "in front only", None, Some(Dot::Muted));
                hint_chip(ui, "off in Windows", None, Some(Dot::Warn));
            };
            let shapes = frame(&ctx, 0.0, vec![], draw);
            assert_eq!(outlined(&shapes, p.chip_fill), vec![false; 4]);
            // The dots, in their state's colours; the text stays `text`.
            for c in [p.ok, p.muted, p.warn] {
                assert!(
                    shapes
                        .iter()
                        .any(|s| matches!(s, Shape::Circle(d) if d.fill == c)),
                    "a {c:?} dot"
                );
            }
            for s in &shapes {
                if let Shape::Text(t) = s {
                    assert_eq!(t.fallback_color, p.text);
                }
            }
            let shapes = frame(&ctx, 1.0, vec![], |ui| {
                click_chip(ui, "Corne", None, Some(Dot::Ok));
            });
            assert!(shapes.iter().any(|s| matches!(
                s,
                Shape::Rect(r) if r.fill == Color32::TRANSPARENT && r.stroke.width > 0.0
            )));
        }
    }

    #[test]
    fn a_chip_cut_short_hovers_with_its_whole_text() {
        assert_eq!(chip_hover("a", Some("why"), false).as_deref(), Some("why"));
        assert_eq!(chip_hover("a", None, false), None);
        assert_eq!(chip_hover("a b", None, true).as_deref(), Some("a b"));
        assert_eq!(
            chip_hover("a b", Some("why"), true).as_deref(),
            Some("a b\nwhy")
        );
    }

    const TIP: &str =
        "The underglow takes the accent colour's hue. The brightness stays the board's.";

    fn dot_row(ui: &mut egui::Ui) -> egui::Rect {
        ui.horizontal(|ui| {
            ui.label("Follow Windows accent");
            info_dot(ui, TIP).rect
        })
        .inner
    }

    fn shown(shapes: &[Shape]) -> bool {
        texts(shapes).iter().any(|t| t == TIP)
    }

    #[test]
    fn the_info_hover_is_the_given_text() {
        let ctx = headless::ctx(Choice::Dark);
        let mut rect = egui::Rect::NOTHING;
        let shapes = frame(&ctx, 0.0, vec![], |ui| rect = dot_row(ui));
        assert!(!shown(&shapes), "no tooltip before a hover");
        let mut last = Vec::new();
        for t in 1..5 {
            let events = if t == 1 { hover(rect.center()) } else { vec![] };
            last = frame(&ctx, t as f64, events, |ui| {
                dot_row(ui);
            });
        }
        assert!(shown(&last), "hovering the ⓘ shows its text, exactly");
    }

    #[test]
    fn the_info_dot_is_never_highlighted_on_its_own() {
        // No first-run highlight (decision 6): frame after frame without input the ⓘ looks the
        // same, in the muted colour, and nothing it runs remembers having been seen.
        let ctx = headless::ctx(Choice::Dark);
        let first = frame(&ctx, 0.0, vec![], |ui| {
            dot_row(ui);
        });
        for t in 1..5 {
            let again = frame(&ctx, t as f64, vec![], |ui| {
                dot_row(ui);
            });
            assert_eq!(format!("{first:?}"), format!("{again:?}"));
        }
        assert!(first
            .iter()
            .any(|s| matches!(s, Shape::Circle(c) if c.stroke.color == DARK.muted)));
        let code = body("info_dot");
        for word in ["highlight", "store::", "seen", "first_run"] {
            assert!(!code.contains(word), "info_dot uses {word}");
        }
    }

    #[test]
    fn keyboard_focus_reaches_the_info_dot_and_enter_or_space_shows_its_text() {
        let ctx = headless::ctx(Choice::Light);
        let draw = |ui: &mut egui::Ui| {
            dot_row(ui);
        };
        frame(&ctx, 0.0, vec![], draw);
        // Tab moves the focus onto the ⓘ (the only control); the text waits for a key.
        let shapes = frame(&ctx, 1.0, tap(Key::Tab), draw);
        assert!(!shown(&shapes));
        let focused = shapes
            .iter()
            .any(|s| matches!(s, Shape::Circle(c) if c.stroke.width == 2.0));
        assert!(focused, "the focus ring");
        let mut t = 2.0;
        let mut step = |events| {
            // A new tooltip's first frame only measures it (egui), so look one frame later.
            t += 1.0;
            frame(&ctx, t, events, draw);
            t += 1.0;
            let first = shown(&frame(&ctx, t, vec![], draw));
            // It stays, with no pointer near it, until the next key.
            t += 1.0;
            let then = shown(&frame(&ctx, t, vec![], draw));
            assert_eq!(first, then);
            first
        };
        assert!(step(tap(Key::Enter)), "Enter shows the text");
        assert!(!step(tap(Key::Enter)), "and Enter hides it");
        assert!(step(tap(Key::Space)), "Space shows it too");
        assert!(!step(tap(Key::Escape)), "Escape hides it");
    }

    #[test]
    fn a_click_on_the_info_dot_shows_its_text() {
        let ctx = headless::ctx(Choice::Dark);
        let mut rect = egui::Rect::NOTHING;
        frame(&ctx, 0.0, vec![], |ui| rect = dot_row(ui));
        let shapes = frame(&ctx, 1.0, click(rect.center()), |ui| {
            dot_row(ui);
        });
        let next = frame(&ctx, 2.0, vec![], |ui| {
            dot_row(ui);
        });
        // A click hides the hover tooltip (egui), so the text shows as pinned.
        assert!(shown(&shapes) || shown(&next));
    }

    #[test]
    fn an_info_bar_is_fill_only_and_shows_its_line_and_action() {
        for choice in [Choice::Dark, Choice::Light] {
            let ctx = headless::ctx(choice);
            let p = if choice == Choice::Dark {
                *DARK
            } else {
                *LIGHT
            };
            let full = "Windows Dynamic Lighting holds the keyboard's lights. Turn off Settings > Personalization > Dynamic Lighting to give them back.";
            let mut clicked = false;
            let draw = |ui: &mut egui::Ui| {
                // A narrow card: the bar wraps, it never folds away.
                ui.set_max_width(220.0);
                let bar = info_bar(
                    ui,
                    Level::Warn,
                    "Windows has the keyboard's lights",
                    full,
                    Some("Open Dynamic Lighting settings"),
                );
                assert!(bar.action.is_some());
                assert!(bar.response.rect.width() > 0.0);
                clicked |= bar.clicked();
            };
            let shapes = frame(&ctx, 0.0, vec![], draw);
            assert_eq!(outlined(&shapes, bar_fill(&p, Level::Warn)), vec![false]);
            let lines = texts(&shapes);
            for want in [
                "Windows has the keyboard's lights",
                "Open Dynamic Lighting settings",
            ] {
                assert!(lines.iter().any(|t| t == want), "{want} on screen");
            }
            assert!(!clicked);
            // The amber dot.
            assert!(shapes
                .iter()
                .any(|s| matches!(s, Shape::Circle(c) if c.fill == p.warn)));
        }
    }

    #[test]
    #[should_panic(expected = "60 characters or less")]
    fn an_info_bar_line_is_short() {
        let ctx = headless::ctx(Choice::Dark);
        frame(&ctx, 0.0, vec![], |ui| {
            info_bar(ui, Level::Warn, &"x".repeat(BAR_MAX_CHARS + 1), "", None);
        });
    }

    #[test]
    fn with_info_puts_the_text_on_the_control_too() {
        let ctx = headless::ctx(Choice::Dark);
        let mut rect = egui::Rect::NOTHING;
        let draw = |ui: &mut egui::Ui, rect: &mut egui::Rect| {
            ui.horizontal(|ui| {
                let c = ui.checkbox(&mut true, "Follow Windows accent");
                *rect = with_info(ui, c, TIP).rect;
            });
        };
        frame(&ctx, 0.0, vec![], |ui| draw(ui, &mut rect));
        let mut last = Vec::new();
        let at = rect.center();
        for t in 1..5 {
            let events = if t == 1 { hover(at) } else { vec![] };
            last = frame(&ctx, t as f64, events, |ui| draw(ui, &mut rect));
        }
        assert!(
            shown(&last),
            "hovering the control's label shows the ⓘ's text"
        );
    }
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
        assert_eq!(
            dark.len(),
            (std::mem::size_of::<Palette>() - 1) / 4,
            "tokens() misses a field"
        );
        for (name, color) in dark.iter().chain(&light) {
            assert_eq!(color.a(), 255, "{name} must be opaque");
        }
        assert!(DARK.dark && !LIGHT.dark);
    }

    #[test]
    fn approved_colours() {
        assert_eq!(DARK.ground, Color32::from_rgb(0x0e, 0x12, 0x11));
        assert_eq!(DARK.surface, Color32::from_rgb(0x16, 0x1c, 0x1a));
        assert_eq!(DARK.surface_raised, Color32::from_rgb(0x1d, 0x24, 0x22));
        assert_eq!(DARK.line, Color32::from_rgb(0x2a, 0x33, 0x30));
        // The orange is the palette file's (docs/palette.md).
        assert_eq!(DARK.accent, role(palette_file::bandit().roles.accent));
        assert_eq!(DARK.muted, Color32::from_rgb(0x8e, 0x9a, 0x94));
    }

    /// WCAG relative luminance contrast.
    fn contrast(a: Color32, b: Color32) -> f32 {
        let l = |c: Color32| {
            let ch = |v: u8| {
                let v = v as f32 / 255.0;
                if v <= 0.03928 {
                    v / 12.92
                } else {
                    ((v + 0.055) / 1.055).powf(2.4)
                }
            };
            0.2126 * ch(c.r()) + 0.7152 * ch(c.g()) + 0.0722 * ch(c.b())
        };
        let (x, y) = (l(a), l(b));
        (x.max(y) + 0.05) / (x.min(y) + 0.05)
    }

    /// The themes the bundled file gives, and those either Bandit orange (#40 Q1) would give
    /// with the hand-picked shades taken out of the file.
    fn every_theme() -> Vec<Palette> {
        let mut out = vec![*DARK, *LIGHT];
        for orange in ["#f08a3c", "#e5732b"] {
            let text = palette_file::BANDIT_JSON
                .replace("\"#f08a3c\"", &format!("\"{orange}\""))
                .replace("\"highlight\": \"#ffb070\",", "")
                .replace("\"accent_light\": \"#b4540f\",", "");
            let roles = palette_file::parse(&text, None).unwrap().unwrap().roles;
            out.extend([dark(&roles), light(&roles)]);
        }
        out
    }

    #[test]
    fn text_reads_on_every_surface() {
        for p in &every_theme() {
            let bars = [Level::Info, Level::Warn, Level::Danger].map(|l| bar_fill(p, l));
            for bg in [
                p.ground,
                p.surface,
                p.surface_raised,
                p.control,
                p.chip_fill,
                p.strip_chip_fill,
            ]
            .into_iter()
            .chain(bars)
            {
                for (name, fg) in [("text", p.text), ("muted", p.muted)] {
                    let c = contrast(fg, bg);
                    assert!(c >= 4.5, "{name} on {bg:?} (dark {}): {c:.2}", p.dark);
                }
            }
            assert!(
                contrast(p.accent, p.surface) >= 4.5,
                "links (dark {})",
                p.dark
            );
            // A flat chip on the strip has no outline: its fill alone sets it off the ground,
            // at least as far as a card's surface stands off it (#800).
            let card = contrast(p.surface, p.ground);
            let chip = contrast(p.strip_chip_fill, p.ground);
            assert!(
                chip >= 1.15 && chip >= card,
                "a strip chip on the ground (dark {}): {chip:.3}, a card {card:.3}",
                p.dark
            );
            assert!(
                contrast(p.on_accent, p.accent) >= 4.5,
                "text on orange (dark {})",
                p.dark
            );
            assert!(
                contrast(p.selected_text, p.selected_fill) >= 4.5,
                "pills (dark {})",
                p.dark
            );
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
        let text = toml::to_string(&UiSettings {
            theme: Choice::System,
        })
        .unwrap();
        assert_eq!(text.trim(), "theme = \"system\"");
        let back: UiSettings = toml::from_str("").unwrap();
        assert_eq!(back.theme, Choice::Dark);
    }

    #[test]
    fn styles_use_the_new_fonts() {
        let ctx = egui::Context::default();
        apply_choice(&ctx, Choice::Light);
        assert_eq!(
            ctx.options(|o| o.theme_preference),
            egui::ThemePreference::Light
        );
        let style = ctx.style_of(egui::Theme::Dark);
        assert_eq!(style.text_styles[&TextStyle::Heading].family, family(HUD));
        assert_eq!(style.visuals.selection.bg_fill, DARK.accent);
        assert_eq!(
            ctx.style_of(egui::Theme::Light).visuals.window_fill,
            LIGHT.ground
        );
    }
}
