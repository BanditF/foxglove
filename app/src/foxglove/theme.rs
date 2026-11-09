//! The app's look (#384, epic #393): named colour tokens for a dark and a light theme, the
//! bundled fonts, and egui `Visuals` built from them. Pages take their colours and fonts from
//! here ([`palette`], [`hud`], [`body`], [`mono`]) instead of writing hex values of their own.
//!
//! The layer overlay keeps its own look: its key labels use `FontFamily::Proportional`, which
//! stays egui's default font. Only the text styles (`Body`, `Heading`, ...) move to the new fonts.

use std::sync::LazyLock;

use egui::{Color32, CornerRadius, RichText, Stroke};
use foxglove_scene::Rgb;
use serde::{Deserialize, Serialize};

use super::palette_file::{self, Roles};
use super::windows_theme::AppMode;

use hub_shell::theme::style_text;
pub use hub_shell::theme::{add_fonts, body, hud, mono, visuals, Palette};

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
    hub_shell::theme::dark(role(roles.accent), role(roles.highlight))
}

/// The light theme around a palette's roles: `accent_light`.
pub fn light(roles: &Roles) -> Palette {
    hub_shell::theme::light(role(roles.accent_light))
}

/// The palette of the theme `ui` is drawn in.
pub fn palette(ui: &egui::Ui) -> &'static Palette {
    if ui.visuals().dark_mode {
        &DARK
    } else {
        &LIGHT
    }
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
    /// What egui is told for this choice. System takes Windows' app mode from
    /// `windows_theme` (the source the wallpapers' "Follow Windows" uses too), and egui's own
    /// reading of the system theme only when Windows doesn't say.
    fn preference(self, windows: Option<AppMode>) -> egui::ThemePreference {
        match (self, windows) {
            (Choice::Dark, _) | (Choice::System, Some(AppMode::Dark)) => {
                egui::ThemePreference::Dark
            }
            (Choice::Light, _) | (Choice::System, Some(AppMode::Light)) => {
                egui::ThemePreference::Light
            }
            (Choice::System, None) => egui::ThemePreference::System,
        }
    }
}

/// Where a context keeps the user's choice (System shows as Dark or Light to egui).
fn choice_id() -> egui::Id {
    egui::Id::new("foxglove.theme.choice")
}

/// The window theme the user chose on `ctx`.
pub fn choice(ctx: &egui::Context) -> Choice {
    ctx.data(|d| d.get_temp::<Choice>(choice_id()))
        .unwrap_or_else(|| match ctx.options(|o| o.theme_preference) {
            egui::ThemePreference::Dark => Choice::Dark,
            egui::ThemePreference::Light => Choice::Light,
            egui::ThemePreference::System => Choice::System,
        })
}

/// Picks `choice` on `ctx`, with Windows' app mode `windows` for System.
fn set_choice(ctx: &egui::Context, choice: Choice, windows: Option<AppMode>) {
    ctx.data_mut(|d| d.insert_temp(choice_id(), choice));
    ctx.set_theme(choice.preference(windows));
}

/// The app's own context, for following Windows' app mode (`windows_theme`).
static APP: std::sync::OnceLock<egui::Context> = std::sync::OnceLock::new();

