// SPDX-License-Identifier: MIT
//! The tray panel (§2.1 job 1, §5.1 v1.4): a grid of `tile` panels, each in the hub's frame,
//! with the ··· menu, Edit tiles and Open Hideout. Which tiles show comes from the manifests'
//! `[[ui.panels]]` and their `when` (§4.6), and from the user's choices ([`TrayLayout`]). The
//! frame draws the module's state from `module.<id>.state` and `module.<id>.restart` (§7.8).
//!
//! Window-free: it draws into any `egui::Ui` and reports what the user asked for, so the shell
//! decides where the panel lives and tests render it without a window.

use crate::model::{PanelHost, PanelKey, PanelState};
use crate::render::{tile_body, tone_rgb, DrawReport, Go};
use egui::{Color32, RichText, Sense, Stroke, Ui};
use hideout_contract::module::Slot;
use std::collections::BTreeSet;
use std::time::{Duration, Instant};

/// Width of a tile in the tray panel.
pub const TILE_WIDTH: f32 = 228.0;
/// Tiles per row.
pub const COLUMNS: usize = 2;

/// What the user chose for the tray panel, kept in the hub's preferences. Tiles are named
/// `<module>/<panel>`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TrayLayout {
    /// The user's order; tiles not in it follow in the hub's order.
    pub order: Vec<String>,
    /// Tiles the user hid.
    pub hidden: BTreeSet<String>,
    /// Tiles the user showed although their `when` is false (an app not seen yet).
    pub shown: BTreeSet<String>,
}

/// The name a tile has in [`TrayLayout`].
pub fn tile_name(key: &PanelKey) -> String {
    format!("{}/{}", key.module, key.panel)
}

/// A pick from a tile's ··· menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TileMenu {
    Open,
    Hide,
    MoveLeft,
    MoveRight,
}

/// What a tile draw saw.
#[derive(Debug, Default)]
pub struct TileReport {
    pub draw: DrawReport,
    /// The tile (outside its button) or the frame's **Open** was pressed: open the module's page.
    pub open: bool,
    pub menu: Option<TileMenu>,
    /// **Update Hideout** on a module that needs a newer hub.
    pub update_hub: bool,
}

/// What the user asked of the tray panel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TrayAction {
    /// Open the module's page (`target` as a button's `opens`: `page`, `page#<node>`,
    /// `settings`).
    Open { module: String, target: String },
    /// Open Hideout (the main window).
    OpenHub,
    /// Update Hideout (a module needs a newer hub).
    UpdateHub,
    /// The layout changed: save it.
    LayoutChanged,
}

#[derive(Debug, Default)]
pub struct TrayReport {
    pub actions: Vec<TrayAction>,
    /// Draw again after this while the panel is open (a restart countdown or a "Last seen"
    /// age is on screen); `None` when nothing on it changes by itself.
    pub repaint_in: Option<Duration>,
}

/// One tile the tray panel knows: shown or not.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TileEntry {
    pub key: PanelKey,
    pub title: String,
    pub shown: bool,
}

/// Every tile the hub knows, in the tray panel's order, and whether it shows: declared in a
/// manifest (or registered by a module the hub has no manifest for), not of a module that is
/// off, not hidden by the user, and its `when` true or shown by the user (§4.6, decision 2).
pub fn tray_tiles(host: &PanelHost, layout: &TrayLayout) -> Vec<TileEntry> {
    let mut all: Vec<TileEntry> = Vec::new();
    let mut seen = BTreeSet::new();
    for m in host.manifests() {
        for p in m.panels.iter().filter(|p| p.slot == Slot::Tile) {
            let key = PanelKey::new(&m.id, &p.id);
            let name = tile_name(&key);
            seen.insert(name.clone());
            let shown = !layout.hidden.contains(&name)
                && (layout.shown.contains(&name) || host.when(p.when.as_deref()));
            all.push(TileEntry {
                key,
                title: p.title.clone(),
                shown,
            });
        }
    }
    for p in host.in_slot(Slot::Tile) {
        let name = tile_name(&p.key);
        if seen.insert(name.clone()) {
            all.push(TileEntry {
                key: p.key.clone(),
                title: p.title.clone(),
                shown: !layout.hidden.contains(&name),
            });
        }
    }
    // Off modules have no tile (the Modules page has Turn on).
    all.retain(|t| host.module_state(&t.key.module) != Some("off"));
    let rank = |t: &TileEntry| {
        layout
            .order
            .iter()
            .position(|n| *n == tile_name(&t.key))
            .unwrap_or(usize::MAX)
    };
    all.sort_by_key(rank);
    all
}

