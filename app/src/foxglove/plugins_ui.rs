//! The Plugins page (#69, redesigned in #389): one card per plugin in a grid (name, one line
//! about it, CATALOG / INSTALLED / SIDELOADED, and its buttons), "Install from file…" in the
//! page header, and the catalog's state in plain words. Installing from a file still asks for
//! the user's approval of the plugin's permissions first.
//!
//! [`group`] gathers a [`View`] and acts on its clicks; [`draw`] paints it, so the previews
//! draw the page with no plugins folder. The installed list is read when the page opens and
//! after every change, not every frame.

use std::cell::RefCell;

use egui::RichText;
use foxglove_plugin_api::permission::Permission;

use super::plugin_icons as icons;
use super::plugin_install::{self, CatalogEntry, Installed, Staged};
use super::theme;

#[derive(Default)]
struct UiState {
    /// Read from disk when the section opens and after every change, not every frame.
    installed: Option<Vec<Installed>>,
    dialog: egui_file_dialog::FileDialog,
    /// A Plugin waiting for the user's approval.
    staged: Option<Staged>,
    confirm_remove: Option<String>,
    /// The installed plugin whose settings are open on its card.
    open: Option<String>,
    message: Option<String>,
    /// Why no plugin is trusted, if so: checked when the catalog may have changed, not every
    /// frame (it reads and verifies the signed files).
    trust: Option<(super::catalog_refresh::Status, Result<(), String>)>,
    /// What the user is typing into a secret's field, by `plugin/name`; never shown back.
    secret_input: std::collections::HashMap<String, String>,
    /// Whether each declared secret is set, read once per plugin list, not every frame.
    secret_set: std::collections::HashMap<String, bool>,
    /// A text setting being edited, by `plugin/name`, until it's committed.
    setting_text: std::collections::HashMap<String, String>,
}

thread_local! { static UI: RefCell<UiState> = RefCell::new(UiState::default()); }

/// A card's status tag.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tag {
    /// In the catalog, not installed.
    Catalog,
    /// Installed, and in the catalog.
    Installed,
    /// Installed from a file, not in the catalog.
    Sideloaded,
}

impl Tag {
    pub fn label(self) -> &'static str {
        match self {
            Tag::Catalog => "CATALOG",
            Tag::Installed => "INSTALLED",
            Tag::Sideloaded => "SIDELOADED",
        }
    }
}

/// One plugin's card.
#[derive(Clone, Debug)]
pub struct Card {
    pub id: String,
    pub name: String,
    pub description: String,
    pub tag: Tag,
    /// It runs outside the sandbox (unrestricted network or the user's files).
    pub unsandboxed: bool,
    /// The installed plugin, for its switches and settings.
    pub installed: Option<Installed>,
    /// While it's on: running, restarting, stopped.
    pub state: Option<String>,
}

/// Whether a plugin with these permissions runs outside the sandbox, as the runner decides it.
pub fn unsandboxed<'a>(permissions: impl IntoIterator<Item = &'a Permission>) -> bool {
    permissions
        .into_iter()
        .any(|p| matches!(p, Permission::Net | Permission::Files))
}

/// The cards: installed plugins first (by name), then the catalog's others in its order. Each
/// plugin once; one that isn't in the catalog is sideloaded.
pub fn cards(installed: &[Installed], catalog: &[CatalogEntry]) -> Vec<Card> {
    let mut out: Vec<Card> = installed
        .iter()
        .map(|p| {
            let listed = catalog.iter().find(|e| e.id == p.manifest.id);
            let description = if p.manifest.description.is_empty() {
                listed.map(|e| e.description.clone()).unwrap_or_default()
            } else {
                p.manifest.description.clone()
            };
            Card {
                id: p.manifest.id.clone(),
                name: format!("{} {}", p.manifest.name, p.manifest.version),
                description,
                tag: if listed.is_some() {
                    Tag::Installed
                } else {
                    Tag::Sideloaded
                },
                unsandboxed: unsandboxed(&p.record.approved),
                installed: Some(p.clone()),
                state: None,
            }
        })
        .collect();
    for e in catalog {
        if out.iter().any(|c| c.id == e.id) {
            continue;
        }
        out.push(Card {
            id: e.id.clone(),
            name: e.name.clone(),
            description: e.description.clone(),
            tag: Tag::Catalog,
            unsandboxed: unsandboxed(&e.permissions),
            installed: None,
            state: None,
        });
    }
    out
}

