//! The settings window's shell (#385, epic #393): a sidebar of pages, a status strip for both
//! halves across the top, and the chosen page below it.
//!
//! The sections themselves are unchanged. Each one names its page through its title
//! ([`SECTIONS`]): while a page is drawn, [`super::settings_ui::section`] skips the sections of
//! the other pages, so KeyPeek's settings code keeps its order and only gains this frame.

use std::cell::Cell;

use egui::{Align, CornerRadius, Layout, Margin, RichText, Sense, Stroke};
use serde::{Deserialize, Serialize};

use super::theme;

/// The sidebar's pages, top to bottom.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Page {
    #[default]
    Keyboard,
    Lights,
    Screens,
    Plugins,
    Settings,
}

impl Page {
    pub const ALL: [Page; 5] = [Page::Keyboard, Page::Lights, Page::Screens, Page::Plugins, Page::Settings];

    pub fn label(self) -> &'static str {
        match self {
            Page::Keyboard => "Keyboard",
            Page::Lights => "Lights",
            Page::Screens => "Screens",
            Page::Plugins => "Plugins",
            Page::Settings => "Settings",
        }
    }

    /// What an empty page says (the Screens sections only show with a keyboard connected).
    fn empty_hint(self) -> &'static str {
        match self {
            Page::Screens => "The screens show here once a keyboard with Foxglove's firmware is connected.",
            _ => "Nothing here yet.",
        }
    }
}

/// Every settings section by title, and the page it lives on (#385). A new section goes in
/// here too; the `every_section_has_a_page` test finds the ones that don't.
pub const SECTIONS: [(&str, Page); 12] = [
    ("Connection", Page::Keyboard),
    ("Keyboard", Page::Keyboard),
    ("Lights", Page::Lights),
    ("Screens", Page::Screens),
    ("Right screen art", Page::Screens),
    ("Plugins", Page::Plugins),
    ("Overlay Appearance", Page::Settings),
    ("Theme", Page::Settings),
    ("Profile", Page::Settings),
    ("Updates", Page::Settings),
    ("Other keyboards (VIA)", Page::Settings),
    // The links, credits and version at the end of the window.
    ("About", Page::Settings),
];

/// The page a section goes on. An unknown title lands on Settings, so it is never lost.
pub fn page_of(title: &str) -> Page {
    SECTIONS.iter().find(|(t, _)| *t == title).map_or(Page::Settings, |(_, p)| *p)
}

thread_local! {
    /// The page being drawn, while [`frame`] draws it; `None` draws every section.
    static SHOWING: Cell<Option<Page>> = const { Cell::new(None) };
    /// Whether any section drew on the page so far.
    static DREW: Cell<bool> = const { Cell::new(false) };
    /// The chosen page, loaded from the store on first use.
    static CURRENT: Cell<Option<Page>> = const { Cell::new(None) };
}

/// Whether the section titled `title` is on the page being drawn.
pub fn shows(title: &str) -> bool {
    let shown = SHOWING.get().is_none_or(|page| page_of(title) == page);
    if shown {
        DREW.set(true);
    }
    shown
}

/// The store's `[window]` section: the last page, kept between runs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct WindowSettings {
    page: Page,
}

fn current() -> Page {
    CURRENT.get().unwrap_or_else(|| {
        let page = super::store::try_load::<WindowSettings>("window").unwrap_or_default().page;
        CURRENT.set(Some(page));
        page
    })
}

/// Shows `page` without saving it, for the previews.
#[cfg(test)]
pub fn preview_page(page: Page) {
    CURRENT.set(Some(page));
}

/// Switches to `page` and remembers it.
pub fn set_page(page: Page) {
    CURRENT.set(Some(page));
    if let Err(e) = super::store::save("window", &WindowSettings { page }) {
        eprintln!("Foxglove: can't save the settings page: {e}");
    }
}

/// Puts `SHOWING` back however the page's drawing ends.
struct Showing;
impl Drop for Showing {
    fn drop(&mut self) {
        SHOWING.set(None);
    }
}

const SIDEBAR_WIDTH: f32 = 196.0;

