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
    /// Scenes across every light (#487): its own page, since a scene spans the devices that
    /// Lights lists one by one.
    Scenes,
    Screens,
    Plugins,
    Settings,
}

impl Page {
    pub const ALL: [Page; 6] = [
        Page::Keyboard,
        Page::Lights,
        Page::Scenes,
        Page::Screens,
        Page::Plugins,
        Page::Settings,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Page::Keyboard => "Keyboard",
            Page::Lights => "Lights",
            Page::Scenes => "Scenes",
            Page::Screens => "Screens",
            Page::Plugins => "Plugins",
            Page::Settings => "App",
        }
    }

    /// What an empty page says (the Screens sections only show with a keyboard connected).
    fn empty_hint(self) -> &'static str {
        match self {
            Page::Screens => {
                "The screens show here once a keyboard with Foxglove's firmware is connected."
            }
            _ => "Nothing here yet.",
        }
    }
}

/// Every settings section by title, and the page it lives on (#385). A new section goes in
/// here too; the `every_section_has_a_page` test finds the ones that don't.
pub const SECTIONS: [(&str, Page); 19] = [
    ("Connection", Page::Keyboard),
    ("Keyboard", Page::Keyboard),
    // The layer overlay's card (#534): KeyPeek's appearance grid, its layer colours, then the
    // keymap source (KeyPeek's "Connection" above). Other keyboards close the page.
    ("Overlay Appearance", Page::Keyboard),
    ("Theme", Page::Keyboard),
    ("Other keyboards (VIA)", Page::Keyboard),
    ("Lights", Page::Lights),
    ("Scenes", Page::Scenes),
    // The art flow's tiers (#516), then the left screen's own settings.
    ("Showing now", Page::Screens),
    ("On the keyboard", Page::Screens),
    ("On this PC", Page::Screens),
    ("Left screen", Page::Screens),
    ("Notifications", Page::Screens),
    ("Plugins", Page::Plugins),
    ("Profile", Page::Settings),
    ("Appearance", Page::Settings),
    ("Startup and updates", Page::Settings),
    ("Live wallpaper", Page::Settings),
    // Copy diagnostics, the log folder and the connection details (#535).
    ("Troubleshooting", Page::Settings),
    // The links, credits and version at the end of the window.
    ("About", Page::Settings),
];

/// The part of its page each section is drawn in, where the page draws in parts. The App page's
/// parts are its two columns (#390): Profiles, Appearance and
/// Troubleshooting on the left, the rest on the right. The
/// Keyboard page's are stacked (#534), so KeyPeek's sections keep their order in its file and
/// still show in the canvas's: the layer overlay's card (its grid, then its colours), the keymap
/// source at the card's foot, then Other keyboards. Unlisted sections are in part 1.
const PARTS: [(&str, usize); 7] = [
    ("Profile", 0),
    ("Appearance", 0),
    ("Troubleshooting", 0),
    ("Overlay Appearance", 0),
    ("Theme", 0),
    ("Connection", 1),
    ("Other keyboards (VIA)", 2),
];

/// How many parts the Keyboard page draws, one under the other.
const KEYBOARD_PARTS: usize = 3;

fn part_of(title: &str) -> usize {
    PARTS
        .iter()
        .find(|(t, _)| *t == title)
        .map_or(1, |(_, c)| *c)
}

/// The page a section goes on. An unknown title lands on App, so it is never lost.
pub fn page_of(title: &str) -> Page {
    SECTIONS
        .iter()
        .find(|(t, _)| *t == title)
        .map_or(Page::Settings, |(_, p)| *p)
}

thread_local! {
    /// The page being drawn, while [`frame`] draws it; `None` draws every section.
    static SHOWING: Cell<Option<Page>> = const { Cell::new(None) };
    /// Whether any section drew on the page so far.
    static DREW: Cell<bool> = const { Cell::new(false) };
    /// The part being drawn on a page drawn in parts; `None` draws them all.
    static COLUMN: Cell<Option<usize>> = const { Cell::new(None) };
    /// The chosen page, loaded from the store on first use.
    static CURRENT: Cell<Option<Page>> = const { Cell::new(None) };
}

