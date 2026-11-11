// SPDX-License-Identifier: MIT
//! The supervisor states #1086 publishes as facts (#983, #1049; contract §7.6, §7.8), drawn on
//! a module's page and its tab, the words from `hideout_supervisor::words` and the plans
//! docs/hideout-supervisor-ux.md, docs/hideout-revoked-ux.md and docs/hideout-popouts-ux.md:
//!
//! - **Waiting** (`waiting`, root cause once): a header line over the live page, "Waiting for
//!   <root>", the reason line naming the root, **Go to <root>**. Never a crash bar or red mark of
//!   its own; the tab has the ring with "Waiting for <root>".
//! - **Needs a newer Hideout** (`needs-hub`): the card with **Update Hideout** (focused) and Go
//!   back to <previous> when one is kept; red mark on the tab.
//! - **Quarantined** (`quarantined`, §5.5): the card "<Module>'s page couldn't be shown", its
//!   reason, **Start again** (focused) and Go back to <previous>; both release the panels.
//! - **Updating** (`updating`): one header line, "Updating… Your place on this page is kept.";
//!   the ring on the tab.
//! - **Update ready** (`update-busy`): a header line, "Waits until you finish <reason>",
//!   **Force** · Not now. The old version runs: no mark on the tab.
//! - **Update waiting for you** (`update-waiting`): a header line with the new permission in
//!   plain words, **Review** · Not now. Review waits for the permission dialog (not built): it is
//!   drawn greyed with "Comes in a later Hideout.", as the search's link was before it existed.
//!
//! Every surface is drawn from the facts as they are now; nothing here keeps a timer or polls.
//! Clean-room: written from the plans and the contract; no code under `app/` was read.

use super::*;

/// States drawn as a line above the live (or swapping) page rather than a card: the card focus
/// rule doesn't apply to them, and a card that comes back afterwards takes the focus again.
pub(super) fn header_line(state: &str) -> bool {
    matches!(
        state,
        "waiting" | "updating" | "update-busy" | "update-waiting"
    )
}

impl Hub {
    /// A module fact as a string: `module.<module>.<k>`.
    fn module_str(&self, module: &str, k: &str) -> Option<String> {
        self.panels
            .fact(&format!("module.{module}.{k}"))
            .and_then(|v| v.as_str())
            .map(String::from)
    }

    /// The root a waiting module names, by its name (`module.<id>.waiting`).
    fn waiting_root(&self, module: &str) -> Option<(String, String)> {
        let root = self.module_str(module, "waiting")?;
        let name = self.panels.module_name(&root);
        Some((root, name))
    }

    /// The tab's state for the states this file draws, `None` for the others (§7.8 words; the
    /// tabs plan's one mark: stopped mark > ring > nothing).
    pub(super) fn state_look(&self, module: &str) -> Option<TabState> {
        Some(match self.panels.module_state(module)? {
            "waiting" => TabState::Waiting(match self.waiting_root(module) {
                Some((_, root)) => words::waiting_for(&root),
                None => hideout_panels::state_words("waiting").into(),
            }),
            "needs-hub" => TabState::Stopped(words::NEEDS_HUB.into()),
            // The tab says its title first ("Art, its page couldn't be shown"), so the words
            // don't name the module again (the tile's "Its tile couldn't be shown").
            "quarantined" => TabState::Stopped(QUARANTINED_TAB.into()),
            "updating" => TabState::Busy(words::UPDATING.into()),
            // The old version keeps running and nothing needs the user yet: no mark.
            "update-busy" | "update-waiting" => TabState::Live,
            _ => return None,
        })
    }

    /// The page's surface for `state`; `false` when this file draws nothing for it.
    pub(super) fn state_surface_ui(
        &mut self,
        ui: &mut egui::Ui,
        t: &Tokens,
        key: &PanelKey,
        name: &str,
        state: &str,
    ) -> bool {
        match state {
            "waiting" => self.waiting_line_ui(ui, t, key),
            "needs-hub" => self.needs_hub_card_ui(ui, t, key),
            "quarantined" => self.quarantined_card_ui(ui, t, key, name),
            "updating" => updating_line_ui(ui, t, &key.module),
            "update-busy" => self.update_line_ui(ui, t, key, false),
            "update-waiting" => self.update_line_ui(ui, t, key, true),
            _ => return false,
        }
        true
    }