/// Keeps the host's tile visibility (`ui.visibility` for tiles, §5.9) in step with the layout.
pub fn sync_hidden(host: &mut PanelHost, layout: &TrayLayout) {
    for t in tray_tiles(host, layout) {
        if host.tile_hidden(&t.key) == t.shown {
            host.set_tile_hidden(&t.key, !t.shown);
        }
    }
}

/// The microphone from Windows' icon font (Segoe Fluent Icons, or Segoe MDL2 Assets on
/// Windows 10; the hub loads it after its text fonts).
pub const MIC_GLYPH: &str = "\u{E720}";

/// The glyph for a published icon name (§5.1 v1.4, `ICONS`), from the system symbol font. An
/// unknown name draws the generic module glyph, so a newer name degrades on an older hub.
pub fn icon_glyph(name: &str) -> &'static str {
    match name {
        "battery" => "\u{1F50B}",
        "bell" => "\u{1F514}",
        "book" => "\u{1F4D6}",
        "brush" => "\u{1F58C}",
        "calendar" => "\u{1F4C5}",
        "camera" => "\u{1F4F7}",
        "chart" => "\u{1F4C8}",
        "chat" => "\u{1F4AC}",
        "cloud" => "\u{2601}",
        "folder" => "\u{1F4C1}",
        "gamepad" => "\u{1F3AE}",
        "gear" => "\u{2699}",
        "headphones" => "\u{1F3A7}",
        "image" => "\u{1F5BC}",
        "info" => "\u{2139}",
        "key" => "\u{1F511}",
        "keyboard" => "\u{2328}",
        "lights" => "\u{1F4A1}",
        "link" => "\u{1F517}",
        "lock" => "\u{1F512}",
        // Segoe's icon font (Microphone): Segoe UI Symbol's U+1F3A4 reads as a pen at tile size.
        "mic" => MIC_GLYPH,
        "music" => "\u{266B}",
        "palette" => "\u{1F3A8}",
        "play" => "\u{25B6}",
        "screen" => "\u{1F5B5}",
        "sparkle" => "\u{2728}",
        "speaker" => "\u{1F50A}",
        "terminal" => "\u{2328}",
        "timer" => "\u{23F1}",
        "video" => "\u{1F3A5}",
        "wifi" => "\u{1F4F6}",
        "bluetooth" => "\u{1F4F6}",
        _ => "\u{25A3}",
    }
}

/// "Last seen 12 s ago" / "Last seen 3 min ago" (§7.8, the stale caption).
pub fn last_seen_words(age: Duration) -> String {
    let s = age.as_secs();
    if s < 60 {
        format!("Last seen {s} s ago")
    } else if s < 3600 {
        format!("Last seen {} min ago", s / 60)
    } else {
        format!("Last seen {} h ago", s / 3600)
    }
}

/// The restart strip's words (§7.8): "Restarting in 4 s (2 of 5)". No module name: the tile's
/// title already says it.
pub fn restart_words(in_s: u64, crash: u64, of: u64) -> String {
    format!("Restarting in {in_s} s ({crash} of {of})")
}

fn rgb(c: (u8, u8, u8)) -> Color32 {
    Color32::from_rgb(c.0, c.1, c.2)
}

/// Draws one tile: the hub's frame around the module's body. See [`tile_ui_at`].
pub fn tile_ui(host: &mut PanelHost, ui: &mut Ui, key: &PanelKey) -> TileReport {
    tile_ui_at(host, ui, key, Instant::now())
}