/// Draws the sidebar, the status strip and the current page; `add` draws the sections, and only
/// those of the current page show.
pub fn frame(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    let p = theme::palette(ui);
    let mut page = current();
    egui::Panel::left("foxglove-sidebar")
        .resizable(false)
        .exact_size(SIDEBAR_WIDTH)
        .frame(egui::Frame::new().fill(p.well).inner_margin(Margin { left: 12, right: 12, top: 16, bottom: 12 }))
        .show(ui, |ui| sidebar(ui, &mut page));
    if page != current() {
        set_page(page);
    }
    let mut go = None;
    let strip = strip_state(ui.ctx());
    egui::Panel::top("foxglove-status")
        .frame(egui::Frame::new().fill(p.ground).inner_margin(Margin::symmetric(18, 10)))
        .show(ui, |ui| status_strip(ui, &strip));
    egui::CentralPanel::default()
        .frame(egui::Frame::new().fill(p.ground).inner_margin(Margin { left: 22, right: 14, top: 14, bottom: 10 }))
        .show(ui, |ui| {
            egui::ScrollArea::vertical().id_salt(("foxglove-page", page)).auto_shrink(false).show(ui, |ui| {
                ui.label(RichText::new(page.label()).font(theme::hud(24.0)).color(p.text_strong));
                if page == Page::Keyboard {
                    // The board and the keyboard's cards lead the page; the device picker
                    // (KeyPeek's Connection section) follows them (#386).
                    go = super::keyboard_page::top(ui);
                }
                let _showing = Showing;
                SHOWING.set(Some(page));
                DREW.set(false);
                add(ui);
                if !DREW.get() {
                    ui.add_space(8.0);
                    ui.weak(page.empty_hint());
                }
                ui.add_space(8.0);
            });
        });
    // A click on the page itself (the board's screens and lights) opens another page.
    if let Some(to) = go.filter(|to| *to != page) {
        set_page(to);
        ui.ctx().request_repaint();
    }
}

fn sidebar(ui: &mut egui::Ui, page: &mut Page) {
    let p = theme::palette(ui);
    ui.horizontal(|ui| {
        fox_mark(ui, 30.0);
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            ui.label(RichText::new("FOXGLOVE").font(theme::hud(18.0)).color(p.text_strong));
            ui.label(RichText::new("STEEL FOR GOOD").font(theme::mono(10.0)).color(p.muted));
        });
    });
    ui.add_space(16.0);
    for item in Page::ALL {
        if nav_button(ui, *page == item, item.label()).clicked() {
            *page = item;
        }
        ui.add_space(2.0);
    }
    ui.with_layout(Layout::bottom_up(Align::Min), |ui| {
        super::profiles_ui::sidebar_picker(ui);
        ui.label(theme::caps_label(ui, "Profile"));
    });
}

/// The fox mark: an orange arrowhead in a ring.
fn fox_mark(ui: &mut egui::Ui, size: f32) {
    let p = theme::palette(ui);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(size, size), Sense::hover());
    let at = |x: f32, y: f32| rect.min + egui::vec2(x, y) * (size / 30.0);
    let painter = ui.painter();
    painter.circle_stroke(at(15.0, 15.0), 13.0 * size / 30.0, Stroke::new(1.0, p.control_line));
    painter.add(egui::Shape::closed_line(
        vec![at(15.0, 3.0), at(27.0, 26.0), at(15.0, 20.0), at(3.0, 26.0)],
        Stroke::new(2.0, p.accent),
    ));
}

/// A sidebar entry: a square dot and the page's name; the current page sits on a raised fill.
fn nav_button(ui: &mut egui::Ui, selected: bool, label: &str) -> egui::Response {
    let p = theme::palette(ui);
    let (rect, response) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 36.0), Sense::click());
    response.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, selected, label));
    let hovered = response.hovered() || response.has_focus();
    let painter = ui.painter();
    if selected {
        painter.rect(rect, CornerRadius::same(6), p.surface_raised, Stroke::new(1.0, p.line), egui::StrokeKind::Inside);
    }
    if response.has_focus() {
        painter.rect_stroke(rect, CornerRadius::same(6), Stroke::new(2.0, p.accent), egui::StrokeKind::Inside);
    }
    let dot = egui::Rect::from_center_size(egui::pos2(rect.left() + 18.0, rect.center().y), egui::vec2(8.0, 8.0));
    painter.rect_filled(dot, CornerRadius::same(2), if selected { p.accent } else { p.control_line });
    let color = if selected || hovered { p.text_strong } else { p.text };
    painter.text(
        egui::pos2(rect.left() + 32.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        theme::body(14.5),
        color,
    );
    response
}