    /// Opens `module`'s page; a module without one opens the Modules page, where its row is.
    fn go_to_module(&mut self, module: &str) {
        let page = self
            .panels
            .in_slot(Slot::Page)
            .find(|p| p.key.module == module)
            .map(|p| p.key.clone());
        match page {
            Some(k) => self.open(TabId::panel(&k.module, &k.panel)),
            None => {
                let modules = TabId::page(MODULES_PAGE);
                if self.target(&modules).is_some() {
                    self.open(modules);
                } else {
                    log::info!("go to {module}: it has no page");
                }
            }
        }
    }

    /// Waiting for another module: the root's name, the reason line, **Go to <root>**. The
    /// first line is a polite live region with the same name while the root is the same, so a
    /// screen reader says it once.
    fn waiting_line_ui(&mut self, ui: &mut egui::Ui, t: &Tokens, key: &PanelKey) {
        let root = self.waiting_root(&key.module);
        let title = root.as_ref().map_or_else(
            || hideout_panels::state_words("waiting").to_string(),
            |(_, n)| words::waiting_for(n),
        );
        // The supervisor's reason ("Link is not running."), else made from the root's name.
        let reason = self
            .module_str(&key.module, "reason")
            .or_else(|| root.as_ref().map(|(_, n)| words::waiting_reason(n)));
        let mut go = false;
        ui.push_id(("hideout.waiting-line", &key.module), |ui| {
            header_frame(ui, t, |ui| {
                let head = ui.label(egui::RichText::new(&title).strong().color(c(t.text)));
                let said = title.clone();
                ui.ctx().accesskit_node_builder(head.id, |n| {
                    n.set_live(egui::accesskit::Live::Polite);
                    n.set_label(said);
                });
                ui.horizontal(|ui| {
                    if let Some(r) = &reason {
                        ui.label(egui::RichText::new(r).color(c(t.text_muted)));
                    }
                    if let Some((_, n)) = &root {
                        go = ui.button(words::go_to(n)).clicked();
                    }
                });
            });
        });
        if go {
            if let Some((r, _)) = root {
                self.go_to_module(&r);
            }
        }
        ui.add_space(12.0);
    }