/// What the page shows.
#[derive(Clone, Debug, Default)]
pub struct View {
    pub cards: Vec<Card>,
    /// The catalog's state in words ("Catalog: up to date."), and whether Check now can run.
    pub catalog: String,
    pub can_check: bool,
    /// One quiet line when installing is off (no signed catalog, or unsigned plugins allowed).
    pub note: Option<(String, bool)>,
    /// Who holds the lights (#376): "Lights: Mic mute (high), under OpenRGB (low)".
    pub lights: String,
}

fn priority_word(p: super::lighting_leases::Priority) -> &'static str {
    use super::lighting_leases::Priority;
    match p {
        Priority::Low => "low",
        Priority::Normal => "normal",
        Priority::High => "high",
    }
}

/// Who holds the lights, in words (#376): the lease showing first, then the ones under it;
/// "yours" when none is held. `name` says whose a connection is. More than one leased device
/// names each device.
pub fn lights_line(h: &super::lighting_leases::Holders, name: impl Fn(u64) -> String) -> String {
    let stack = |leases: &[(u64, super::lighting_leases::Priority)]| {
        let mut words = leases
            .iter()
            .map(|(conn, p)| format!("{} ({})", name(*conn), priority_word(*p)));
        let top = words.next().unwrap_or_default();
        let under: Vec<String> = words.collect();
        if under.is_empty() {
            top
        } else {
            format!("{top}, under {}", under.join(", "))
        }
    };
    let held = match h.devices.as_slice() {
        [] => return "Lights: yours".into(),
        [one] => stack(&one.leases),
        many => many
            .iter()
            .map(|d| format!("{}: {}", d.device, stack(&d.leases)))
            .collect::<Vec<_>>()
            .join("; "),
    };
    if h.paused {
        format!("Lights: yours while Foxglove is paused, then {held}")
    } else {
        format!("Lights: {held}")
    }
}

/// A connection's plugin by its name in its manifest, or "a tool" for the local API's clients.
fn holder_name(conn: u64, installed: &[Installed]) -> String {
    if conn == super::scene_run::SCENE_CONN {
        return "a scene".into();
    }
    match foxglove_plugin_host::plugin_of(conn) {
        Some(id) => installed
            .iter()
            .find(|p| p.manifest.id == id)
            .map_or(id, |p| p.manifest.name.clone()),
        None => "a tool".into(),
    }
}

/// What a click asks for.
#[derive(Clone, Debug, PartialEq)]
pub enum Intent {
    CheckCatalog,
    Enable(String, bool),
    Remove(String),
}

#[cfg(test)]
thread_local! {
    /// The previews' page, so they never read a plugins folder.
    pub static PREVIEW: RefCell<Option<View>> = const { RefCell::new(None) };
}

/// "Install from file…", in the page's header (#389).
pub fn header(ui: &mut egui::Ui) {
    // Not while a plugin waits for approval: one install at a time.
    let waiting = UI.with_borrow(|state| state.staged.is_some());
    let button = ui.add_enabled(!waiting, egui::Button::new("Install from file…"));
    if button
        .on_hover_text("Install a plugin package (.zip) you have")
        .clicked()
    {
        UI.with_borrow_mut(|state| state.dialog.pick_file());
    }
}

/// The catalog's freshness in plain words. Opening the page asks for a check when one is due
/// (at most every 10 minutes, and never while backing off).
fn catalog_words(ui: &egui::Ui) -> (String, bool) {
    use super::catalog_refresh::{self, Status, Trigger};
    catalog_refresh::set_wake(crate::ui_wake::UiWake::from_ctx(ui.ctx()));
    catalog_refresh::request(Trigger::TabOpened);
    let status = catalog_refresh::status();
    let words = match &status {
        Status::Idle => "Catalog: not checked yet".to_string(),
        Status::Checking => "Catalog: checking…".to_string(),
        Status::UpToDate { .. } => "Catalog: up to date".to_string(),
        Status::Failed(e) => format!("Catalog: couldn't check ({e})"),
    };
    (words, status != Status::Checking)
}