/// Whether the section titled `title` is on the page being drawn.
pub fn shows(title: &str) -> bool {
    let shown = SHOWING.get().is_none_or(|page| page_of(title) == page)
        && COLUMN.get().is_none_or(|c| part_of(title) == c);
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
        let page = super::store::try_load::<WindowSettings>("window")
            .unwrap_or_default()
            .page;
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
    if page != Page::Lights {
        // Leaving the Lights page ends the colour calibrator as Cancel would.
        super::calibrate_ui::cancel();
        super::led_map_ui::cancel();
        super::pc_lights_ui::forget_remove();
    }
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
        COLUMN.set(None);
    }
}

const SIDEBAR_WIDTH: f32 = 196.0;
/// Room at the page's right for its scroll bar (10 points wide while the pointer is on it).
const SCROLL_GUTTER: f32 = 12.0;

/// Draws the sidebar, the status strip and the current page; `add` draws the sections, and only
/// those of the current page show. The App page calls it once per column (#390), the Keyboard
/// page once per part (#534).
pub fn frame(ui: &mut egui::Ui, mut add: impl FnMut(&mut egui::Ui)) {
    let p = theme::palette(ui);
    let mut page = current();
    egui::Panel::left("foxglove-sidebar")
        .resizable(false)
        .exact_size(SIDEBAR_WIDTH)
        .frame(egui::Frame::new().fill(p.well).inner_margin(Margin {
            left: 12,
            right: 12,
            top: 16,
            bottom: 12,
        }))
        .show(ui, |ui| sidebar(ui, &mut page));
    if page != current() {
        set_page(page);
    }
    let mut go = None;
    let strip = strip_state(ui.ctx());
    egui::Panel::top("foxglove-status")
        .frame(
            egui::Frame::new()
                .fill(p.ground)
                .inner_margin(Margin::symmetric(18, 10)),
        )
        .show(ui, |ui| status_strip(ui, &strip));
    egui::CentralPanel::default()
        .frame(egui::Frame::new().fill(p.ground).inner_margin(Margin {
            left: 22,
            // The rest of the right margin is the scroll bar's gutter, below.
            right: 2,
            top: 14,
            bottom: 10,
        }))
        .show(ui, |ui| {
            egui::ScrollArea::vertical()
                .id_salt(("foxglove-page", page))
                .auto_shrink(false)
                .show(ui, |ui| {
                    // A gutter for the scroll bar, which floats over the page's right edge:
                    // without it the bar covers the cards' edge and the controls there.
                    ui.set_max_width((ui.available_width() - SCROLL_GUTTER).max(0.0));
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(page.label())
                                .font(theme::hud(24.0))
                                .color(p.text_strong),
                        );
                        if page == Page::Plugins {
                            // "Install from file…" sits in the page's header (#389).
                            ui.with_layout(
                                Layout::right_to_left(Align::Center),
                                super::plugins_ui::header,
                            );
                        }
                    });
                    let _showing = Showing;
                    SHOWING.set(Some(page));
                    DREW.set(false);
                    if page == Page::Keyboard {
                        // The board and the keyboard's cards lead the page; the layer overlay's
                        // card and Other keyboards follow them (#386, #534).
                        go = super::keyboard_page::top(ui);
                    }
                    if page == Page::Scenes {
                        // The room map, the effect and the presets (#487).
                        super::scenes_ui::page(ui);
                    }
                    if page == Page::Screens {
                        // Showing now, On the keyboard, On this PC, the left screen (#516).
                        super::screens_ui::page(ui);
                    }
                    // A grid of cards (#390): each column draws only its own sections. The left one
                    // holds Profiles, whose rules need about 460 points; narrower windows stack
                    // the cards.
                    const GAP: f32 = 16.0;
                    let width = ui.available_width();
                    let left = (width * 0.55).max(460.0);
                    let right = width - left - GAP;
                    if page == Page::Keyboard {
                        for part in 0..KEYBOARD_PARTS {
                            COLUMN.set(Some(part));
                            add(ui);
                        }
                        COLUMN.set(None);
                    } else if page == Page::Settings && right >= 300.0 {
                        ui.horizontal_top(|ui| {
                            ui.spacing_mut().item_spacing.x = GAP;
                            for (i, w) in [left, right].into_iter().enumerate() {
                                ui.allocate_ui_with_layout(
                                    egui::vec2(w, 0.0),
                                    Layout::top_down(Align::Min),
                                    |ui| {
                                        ui.set_width(w);
                                        COLUMN.set(Some(i));
                                        add(ui);
                                    },
                                );
                            }
                        });
                        COLUMN.set(None);
                    } else {
                        add(ui);
                    }
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
            ui.label(
                RichText::new("FOXGLOVE")
                    .font(theme::hud(18.0))
                    .color(p.text_strong),
            );
            ui.label(
                RichText::new("STEEL FOR GOOD")
                    .font(theme::mono(10.0))
                    .color(p.muted),
            );
        });
    });
    ui.add_space(16.0);
    for item in Page::ALL {
        if nav_button(ui, *page == item, item.label()).clicked() {
            *page = item;
        }
        ui.add_space(2.0);
    }
}