    /// Needs a newer Hideout: the card, **Update Hideout** (focused when it appears; Esc does
    /// nothing) and Go back to <previous> only with a previous version kept.
    fn needs_hub_card_ui(&mut self, ui: &mut egui::Ui, t: &Tokens, key: &PanelKey) {
        let previous = self.module_str(&key.module, "previous");
        let focus = self.card_takes_focus(ui, key, "needs-hub");
        let (mut update, mut back) = (false, false);
        ui.push_id(("hideout.needs-hub-card", &key.module), |ui| {
            region_card(ui, t, words::NEEDS_HUB, |ui| {
                ui.label(
                    egui::RichText::new(format!("\u{26A0} {}", words::NEEDS_HUB))
                        .size(16.0)
                        .strong()
                        .color(c(t.danger)),
                );
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    update = main_button(ui, t, words::UPDATE_HIDEOUT, focus).clicked();
                    if let Some(p) = &previous {
                        back = ui.button(words::go_back(p)).clicked();
                    }
                });
            });
        });
        if let Some(h) = self.hooks.as_mut() {
            if update {
                h.update_hub_for(&key.module);
            }
            if back {
                h.go_back(&key.module);
            }
        }
        ui.add_space(12.0);
    }

    /// Quarantined: "<Module>'s page couldn't be shown", the supervisor's reason, **Start again**
    /// (focused; Esc does nothing) and Go back to <previous>. Either releases the panels.
    fn quarantined_card_ui(&mut self, ui: &mut egui::Ui, t: &Tokens, key: &PanelKey, name: &str) {
        let module = &key.module;
        let title = words::quarantined_title(name);
        let reason = self
            .module_str(module, "reason")
            .unwrap_or_else(|| words::QUARANTINED_LINE.into());
        let previous = self.module_str(module, "previous");
        let focus = self.card_takes_focus(ui, key, "quarantined");
        let (mut again, mut back) = (false, false);
        ui.push_id(("hideout.quarantined-card", module), |ui| {
            region_card(ui, t, &title, |ui| {
                ui.label(
                    egui::RichText::new(format!("\u{26A0} {title}"))
                        .size(16.0)
                        .strong()
                        .color(c(t.danger)),
                );
                ui.add_space(4.0);
                ui.label(egui::RichText::new(&reason).color(c(t.text)));
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    again = main_button(ui, t, words::START_AGAIN, focus).clicked();
                    if let Some(p) = &previous {
                        back = ui.button(words::go_back(p)).clicked();
                    }
                });
            });
        });
        if again || back {
            self.panels.release(module);
        }
        if let Some(h) = self.hooks.as_mut() {
            if again {
                h.start_again(module);
            }
            if back {
                h.go_back(module);
            }
        }
        ui.add_space(12.0);
    }

    /// Update ready (`busy`: the module's reason, **Force** · Not now) or Update waiting for you
    /// (`asks`: the new permission, **Review** · Not now). Over the live page; no focus is taken
    /// (the old version still runs and the user may be using it).
    fn update_line_ui(&mut self, ui: &mut egui::Ui, t: &Tokens, key: &PanelKey, asks: bool) {
        let module = &key.module;
        let title = if asks {
            words::UPDATE_WAITING
        } else {
            words::UPDATE_READY
        };
        let reason = self.module_str(module, "reason");
        let (mut force, mut later) = (false, false);
        ui.push_id(("hideout.update-line", module), |ui| {
            header_frame(ui, t, |ui| {
                let head = ui.label(egui::RichText::new(title).strong().color(c(t.text)));
                ui.ctx().accesskit_node_builder(head.id, |n| {
                    n.set_live(egui::accesskit::Live::Polite);
                    n.set_label(title);
                });
                if let Some(r) = &reason {
                    ui.label(egui::RichText::new(r).color(c(t.text_muted)));
                }
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if asks {
                        // The permission dialog (§6.4) isn't built: nothing can grant yet.
                        let b = ui
                            .add_enabled(false, egui::Button::new(words::REVIEW))
                            .on_disabled_hover_text(words::COMES_LATER);
                        ui.ctx().accesskit_node_builder(b.id, |n| {
                            n.set_description(words::COMES_LATER)
                        });
                    } else {
                        force = main_button(ui, t, words::FORCE, false).clicked();
                    }
                    later = ui.button(words::NOT_NOW).clicked();
                });
            });
        });
        if let Some(h) = self.hooks.as_mut() {
            if force {
                h.force_update(module);
            }
            if later {
                h.not_now(module);
            }
        }
        ui.add_space(12.0);
    }
}

/// A quarantined module's tab words, after its title.
pub(super) const QUARANTINED_TAB: &str = "Its page couldn't be shown";

/// The Modules page's id (the hub's own page).
const MODULES_PAGE: &str = "hub.modules";

/// Updating (hot swap): one muted line, "Updating… Your place on this page is kept." (the
/// pop-outs plan's wording), a polite live region said once.
fn updating_line_ui(ui: &mut egui::Ui, t: &Tokens, module: &str) {
    let line = updating_line();
    ui.push_id(("hideout.updating-line", module), |ui| {
        let r = ui.label(egui::RichText::new(&line).color(c(t.text_muted)));
        let said = line.clone();
        ui.ctx().accesskit_node_builder(r.id, |n| {
            n.set_live(egui::accesskit::Live::Polite);
            n.set_label(said);
        });
    });
    ui.add_space(12.0);
}

/// "Updating… Your place on this page is kept."
pub(super) fn updating_line() -> String {
    format!("{} {}", words::UPDATING, words::PLACE_KEPT)
}

/// A header line's frame: the surface fill with the border, slimmer than a card.
fn header_frame(ui: &mut egui::Ui, t: &Tokens, add: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new()
        .fill(c(t.surface))
        .stroke(egui::Stroke::new(1.0_f32, c(t.border)))
        .corner_radius(egui::CornerRadius::same(6))
        .inner_margin(egui::Margin::symmetric(14, 10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui);
        });
}