fn plugin_state(id: &str) -> String {
    use foxglove_plugin_host::runner::PluginState;
    match super::plugin_runner::state(id) {
        Some(PluginState::Running) => "Running".to_string(),
        Some(PluginState::Restarting { reason, wait }) => {
            format!("Restarting in {} s: {reason}", wait.as_secs())
        }
        Some(PluginState::Disabled(reason)) => {
            format!("Stopped after repeated failures ({reason}). Turn it off and on to retry.")
        }
        Some(PluginState::Stopped) | None => "Starting".to_string(),
    }
}

/// The Plugins section: the page, and what its clicks do.
pub fn group(ui: &mut egui::Ui) {
    super::settings_ui::section(ui, "Plugins", |ui| {
        #[cfg(test)]
        if let Some(v) = PREVIEW.with_borrow(|v| v.clone()) {
            UI.with_borrow_mut(|state| draw(ui, &v, state));
            return;
        }
        UI.with_borrow_mut(|state| {
            state.dialog.update(ui.ctx());
            if let Some(path) = state.dialog.take_picked() {
                match plugin_install::stage(&path) {
                    Ok(staged) => state.staged = Some(staged),
                    Err(e) => {
                        state.message = Some(format!("Can't install {}: {e}", path.display()))
                    }
                }
            }
            if let Some(staged) = state.staged.take() {
                state.staged = super::settings_ui::card(ui, "Install from file", |ui| {
                    approval(ui, staged, state)
                });
                return;
            }
            let installed = state
                .installed
                .get_or_insert_with(plugin_install::installed)
                .clone();
            let note = if super::plugin_trust::dev_unsigned() {
                Some((
                    "Developer mode: unsigned plugins are allowed (FOXGLOVE_DEV_UNSIGNED_PLUGINS)."
                        .to_string(),
                    true,
                ))
            } else {
                let status = super::catalog_refresh::status();
                if state.trust.as_ref().is_none_or(|(s, _)| *s != status) {
                    state.trust = Some((status, super::plugin_trust::catalog().map(|_| ())));
                }
                match &state.trust {
                    Some((_, Err(e))) => Some((
                        format!("Only signed plugins install and run, and {e}."),
                        false,
                    )),
                    _ => None,
                }
            };
            let (catalog, can_check) = catalog_words(ui);
            let mut cards = cards(&installed, &plugin_install::catalog());
            for c in cards
                .iter_mut()
                .filter(|c| c.installed.as_ref().is_some_and(|p| p.record.enabled))
            {
                c.state = Some(plugin_state(&c.id));
            }
            super::lighting_leases::set_wake(crate::ui_wake::UiWake::from_ctx(ui.ctx()));
            let lights = lights_line(&super::lighting_leases::holding(), |conn| {
                holder_name(conn, &installed)
            });
            let view = View {
                cards,
                catalog,
                can_check,
                note,
                lights,
            };
            for intent in draw(ui, &view, state) {
                match intent {
                    Intent::CheckCatalog => {
                        super::catalog_refresh::request(super::catalog_refresh::Trigger::Manual)
                    }
                    Intent::Enable(id, on) => {
                        state.message = plugin_install::set_enabled(&id, on).err();
                        state.installed = None;
                        super::plugin_runner::sync();
                    }
                    Intent::Remove(id) => {
                        super::plugin_runner::stop(&id);
                        state.message = plugin_install::remove(&id).err();
                        super::plugin_runner::sync();
                        state.installed = None;
                        if state.open.as_deref() == Some(id.as_str()) {
                            state.open = None;
                        }
                    }
                }
            }
        });
    });
}