/// The Bandit fox, in its 680 x 760 crop: the shapes `scripts/app_icons.py` draws the app icon
/// and the tray glyphs from (#299, #403). The sidebar's mark is the tray's solid glyph.
mod fox {
    pub const W: f32 = 680.0;
    pub const H: f32 = 760.0;
    pub const EAR: [(f32, f32); 3] = [(20.0, 80.0), (245.0, 300.0), (115.0, 430.0)];
    pub const FACE: [(f32, f32); 6] = [
        (238.0, 296.0),
        (442.0, 296.0),
        (608.0, 462.0),
        (402.0, 724.0),
        (278.0, 724.0),
        (72.0, 462.0),
    ];
    pub const CHEEK: [(f32, f32); 4] =
        [(8.0, 466.0), (170.0, 494.0), (310.0, 748.0), (250.0, 748.0)];
    pub const MASK: [(f32, f32); 4] = [
        (70.0, 456.0),
        (268.0, 500.0),
        (300.0, 700.0),
        (180.0, 572.0),
    ];
    pub const NOSE: [(f32, f32); 3] = [(300.0, 704.0), (380.0, 704.0), (340.0, 752.0)];

    /// One convex piece of the fox, in crop coordinates.
    pub type Piece = Vec<(f32, f32)>;

    pub fn mirror(p: &[(f32, f32)]) -> Piece {
        p.iter().map(|&(x, y)| (W - x, y)).collect()
    }

    /// The solid fox as convex pieces (ears, cheeks, face), then the holes (mask, nose).
    pub fn pieces() -> (Vec<Piece>, Vec<Piece>) {
        let solid = vec![
            EAR.to_vec(),
            mirror(&EAR),
            CHEEK.to_vec(),
            mirror(&CHEEK),
            FACE.to_vec(),
        ];
        let holes = vec![MASK.to_vec(), mirror(&MASK), NOSE.to_vec()];
        (solid, holes)
    }

    /// The points in clockwise order on screen (y down), as egui's convex fill wants them.
    pub fn clockwise(mut p: Vec<(f32, f32)>) -> Vec<(f32, f32)> {
        let n = p.len();
        let twice_area: f32 = (0..n)
            .map(|i| p[i].0 * p[(i + 1) % n].1 - p[(i + 1) % n].0 * p[i].1)
            .sum();
        if twice_area < 0.0 {
            p.reverse();
        }
        p
    }
}

/// The fox mark: the Bandit fox as the tray shows it (solid, the mask cut out), in orange.
fn fox_mark(ui: &mut egui::Ui, size: f32) {
    let p = theme::palette(ui);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(size, size), Sense::hover());
    // The tray glyph's framing: 6% margin top and bottom, centred across.
    let margin = size * 0.06;
    let scale = (size - 2.0 * margin) / fox::H;
    let origin = rect.min + egui::vec2((size - fox::W * scale) / 2.0, margin);
    // A hairline in the fill's own colour closes the anti-aliased seams where pieces meet.
    let shape = |points: Vec<(f32, f32)>, fill: egui::Color32| {
        let pts = fox::clockwise(points)
            .into_iter()
            .map(|(x, y)| origin + egui::vec2(x, y) * scale)
            .collect();
        egui::Shape::convex_polygon(pts, fill, Stroke::new(0.6, fill))
    };
    let (solid, holes) = fox::pieces();
    let painter = ui.painter();
    for piece in solid {
        painter.add(shape(piece, p.accent));
    }
    // Cut out in the sidebar's own colour, like the glyph's transparent mask.
    for hole in holes {
        painter.add(shape(hole, p.well));
    }
}