/// What the status strip shows, gathered once per frame.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StripState {
    /// A keyboard with Foxglove's firmware answers.
    pub keyboard: bool,
    /// The left half talks to this PC over USB (`Some(false)`: Bluetooth).
    pub usb: Option<bool>,
    /// Each half's build (`version · sha`), or why it couldn't be read; `None` while unknown.
    pub left_build: Option<Result<String, String>>,
    pub right_build: Option<Result<String, String>>,
    pub right_battery: Option<i64>,
    /// The active Bluetooth profile, from 0.
    pub profile: Option<u8>,
    /// The profile this PC is paired on, from 0.
    pub this_pc: Option<u8>,
    /// Set when unplugging now would leave the keyboard unable to type: how to fix it.
    pub unplug_warning: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Tone {
    Ok,
    Warn,
    Off,
}

/// One chip of the strip: a status dot, an optional bold name, the words, and a hover text.
#[derive(Clone, Debug, PartialEq)]
pub struct Chip {
    pub tone: Option<Tone>,
    pub name: &'static str,
    pub text: String,
    pub hover: Option<String>,
}

/// The strip's chips: left (connection, firmware), right (link, battery), the Bluetooth profile,
/// and a plain-word warning when unplugging would leave the keyboard unable to type.
pub fn chips(s: &StripState) -> Vec<Chip> {
    let chip = |tone, name, text: String| Chip { tone: Some(tone), name, text, hover: None };
    if !s.keyboard {
        return vec![
            chip(Tone::Off, "Left", "not connected".into()),
            chip(Tone::Off, "Right", "not connected".into()),
        ];
    }
    let mut out = Vec::new();
    let on = match s.usb {
        Some(true) => "on USB",
        Some(false) => "on Bluetooth",
        None => "connected",
    };
    out.push(match &s.left_build {
        Some(Ok(build)) => chip(Tone::Ok, "Left", format!("{on} · {build}")),
        Some(Err(e)) => Chip { hover: Some(e.clone()), ..chip(Tone::Warn, "Left", format!("{on} · firmware unknown")) },
        None => chip(Tone::Ok, "Left", on.to_string()),
    });
    let battery = s.right_battery.map(|b| format!(" · battery {b}%")).unwrap_or_default();
    out.push(match &s.right_build {
        Some(Ok(build)) => Chip { hover: Some(build.clone()), ..chip(Tone::Ok, "Right", format!("linked{battery}")) },
        Some(Err(e)) => Chip { hover: Some(e.clone()), ..chip(Tone::Warn, "Right", format!("no reply yet{battery}")) },
        None => chip(Tone::Off, "Right", format!("asking…{battery}")),
    });
    if let Some(active) = s.profile {
        let whose = match s.this_pc {
            Some(pc) if pc == active => " · this PC",
            Some(_) => " · another PC",
            None => "",
        };
        out.push(Chip { tone: None, name: "", text: format!("Bluetooth profile {}{whose}", active.saturating_add(1)), hover: None });
    }
    if let Some(fix) = &s.unplug_warning {
        out.push(Chip {
            tone: Some(Tone::Warn),
            name: "",
            text: "Don't unplug: the keyboard couldn't type".into(),
            hover: Some(fix.clone()),
        });
    }
    out
}

#[cfg(test)]
thread_local! {
    /// The preview's strip, so it never asks a real keyboard.
    pub static PREVIEW_STRIP: std::cell::RefCell<Option<StripState>> = const { std::cell::RefCell::new(None) };
}