/// Draws the tile `key` at `now` (§5.1 v1.4, §7.8): icon, title and ··· menu from the manifest;
/// then by the module's state:
/// - running: the module's body;
/// - restarting: the amber strip "Restarting in <n> s (<k> of 5)", the body greyed, "Last seen";
/// - stopped after its crashes: red outline and mark, "Stopped after 5 crashes", **Open** only;
/// - needs a newer hub: red mark, "Needs a newer Hideout", **Update Hideout**;
/// - a tile that couldn't be shown: "Its tile couldn't be shown", **Open**;
/// - not registered yet: "Starting…".
///
/// Each tile draws in its own guarded scope, like a page (§5.5).
pub fn tile_ui_at(host: &mut PanelHost, ui: &mut Ui, key: &PanelKey, now: Instant) -> TileReport {
    let mut report = TileReport::default();
    let decl = host
        .manifest(&key.module)
        .and_then(|m| m.panels.iter().find(|p| p.id == key.panel))
        .map(|p| (p.title.clone(), p.icon.clone()));
    let panel = host
        .panel(key)
        .map(|p| (p.title.clone(), p.icon.clone(), p.state.clone()));
    let Some((title, icon)) = decl.or_else(|| panel.as_ref().map(|p| (p.0.clone(), p.1.clone())))
    else {
        return report;
    };
    let module_name = host.module_name(&key.module);
    let fact_state = host.module_state(&key.module).map(String::from);
    let state = panel.as_ref().map(|p| p.2.clone());
    let greyed = match &state {
        Some(PanelState::Greyed(s)) => Some(s.clone()),
        _ => None,
    };
    let current = fact_state.clone().or(greyed.clone());
    let stopped = matches!(current.as_deref(), Some("crash-stopped" | "crash-looped"));
    let needs_hub = current.as_deref() == Some("needs-hub");
    let unshowable = matches!(
        state,
        Some(PanelState::Broken | PanelState::Quarantined | PanelState::Rejected(_))
    ) || current.as_deref() == Some("quarantined");
    let red = stopped || needs_hub || unshowable;
    let danger = ui.visuals().error_fg_color;
    let dark = ui.visuals().dark_mode;
    let amber = rgb(tone_rgb("attention", dark).unwrap_or((0xF5, 0xC4, 0x51)));
    let mut frame = egui::Frame::group(ui.style())
        .inner_margin(10.0)
        .corner_radius(8.0);
    if red {
        frame = frame.stroke(Stroke::new(1.5_f32, danger));
    }
    let inner = frame.show(ui, |ui| {
        ui.set_width(TILE_WIDTH);
        ui.vertical(|ui| {
            ui.horizontal(|ui| {
                let glyph = RichText::new(icon_glyph(&icon)).size(15.0);
                ui.label(if red { glyph.color(danger) } else { glyph });
                if red {
                    ui.label(RichText::new("\u{26A0}").color(danger));
                }
                ui.label(RichText::new(&title).strong());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.menu_button("\u{00B7}\u{00B7}\u{00B7}", |ui| {
                        let pick = [
                            (TileMenu::Open, format!("Open {module_name}")),
                            (TileMenu::Hide, "Hide tile".into()),
                            (TileMenu::MoveLeft, "Move left".into()),
                            (TileMenu::MoveRight, "Move right".into()),
                        ];
                        for (m, words) in pick {
                            if ui.button(words).clicked() {
                                report.menu = Some(m);
                                ui.close();
                            }
                        }
                    });
                });
            });
            if stopped || unshowable {
                let words = if stopped {
                    crate::model::state_words("crash-stopped")
                } else {
                    "Its tile couldn't be shown"
                };
                ui.label(RichText::new(words).color(danger));
                report.open |= ui.button("Open").clicked();
                return;
            }
            if needs_hub {
                ui.label(RichText::new(crate::model::state_words("needs-hub")).color(danger));
                report.update_hub |= ui.button("Update Hideout").clicked();
                return;
            }
            let restarting = current.as_deref() == Some("restarting");
            if restarting {
                if let Some(r) = host.restart(&key.module, now) {
                    egui::Frame::new()
                        .fill(amber.gamma_multiply(0.18))
                        .corner_radius(4.0)
                        .inner_margin(egui::Margin::symmetric(6, 2))
                        .show(ui, |ui| {
                            ui.label(
                                RichText::new(restart_words(r.in_s, r.crash, r.of)).color(amber),
                            );
                        });
                }
            }
            if panel.is_none() {
                let words = match current.as_deref() {
                    Some(s) if s != "running" => crate::model::state_words(s),
                    _ => "Starting\u{2026}",
                };
                ui.label(RichText::new(words).weak());
                return;
            }
            if let Some(s) = greyed.as_deref().filter(|s| *s != "restarting") {
                ui.label(RichText::new(crate::model::state_words(s)).weak());
            }
            let body = ui.scope(|ui| tile_body(host, ui, key));
            report.draw = body.inner;
            if restarting {
                body.response.on_hover_text(format!(
                    "{module_name} is restarting. A key press waits up to 2 s."
                ));
            }
            if greyed.is_some() {
                if let Some(age) = host.last_seen(&key.module, now) {
                    ui.label(RichText::new(last_seen_words(age)).small().weak());
                }
            }
        });
    });
    // A click on the tile outside its button opens the module's page (§5.1).
    if inner.response.interact(Sense::click()).clicked() {
        report.open = true;
    }
    report
}