/// A sidebar entry: a square dot and the page's name; the current page sits on a raised fill.
fn nav_button(ui: &mut egui::Ui, selected: bool, label: &str) -> egui::Response {
    let p = theme::palette(ui);
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 36.0), Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, selected, label)
    });
    let hovered = response.hovered() || response.has_focus();
    let painter = ui.painter();
    if selected {
        painter.rect(
            rect,
            CornerRadius::same(6),
            p.surface_raised,
            Stroke::new(1.0, p.line),
            egui::StrokeKind::Inside,
        );
    }
    if response.has_focus() {
        painter.rect_stroke(
            rect,
            CornerRadius::same(6),
            Stroke::new(2.0, p.accent),
            egui::StrokeKind::Inside,
        );
    }
    let dot = egui::Rect::from_center_size(
        egui::pos2(rect.left() + 18.0, rect.center().y),
        egui::vec2(8.0, 8.0),
    );
    painter.rect_filled(
        dot,
        CornerRadius::same(2),
        if selected { p.accent } else { p.control_line },
    );
    let color = if selected || hovered {
        p.text_strong
    } else {
        p.text
    };
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
    /// How Foxglove's host link reaches the keyboard (`USB` or `BLE`), when known.
    pub host_link: Option<&'static str>,
    /// Each half's build (`version · sha`), or why it couldn't be read; `None` while unknown.
    pub left_build: Option<Result<String, String>>,
    pub right_build: Option<Result<String, String>>,
    pub right_battery: Option<i64>,
    /// The left half's own battery, once its firmware reports it (#373).
    pub left_battery: Option<i64>,
    /// The active Bluetooth profile, from 0.
    pub profile: Option<u8>,
    /// The profile this PC is paired on, from 0.
    pub this_pc: Option<u8>,
    /// Set when unplugging now would leave the keyboard unable to type: how to fix it.
    pub unplug_warning: Option<String>,
    /// The board's parts (#559): a chip each, named from its profile.
    pub parts: super::parts::Parts,
}

impl StripState {
    /// The parts the strip shows: the profile's, or two once a keyboard without one shows a
    /// second part (a right build or battery).
    pub fn parts(&self) -> super::parts::Parts {
        let second = self.right_battery.is_some() || matches!(self.right_build, Some(Ok(_)));
        self.parts.clone().seen(if second { 2 } else { 1 })
    }
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
    pub name: String,
    pub text: String,
    pub hover: Option<String>,
}

/// How the left half is connected, in a few words: how it types to this PC, and Foxglove's
/// host link too when that runs over the other transport (USB Raw HID is silent before firmware
/// #434, so the link can be BLE while typing goes over USB).
pub fn left_link(usb: Option<bool>, host_link: Option<&str>) -> String {
    let typing = match usb {
        Some(true) => "USB",
        Some(false) => "BLE",
        None => return "connected".into(),
    };
    match host_link {
        Some(link) if link != typing => format!("{typing} (typing) · host link {link}"),
        _ if typing == "USB" => "on USB".into(),
        _ => "on Bluetooth".into(),
    }
}