fn strip_state(ctx: &egui::Context) -> StripState {
    #[cfg(test)]
    if let Some(s) = PREVIEW_STRIP.with_borrow(|s| s.clone()) {
        return s;
    }
    use foxglove_devices::facts::Value;
    let facts = super::devices::bus().facts();
    let keyboard = facts.get("keyboard.connected") == Some(Value::Bool(true));
    let battery = match facts.get("keyboard.right.battery") {
        Some(Value::Int(b)) => Some(b),
        _ => None,
    };
    let link = super::connection::summary(ctx);
    let (left_build, right_build) = match super::keyboard_settings::firmware(ctx) {
        super::keyboard_settings::Firmware::Known { left, right } => {
            let text = |r: Result<digitalis::protocol::BuildInfo, String>| r.map(|b| format!("{} · {}", b.version, b.sha));
            (Some(text(left)), Some(text(right)))
        }
        _ => (None, None),
    };
    StripState {
        keyboard,
        usb: link.usb,
        left_build,
        right_build,
        right_battery: battery,
        profile: link.profile,
        this_pc: link.this_pc,
        unplug_warning: link.unplug_warning,
    }
}

fn status_strip(ui: &mut egui::Ui, s: &StripState) {
    ui.horizontal(|ui| {
        // The chips wrap in what's left beside the button.
        const BUTTON_ROOM: f32 = 150.0;
        let chips_width = (ui.available_width() - BUTTON_ROOM).max(120.0);
        let wrap = Layout::left_to_right(Align::Center).with_main_wrap(true);
        ui.allocate_ui_with_layout(egui::vec2(chips_width, 0.0), wrap, |ui| {
            ui.spacing_mut().item_spacing = egui::vec2(8.0, 6.0);
            for c in chips(s) {
                chip(ui, &c);
            }
        });
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            // Wired once the diagnostics report lands (#371).
            ui.add_enabled(false, egui::Button::new("Copy diagnostics"))
                .on_disabled_hover_text("Copies a report for bug reports. Coming in a later update.");
        });
    });
}

/// One chip, sized before it's placed so the strip can wrap it to the next row.
fn chip(ui: &mut egui::Ui, c: &Chip) {
    let p = theme::palette(ui);
    let warn = c.tone == Some(Tone::Warn);
    let mut job = egui::text::LayoutJob::default();
    let font = theme::body(13.0);
    if !c.name.is_empty() {
        job.append(c.name, 0.0, egui::TextFormat::simple(font.clone(), p.text_strong));
    }
    let color = if warn && c.name.is_empty() { p.warn } else { p.text };
    job.append(&c.text, if c.name.is_empty() { 0.0 } else { 6.0 }, egui::TextFormat::simple(font, color));
    let galley = ui.ctx().fonts_mut(|f| f.layout_job(job));
    let pad = egui::vec2(10.0, 6.0);
    let dot = if c.tone.is_some() { 14.0 } else { 0.0 };
    let size = egui::vec2(dot + galley.size().x, galley.size().y) + pad * 2.0;
    let (rect, response) = ui.allocate_exact_size(size, Sense::hover());
    let line = if warn { p.warn.gamma_multiply(0.45) } else { p.line };
    let painter = ui.painter();
    painter.rect(rect, CornerRadius::same(6), p.surface, Stroke::new(1.0, line), egui::StrokeKind::Inside);
    if let Some(tone) = c.tone {
        let color = match tone {
            Tone::Ok => p.ok,
            Tone::Warn => p.warn,
            Tone::Off => p.muted,
        };
        painter.circle_filled(egui::pos2(rect.left() + pad.x + 4.0, rect.center().y), 4.0, color);
    }
    painter.galley(rect.min + pad + egui::vec2(dot, 0.0), galley, p.text);
    if let Some(h) = &c.hover {
        response.on_hover_text(h);
    }
}