/// Paints the page; returns what was clicked this frame.
fn draw(ui: &mut egui::Ui, v: &View, state: &mut UiState) -> Vec<Intent> {
    let mut out = Vec::new();
    let p = theme::palette(ui);
    ui.label(RichText::new(&v.lights).strong()).on_hover_text(
        "Which plugin is driving the keyboard's lights now, and which wait under it",
    );
    ui.label(
        RichText::new("Each plugin runs as its own program with only the permissions you approve.")
            .color(p.muted),
    );
    ui.horizontal_wrapped(|ui| {
        ui.label(&v.catalog);
        if ui
            .add_enabled(v.can_check, egui::Button::new("Check now").small())
            .clicked()
        {
            out.push(Intent::CheckCatalog);
        }
    });
    if let Some((note, warn)) = &v.note {
        let color = if *warn { p.warn } else { p.muted };
        ui.label(RichText::new(note).color(color));
    }
    ui.add_space(8.0);
    if v.cards.is_empty() {
        ui.vertical_centered(|ui| {
            ui.add(icons::empty(ui, 120.0));
            ui.weak("No plugins yet. Install one from a file.");
        });
    }
    let columns = if ui.available_width() >= 760.0 { 3 } else { 2 };
    let gap = 14.0;
    let width = ((ui.available_width() - gap * (columns - 1) as f32) / columns as f32).floor();
    for (i, row) in v.cards.chunks(columns).enumerate() {
        // The cards in a row share the tallest one's height, as drawn; it's measured as they
        // draw and settles on the next frame, which a change asks for once.
        let row_id = ui.id().with(("plugin-row", i, columns));
        let height: f32 = ui.ctx().data(|d| d.get_temp(row_id)).unwrap_or(0.0);
        let mut tallest = 0.0f32;
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            for card in row {
                ui.allocate_ui_with_layout(
                    egui::vec2(width, 0.0),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.set_width(width);
                        tallest = tallest.max(card_ui(ui, card, height, state, &mut out));
                    },
                );
            }
        });
        if (tallest - height).abs() > 0.5 {
            ui.ctx().data_mut(|d| d.insert_temp(row_id, tallest));
            ui.ctx().request_repaint();
        }
        ui.add_space(gap);
    }
    if let Some(m) = &state.message {
        ui.colored_label(p.danger, m);
    }
    out
}

/// One card: the name and tag, a line about it, and its buttons; an installed plugin's
/// settings open on the card. At least `height` tall; returns the height its contents need.
fn card_ui(
    ui: &mut egui::Ui,
    c: &Card,
    height: f32,
    state: &mut UiState,
    out: &mut Vec<Intent>,
) -> f32 {
    theme::card(ui)
        .inner_margin(egui::Margin::symmetric(18, 16))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            let top = ui.cursor().top();
            card_contents(ui, c, state, out);
            let natural = ui.cursor().top() - top;
            ui.add_space((height - natural).max(0.0));
            natural
        })
        .inner
}

