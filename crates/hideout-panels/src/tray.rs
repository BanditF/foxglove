// SPDX-License-Identifier: MIT
//! The tray panel (§2.1 job 1, §5.1 v1.4): a grid of `tile` panels, each in the hub's frame,
//! with the ··· menu, Edit tiles and Open Hideout. Which tiles show comes from the manifests'
//! `[[ui.panels]]` and their `when` (§4.6), and from the user's choices ([`TrayLayout`]). The
//! frame draws the module's state from `module.<id>.state` and `module.<id>.restart` (§7.8).
//!
//! Window-free: it draws into any `egui::Ui` and reports what the user asked for, so the shell
//! decides where the panel lives and tests render it without a window.

use crate::model::{PanelHost, PanelKey, PanelState};
use crate::render::{tile_body, tone_rgb, DrawReport, Glance, Go};
use egui::{Color32, FocusDirection, Id, Key, Modifiers, Popup, RichText, Sense, Stroke, Ui};
use hideout_contract::module::Slot;
use std::collections::{BTreeSet, HashMap};
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

/// The keyboard focus id of the tile `key`. The shell gives it the focus when the flyout opens
/// ([`focus_first_tile`]); arrows move it between tiles in grid order.
pub fn tile_focus_id(key: &PanelKey) -> Id {
    Id::new(("hideout.tray.tile", &key.module, &key.panel))
}

/// Whether the keyboard is in use in the tray panel: only then does the focused tile show its
/// ring (focus-visible, as in the Windows flyouts). Set by a key the panel answers to.
fn keyboard_id() -> Id {
    Id::new("hideout.tray.keyboard")
}

/// The tile drawn with the focus ring this frame (for tests and the lab).
fn ring_id() -> Id {
    Id::new("hideout.tray.ring")
}

/// Gives the keyboard focus to the first tile the tray panel shows (the flyout just opened,
/// docs/hideout-plugin-modules-ux.md "Keyboard and screen reader"). `from_keyboard`: the
/// flyout was opened from the keyboard, so the ring shows at once; after a pointer open it
/// shows only once a key is pressed in the panel. False with no tile.
pub fn focus_first_tile(
    ctx: &egui::Context,
    host: &PanelHost,
    layout: &TrayLayout,
    from_keyboard: bool,
) -> bool {
    ctx.data_mut(|d| d.insert_temp(keyboard_id(), from_keyboard));
    let Some(first) = tray_tiles(host, layout).into_iter().find(|t| t.shown) else {
        return false;
    };
    ctx.memory_mut(|m| m.request_focus(tile_focus_id(&first.key)));
    true
}

/// The focus id of the tile drawn with the focus ring in the last frame; `None` when no ring
/// showed (no tile focused, or the keyboard not used yet).
pub fn ring_on(ctx: &egui::Context) -> Option<Id> {
    ctx.data(|d| d.get_temp::<Option<Id>>(ring_id())).flatten()
}

/// The tile that the arrow keys go to from tile `at` of `n` in a grid of [`COLUMNS`]: Left and
/// Right step through the grid order (into the row before or after), Up and Down a row, Down
/// from above a short last row to its last tile. It stays put at an edge.
pub fn arrow_target(at: usize, n: usize, key: Key) -> usize {
    if n == 0 {
        return 0;
    }
    let last = n - 1;
    match key {
        Key::ArrowLeft => at.saturating_sub(1),
        Key::ArrowRight => (at + 1).min(last),
        Key::ArrowUp => at.checked_sub(COLUMNS).unwrap_or(at),
        Key::ArrowDown if at / COLUMNS < last / COLUMNS => (at + COLUMNS).min(last),
        _ => at,
    }
}