/// Draws the tray panel: the tile grid (or Edit tiles), then Open Hideout and Edit tiles.
/// `editing` is the panel's own view state.
pub fn tray_panel_ui(
    host: &mut PanelHost,
    ui: &mut Ui,
    layout: &mut TrayLayout,
    editing: &mut bool,
    now: Instant,
) -> TrayReport {
    let mut report = TrayReport::default();
    let tiles = tray_tiles(host, layout);
    if *editing {
        ui.label(RichText::new("Edit tiles").strong().size(16.0));
        ui.label(RichText::new("Choose which tiles show here.").weak());
        ui.add_space(6.0);
        for t in &tiles {
            let mut on = t.shown;
            let name = host.module_name(&t.key.module);
            if ui.checkbox(&mut on, &name).changed() {
                let n = tile_name(&t.key);
                if on {
                    layout.hidden.remove(&n);
                    layout.shown.insert(n);
                } else {
                    layout.shown.remove(&n);
                    layout.hidden.insert(n);
                }
                report.actions.push(TrayAction::LayoutChanged);
            }
        }
        ui.add_space(8.0);
        if ui.button("Done").clicked() {
            *editing = false;
        }
        sync_hidden(host, layout);
        return report;
    }
    let shown: Vec<TileEntry> = tiles.iter().filter(|t| t.shown).cloned().collect();
    if shown.is_empty() {
        ui.label(RichText::new("No tiles to show").strong());
        ui.label(RichText::new("Edit tiles to choose which modules show here.").weak());
    }
    let mut picks = Vec::new();
    egui::Grid::new("hideout.tray.tiles")
        .num_columns(COLUMNS)
        .spacing([8.0, 8.0])
        .show(ui, |ui| {
            for (i, t) in shown.iter().enumerate() {
                let r = tile_ui_at(host, ui, &t.key, now);
                picks.push((t.key.clone(), r));
                if (i + 1) % COLUMNS == 0 {
                    ui.end_row();
                }
            }
        });
    for (key, r) in picks {
        let m = key.module.clone();
        if r.open || r.menu == Some(TileMenu::Open) {
            report.actions.push(TrayAction::Open {
                module: m.clone(),
                target: "page".into(),
            });
        }
        if r.update_hub {
            report.actions.push(TrayAction::UpdateHub);
        }
        for g in r.draw.go {
            if let Go::Open { module, target } = g {
                report.actions.push(TrayAction::Open { module, target });
            }
        }
        match r.menu {
            Some(TileMenu::Hide) => {
                let n = tile_name(&key);
                layout.shown.remove(&n);
                layout.hidden.insert(n);
                report.actions.push(TrayAction::LayoutChanged);
            }
            Some(dir @ (TileMenu::MoveLeft | TileMenu::MoveRight)) => {
                let mut order: Vec<String> = tiles.iter().map(|t| tile_name(&t.key)).collect();
                let n = tile_name(&key);
                let at = order.iter().position(|x| *x == n).unwrap_or(0);
                // Past the next shown tile, so a hidden tile in between doesn't swallow the move.
                let is_shown = |name: &String| shown.iter().any(|t| tile_name(&t.key) == *name);
                let to = if dir == TileMenu::MoveLeft {
                    (0..at).rev().find(|&j| is_shown(&order[j]))
                } else {
                    (at + 1..order.len()).find(|&j| is_shown(&order[j]))
                };
                if let Some(to) = to {
                    let item = order.remove(at);
                    order.insert(to, item);
                    layout.order = order;
                    report.actions.push(TrayAction::LayoutChanged);
                }
            }
            _ => {}
        }
    }
    ui.add_space(6.0);
    ui.separator();
    ui.horizontal(|ui| {
        if ui.button("Open Hideout").clicked() {
            report.actions.push(TrayAction::OpenHub);
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("Edit tiles").clicked() {
                *editing = true;
            }
        });
    });
    sync_hidden(host, layout);
    let ticking = shown.iter().any(|t| {
        host.module_state(&t.key.module) == Some("restarting")
            || host.last_seen(&t.key.module, now).is_some()
                && host.panel(&t.key).is_some_and(|p| !p.live())
    });
    if ticking {
        report.repaint_in = Some(Duration::from_secs(1));
    }
    report
}