fn card_contents(ui: &mut egui::Ui, c: &Card, state: &mut UiState, out: &mut Vec<Intent>) {
    let p = theme::palette(ui);
    let dir = c
        .installed
        .as_ref()
        .and_then(|_| plugin_install::plugins_dir())
        .map(|d| d.join(&c.id));
    ui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let color = match c.tag {
                Tag::Catalog => p.muted,
                Tag::Installed => p.ok,
                Tag::Sideloaded => p.warn,
            };
            let tag = ui.label(
                RichText::new(c.tag.label())
                    .font(theme::mono(11.0))
                    .color(color),
            );
            if c.tag == Tag::Sideloaded {
                tag.on_hover_text("Not from the Foxglove catalog");
            }
            // The name takes what's left, cut short rather than running into the tag.
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                ui.add(icons::glyph(ui, &c.id, dir.as_deref(), 20.0));
                ui.add(egui::Label::new(theme::heading(ui, &c.name)).truncate());
            });
        });
    });
    ui.label(RichText::new(&c.description).color(p.muted));
    if c.unsandboxed {
        ui.label(
            RichText::new("Runs outside the sandbox: it can use your files or the network.")
                .color(p.warn),
        );
    }
    let Some(plugin) = &c.installed else {
        // The catalog lists plugins before their downloads exist; nothing to fetch yet.
        ui.add_enabled(false, theme::primary_button(ui, "Install"))
            .on_disabled_hover_text(
                "Not downloadable yet. Install it from a file when you have one.",
            );
        return;
    };
    if let Some(s) = &c.state {
        ui.label(RichText::new(s).color(p.muted));
    }
    let id = &plugin.manifest.id;
    ui.horizontal_wrapped(|ui| {
        let mut on = plugin.record.enabled;
        if ui.checkbox(&mut on, "On").changed() {
            out.push(Intent::Enable(id.clone(), on));
        }
        let open = state.open.as_deref() == Some(id.as_str());
        if theme::pill(ui, open, "Options").clicked() {
            state.open = if open { None } else { Some(id.clone()) };
        }
        let confirming = state.confirm_remove.as_deref() == Some(id.as_str());
        if ui
            .button(if confirming {
                "Remove? Click again"
            } else {
                "Remove"
            })
            .clicked()
        {
            if confirming {
                state.confirm_remove = None;
                out.push(Intent::Remove(id.clone()));
            } else {
                state.confirm_remove = Some(id.clone());
            }
        }
    });
    if state.open.as_deref() == Some(id.as_str()) {
        ui.add_space(4.0);
        ui.separator();
        let perms: Vec<String> = plugin
            .record
            .approved
            .iter()
            .map(|p| p.describe())
            .collect();
        ui.weak(format!(
            "May {}.",
            if perms.is_empty() {
                "do nothing beyond running".into()
            } else {
                perms.join(", ")
            }
        ));
        action_keys(ui, plugin, state);
        secret_fields(ui, plugin, state);
        setting_fields(ui, plugin, state);
    }
}

/// A control for each setting the plugin declares (#69); a change is saved and sent to the
/// plugin at once (text when the field loses focus).
fn setting_fields(ui: &mut egui::Ui, plugin: &Installed, state: &mut UiState) {
    use foxglove_plugin_api::manifest::SettingKind;
    use serde_json::Value;
    if plugin.manifest.settings.is_empty() {
        return;
    }
    let id = &plugin.manifest.id;
    let values = super::plugin_settings::values_for(
        &plugin.manifest,
        super::plugin_settings::stored().get(id.as_str()),
    );
    for decl in &plugin.manifest.settings {
        let current = values
            .get(&decl.name)
            .cloned()
            .unwrap_or_else(|| decl.default.clone());
        ui.horizontal(|ui| {
            ui.add_space(16.0);
            let changed = match decl.kind {
                SettingKind::Bool => {
                    let mut on = current.as_bool().unwrap_or(false);
                    ui.checkbox(&mut on, &decl.label)
                        .changed()
                        .then(|| Value::from(on))
                }
                SettingKind::Int => {
                    ui.label(&decl.label);
                    let mut n = current.as_i64().unwrap_or(0);
                    let range = decl.min.unwrap_or(i64::MIN)..=decl.max.unwrap_or(i64::MAX);
                    ui.add(egui::DragValue::new(&mut n).range(range))
                        .changed()
                        .then(|| Value::from(n))
                }
                SettingKind::Text => {
                    ui.label(&decl.label);
                    let key = format!("{id}/{}", decl.name);
                    let text = state
                        .setting_text
                        .entry(key.clone())
                        .or_insert_with(|| current.as_str().unwrap_or_default().to_string());
                    let done = ui.text_edit_singleline(text).lost_focus();
                    let text = text.clone();
                    (done && Some(text.as_str()) != current.as_str()).then(|| Value::from(text))
                }
            };
            if let Some(v) = changed {
                state.message = super::plugin_settings::set(id, decl, v).err();
            }
        });
    }
}

