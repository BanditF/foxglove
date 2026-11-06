//! The keyboard switcher (#800, C2; docs/design/device-focus-layouts-thumbnails-lights-declutter.md
//! section 1): the first item of the status strip names the focused keyboard, and with more than
//! one keyboard it is a clickable chip whose list ("Keyboard shown in Foxglove") picks another.
//! One click opens the list, one click picks. With one keyboard it is a flat chip with no ▾: there
//! is no choice to offer.
//!
//! The pages follow the focus through [`Focused`]: card titles take its name, and a keyboard
//! Foxglove can't change (VIA, LampArray only) gets muted controls and a "read-only" chip.
//!
//! Everything here reads C1's model (`keyboard_focus`), which the device layer fills on its own
//! events: switching repaints from those caches and asks no device anything on the UI thread.

use egui::{CornerRadius, Sense, Stroke};

use super::keyboard_focus::{self as kf, Keyboard, Source};
use super::theme;

/// The read-only chip's words.
pub const READ_ONLY: &str = "read-only";

/// The list's title.
pub const LIST_TITLE: &str = "Keyboard shown in Foxglove";

/// The ▾ after the focused keyboard's name, from the icon font every face falls back to.
pub const CARET: &str = egui_phosphor::regular::CARET_DOWN;

/// What a page says about the focused keyboard.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Focused {
    /// The keyboard's name, for the card titles ("Eyelash Corne", not "Keyboard").
    pub name: String,
    /// For a keyboard Foxglove can't change: the read-only chip's hover, why its controls are
    /// muted.
    pub read_only: Option<String>,
}

impl Focused {
    pub fn of(k: &Keyboard) -> Focused {
        Focused {
            name: k.name.clone(),
            read_only: read_only_hover(k.source),
        }
    }

    /// The focused keyboard, from C1's cache; `None` before any keyboard was found.
    pub fn now() -> Option<Focused> {
        kf::current().as_ref().map(Focused::of)
    }
}

/// The card title for `focus`: its name, else `fallback` (no keyboard found yet).
pub fn title<'a>(focus: Option<&'a Focused>, fallback: &'a str) -> &'a str {
    focus
        .map(|f| f.name.as_str())
        .filter(|n| !n.is_empty())
        .unwrap_or(fallback)
}

/// The card titles' hover: where the keyboard is picked.
pub const TITLE_HOVER: &str =
    "The keyboard shown in Foxglove. Pick another at the start of the status strip.";

/// Why a keyboard found over `source` has muted controls, as the read-only chip's hover; `None`
/// for the host link, which has them all.
pub fn read_only_hover(source: Source) -> Option<String> {
    let limit = source.limit()?;
    let mut chars = limit.chars();
    let limit = chars
        .next()
        .map(|c| c.to_uppercase().chain(chars).collect::<String>())
        .unwrap_or_default();
    Some(match source {
        Source::Via => format!(
            "{limit}: Foxglove shows this keyboard's layout but doesn't change its keymap, \
             lights or firmware."
        ),
        _ => format!("{limit}: Foxglove can't read or change its keymap or firmware."),
    })
}

/// The muted "read-only" chip (flat, a grey dot); `hover` says why.
pub fn read_only_chip(ui: &mut egui::Ui, hover: &str) -> egui::Response {
    theme::hint_chip(ui, READ_ONLY, Some(hover), Some(theme::Dot::Muted))
}

/// One row of the list.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Row {
    pub key: String,
    pub name: String,
    /// "protocol · link · what Foxglove can do": "ZMK · USB · Foxglove firmware".
    pub detail: String,
    /// `false`: it went this run, greyed.
    pub connected: bool,
    pub focused: bool,
    /// The read-only chip's hover, for a keyboard Foxglove can't change.
    pub read_only: Option<String>,
}

/// The switcher: every keyboard known this run, in the order first found.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Switcher {
    pub rows: Vec<Row>,
}

