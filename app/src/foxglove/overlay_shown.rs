//! "Show layer overlay": the layer overlay on or off.
//!
//! - **One setting:** the store's `[overlay_shown] on` (on by default). The checkbox at the top of
//!   the Keyboard page's Layer overlay card and the local API's `overlay.shown` /
//!   `overlay.set_shown` (API 1.13) all read and write it through [`on`] and [`set`]; nothing else
//!   keeps a copy. There's no tray toggle or hotkey for it, and no "Never" choice it replaces.
//! - **Off hides it at once** ([`visible`]), Settings' preview too, and keeps every other overlay
//!   setting as it is (they live in `settings.ini` and the profiles, untouched). While it's off the
//!   card shows them muted ([`mute_if_off`]), not hidden. The keymap editor, which picks keys on
//!   the overlay, still shows it while open.
//! - **Machine-wide:** it isn't a profiled section, so a profile switch leaves it as it is, and a
//!   save is read-modify-write under the store's lock ([`super::store::update`]).

use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};

/// The store's section.
pub const SECTION: &str = "overlay_shown";

/// The checkbox, and the name the API docs use.
pub const LABEL: &str = "Show layer overlay";

/// The checkbox's hover and ⓘ: what it does, without repeating the label beside it (#839).
pub const TIP: &str = "The active layer's keys pop up on screen while you hold or turn on a \
                       layer. Off hides it and keeps the settings below.";

/// `[overlay_shown]`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
struct Saved {
    on: bool,
}

impl Default for Saved {
    fn default() -> Self {
        Saved { on: true }
    }
}

#[cfg(test)]
thread_local! {
    /// Tests share one settings folder: a test that saves keeps its own section.
    pub static TEST_SECTION: std::cell::Cell<Option<&'static str>> = const { std::cell::Cell::new(None) };
    /// The previews draw the card with the setting as given, without the store.
    pub static PREVIEW: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
}

fn section() -> super::store::Section {
    #[cfg(test)]
    if let Some(s) = TEST_SECTION.get() {
        return super::store::test_section(s);
    }
    super::store::section("foxglove.overlay", SECTION)
}

/// The saved value, read once per section: the overlay asks every frame.
static READ: Mutex<Option<(&'static str, bool)>> = Mutex::new(None);
/// Wakes the overlay's frame after a change from another thread (the API).
static WAKE: OnceLock<crate::ui_wake::UiWake> = OnceLock::new();

/// Whether the layer overlay shows at all.
pub fn on() -> bool {
    #[cfg(test)]
    if let Some(on) = PREVIEW.get() {
        return on;
    }
    let handle = section();
    let section = handle.name();
    let mut read = READ.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((s, on)) = *read {
        if s == section {
            return on;
        }
    }
    // A store that can't be read right now isn't kept: the next call reads again.
    match handle.load::<Saved>() {
        Ok(saved) => {
            let on = saved.unwrap_or_default().on;
            *read = Some((section, on));
            on
        }
        Err(_) => Saved::default().on,
    }
}

/// Turns it on or off: saved (read-modify-write), logged when it changed, and the overlay woken.
/// What it is now, as saved.
pub fn set(on: bool) -> std::io::Result<bool> {
    let before = self::on();
    let handle = section();
    let section = handle.name();
    let saved = handle.update(|s: &mut Saved| s.on = on)?;
    *READ.lock().unwrap_or_else(|e| e.into_inner()) = Some((section, saved.on));
    if before != saved.on {
        eprintln!(
            "Foxglove: layer overlay {}",
            if saved.on { "on" } else { "off" }
        );
    }
    if let Some(wake) = WAKE.get() {
        wake.request_repaint();
    }
    Ok(saved.on)
}

/// The overlay's frame registers itself to be woken by [`set`].
pub fn watch(ctx: &egui::Context) {
    let _ = WAKE.set(crate::ui_wake::UiWake::from_ctx(ctx));
}

/// What decides whether the overlay draws this frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Shows {
    /// "Show layer overlay".
    pub on: bool,
    /// The keymap editor is open (it picks keys on the overlay).
    pub editor: bool,
    /// Settings is open (it previews the overlay).
    pub settings: bool,
    /// An opacity slider is being dragged (it previews the overlay).
    pub previewing: bool,
    /// Foxglove is paused.
    pub paused: bool,
    /// A layer has it up now.
    pub layer: bool,
}

/// Whether the overlay draws: off hides it, previews included; the editor still has it.
pub fn visible(s: Shows) -> bool {
    s.editor || (s.on && (s.settings || s.previewing || (!s.paused && s.layer)))
}

/// The card's first row, in its two-column grid: the checkbox and its ⓘ.
pub fn row(ui: &mut egui::Ui) {
    let was = on();
    let mut now = was;
    let changed = ui
        .horizontal(|ui| {
            let c = ui.checkbox(&mut now, LABEL);
            super::theme::with_info(ui, c, TIP).changed()
        })
        .inner;
    ui.end_row();
    if changed && now != was {
        if let Err(e) = set(now) {
            eprintln!("Foxglove: can't save the layer overlay's on/off: {e}");
        }
    }
}