/// What the keyboard asked of one tile this draw.
#[derive(Clone, Copy, Debug, Default)]
struct TileKeys {
    /// Space: press the tile's button.
    press: bool,
    /// Shift+F10 or the Menu key: open the ··· menu.
    menu: bool,
    /// The keyboard is in use: a focused tile shows its ring.
    ring: bool,
    /// The row's height (its tallest tile's content, last frame): tiles in a row are equal.
    min_height: f32,
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
    /// The height of the tile's own content (inside its frame), before the row's height.
    pub content_height: f32,
    /// The tile's phase this draw (`None` when it drew nothing).
    pub phase: Option<TilePhase>,
}

/// A tile's phase: its state without what ticks, so a change of it is worth saying
/// (docs/hideout-plugin-modules-ux.md, Keyboard and screen reader). Decided from the drawn tile
/// alone, the same for every module:
/// - the frame's state words ("Restarting", "Stopped after 5 crashes");
/// - the glance's `tone` (Focus to Rest is `active` to `ok`);
/// - the glance's word, unless it has a digit: "Muted" and "Live" are states, "18:42" and
///   "42 %" are readings that tick.
///
/// The sub line and the meter never count: they say what the glance is about, or how far along.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TilePhase {
    pub key: PanelKey,
    frame: Vec<String>,
    tone: Option<String>,
    word: Option<String>,
    /// What is said when the phase changes: "Mic mute, Muted", "Focus timer, Rest".
    pub words: String,
}

impl TilePhase {
    /// The phase of a tile titled `title` whose frame said `frame` and whose body drew `draw`.
    /// The words: the title, then the frame's state words; or else the glance's word; or else
    /// the sub line's first part ("Rest · then round 3" says "Rest"); or else the glance.
    fn of(key: &PanelKey, title: &str, frame: &[String], draw: &DrawReport) -> TilePhase {
        let ticks = |t: &str| t.chars().any(|c| c.is_ascii_digit());
        let glance = draw.glance.clone().unwrap_or_default();
        let word = (!glance.text.is_empty() && !ticks(&glance.text)).then(|| glance.text.clone());
        // The sub line is a `label`'s text only: never a button's or a key chip's.
        let sub = draw
            .sub
            .as_ref()
            .filter(|t| !t.is_empty())
            .map(|t| t.split(" \u{00B7} ").next().unwrap_or(t).to_string());
        let what = if !frame.is_empty() {
            frame.join(", ")
        } else if let Some(w) = &word {
            w.clone()
        } else {
            sub.unwrap_or_else(|| glance.text.clone())
        };
        let words = [title.to_string(), what]
            .into_iter()
            .filter(|t| !t.is_empty())
            .collect::<Vec<_>>()
            .join(", ");
        let Glance { tone, .. } = glance;
        TilePhase {
            key: key.clone(),
            frame: frame.to_vec(),
            tone,
            word,
            words,
        }
    }

    fn same(&self, other: &TilePhase) -> bool {
        (&self.frame, &self.tone, &self.word) == (&other.frame, &other.tone, &other.word)
    }
}

/// At most one announcement per tile in this long.
pub const ANNOUNCE_EVERY: Duration = Duration::from_secs(5);

/// The tray panel's polite announcements while it is open (#991): a tile's phase change is said
/// once, at most every [`ANNOUNCE_EVERY`] per tile; a change inside that window is said with the
/// first draw after it, if the phase is still not the one last said. The shell keeps one per
/// open flyout, so what changed while it was closed is never said: the first sight of a tile is
/// only remembered. It asks for no frame of its own except one: a change held back by the
/// window gives [`TileAnnouncer::wake_in`] once, so it is said when the window ends.
#[derive(Debug, Default)]
pub struct TileAnnouncer {
    tiles: HashMap<PanelKey, Seen>,
    /// The wake a change held back in this draw asks for.
    wake: Option<Duration>,
}

/// One tile's announcement state.
#[derive(Debug)]
struct Seen {
    /// The phase last said (or first seen).
    last: TilePhase,
    /// When it was last said.
    at: Option<Instant>,
    /// A change is held back by the window (its wake was asked for).
    held: bool,
}