impl Switcher {
    /// From the focus model's keyboards; `host_link` is how the host link reaches its keyboard
    /// (`USB` or `BLE`), when known.
    pub fn new(keyboards: &[Keyboard], focus: Option<&str>, host_link: Option<&str>) -> Switcher {
        Switcher {
            rows: keyboards
                .iter()
                .map(|k| Row {
                    key: k.key.clone(),
                    name: k.name.clone(),
                    detail: detail(k, host_link),
                    connected: k.connected,
                    focused: focus == Some(k.key.as_str()),
                    read_only: read_only_hover(k.source),
                })
                .collect(),
        }
    }

    /// The switcher as the caches have it now; asks no device.
    pub fn now() -> Switcher {
        let focus = kf::current().map(|k| k.key);
        Switcher::new(
            &kf::keyboards(),
            focus.as_deref(),
            super::devices::host_link(),
        )
    }

    pub fn focused(&self) -> Option<&Row> {
        self.rows.iter().find(|r| r.focused)
    }

    /// A choice to offer: more than one keyboard.
    pub fn can_switch(&self) -> bool {
        self.rows.len() > 1
    }
}

/// A list row's second line: "ZMK · USB · Foxglove firmware", "VIA · USB · read-only",
/// "ZMK · Bluetooth · not connected".
pub fn detail(k: &Keyboard, host_link: Option<&str>) -> String {
    let protocol = match k.source {
        Source::HostLink => "ZMK",
        Source::Via => "VIA",
        Source::LampArray => "Dynamic Lighting",
    };
    let link = match k.source {
        Source::HostLink => host_link.map(|l| if l == "BLE" { "Bluetooth" } else { l }),
        // VIA's raw HID runs over USB; Windows doesn't say how a LampArray is connected.
        Source::Via => Some("USB"),
        Source::LampArray => None,
    };
    let can = if !k.connected {
        "not connected"
    } else {
        match k.source {
            Source::HostLink => "Foxglove firmware",
            Source::Via => "read-only",
            Source::LampArray => "lights only",
        }
    };
    [Some(protocol), link, Some(can)]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · ")
}

/// The closed switcher's hover with a choice: "2 keyboards · switch".
pub fn switch_hover(s: &Switcher) -> String {
    format!("{} keyboards · switch", s.rows.len())
}

fn popup_id() -> egui::Id {
    egui::Id::new("foxglove-focus-switcher")
}