/// A field for each secret the plugin declares (#213): the value goes straight into Windows'
/// credential store and is never shown again.
fn secret_fields(ui: &mut egui::Ui, plugin: &Installed, state: &mut UiState) {
    use foxglove_plugin_host::secrets;
    let id = &plugin.manifest.id;
    for decl in &plugin.manifest.secrets {
        let key = format!("{id}/{}", decl.name);
        let set = *state
            .secret_set
            .entry(key.clone())
            .or_insert_with(|| secrets::get(id, &decl.name).ok().flatten().is_some());
        ui.horizontal(|ui| {
            ui.add_space(16.0);
            ui.label(&decl.label);
            let input = state.secret_input.entry(key.clone()).or_default();
            ui.add(
                egui::TextEdit::singleline(input)
                    .password(true)
                    .desired_width(160.0)
                    .hint_text(if set { "saved" } else { "not set" }),
            );
            if ui
                .add_enabled(!input.is_empty(), egui::Button::new("Save"))
                .clicked()
            {
                state.message = secrets::set(id, &decl.name, Some(input.as_str())).err();
                input.clear();
                state.secret_set.remove(&key);
            }
            if set && ui.small_button("Clear").clicked() {
                state.message = secrets::set(id, &decl.name, None).err();
                state.secret_set.remove(&key);
            }
        });
    }
}

/// A key (F13–F24) for each of the plugin's actions (#212).
fn action_keys(ui: &mut egui::Ui, plugin: &Installed, state: &mut UiState) {
    use super::plugin_actions::{self, KEYS};
    if plugin.manifest.actions.is_empty() {
        return;
    }
    let bindings = plugin_actions::bindings();
    let taken = plugin_actions::taken();
    for action in &plugin.manifest.actions {
        ui.horizontal(|ui| {
            ui.add_space(16.0);
            ui.label(&action.name);
            let current = plugin_actions::key_for(&bindings, &plugin.manifest.id, &action.id);
            let mut chosen = current.clone();
            egui::ComboBox::from_id_salt(("plugin-action", &plugin.manifest.id, &action.id))
                .selected_text(current.as_deref().unwrap_or("no key"))
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut chosen, None, "no key");
                    for key in KEYS {
                        let used_by = bindings
                            .get(key)
                            .filter(|b| b.plugin != plugin.manifest.id || b.action != action.id);
                        let label = if used_by.is_some() {
                            format!("{key} (in use)")
                        } else {
                            key.to_string()
                        };
                        ui.selectable_value(&mut chosen, Some(key.to_string()), label);
                    }
                });
            if chosen != current {
                state.message =
                    plugin_actions::bind(&plugin.manifest.id, &action.id, chosen.as_deref()).err();
            }
            if current.as_ref().is_some_and(|k| taken.contains(k)) {
                ui.colored_label(ui.visuals().warn_fg_color, "another program holds this key");
            }
        });
    }
    ui.weak("    Put F13–F24 on a key in ZMK Studio or VIA; it types nothing, and Foxglove passes it to the plugin.");
}

/// Shows what a staged plugin may do; returns it again while the user hasn't decided.
fn approval(ui: &mut egui::Ui, staged: Staged, state: &mut UiState) -> Option<Staged> {
    let m = &staged.manifest;
    ui.horizontal(|ui| {
        ui.add(icons::tile(ui, &m.id, Some(&staged.dir), 48.0));
        ui.strong(format!("Install {} {}?", m.name, m.version));
    });
    if !m.description.is_empty() {
        ui.label(&m.description);
    }
    ui.weak(format!("{} · built for API {}", m.id, m.api));
    if staged.new_permissions.is_empty() {
        ui.label("It asks for no new permissions.");
    } else {
        ui.label("It will be able to:");
        for p in &staged.new_permissions {
            if p.sensitive() {
                ui.colored_label(
                    ui.visuals().warn_fg_color,
                    format!("• {} (sensitive)", p.describe()),
                );
            } else {
                ui.label(format!("• {}", p.describe()));
            }
        }
    }
    let mut decided = None;
    ui.horizontal(|ui| {
        if ui.button("Install").clicked() {
            decided = Some(true);
        }
        if ui.button("Cancel").clicked() {
            decided = Some(false);
        }
    });
    match decided {
        None => Some(staged),
        Some(install) => {
            state.installed = None;
            if install {
                let name = staged.manifest.name.clone();
                super::plugin_runner::stop(&staged.manifest.id);
                state.message = plugin_install::commit(staged, true)
                    .err()
                    .map(|e| format!("Can't install {name}: {e}"));
                super::plugin_runner::sync();
            } else {
                plugin_install::discard(staged);
            }
            None
        }
    }
}

