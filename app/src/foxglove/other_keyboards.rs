//! The keyboards under Other devices (#820; docs/design/
//! device-focus-layouts-thumbnails-lights-declutter.md, section 1, "Rules"): a keyboard that
//! isn't the focus (a read-only VIA keyboard, a LampArray-only one, or the host-link keyboard
//! while another is shown) is listed there with its thumbnail and a "Show in Foxglove" link
//! that makes it the focus (`keyboard_focus::set`, through the switcher's own `focus_ui::pick`).
//! The pages follow the focus, so they show it on the next frame.
//!
//! A keyboard that has its own row there already (a Dynamic Lighting keyboard: its lights) gets
//! the link on that row. The focused keyboard has the page's keyboard card, so its row isn't
//! listed again: a focused LampArray keyboard's lights are the card's (#825,
//! `lights_page::Lamp`). Everything here reads C1's model as [`Keyboards`] holds it: no device
//! is asked.

use std::collections::BTreeMap;

use egui::RichText;
use foxglove_devices::pc_lighting::Control;

use super::device_thumb::{self as thumb, Thumb, Tint};
use super::focus_ui;
use super::key_layout::KeyLayout;
use super::keyboard_focus::{self as kf, Keyboard};
use super::pc_lights::Row;
use super::theme;

/// The focus model as Other devices reads it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Keyboards {
    /// Every keyboard known this run, gone ones too (`keyboard_focus::keyboards`).
    pub all: Vec<Keyboard>,
    /// The focused keyboard's key.
    pub focus: Option<String>,
    /// How the host link reaches its keyboard (`USB` or `BLE`), for a row's line.
    pub host_link: Option<String>,
    /// The keys of each keyboard listed on a row of its own, by key, for its thumbnail.
    pub layouts: BTreeMap<String, KeyLayout>,
}

impl Keyboards {
    /// From C1's caches, with `devices` the card's rows; asks no device.
    pub fn now(devices: &[Row]) -> Keyboards {
        let mut k = Keyboards {
            all: kf::keyboards(),
            focus: kf::current().map(|k| k.key),
            host_link: super::devices::host_link().map(String::from),
            layouts: BTreeMap::new(),
        };
        k.layouts = k
            .own_rows(devices)
            .into_iter()
            .map(|kb| (kb.key.clone(), kf::layout(kb)))
            .collect();
        k
    }

    fn focused(&self, k: &Keyboard) -> bool {
        self.focus.as_deref() == Some(k.key.as_str())
    }

    /// The keyboard a device row is: the Dynamic Lighting keyboard with its key, or one with a
    /// host-link or VIA keyboard's VID/PID (C1 lists those as that keyboard).
    pub fn of_row(&self, row: &Row) -> Option<&Keyboard> {
        self.all.iter().find(|k| is_row_of(k, row))
    }

    /// Whether the card lists `row`: not when it is the focused keyboard, which has the page's
    /// keyboard card (a LampArray keyboard's lights too, #825).
    pub fn listed(&self, row: &Row) -> bool {
        !self.of_row(row).is_some_and(|k| self.focused(k))
    }

    /// The keyboard whose "Show in Foxglove" link `row` carries: a connected one that isn't
    /// the focus.
    pub fn show_on(&self, row: &Row) -> Option<&Keyboard> {
        self.of_row(row).filter(|k| k.connected && !self.focused(k))
    }

    /// The keyboards listed on rows of their own: connected, not the focus, and not already a
    /// device row (`devices`).
    pub fn own_rows(&self, devices: &[Row]) -> Vec<&Keyboard> {
        self.all
            .iter()
            .filter(|k| k.connected && !self.focused(k))
            .filter(|k| {
                !devices
                    .iter()
                    .any(|r| self.of_row(r).is_some_and(|o| o.key == k.key))
            })
            .collect()
    }

    /// A keyboard row's line: the switcher's "protocol · link · what Foxglove can do".
    pub fn detail(&self, k: &Keyboard) -> String {
        focus_ui::detail(k, self.host_link.as_deref())
    }
}

/// Whether device row `row` is keyboard `k`'s lights: its own key, or a Dynamic Lighting row
/// with its VID/PID (C1 lists a LampArray with a keyboard's VID/PID as that keyboard).
pub fn is_row_of(k: &Keyboard, row: &Row) -> bool {
    k.key == row.key
        || (row.control == Control::LampArray && vid_pid(&row.key) == Some((k.vid, k.pid)))
}

/// A Lights key's VID and PID: `vvvv:pppp`, maybe followed by a serial.
fn vid_pid(key: &str) -> Option<(u16, u16)> {
    let (vid, rest) = key.split_once(':')?;
    Some((
        u16::from_str_radix(vid, 16).ok()?,
        u16::from_str_radix(rest.get(..4)?, 16).ok()?,
    ))
}

#[cfg(test)]
thread_local! {
    /// Where the links landed this frame, by keyboard key, for the click tests.
    pub static LAST_LINKS: std::cell::RefCell<Vec<(String, egui::Rect)>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// The "Show in Foxglove" link for `k` (accent text, the design's link): whether it was clicked.
pub fn link(ui: &mut egui::Ui, k: &Keyboard) -> bool {
    let r = ui
        .add(egui::Link::new(focus_ui::SHOW))
        .on_hover_text(focus_ui::show_hover(&k.name));
    #[cfg(test)]
    LAST_LINKS.with_borrow_mut(|l| l.push((k.key.clone(), r.rect)));
    r.clicked()
}

/// The space before a row's thumbnail: where a device row has its chevron, so the thumbnails
/// and names line up with theirs (`settings_ui::expander_lead`).
const LEAD: f32 = 26.0;

/// A keyboard on a row of its own, lined up with the device rows: its thumbnail (its own keys,
/// grey: its colours aren't known here), its name, its line ("VIA · USB · read-only") and the
/// link at the row's end. There is nothing to open, so no chevron. Whether the link was clicked.
pub fn row(ui: &mut egui::Ui, k: &Keyboard, detail: &str, layout: Option<&KeyLayout>) -> bool {
    let p = theme::palette(ui);
    let height = thumb::ROW.y + 4.0;
    ui.allocate_ui_with_layout(
        egui::vec2(ui.available_width(), height),
        egui::Layout::right_to_left(egui::Align::Center),
        |ui| {
            ui.set_min_height(height);
            // The device rows' spacing: the default up to the name, 6 after it.
            let spacing = ui.spacing().item_spacing.x;
            ui.spacing_mut().item_spacing.x = 6.0;
            // Room for the link's focus ring at the card's edge, as the device rows' buttons.
            ui.add_space(6.0);
            let clicked = link(ui, k);
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = spacing;
                ui.add_space(LEAD);
                let size = if ui.available_width() >= 360.0 {
                    thumb::ROW
                } else {
                    thumb::SMALL
                };
                let keys = layout
                    .cloned()
                    .unwrap_or_else(|| super::key_layout::resolve(None, &[], 0));
                thumb::row_thumb(
                    ui,
                    size,
                    &k.key,
                    Thumb::keyboard(keys, Tint::Unknown),
                    false,
                );
                ui.add_space(thumb::GAP);
                ui.add(
                    egui::Label::new(RichText::new(&k.name).color(p.text_strong))
                        .truncate()
                        .selectable(false),
                );
                ui.spacing_mut().item_spacing.x = 6.0;
                ui.add(
                    egui::Label::new(RichText::new(detail).color(p.muted))
                        .truncate()
                        .selectable(false),
                );
            });
            clicked
        },
    )
    .inner
}

#[cfg(test)]
#[path = "other_keyboards_tests.rs"]
mod tests;