/// Mutes the rest of `ui` while the overlay is off: slider fills and check marks in the muted
/// token instead of the accent, everything at the theme's disabled alpha. The rows stay
/// operable, so the overlay can be set up before it's turned back on.
pub fn mute_if_off(ui: &mut egui::Ui) {
    if on() {
        return;
    }
    let muted = super::theme::palette(ui).muted;
    let v = ui.visuals_mut();
    v.selection.bg_fill = muted;
    let w = &mut v.widgets;
    for state in [&mut w.inactive, &mut w.hovered, &mut w.active, &mut w.open] {
        state.fg_stroke.color = muted;
    }
    let alpha = ui.visuals().disabled_alpha();
    ui.multiply_opacity(alpha);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn own_section(name: &'static str) {
        crate::foxglove::store::test_config_dir();
        TEST_SECTION.set(Some(name));
    }

    #[test]
    fn on_by_default() {
        own_section("overlay_shown_test_default");
        assert!(on());
        assert_eq!(Saved::default(), Saved { on: true });
        // A section without the key (hand-edited) is on too.
        assert_eq!(toml::from_str::<Saved>("").unwrap(), Saved { on: true });
    }

    #[test]
    fn off_hides_it_previews_included_but_not_the_editor() {
        let up = Shows {
            on: true,
            layer: true,
            ..Default::default()
        };
        assert!(visible(up));
        let off = Shows { on: false, ..up };
        assert!(!visible(off), "off hides a layer's overlay");
        assert!(
            !visible(Shows {
                settings: true,
                previewing: true,
                ..off
            }),
            "and Settings' preview"
        );
        assert!(
            visible(Shows {
                editor: true,
                ..off
            }),
            "the keymap editor still picks keys on it"
        );
        // On, it's as before: paused hides a layer's, Settings previews it.
        assert!(!visible(Shows { paused: true, ..up }));
        assert!(visible(Shows {
            paused: true,
            settings: true,
            ..up
        }));
        assert!(!visible(Shows {
            on: true,
            ..Default::default()
        }));
    }

    #[test]
    fn set_persists_and_every_reader_sees_it() {
        own_section("overlay_shown_test_persist");
        assert!(!set(false).unwrap());
        // The one setting: what the checkbox and the API read.
        assert!(!on());
        // Saved in the store's own section, read from disk again.
        *READ.lock().unwrap() = None;
        let saved: Saved = crate::foxglove::store::try_load("overlay_shown_test_persist").unwrap();
        assert_eq!(saved, Saved { on: false });
        assert!(!on());
        assert!(set(true).unwrap());
        *READ.lock().unwrap() = None;
        assert!(on());
    }

    #[test]
    fn off_keeps_the_other_overlay_settings() {
        own_section("overlay_shown_test_keeps");
        let ini = crate::foxglove::store::test_config_dir().join("settings.ini");
        let custom = crate::settings::Settings {
            size: 77,
            timeout: 1234,
            opacity: 45,
            ..Default::default()
        };
        // Its own file, so no other test's settings.ini is touched.
        let path = ini.with_file_name("overlay_shown_test_keeps.ini");
        custom.save_to_file(&path).unwrap();
        set(false).unwrap();
        set(true).unwrap();
        assert!(crate::settings::Settings::load_from_file(&path) == Some(custom));
        let _ = std::fs::remove_file(path);
    }

    /// The card's rows as drawn with the setting `shown`: whether a checkbox below the row can
    /// still be used, and every fill colour painted.
    fn draw_rows(shown: bool) -> (bool, Vec<egui::Color32>) {
        fn fills(shape: &egui::Shape, out: &mut Vec<egui::Color32>) {
            match shape {
                egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| fills(s, out)),
                egui::Shape::Rect(r) => out.push(r.fill),
                egui::Shape::Circle(c) => out.push(c.fill),
                _ => {}
            }
        }
        PREVIEW.set(Some(shown));
        let ctx = egui::Context::default();
        let mut fonts = egui::FontDefinitions::default();
        crate::foxglove::theme::add_fonts(&mut fonts);
        ctx.set_fonts(fonts);
        crate::foxglove::theme::apply_choice(&ctx, crate::foxglove::theme::Choice::Dark);
        let mut enabled = false;
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
            egui::Grid::new("t").num_columns(2).show(ui, |ui| {
                row(ui);
                mute_if_off(ui);
                ui.label("Opacity");
                ui.add(egui::Slider::new(&mut 60u8, 0..=100));
                ui.end_row();
                enabled = ui.checkbox(&mut true, "Fit long labels").enabled();
                ui.end_row();
            });
        });
        out.textures_delta.clear(); // nothing is painted here
        PREVIEW.set(None);
        let mut colours = Vec::new();
        for clipped in &out.shapes {
            fills(&clipped.shape, &mut colours);
        }
        (enabled, colours)
    }

    #[test]
    fn off_mutes_the_rows_but_keeps_them_operable() {
        let accent = crate::foxglove::theme::DARK.accent;
        let (on_enabled, on_fills) = draw_rows(true);
        assert!(on_enabled);
        assert!(
            on_fills.contains(&accent),
            "on: the slider's fill is the accent"
        );
        let (off_enabled, off_fills) = draw_rows(false);
        assert!(off_enabled, "off: the rows stay operable");
        let half = accent.gamma_multiply(0.5);
        assert!(
            !off_fills.iter().any(|c| *c == accent || *c == half),
            "off: no slider fill in the accent colour"
        );
    }

    #[test]
    fn the_tip_does_not_repeat_the_label() {
        assert!(!crate::foxglove::theme::repeats_label(LABEL, TIP));
    }
}