/// A held-back change is drawn this long after its window ends, so the wake's frame is never a
/// hair early.
const WAKE_MARGIN: Duration = Duration::from_millis(50);

impl TileAnnouncer {
    /// Takes this draw's tile phases at `now`; returns what to say, if anything (several tiles
    /// at once are one sentence each).
    pub fn see(&mut self, phases: &[TilePhase], now: Instant) -> Option<String> {
        // A tile not drawn (hidden, or Edit tiles is open) starts again from its next sight.
        self.tiles.retain(|k, _| phases.iter().any(|p| p.key == *k));
        self.wake = None;
        let mut say = Vec::new();
        for p in phases {
            let Some(t) = self.tiles.get_mut(&p.key) else {
                self.tiles.insert(
                    p.key.clone(),
                    Seen {
                        last: p.clone(),
                        at: None,
                        held: false,
                    },
                );
                continue;
            };
            if t.last.same(p) {
                // Back to what was said: nothing is owed.
                t.held = false;
                continue;
            }
            let since = t.at.map(|at| now.saturating_duration_since(at));
            match since.filter(|s| *s < ANNOUNCE_EVERY) {
                None => {
                    t.last = p.clone();
                    t.at = Some(now);
                    t.held = false;
                    say.push(p.words.clone());
                }
                Some(s) if !t.held => {
                    t.held = true;
                    let left = ANNOUNCE_EVERY - s + WAKE_MARGIN;
                    self.wake = Some(self.wake.map_or(left, |w| w.min(left)));
                }
                Some(_) => {}
            }
        }
        (!say.is_empty()).then(|| say.join(". "))
    }

    /// After [`TileAnnouncer::see`]: draw again in this long, because a change was held back by
    /// the 5 s window in that draw. Given once per held change (one bounded wake, not a poll).
    pub fn wake_in(&self) -> Option<Duration> {
        self.wake
    }
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
    /// Each drawn tile's phase, in grid order (for [`TileAnnouncer`]).
    pub phases: Vec<TilePhase>,
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
    tile_ui_keyed(host, ui, key, now, TileKeys::default())
}