#[cfg(test)]
thread_local! {
    /// The previews draw the list open.
    pub static PREVIEW_OPEN: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// Where the list's rows landed, for the click tests.
    pub static LAST_ROWS: std::cell::RefCell<Vec<egui::Rect>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// The switcher at the head of the status strip; returns the key of a keyboard picked from its
/// list. Nothing with no keyboard; a flat chip with one; a clickable chip with ▾ and its list
/// with more.
pub fn switcher(ui: &mut egui::Ui, s: &Switcher) -> Option<String> {
    let focused = s.focused().or(s.rows.first())?;
    let dot = (!focused.connected).then_some(theme::Dot::Muted);
    let gone = if focused.connected {
        String::new()
    } else {
        " · not connected".to_string()
    };
    if !s.can_switch() {
        theme::hint_chip(
            ui,
            &focused.name,
            Some(&format!("{}{gone}", focused.detail)),
            dot,
        );
        return None;
    }
    let chip = theme::click_chip(
        ui,
        &format!("{} {CARET}", focused.name),
        Some(&format!("{}{gone}", switch_hover(s))),
        dot,
    );
    #[cfg(test)]
    if PREVIEW_OPEN.get() {
        egui::Popup::open_id(ui.ctx(), popup_id());
    }
    let mut picked = None;
    egui::Popup::menu(&chip).id(popup_id()).show(|ui| {
        ui.label(theme::caps_label(ui, LIST_TITLE));
        ui.add_space(2.0);
        #[cfg(test)]
        let mut rects = Vec::new();
        for row in &s.rows {
            let r = list_row(ui, row);
            #[cfg(test)]
            rects.push(r.rect);
            if r.clicked() && !row.focused {
                picked = Some(row.key.clone());
            }
        }
        #[cfg(test)]
        LAST_ROWS.set(rects);
    });
    picked
}

/// A list row's size, and the thumbnail's slot at its left (D2 #804 draws the device there).
const ROW: egui::Vec2 = egui::vec2(320.0, 46.0);
const THUMB: egui::Vec2 = egui::vec2(44.0, 30.0);

/// One keyboard in the list: its thumbnail, its name and "protocol · link · what Foxglove can
/// do"; the focused one on the selected fill with a check, a gone one greyed.
fn list_row(ui: &mut egui::Ui, row: &Row) -> egui::Response {
    let p = theme::palette(ui);
    let (rect, response) = ui.allocate_exact_size(ROW, Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::WidgetType::SelectableLabel,
            true,
            row.focused,
            &row.name,
        )
    });
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        let radius = CornerRadius::same(6);
        if row.focused {
            painter.rect_filled(rect, radius, p.selected_fill);
        } else if response.hovered() || response.has_focus() {
            painter.rect_filled(rect, radius, p.control);
        }
        if response.has_focus() {
            painter.rect_stroke(
                rect,
                radius,
                Stroke::new(2.0, p.accent),
                egui::StrokeKind::Inside,
            );
        }
        let thumb = egui::Rect::from_min_size(
            egui::pos2(rect.left() + 8.0, rect.center().y - THUMB.y / 2.0),
            THUMB,
        );
        keyboard_glyph(painter, thumb, p, row.connected);
        let (name, detail) = if row.connected {
            (p.text_strong, p.muted)
        } else {
            (p.muted, p.muted.gamma_multiply(0.8))
        };
        let x = thumb.right() + 10.0;
        painter.text(
            egui::pos2(x, rect.center().y - 1.0),
            egui::Align2::LEFT_BOTTOM,
            &row.name,
            theme::body(13.5),
            name,
        );
        painter.text(
            egui::pos2(x, rect.center().y + 1.0),
            egui::Align2::LEFT_TOP,
            &row.detail,
            theme::body(12.0),
            detail,
        );
        if row.focused {
            painter.text(
                egui::pos2(rect.right() - 12.0, rect.center().y),
                egui::Align2::RIGHT_CENTER,
                egui_phosphor::regular::CHECK,
                theme::body(14.0),
                p.accent,
            );
        }
    }
    if row.focused {
        response
    } else {
        response.on_hover_text(format!("Show {} in Foxglove", row.name))
    }
}

/// The thumbnail's place until D2 (#804) draws each device: a plain keyboard, outlined, with
/// three rows of keys; grey when the keyboard went.
fn keyboard_glyph(painter: &egui::Painter, rect: egui::Rect, p: &theme::Palette, lit: bool) {
    let line = if lit {
        p.control_line
    } else {
        p.control_line.gamma_multiply(0.6)
    };
    painter.rect_stroke(
        rect,
        CornerRadius::same(4),
        Stroke::new(1.0, line),
        egui::StrokeKind::Inside,
    );
    let inner = rect.shrink2(egui::vec2(5.0, 6.0));
    let (cols, rows) = (6, 3);
    let key = egui::vec2(inner.width() / cols as f32, inner.height() / rows as f32);
    for r in 0..rows {
        for c in 0..cols {
            let min = inner.min + egui::vec2(c as f32 * key.x, r as f32 * key.y);
            painter.rect_filled(
                egui::Rect::from_min_size(min, key - egui::vec2(1.5, 1.5)),
                CornerRadius::same(1),
                line,
            );
        }
    }
}

/// The user picked `key`: it becomes the focus (C1 saves and announces it). Find my LEDs and the
/// calibrator end as on leaving the page, since they belonged to the keyboard shown before; the
/// next frame draws the new one from the caches.
pub fn pick(ctx: &egui::Context, key: &str) {
    if kf::current().is_some_and(|k| k.key == key) {
        return;
    }
    match kf::set(key) {
        Ok(_) => {
            super::calibrate_ui::cancel();
            super::led_map_ui::cancel();
        }
        Err(e) => eprintln!("Foxglove: can't show that keyboard: {e}"),
    }
    ctx.request_repaint();
}

#[cfg(test)]
#[path = "focus_ui_tests.rs"]
pub(crate) mod tests;