/// Windows' app mode changed (`windows_theme`): the app follows it when its choice is System.
pub fn windows_changed(windows: Option<AppMode>) {
    let Some(ctx) = APP.get() else { return };
    if choice(ctx) == Choice::System {
        set_choice(ctx, Choice::System, windows);
        ctx.request_repaint();
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
    let windows = (choice == Choice::System)
        .then(super::windows_theme::current)
        .flatten();
    set_choice(ctx, choice, windows);
}

/// Startup: the saved theme choice (dark when none is saved). The app then follows Windows'
/// app mode while the choice is System.
pub fn apply(ctx: &egui::Context) {
    let saved = super::store::try_load::<UiSettings>("ui").unwrap_or_default();
    apply_choice(ctx, saved.theme);
    let _ = APP.set(ctx.clone());
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
/// `hover` is shown as given: it says what the control does and doesn't start with the
/// control's own label, which is right beside it ([`repeats_label`]). A shortened label's whole
/// name may lead it (K18), since that name is said nowhere else.
pub fn with_info(ui: &mut egui::Ui, control: egui::Response, hover: &str) -> egui::Response {
    let control = control.on_hover_text(hover);
    info_dot(ui, hover);
    control
}

/// Whether `hover` opens with `label` (case and trailing punctuation aside): a tooltip that
/// only repeats what its control already says. The tests hold every ⓘ to it.
#[cfg(test)]
pub fn repeats_label(label: &str, hover: &str) -> bool {
    let words = |s: &str| {
        s.to_lowercase()
            .split(|c: char| !c.is_alphanumeric() && c != '\'')
            .filter(|w| !w.is_empty())
            .map(str::to_owned)
            .collect::<Vec<_>>()
    };
    let (label, hover) = (words(label), words(hover));
    !label.is_empty() && hover.starts_with(&label)
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
    let current = choice(ui.ctx());
    ui.horizontal_wrapped(|ui| {
        ui.label("Window theme");
        for (choice, label) in [
            (Choice::Dark, "Dark"),
            (Choice::Light, "Light"),
            (Choice::System, "System"),
        ] {
            if pill(ui, current == choice, label).clicked() && current != choice {
                let windows = (choice == Choice::System)
                    .then(super::windows_theme::current)
                    .flatten();
                set_choice(ui.ctx(), choice, windows);
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

    /// #839: no ⓘ's tooltip starts by repeating its control's label ("Animate on e-paper:
    /// plays…" read the label twice). Every `with_info` call in app/src, found by walking the
    /// source at test time, must be listed here with its label and the text it is given.
    ///
    /// The four approved declutter exceptions (#794 K18, K19, K24, K26) show a short label on
    /// the card and lead the tooltip with the switch's whole name, which is said nowhere else.
    /// They are listed with the short label, which the tooltip doesn't repeat.
    #[test]
    fn no_tooltip_starts_with_its_controls_label() {
        use crate::foxglove::{
            art_encode, art_ui, battery_dim_ui, follow_typing, lights_page, overlay_shown,
            pc_lights_ui, theme_preview,
        };
        assert!(repeats_label(
            "Animate on e-paper",
            "Animate on e-paper: plays…"
        ));
        assert!(repeats_label("Match keyboard", "match keyboard. Shows…"));
        assert!(!repeats_label(
            "Follow Windows accent",
            "Follow the Windows accent colour: …"
        ));
        let parts = crate::foxglove::parts::Parts::of(None);
        let epaper = art_ui::tests::epaper_screens();
        let pair = |label: &str, hover: String| (label.to_string(), hover);
        const PREVIEW_ACCENT: &str =
            "Follow the Windows accent colour: the underglow takes the accent colour's hue.";
        // (file under app/src, the call's hover argument as written without whitespace, its
        // declutter exception, the (label, hover) pairs it can show).
        type Listed = (
            &'static str,
            String,
            Option<&'static str>,
            Vec<(String, String)>,
        );
        let listed: Vec<Listed> = vec![
            (
                "foxglove/art_ui.rs",
                "&epaper_tip(pace)".into(),
                None,
                vec![pair(
                    art_ui::EPAPER_SWITCH,
                    art_ui::epaper_tip(art_encode::Pace::of(&epaper[0], false)),
                )],
            ),
            (
                "foxglove/art_ui.rs",
                "super::art_encode::FIT_RULE".into(),
                None,
                vec![pair(art_ui::FITTED_NOTE, art_encode::FIT_RULE.into())],
            ),
            (
                "foxglove/battery_dim_ui.rs",
                "&hover(config.enabled)".into(),
                Some("K26"),
                vec![
                    pair(battery_dim_ui::SHORT, battery_dim_ui::hover(true)),
                    pair(battery_dim_ui::SHORT, battery_dim_ui::hover(false)),
                ],
            ),
            (
                "foxglove/follow_typing.rs",
                "&hover(was)".into(),
                None,
                vec![
                    pair(follow_typing::LABEL, follow_typing::hover(true)),
                    pair(follow_typing::LABEL, follow_typing::hover(false)),
                ],
            ),
            (
                "foxglove/lights_page.rs",
                "&accent_hover()".into(),
                Some("K18"),
                vec![pair(lights_page::ACCENT_SHORT, lights_page::accent_hover())],
            ),
            (
                "foxglove/lights_page.rs",
                "&wallpaper_hover()".into(),
                Some("K19"),
                vec![pair(
                    lights_page::WALLPAPER_SHORT,
                    lights_page::wallpaper_hover(),
                )],
            ),
            (
                "foxglove/lights_page.rs",
                "&follow_hover(parts)".into(),
                Some("K24"),
                vec![(
                    lights_page::follow_short(&parts),
                    lights_page::follow_hover(&parts),
                )],
            ),
            (
                "foxglove/overlay_shown.rs",
                "TIP".into(),
                None,
                vec![pair(overlay_shown::LABEL, overlay_shown::TIP.into())],
            ),
            (
                "foxglove/pc_lights_ui.rs",
                "&tip".into(),
                None,
                vec![pair(
                    pc_lights_ui::OPENRGB_SHORT,
                    pc_lights_ui::OPENRGB_TIP.into(),
                )],
            ),
            (
                "foxglove/pc_lights_ui.rs",
                "MATCH_TIP".into(),
                None,
                vec![pair(
                    pc_lights_ui::MATCH_SHORT,
                    pc_lights_ui::MATCH_TIP.into(),
                )],
            ),
            // The tests' own calls: this module and the disclosure preview sheet.
            (
                "foxglove/theme.rs",
                "TIP".into(),
                None,
                vec![pair("Follow Windows accent", TIP.into())],
            ),
            (
                "foxglove/theme_preview.rs",
                format!("{PREVIEW_ACCENT:?}"),
                None,
                vec![pair("Follow Windows accent", PREVIEW_ACCENT.into())],
            ),
            (
                "foxglove/theme_preview.rs",
                "PINNED".into(),
                None,
                vec![pair("Right follows left", theme_preview::PINNED.into())],
            ),
        ];
        for (_, _, _, pairs) in &listed {
            for (label, hover) in pairs {
                assert!(
                    !repeats_label(label, hover),
                    "{label:?} repeats in {hover:?}"
                );
            }
        }
        let mut exceptions: Vec<&str> = listed.iter().filter_map(|l| l.2).collect();
        exceptions.sort();
        assert_eq!(
            exceptions,
            ["K18", "K19", "K24", "K26"],
            "only the four approved declutter exceptions"
        );

        // Every call site in the source is listed: a new `with_info` needs its pair here too.
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let sites = with_info_sites(&src);
        let unlisted: Vec<String> = sites
            .iter()
            .filter(|(file, _, arg)| !listed.iter().any(|l| l.0 == file && &l.1 == arg))
            .map(|(file, line, arg)| format!("app/src/{file}:{line} (hover {arg})"))
            .collect();
        assert!(
            unlisted.is_empty(),
            "with_info calls missing from this test's list: {unlisted:#?}"
        );
        let stale: Vec<String> = listed
            .iter()
            .filter(|l| !sites.iter().any(|(f, _, a)| f == l.0 && a == &l.1))
            .map(|l| format!("app/src/{} (hover {})", l.0, l.1))
            .collect();
        assert!(
            stale.is_empty(),
            "listed calls not in the source: {stale:#?}"
        );
    }

    /// Every call of `with_info` in the `.rs` files under `dir`: (path from `dir` with `/`, line,
    /// its last argument without whitespace). Comments and string literals are skipped, and so
    /// is the function's own definition.
    fn with_info_sites(dir: &std::path::Path) -> Vec<(String, usize, String)> {
        fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            for entry in std::fs::read_dir(dir).expect("read a source dir") {
                let path = entry.expect("a dir entry").path();
                if path.is_dir() {
                    walk(&path, out);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    out.push(path);
                }
            }
        }
        let mut files = Vec::new();
        walk(dir, &mut files);
        files.sort();
        let mut sites = Vec::new();
        for path in files {
            let text = std::fs::read_to_string(&path).expect("read a source file");
            let name = path
                .strip_prefix(dir)
                .expect("under the source dir")
                .to_string_lossy()
                .replace('\\', "/");
            for (line, arg) in calls_in(&text) {
                sites.push((name.clone(), line, arg));
            }
        }
        sites
    }

    /// The `with_info(…)` calls in `text`: (line, last argument without whitespace).
    fn calls_in(text: &str) -> Vec<(usize, String)> {
        let c: Vec<char> = text.chars().collect();
        let ident = |ch: char| ch.is_alphanumeric() || ch == '_';
        let name: Vec<char> = "with_info".chars().collect();
        let mut out = Vec::new();
        let (mut i, mut line) = (0, 1);
        while i < c.len() {
            if let Some(end) = skip_literal(&c, i) {
                line += c[i..end].iter().filter(|&&ch| ch == '\n').count();
                i = end;
                continue;
            }
            let starts = c[i..].starts_with(&name)
                && (i == 0 || !ident(c[i - 1]))
                && c.get(i + name.len()) == Some(&'(');
            if !starts {
                if c[i] == '\n' {
                    line += 1;
                }
                i += 1;
                continue;
            }
            let before: String = c[i.saturating_sub(8)..i].iter().collect();
            let definition = before.trim_end().ends_with("fn");
            // The arguments, up to the matching parenthesis.
            let open = i + name.len();
            let (mut depth, mut j) = (0usize, open);
            let mut args = Vec::new();
            while j < c.len() {
                if let Some(end) = skip_literal(&c, j) {
                    args.extend_from_slice(&c[j..end]);
                    j = end;
                    continue;
                }
                match c[j] {
                    '(' | '[' | '{' => depth += 1,
                    ')' | ']' | '}' => depth -= 1,
                    _ => {}
                }
                if depth == 0 {
                    break;
                }
                if j > open {
                    args.push(c[j]);
                }
                j += 1;
            }
            if !definition {
                out.push((line, last_arg(&args)));
            }
            i += name.len();
        }
        out
    }

    /// Where the comment, string or char literal starting at `i` ends, if one starts there.
    fn skip_literal(c: &[char], i: usize) -> Option<usize> {
        let at = |k: usize| c.get(k).copied();
        let after_ident = i > 0 && (c[i - 1].is_alphanumeric() || c[i - 1] == '_');
        match (c[i], at(i + 1)) {
            ('/', Some('/')) => Some((i..c.len()).find(|&k| c[k] == '\n').unwrap_or(c.len())),
            ('/', Some('*')) => {
                let mut k = i + 2;
                while k + 1 < c.len() && !(c[k] == '*' && c[k + 1] == '/') {
                    k += 1;
                }
                Some((k + 2).min(c.len()))
            }
            ('r', Some('"' | '#')) if !after_ident => {
                let hashes = c[i + 1..].iter().take_while(|&&ch| ch == '#').count();
                if at(i + 1 + hashes) != Some('"') {
                    return None;
                }
                let close: Vec<char> = std::iter::once('"')
                    .chain(std::iter::repeat_n('#', hashes))
                    .collect();
                let from = i + 2 + hashes;
                Some(
                    (from..c.len())
                        .find(|&k| c[k..].starts_with(&close))
                        .map_or(c.len(), |k| k + close.len()),
                )
            }
            ('"', _) => {
                let mut k = i + 1;
                while k < c.len() && c[k] != '"' {
                    k += if c[k] == '\\' { 2 } else { 1 };
                }
                Some((k + 1).min(c.len()))
            }
            // A char literal ('x', '\n'), not a lifetime ('a).
            ('\'', Some('\\')) => (i + 3..c.len()).find(|&k| c[k] == '\'').map(|k| k + 1),
            ('\'', Some(_)) if at(i + 2) == Some('\'') => Some(i + 3),
            _ => None,
        }
    }

    /// The last top-level argument in `args`, whitespace removed outside string literals. A
    /// trailing comma (rustfmt's, on a call split over lines) ends no argument.
    fn last_arg(args: &[char]) -> String {
        let (mut depth, mut i) = (0i32, 0);
        let mut commas = Vec::new();
        while i < args.len() {
            if let Some(end) = skip_literal(args, i) {
                i = end;
                continue;
            }
            match args[i] {
                '(' | '[' | '{' => depth += 1,
                ')' | ']' | '}' => depth -= 1,
                ',' if depth == 0 => commas.push(i),
                _ => {}
            }
            i += 1;
        }
        let mut end = args.len();
        while let Some(&k) = commas.last() {
            if args[k + 1..end].iter().all(|ch| ch.is_whitespace()) {
                end = k;
                commas.pop();
            } else {
                break;
            }
        }
        let start = commas.last().map_or(0, |&k| k + 1);
        let arg = &args[start..end];
        let mut out = String::new();
        let mut k = 0;
        while k < arg.len() {
            if let Some(e) = skip_literal(arg, k) {
                out.extend(&arg[k..e]);
                k = e;
            } else {
                if !arg[k].is_whitespace() {
                    out.push(arg[k]);
                }
                k += 1;
            }
        }
        out
    }

    #[test]
    fn the_call_scan_skips_comments_strings_and_the_definition() {
        let text = "// with_info(ui, c, A)\n\
                    let s = \"with_info(x)\";\n\
                    pub fn with_info(ui: &mut Ui, control: Response, hover: &str) {}\n\
                    let _ = theme::with_info(ui, c, &hover(on));\n\
                    super::theme::with_info(\n    ui,\n    c,\n    &f(a, b),\n);\n\
                    with_info(ui, c, \"a, b\");\n\
                    foo_with_info(ui, c, X);\n";
        assert_eq!(
            calls_in(text),
            vec![
                (4, "&hover(on)".to_string()),
                (5, "&f(a,b)".to_string()),
                (10, "\"a, b\"".to_string()),
            ]
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::TextStyle;
    use hub_shell::theme::{family, HUD};

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
    fn system_follows_windows_app_mode() {
        use egui::ThemePreference as P;
        assert_eq!(Choice::System.preference(Some(AppMode::Light)), P::Light);
        assert_eq!(Choice::System.preference(Some(AppMode::Dark)), P::Dark);
        // Windows doesn't say: egui's own reading of the system theme.
        assert_eq!(Choice::System.preference(None), P::System);
        // An explicit choice wins.
        assert_eq!(Choice::Dark.preference(Some(AppMode::Light)), P::Dark);
        assert_eq!(Choice::Light.preference(Some(AppMode::Dark)), P::Light);
        // The picker still shows System while egui is told Light.
        let ctx = egui::Context::default();
        set_choice(&ctx, Choice::System, Some(AppMode::Light));
        assert_eq!(ctx.options(|o| o.theme_preference), P::Light);
        assert_eq!(choice(&ctx), Choice::System);
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