/// [`tile_ui_at`] with the keyboard's asks. The tile is one focusable node whose accessible name
/// is its title, value and sub line ("Mic mute, Muted, Default communications mic").
fn tile_ui_keyed(
    host: &mut PanelHost,
    ui: &mut Ui,
    key: &PanelKey,
    now: Instant,
    keys: TileKeys,
) -> TileReport {
    let mut report = TileReport::default();
    // What a screen reader hears besides the body's texts: the frame's state words.
    let mut spoken: Vec<String> = Vec::new();
    let menu_focus = Id::new(("hideout.tray.menu-focus", &key.module, &key.panel));
    let menu_return = Id::new(("hideout.tray.menu-return", &key.module, &key.panel));
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
        // As tall as the row's tallest tile, content at the top (Quick Settings).
        ui.set_min_height(keys.min_height);
        let content = ui.vertical(|ui| {
            ui.horizontal(|ui| {
                let glyph = RichText::new(icon_glyph(&icon)).size(15.0);
                ui.label(if red { glyph.color(danger) } else { glyph });
                if red {
                    ui.label(RichText::new("\u{26A0}").color(danger));
                }
                ui.label(RichText::new(&title).strong());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let menu = ui.menu_button("\u{00B7}\u{00B7}\u{00B7}", |ui| {
                        // Opened from the keyboard: its first item takes the focus.
                        let focus = ui.data_mut(|d| d.remove_temp::<bool>(menu_focus));
                        let pick = [
                            (TileMenu::Open, format!("Open {module_name}")),
                            (TileMenu::Hide, "Hide tile".into()),
                            (TileMenu::MoveLeft, "Move left".into()),
                            (TileMenu::MoveRight, "Move right".into()),
                        ];
                        for (i, (m, words)) in pick.into_iter().enumerate() {
                            let b = ui.button(words);
                            if i == 0 && focus == Some(true) {
                                b.request_focus();
                            }
                            if b.clicked() {
                                report.menu = Some(m);
                                ui.close();
                            }
                        }
                    });
                    let id = Popup::default_response_id(&menu.response);
                    if keys.menu {
                        Popup::open_id(ui.ctx(), id);
                        ui.data_mut(|d| {
                            d.insert_temp(menu_focus, true);
                            d.insert_temp(menu_return, true);
                        });
                        ui.ctx().request_repaint();
                    } else if !Popup::is_id_open(ui.ctx(), id)
                        && ui.data_mut(|d| d.remove_temp::<bool>(menu_return)) == Some(true)
                    {
                        // A menu opened from the keyboard closed (Esc): the focus goes back to
                        // its tile, as in a Windows menu.
                        ui.memory_mut(|m| m.request_focus(tile_focus_id(key)));
                    }
                });
            });
            if stopped || unshowable {
                let words = if stopped {
                    crate::model::state_words("crash-stopped")
                } else {
                    "Its tile couldn't be shown"
                };
                spoken.push(words.to_string());
                ui.label(RichText::new(words).color(danger));
                report.open |= ui.button("Open").clicked();
                return;
            }
            if needs_hub {
                spoken.push(crate::model::state_words("needs-hub").to_string());
                ui.label(RichText::new(crate::model::state_words("needs-hub")).color(danger));
                report.update_hub |= ui
                    .button("Update Hideout")
                    .on_hover_text("Opens the Hideout download page in your browser.")
                    .clicked();
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
                    spoken.push("Restarting".into());
                }
            }
            if panel.is_none() {
                let words = match current.as_deref() {
                    Some(s) if s != "running" => crate::model::state_words(s),
                    _ => "Starting\u{2026}",
                };
                spoken.push(words.to_string());
                ui.label(RichText::new(words).weak());
                return;
            }
            if let Some(s) = greyed.as_deref().filter(|s| *s != "restarting") {
                ui.label(RichText::new(crate::model::state_words(s)).weak());
            }
            let body = ui.scope(|ui| tile_body(host, ui, key, keys.press));
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
        content.response.rect.height()
    });
    report.content_height = inner.inner;
    report.phase = Some(TilePhase::of(key, &title, &spoken, &report.draw));
    // A click on the tile outside its button opens the module's page (§5.1); so does Enter on
    // the focused tile.
    let tile = ui.interact(inner.response.rect, tile_focus_id(key), Sense::click());
    let name: Vec<String> = std::iter::once(title)
        .chain(spoken)
        // Some readers say "dot" for the middle dot: a comma in the spoken name only.
        .chain(
            report
                .draw
                .texts
                .iter()
                .map(|t| t.replace(" \u{00B7} ", ", ")),
        )
        .filter(|t| !t.is_empty())
        .collect();
    let name = name.join(", ");
    tile.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &name));
    if tile.has_focus() && keys.ring {
        ui.data_mut(|d| d.insert_temp(ring_id(), Some(tile_focus_id(key))));
        // The accent (the selection fill), like the hub's other focus marks.
        let ring = ui.visuals().selection.bg_fill;
        ui.painter().rect_stroke(
            inner.response.rect,
            8.0,
            Stroke::new(2.0_f32, ring),
            egui::StrokeKind::Outside,
        );
    }
    if tile.clicked() {
        report.open = true;
    }
    report
}

