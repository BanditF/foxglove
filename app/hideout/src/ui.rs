//! The hub's window frame (spec §2.1, job 2): the sidebar, the page, the status strip and the
//! command palette. For now (H2.1) the only page is Modules, and it is empty: the registry
//! (H2.2) fills it. Nothing here names a module (§2.2); what shows comes from data.

use egui::{Color32, Key, Modifiers, RichText};
use hub_shell::theme::{self, Palette};

use crate::config::ThemeChoice;

/// The hub's own pages. Module pages join the sidebar from manifests (H2.4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    Modules,
}

impl Page {
    const ALL: [Page; 1] = [Page::Modules];

    fn label(self) -> &'static str {
        match self {
            Page::Modules => "Modules",
        }
    }
}

/// One entry of the command palette (§2.6): a module's declared action or page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaletteEntry {
    /// The plain module name and verb, never an id ("AI: Free GPU").
    pub label: String,
}

/// What the frame shows and remembers between frames.
pub struct Hub {
    pub page: Page,
    pub palette_open: bool,
    pub query: String,
    /// From manifests and the registry only (§2.6); empty until H2.2.
    pub entries: Vec<PaletteEntry>,
}

impl Default for Hub {
    fn default() -> Self {
        Self {
            page: Page::Modules,
            palette_open: false,
            query: String::new(),
            entries: Vec::new(),
        }
    }
}

/// Bandit orange, from the bundled palette file the app uses too.
const BANDIT: &str = include_str!("../../resources/palettes/bandit.json");

fn role(name: &str, fallback: Color32) -> Color32 {
    let roles: serde_json::Value = serde_json::from_str(BANDIT).unwrap_or_default();
    roles["roles"][name]
        .as_str()
        .and_then(|hex| hex.strip_prefix('#'))
        .and_then(|hex| u32::from_str_radix(hex, 16).ok())
        .map(|rgb| Color32::from_rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8))
        .unwrap_or(fallback)
}

/// The palette for a theme choice.
pub fn palette(choice: ThemeChoice) -> Palette {
    match choice {
        ThemeChoice::Dark => theme::dark(
            role("accent", Color32::from_rgb(0xf0, 0x8a, 0x3c)),
            role("highlight", Color32::from_rgb(0xff, 0xb0, 0x70)),
        ),
        ThemeChoice::Light => {
            theme::light(role("accent_light", Color32::from_rgb(0xb4, 0x54, 0x0f)))
        }
    }
}

/// Fonts, text styles and colours on `ctx`.
pub fn apply_theme(ctx: &egui::Context, choice: ThemeChoice) {
    let mut fonts = egui::FontDefinitions::default();
    theme::add_fonts(&mut fonts);
    ctx.set_fonts(fonts);
    ctx.all_styles_mut(theme::style_text);
    ctx.set_visuals(theme::visuals(&palette(choice)));
}

impl Hub {
    /// Draws the whole frame into the window's `ui`.
    pub fn draw(&mut self, ui: &mut egui::Ui, version: &str) {
        let ctx = ui.ctx().clone();
        if ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::K)) {
            self.palette_open = !self.palette_open;
            self.query.clear();
        }
        if self.palette_open && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
            self.palette_open = false;
        }
        egui::Panel::bottom("hideout-status").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("Hideout {version}")).font(theme::mono(11.0)));
                ui.separator();
                ui.label(RichText::new("No modules running").font(theme::mono(11.0)));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(RichText::new("Ctrl+K").font(theme::mono(11.0)).weak());
                });
            });
        });
        egui::Panel::left("hideout-sidebar")
            .resizable(false)
            .exact_size(180.0)
            .show(ui, |ui| {
                ui.add_space(8.0);
                ui.label(RichText::new("HIDEOUT").font(theme::hud(17.0)));
                ui.add_space(8.0);
                for page in Page::ALL {
                    if ui
                        .selectable_label(self.page == page, page.label())
                        .clicked()
                    {
                        self.page = page;
                    }
                }
            });
        egui::CentralPanel::default().show(ui, |ui| match self.page {
            Page::Modules => modules_page(ui),
        });
        if self.palette_open {
            self.palette(&ctx);
        }
    }

    fn palette(&mut self, ctx: &egui::Context) {
        egui::Window::new("Command palette")
            .title_bar(false)
            .anchor(egui::Align2::CENTER_TOP, [0.0, 60.0])
            .resizable(false)
            .collapsible(false)
            .show(ctx, |ui| {
                ui.set_width(420.0);
                ui.text_edit_singleline(&mut self.query).request_focus();
                let query = self.query.to_lowercase();
                let shown: Vec<_> = self
                    .entries
                    .iter()
                    .filter(|e| e.label.to_lowercase().contains(&query))
                    .collect();
                if shown.is_empty() {
                    ui.weak("Nothing to run yet: modules add their actions here.");
                }
                for entry in shown {
                    let _ = ui.selectable_label(false, &entry.label);
                }
            });
    }
}