/// The strip's chips: the part the PC talks to (connection, firmware), on a split the other
/// half (link, battery), the Bluetooth profile, and a plain-word warning when unplugging would
/// leave the keyboard unable to type. Named by the board's parts (#559).
pub fn chips(s: &StripState) -> Vec<Chip> {
    let parts = s.parts();
    let (first, second) = (parts.short(0), parts.short(1));
    let chip = |tone, name: &str, text: String| Chip {
        tone: Some(tone),
        name: name.to_string(),
        text,
        hover: None,
    };
    if !s.keyboard {
        let mut out = vec![chip(Tone::Off, &first, "not connected".into())];
        if parts.is_split() {
            out.push(chip(Tone::Off, &second, "not connected".into()));
        }
        return out;
    }
    let mut out = Vec::new();
    let on = left_link(s.usb, s.host_link);
    let left_battery = s
        .left_battery
        .map(|b| format!(" · battery {b}%"))
        .unwrap_or_default();
    out.push(match &s.left_build {
        Some(Ok(build)) => chip(Tone::Ok, &first, format!("{on} · {build}{left_battery}")),
        Some(Err(e)) => Chip {
            hover: Some(e.clone()),
            ..chip(
                Tone::Warn,
                &first,
                format!("{on} · firmware unknown{left_battery}"),
            )
        },
        None => chip(Tone::Ok, &first, format!("{on}{left_battery}")),
    });
    let battery = s
        .right_battery
        .map(|b| format!(" · battery {b}%"))
        .unwrap_or_default();
    if parts.is_split() {
        out.push(match &s.right_build {
            Some(Ok(build)) => Chip {
                hover: Some(build.clone()),
                ..chip(Tone::Ok, &second, format!("linked{battery}"))
            },
            Some(Err(e)) => Chip {
                hover: Some(e.clone()),
                ..chip(Tone::Warn, &second, format!("no reply yet{battery}"))
            },
            None => chip(Tone::Off, &second, format!("asking…{battery}")),
        });
    }
    if let Some(active) = s.profile {
        let whose = match s.this_pc {
            Some(pc) if pc == active => " · this PC",
            Some(_) => " · another PC",
            None => "",
        };
        out.push(Chip {
            tone: None,
            name: String::new(),
            text: format!("Bluetooth slot {}{whose}", active.saturating_add(1)),
            hover: None,
        });
    }
    if let Some(fix) = &s.unplug_warning {
        out.push(Chip {
            tone: Some(Tone::Warn),
            name: String::new(),
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

pub fn strip_state(ctx: &egui::Context) -> StripState {
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
    let left_battery = match facts.get("keyboard.left.battery") {
        Some(Value::Int(b)) => Some(b),
        _ => None,
    };
    let link = super::connection::summary(ctx);
    let (left_build, right_build) = match super::keyboard_settings::firmware(ctx) {
        super::keyboard_settings::Firmware::Known { left, right } => {
            let text = |r: Result<digitalis::protocol::BuildInfo, String>| {
                r.map(|b| format!("{} · {}", b.version, b.sha))
            };
            (Some(text(left)), Some(text(right)))
        }
        _ => (None, None),
    };
    StripState {
        keyboard,
        usb: link.usb,
        host_link: super::devices::host_link(),
        left_build,
        right_build,
        right_battery: battery,
        left_battery,
        profile: link.profile,
        this_pc: link.this_pc,
        unplug_warning: link.unplug_warning,
        parts: super::parts::Parts::current(),
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
        // The window's header, at its top right (#535): the profile picker on every page, then
        // Copy diagnostics (#371, also in App > Troubleshooting).
        ui.with_layout(Layout::top_down(Align::Max), |ui| {
            ui.spacing_mut().item_spacing.y = 6.0;
            super::profiles_ui::header_picker(ui, BUTTON_ROOM - 8.0);
            super::troubleshooting::copy_diagnostics(ui);
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
        job.append(
            &c.name,
            0.0,
            egui::TextFormat::simple(font.clone(), p.text_strong),
        );
    }
    let color = if warn && c.name.is_empty() {
        p.warn
    } else {
        p.text
    };
    job.append(
        &c.text,
        if c.name.is_empty() { 0.0 } else { 6.0 },
        egui::TextFormat::simple(font, color),
    );
    let pad = egui::vec2(10.0, 6.0);
    let dot = if c.tone.is_some() { 14.0 } else { 0.0 };
    // One row, cut with "…" at the strip's width: a chip wider than the strip would widen the
    // settings window when the keyboard connects. The hover then gives the whole text.
    job.wrap = egui::text::TextWrapping {
        max_width: (ui.max_rect().width() - pad.x * 2.0 - dot).max(40.0),
        max_rows: 1,
        break_anywhere: true,
        overflow_character: Some('…'),
    };
    let galley = ui.ctx().fonts_mut(|f| f.layout_job(job));
    let elided = galley.elided;
    let size = egui::vec2(dot + galley.size().x, galley.size().y) + pad * 2.0;
    let (rect, response) = ui.allocate_exact_size(size, Sense::hover());
    let line = if warn {
        p.warn.gamma_multiply(0.45)
    } else {
        p.line
    };
    let painter = ui.painter();
    painter.rect(
        rect,
        CornerRadius::same(6),
        p.surface,
        Stroke::new(1.0, line),
        egui::StrokeKind::Inside,
    );
    if let Some(tone) = c.tone {
        let color = match tone {
            Tone::Ok => p.ok,
            Tone::Warn => p.warn,
            Tone::Off => p.muted,
        };
        painter.circle_filled(
            egui::pos2(rect.left() + pad.x + 4.0, rect.center().y),
            4.0,
            color,
        );
    }
    painter.galley(rect.min + pad + egui::vec2(dot, 0.0), galley, p.text);
    let hover = match (&c.hover, elided) {
        (Some(h), true) => Some(format!(
            "{} {}
{h}",
            c.name, c.text
        )),
        (Some(h), false) => Some(h.clone()),
        (None, true) => Some(format!("{} {}", c.name, c.text)),
        (None, false) => None,
    };
    if let Some(h) = hover {
        response.on_hover_text(h.trim_start());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The sidebar's fox is the one the app icon and tray glyphs are drawn from.
    #[test]
    fn the_fox_matches_app_icons_py() {
        let py = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../scripts/app_icons.py"
        ))
        .expect("app_icons.py");
        let fmt = |name: &str, pts: &[(f32, f32)]| {
            let inner: Vec<String> = pts.iter().map(|(x, y)| format!("({x}, {y})")).collect();
            format!("{name} = [{}]", inner.join(", "))
        };
        for (name, pts) in [
            ("EAR", &fox::EAR[..]),
            ("FACE", &fox::FACE[..]),
            ("CHEEK", &fox::CHEEK[..]),
            ("MASK", &fox::MASK[..]),
            ("NOSE", &fox::NOSE[..]),
        ] {
            let line = fmt(name, pts);
            assert!(
                py.lines().any(|l| l.trim() == line),
                "{line} not in app_icons.py"
            );
        }
        assert!(py.contains("FW, FH = 680.0, 760.0"));
    }

    #[test]
    fn fox_pieces_are_clockwise() {
        let (solid, holes) = fox::pieces();
        for piece in solid.into_iter().chain(holes) {
            let cw = fox::clockwise(piece.clone());
            let n = cw.len();
            let area: f32 = (0..n)
                .map(|i| cw[i].0 * cw[(i + 1) % n].1 - cw[(i + 1) % n].0 * cw[i].1)
                .sum();
            assert!(area > 0.0, "{piece:?}");
        }
    }

    #[test]
    fn sections_land_on_their_pages() {
        // The table from #385, with the layer overlay and other keyboards on Keyboard (#534).
        for (title, page) in [
            ("Keyboard", Page::Keyboard),
            ("Connection", Page::Keyboard),
            ("Overlay Appearance", Page::Keyboard),
            ("Theme", Page::Keyboard),
            ("Other keyboards (VIA)", Page::Keyboard),
            ("Lights", Page::Lights),
            ("Scenes", Page::Scenes),
            ("Showing now", Page::Screens),
            ("On the keyboard", Page::Screens),
            ("On this PC", Page::Screens),
            ("Left screen", Page::Screens),
            ("Plugins", Page::Plugins),
            ("Profile", Page::Settings),
            ("Appearance", Page::Settings),
            ("Startup and updates", Page::Settings),
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
    fn the_settings_page_splits_into_two_columns() {
        SHOWING.set(Some(Page::Settings));
        COLUMN.set(Some(0));
        assert!(shows("Profile") && shows("Appearance"));
        assert!(!shows("Startup and updates") && !shows("About"));
        COLUMN.set(Some(1));
        assert!(!shows("Profile") && !shows("Appearance"));
        assert!(shows("Startup and updates") && shows("Live wallpaper") && shows("About"));
        assert!(!shows("Lights"), "still only this page's sections");
        // Nothing about the overlay or other keyboards is on App any more (#534).
        COLUMN.set(None);
        for title in [
            "Overlay Appearance",
            "Theme",
            "Connection",
            "Other keyboards (VIA)",
        ] {
            assert!(!shows(title), "{title} is on App");
        }
        drop(Showing);
        assert_eq!((SHOWING.get(), COLUMN.get()), (None, None));
    }

    /// The Keyboard page (#534): the layer overlay's card, its keymap source at the foot, then
    /// Other keyboards, whatever order KeyPeek's file draws them in.
    #[test]
    fn the_keyboard_page_draws_the_overlay_card_then_other_keyboards() {
        SHOWING.set(Some(Page::Keyboard));
        let order: Vec<Vec<&str>> = (0..KEYBOARD_PARTS)
            .map(|part| {
                COLUMN.set(Some(part));
                // KeyPeek's order in ui_settings.rs, then Foxglove's.
                [
                    "Connection",
                    "Overlay Appearance",
                    "Theme",
                    "Other keyboards (VIA)",
                ]
                .into_iter()
                .filter(|t| shows(t))
                .collect()
            })
            .collect();
        assert_eq!(
            order,
            [
                vec!["Overlay Appearance", "Theme"],
                vec!["Connection"],
                vec!["Other keyboards (VIA)"],
            ]
        );
        drop(Showing);
    }

    #[test]
    fn the_page_is_stored_by_name() {
        let text = toml::to_string(&WindowSettings {
            page: Page::Screens,
        })
        .unwrap();
        assert_eq!(text.trim(), "page = \"screens\"");
        let back: WindowSettings = toml::from_str("").unwrap();
        assert_eq!(back.page, Page::Keyboard);
        let unknown: Result<WindowSettings, _> = toml::from_str("page = \"lab\"");
        assert!(
            unknown.is_err(),
            "an unknown page fails the load, and try_load falls back"
        );
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
        let corne = StripState {
            parts: crate::foxglove::fixtures::users_desk::board().parts(),
            ..Default::default()
        };
        let c = chips(&corne);
        assert_eq!(c.len(), 2);
        assert!(c
            .iter()
            .all(|c| c.tone == Some(Tone::Off) && c.text == "not connected"));
        assert_eq!((c[0].name.as_str(), c[1].name.as_str()), ("Left", "Right"));
        // #559: a one-piece board (or none known) is one chip, the keyboard's.
        let c = chips(&StripState::default());
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].name, "Keyboard");
    }

    /// #559: the second desk's one-piece board: one chip for the keyboard, no half.
    #[test]
    fn strip_with_a_one_piece_board() {
        let s = StripState {
            keyboard: true,
            usb: Some(true),
            left_build: Some(Ok("0.1.0 · e255d32".into())),
            right_build: Some(Err("no right half on this board".into())),
            profile: Some(0),
            this_pc: Some(0),
            parts: crate::foxglove::fixtures::second_desk::board().parts(),
            ..Default::default()
        };
        let c = chips(&s);
        assert_eq!(c.len(), 2, "the keyboard and the Bluetooth slot");
        assert_eq!(c[0].name, "Keyboard");
        assert!(c.iter().all(|c| !c.name.contains("Right")));
    }

    #[test]
    fn strip_with_both_halves() {
        let s = StripState {
            keyboard: true,
            usb: Some(true),
            left_build: Some(Ok("0.1.0 · e255d32".into())),
            right_build: Some(Err("no reply from the keyboard".into())),
            right_battery: Some(61),
            left_battery: Some(95),
            profile: Some(0),
            this_pc: Some(0),
            unplug_warning: None,
            host_link: Some("USB"),
            parts: crate::foxglove::fixtures::users_desk::board().parts(),
        };
        let c = chips(&s);
        assert_eq!(c[0].text, "on USB · 0.1.0 · e255d32 · battery 95%");
        // Typing over USB, the host link over BLE (USB Raw HID silent before #434).
        let ble = chips(&StripState {
            host_link: Some("BLE"),
            ..s.clone()
        });
        assert_eq!(
            ble[0].text,
            "USB (typing) · host link BLE · 0.1.0 · e255d32 · battery 95%"
        );
        assert_eq!(
            (c[1].tone, c[1].text.as_str()),
            (Some(Tone::Warn), "no reply yet · battery 61%")
        );
        assert_eq!(c[2].text, "Bluetooth slot 1 · this PC");
        assert_eq!(c.len(), 3, "no warning while it's safe to unplug");
    }

    #[test]
    fn the_left_link_names_both_transports_when_they_differ() {
        assert_eq!(
            left_link(Some(true), Some("BLE")),
            "USB (typing) · host link BLE"
        );
        assert_eq!(
            left_link(Some(false), Some("USB")),
            "BLE (typing) · host link USB"
        );
        assert_eq!(left_link(Some(true), Some("USB")), "on USB");
        assert_eq!(left_link(Some(false), Some("BLE")), "on Bluetooth");
        assert_eq!(left_link(Some(false), None), "on Bluetooth");
        assert_eq!(left_link(None, Some("BLE")), "connected");
    }

    #[test]
    fn strip_warns_before_an_unsafe_unplug() {
        let s = StripState {
            keyboard: true,
            usb: Some(true),
            profile: Some(1),
            this_pc: Some(0),
            unplug_warning: Some("select Bluetooth slot 1".into()),
            parts: crate::foxglove::fixtures::users_desk::board().parts(),
            ..Default::default()
        };
        let c = chips(&s);
        assert_eq!(c[2].text, "Bluetooth slot 2 · another PC");
        let warn = c.last().unwrap();
        assert_eq!(warn.tone, Some(Tone::Warn));
        assert_eq!(warn.hover.as_deref(), Some("select Bluetooth slot 1"));
        assert_eq!(c[1].text, "asking…");
    }
}