/// The settings window's frame: the theme's window look with no inner margin, so the sidebar
/// and the status strip reach its edges.
pub fn window_frame(ctx: &egui::Context) -> egui::Frame {
    egui::Frame::window(&ctx.global_style()).inner_margin(Margin::ZERO)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sections_land_on_their_pages() {
        // The table from #385.
        for (title, page) in [
            ("Keyboard", Page::Keyboard),
            ("Lights", Page::Lights),
            ("Screens", Page::Screens),
            ("Right screen art", Page::Screens),
            ("Plugins", Page::Plugins),
            ("Overlay Appearance", Page::Settings),
            ("Theme", Page::Settings),
            ("Profile", Page::Settings),
            ("Updates", Page::Settings),
            ("Other keyboards (VIA)", Page::Settings),
        ] {
            assert_eq!(page_of(title), page, "{title}");
        }
        assert_eq!(page_of("Something new"), Page::Settings);
    }

    /// Every `section(ui, "…")` and `titled_group(ui, "…")` in the app is in [`SECTIONS`], so
    /// moving to pages loses none of them.
    #[test]
    fn every_section_has_a_page() {
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut missing = Vec::new();
        let mut stack = vec![src];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                // This file names the pattern in its docs.
                if path.extension().is_none_or(|e| e != "rs") || path.ends_with("shell.rs") {
                    continue;
                }
                let text = std::fs::read_to_string(&path).unwrap();
                for call in ["section(ui, \"", "titled_group(ui, \""] {
                    for (i, _) in text.match_indices(call) {
                        let rest = &text[i + call.len()..];
                        let title = &rest[..rest.find('"').unwrap()];
                        // KeyPeek's keymap editor has its own titled groups, not settings sections.
                        if path.to_string_lossy().contains("keymap_editor") {
                            continue;
                        }
                        if !SECTIONS.iter().any(|(t, _)| *t == title) {
                            missing.push(format!("{title} ({})", path.display()));
                        }
                    }
                }
            }
        }
        assert!(missing.is_empty(), "sections without a page: {missing:?}");
    }

    #[test]
    fn only_the_current_pages_sections_show() {
        assert!(shows("Plugins"), "outside the shell everything shows");
        SHOWING.set(Some(Page::Lights));
        DREW.set(false);
        assert!(!shows("Keyboard"));
        assert!(!DREW.get());
        assert!(shows("Lights"));
        assert!(DREW.get());
        SHOWING.set(None);
    }

    #[test]
    fn the_page_is_stored_by_name() {
        let text = toml::to_string(&WindowSettings { page: Page::Screens }).unwrap();
        assert_eq!(text.trim(), "page = \"screens\"");
        let back: WindowSettings = toml::from_str("").unwrap();
        assert_eq!(back.page, Page::Keyboard);
        let unknown: Result<WindowSettings, _> = toml::from_str("page = \"lab\"");
        assert!(unknown.is_err(), "an unknown page fails the load, and try_load falls back");
    }

    #[test]
    fn the_page_is_remembered() {
        super::super::store::test_config_dir();
        set_page(Page::Plugins);
        CURRENT.set(None);
        assert_eq!(current(), Page::Plugins);
        set_page(Page::Keyboard);
    }

    #[test]
    fn strip_without_a_keyboard() {
        let c = chips(&StripState::default());
        assert_eq!(c.len(), 2);
        assert!(c.iter().all(|c| c.tone == Some(Tone::Off) && c.text == "not connected"));
    }

    #[test]
    fn strip_with_both_halves() {
        let s = StripState {
            keyboard: true,
            usb: Some(true),
            left_build: Some(Ok("0.1.0 · e255d32".into())),
            right_build: Some(Err("no reply from the keyboard".into())),
            right_battery: Some(61),
            profile: Some(0),
            this_pc: Some(0),
            unplug_warning: None,
        };
        let c = chips(&s);
        assert_eq!(c[0].text, "on USB · 0.1.0 · e255d32");
        assert_eq!((c[1].tone, c[1].text.as_str()), (Some(Tone::Warn), "no reply yet · battery 61%"));
        assert_eq!(c[2].text, "Bluetooth profile 1 · this PC");
        assert_eq!(c.len(), 3, "no warning while it's safe to unplug");
    }

    #[test]
    fn strip_warns_before_an_unsafe_unplug() {
        let s = StripState {
            keyboard: true,
            usb: Some(true),
            profile: Some(1),
            this_pc: Some(0),
            unplug_warning: Some("select profile 1".into()),
            ..Default::default()
        };
        let c = chips(&s);
        assert_eq!(c[2].text, "Bluetooth profile 2 · another PC");
        let warn = c.last().unwrap();
        assert_eq!(warn.tone, Some(Tone::Warn));
        assert_eq!(warn.hover.as_deref(), Some("select profile 1"));
        assert_eq!(c[1].text, "asking…");
    }
}