fn modules_page(ui: &mut egui::Ui) {
    ui.heading("Modules");
    ui.add_space(12.0);
    ui.label("No modules installed.");
    ui.weak("Modules you install show here, with their state and version.");
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One frame of the hub in a headless context: no window.
    fn frame(hub: &mut Hub, ctx: &egui::Context, events: Vec<egui::Event>) -> Vec<String> {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(880.0, 600.0),
            )),
            events,
            ..Default::default()
        };
        let mut out = ctx.run_ui(input, |ui| hub.draw(ui, "0.1.0"));
        out.textures_delta.clear();
        let mut texts = Vec::new();
        for clipped in out.shapes {
            collect(&clipped.shape, &mut texts);
        }
        texts
    }

    fn collect(shape: &egui::Shape, out: &mut Vec<String>) {
        match shape {
            egui::Shape::Text(t) => out.push(t.galley.text().to_owned()),
            egui::Shape::Vec(v) => v.iter().for_each(|s| collect(s, out)),
            _ => {}
        }
    }

    fn ctrl_k() -> egui::Event {
        egui::Event::Key {
            key: Key::K,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::COMMAND,
        }
    }

    #[test]
    fn it_starts_empty_with_only_the_modules_page() {
        let ctx = egui::Context::default();
        apply_theme(&ctx, ThemeChoice::Dark);
        let mut hub = Hub::default();
        let _ = frame(&mut hub, &ctx, vec![]);
        let texts = frame(&mut hub, &ctx, vec![]);
        assert_eq!(hub.page, Page::Modules);
        assert_eq!(Page::ALL.len(), 1, "the hub's only own page");
        for want in [
            "HIDEOUT",
            "Modules",
            "No modules installed.",
            "Hideout 0.1.0",
        ] {
            assert!(
                texts.iter().any(|t| t == want),
                "{want} missing in {texts:?}"
            );
        }
    }

    #[test]
    fn ctrl_k_opens_the_palette_and_escape_closes_it() {
        let ctx = egui::Context::default();
        apply_theme(&ctx, ThemeChoice::Light);
        let mut hub = Hub::default();
        let _ = frame(&mut hub, &ctx, vec![ctrl_k()]);
        assert!(hub.palette_open);
        let texts = frame(&mut hub, &ctx, vec![]);
        assert!(texts.iter().any(|t| t.starts_with("Nothing to run yet")));
        let esc = egui::Event::Key {
            key: Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::NONE,
        };
        let _ = frame(&mut hub, &ctx, vec![esc]);
        assert!(!hub.palette_open);
    }

    #[test]
    fn the_accent_is_bandit_orange_from_the_palette_file() {
        assert_eq!(
            palette(ThemeChoice::Dark).accent,
            Color32::from_rgb(0xf0, 0x8a, 0x3c)
        );
        assert_eq!(
            palette(ThemeChoice::Light).accent,
            Color32::from_rgb(0xb4, 0x54, 0x0f)
        );
    }
}