#[cfg(test)]
pub mod tests {
    use super::super::plugin_install::PluginRecord;
    use super::*;
    use foxglove_plugin_api::manifest::Manifest;

    fn installed(id: &str, name: &str, approved: Vec<Permission>) -> Installed {
        let toml = format!(
            "id = \"{id}\"
name = \"{name}\"
version = \"1.0.0\"
api = \"1.0\"
run = [\"p.exe\"]
"
        );
        let manifest =
            Manifest::parse(&toml, foxglove_plugin_api::API_VERSION).expect("a manifest");
        Installed {
            manifest,
            record: PluginRecord {
                enabled: true,
                approved,
            },
        }
    }

    #[test]
    fn each_plugin_has_one_card_with_its_tag() {
        let catalog = plugin_install::catalog();
        let mic = &catalog
            .iter()
            .find(|e| e.id == "org.foxglove.mic-mute")
            .expect("mic mute is listed")
            .id;
        let mine = vec![
            installed(mic, "Mic mute", vec![Permission::DevicesRead]),
            installed("org.example.clock", "Clock", vec![Permission::Net]),
        ];
        let cards = cards(&mine, &catalog);
        assert_eq!(
            cards.len(),
            catalog.len() + 1,
            "the sideloaded one is added, the installed one isn't doubled"
        );
        assert_eq!(
            (cards[0].tag, cards[0].id.as_str()),
            (Tag::Installed, mic.as_str())
        );
        assert!(
            !cards[0].description.is_empty(),
            "an installed plugin with no description of its own takes the catalog's"
        );
        assert_eq!(cards[1].tag, Tag::Sideloaded);
        assert!(
            cards[1].unsandboxed,
            "unrestricted network runs outside the sandbox"
        );
        assert!(cards[2..]
            .iter()
            .all(|c| c.tag == Tag::Catalog && c.installed.is_none()));
        let ids: std::collections::HashSet<_> = cards.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids.len(), cards.len());
    }

    #[test]
    fn corsair_says_it_runs_outside_the_sandbox() {
        let cards = cards(&[], &plugin_install::catalog());
        let icue = cards
            .iter()
            .find(|c| c.id == "org.foxglove.icue")
            .expect("iCUE is listed");
        assert!(
            icue.unsandboxed,
            "it loads iCUE's DLL with the files permission"
        );
        let mic = cards
            .iter()
            .find(|c| c.id == "org.foxglove.mic-mute")
            .unwrap();
        assert!(!mic.unsandboxed);
        // The scoped network forms stay in the sandbox.
        let obs = cards
            .iter()
            .find(|c| c.id == "org.foxglove.broadcast-status")
            .unwrap();
        assert!(!obs.unsandboxed);
    }

    /// #376: the Plugins tab's line, from no lease to several stacked ones.
    #[test]
    fn lights_line_names_who_holds_the_lights() {
        use crate::foxglove::lighting_leases::{Holders, Holding, Priority};
        let name = |conn: u64| {
            ["", "Mic mute", "OpenRGB", "Pomodoro"]
                .get(conn as usize)
                .map_or("a tool".to_string(), |s| s.to_string())
        };
        let kb = |leases: Vec<(u64, Priority)>| Holding {
            device: "kb".into(),
            leases,
        };
        assert_eq!(lights_line(&Holders::default(), name), "Lights: yours");
        let one = Holders {
            devices: vec![kb(vec![(2, Priority::Low)])],
            paused: false,
        };
        assert_eq!(lights_line(&one, name), "Lights: OpenRGB (low)");
        let stacked = Holders {
            devices: vec![kb(vec![
                (1, Priority::High),
                (3, Priority::Normal),
                (2, Priority::Low),
            ])],
            paused: false,
        };
        assert_eq!(
            lights_line(&stacked, name),
            "Lights: Mic mute (high), under Pomodoro (normal), OpenRGB (low)"
        );
        let paused = Holders {
            paused: true,
            ..stacked.clone()
        };
        assert_eq!(lights_line(&paused, name), "Lights: yours while Foxglove is paused, then Mic mute (high), under Pomodoro (normal), OpenRGB (low)");
        let two = Holders {
            devices: vec![
                kb(vec![(1, Priority::High)]),
                Holding {
                    device: "via:4653:0001".into(),
                    leases: vec![(9, Priority::Normal)],
                },
            ],
            paused: false,
        };
        assert_eq!(
            lights_line(&two, name),
            "Lights: kb: Mic mute (high); via:4653:0001: a tool (normal)"
        );
    }