/// The keyboard inside the tray panel while one of its tiles has the focus: arrows move it in
/// grid order, Space presses that tile's button and Shift+F10 opens its ··· menu (Enter is
/// the tile's own click). Taken here, before the grid draws, so egui's own arrow navigation
/// (to the nearest widget, a tile's button) and Space's click on the tile don't run.
fn tile_keys(ui: &mut Ui, shown: &[TileEntry]) -> Vec<TileKeys> {
    let mut keys = vec![TileKeys::default(); shown.len()];
    let ids: Vec<Id> = shown.iter().map(|t| tile_focus_id(&t.key)).collect();
    // A key the panel answers to (arrows, Tab, Space, Enter, Shift+F10 or the Menu key) puts
    // it in keyboard mode: the focus ring shows from now on.
    let keyed = ui.input(|i| {
        i.events.iter().any(|e| {
            matches!(e, egui::Event::Key { key, pressed: true, modifiers, .. }
                if matches!(key, Key::ArrowLeft | Key::ArrowRight | Key::ArrowUp
                    | Key::ArrowDown | Key::Tab | Key::Space | Key::Enter)
                    || (*key == Key::F10 && modifiers.shift))
        })
    });
    let ring = ui.data_mut(|d| {
        let on = d.get_temp::<bool>(keyboard_id()).unwrap_or(false) || keyed;
        d.insert_temp(keyboard_id(), on);
        d.insert_temp::<Option<Id>>(ring_id(), None);
        on
    });
    let rows: Vec<f32> = ui
        .data(|d| d.get_temp::<Vec<f32>>(rows_id()))
        .unwrap_or_default();
    for (i, k) in keys.iter_mut().enumerate() {
        k.ring = ring;
        k.min_height = rows.get(i / COLUMNS).copied().unwrap_or(0.0);
    }
    let Some(at) = ui.memory(|m| ids.iter().position(|id| m.has_focus(*id))) else {
        return keys;
    };
    let (arrow, press, menu) = ui.input_mut(|i| {
        let arrow = [
            Key::ArrowLeft,
            Key::ArrowRight,
            Key::ArrowUp,
            Key::ArrowDown,
        ]
        .into_iter()
        .find(|k| i.consume_key(Modifiers::NONE, *k));
        let press = i.consume_key(Modifiers::NONE, Key::Space);
        let menu = i.consume_key(Modifiers::SHIFT, Key::F10);
        (arrow, press, menu)
    });
    if let Some(k) = arrow {
        let to = arrow_target(at, ids.len(), k);
        ui.memory_mut(|m| {
            m.request_focus(ids[to]);
            m.move_focus(FocusDirection::None);
        });
    }
    keys[at].press = press;
    keys[at].menu = menu;
    keys
}

/// Each grid row's height (its tallest tile's content), kept between frames.
fn rows_id() -> Id {
    Id::new("hideout.tray.rows")
}

/// The Menu key (the context-menu key) on the focused tile. egui has no key for it, so the shell
/// reports it from the window's `WM_CONTEXTMENU` and this queues it as Shift+F10.
pub fn menu_key(ctx: &egui::Context) {
    ctx.input_mut(|i| {
        for pressed in [true, false] {
            i.events.push(egui::Event::Key {
                key: Key::F10,
                physical_key: None,
                pressed,
                repeat: false,
                modifiers: Modifiers::SHIFT,
            });
        }
    });
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
    let keys = tile_keys(ui, &shown);
    let mut picks = Vec::new();
    egui::Grid::new("hideout.tray.tiles")
        .num_columns(COLUMNS)
        .spacing([8.0, 8.0])
        .show(ui, |ui| {
            for (i, t) in shown.iter().enumerate() {
                let r = tile_ui_keyed(host, ui, &t.key, now, keys[i]);
                picks.push((t.key.clone(), r));
                if (i + 1) % COLUMNS == 0 {
                    ui.end_row();
                }
            }
        });
    // Equal heights per row: the tallest tile's content this frame; a change draws once more.
    let rows: Vec<f32> = picks
        .chunks(COLUMNS)
        .map(|row| {
            row.iter()
                .map(|(_, r)| r.content_height)
                .fold(0.0, f32::max)
        })
        .collect();
    let before = ui.data(|d| d.get_temp::<Vec<f32>>(rows_id()));
    if before.as_ref() != Some(&rows) {
        ui.data_mut(|d| d.insert_temp(rows_id(), rows));
        ui.ctx().request_repaint();
    }
    for (key, r) in picks {
        report.phases.extend(r.phase.clone());
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