    /// The same line from a real arbiter: a grant changes it, a release changes it back.
    #[test]
    fn granting_and_releasing_a_lease_changes_the_line() {
        use crate::foxglove::lighting_leases::{Arbiter, Lease, Priority};
        use foxglove_devices::{Effect, Lighting, LightingState};
        struct Lights(std::cell::RefCell<LightingState>);
        impl Lighting for Lights {
            fn layout(&self) -> foxglove_devices::Result<Vec<foxglove_devices::Zone>> {
                Ok(vec![])
            }
            fn state(&self) -> foxglove_devices::Result<LightingState> {
                Ok(self.0.borrow().clone())
            }
            fn set_on(&self, on: bool) -> foxglove_devices::Result<()> {
                self.0.borrow_mut().on = on;
                Ok(())
            }
            fn set_color(&self, hue: u16, sat: u8, brightness: u8) -> foxglove_devices::Result<()> {
                let mut s = self.0.borrow_mut();
                (s.hue, s.sat, s.brightness) = (hue, sat, brightness);
                Ok(())
            }
            fn set_effect(&self, effect: Effect) -> foxglove_devices::Result<()> {
                self.0.borrow_mut().effect = effect;
                Ok(())
            }
            fn set_speed(&self, speed: u8) -> foxglove_devices::Result<()> {
                self.0.borrow_mut().speed = speed;
                Ok(())
            }
        }
        let kb = Lights(std::cell::RefCell::new(LightingState {
            on: true,
            hue: 30,
            sat: 100,
            brightness: 80,
            effect: Effect {
                id: "0".into(),
                params: vec![],
            },
            speed: 1,
            settled: true,
        }));
        let lease = |conn, priority| Lease {
            id: 0,
            conn,
            device: "kb".into(),
            priority,
            hue: 0,
            sat: 100,
            brightness: 80,
            effect: "0".into(),
        };
        let name = |conn: u64| {
            if conn == 1 {
                "Mic mute".to_string()
            } else {
                "OpenRGB".to_string()
            }
        };
        let mut a = Arbiter::default();
        assert_eq!(lights_line(&a.holders(), name), "Lights: yours");
        a.add(lease(2, Priority::Low), &kb).unwrap();
        let mute = a.add(lease(1, Priority::High), &kb).unwrap();
        assert_eq!(
            lights_line(&a.holders(), name),
            "Lights: Mic mute (high), under OpenRGB (low)"
        );
        for d in a.end(|l| l.id == mute) {
            a.apply(&d, &kb).unwrap();
        }
        assert_eq!(lights_line(&a.holders(), name), "Lights: OpenRGB (low)");
        for d in a.end(|_| true) {
            a.apply(&d, &kb).unwrap();
        }
        assert_eq!(lights_line(&a.holders(), name), "Lights: yours");
    }

    #[test]
    fn tags_in_words() {
        assert_eq!(
            [Tag::Catalog, Tag::Installed, Tag::Sideloaded].map(Tag::label),
            ["CATALOG", "INSTALLED", "SIDELOADED"]
        );
    }

    /// The previews' page: Mic mute installed and running, a sideloaded clock, the catalog.
    pub fn sample() -> View {
        let catalog = plugin_install::catalog();
        let mut cards = cards(
            &[installed(
                "org.foxglove.mic-mute",
                "Mic mute",
                vec![Permission::DevicesRead, Permission::LightingWrite],
            )],
            &catalog,
        );
        cards[0].state = Some("Running".into());
        let lights = "Lights: Mic mute (high), under OpenRGB (low)".into();
        View {
            cards,
            catalog: "Catalog: up to date".into(),
            can_check: true,
            note: None,
            lights,
        }
    }
}
