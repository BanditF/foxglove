// SPDX-License-Identifier: MIT
//! The hub window: sidebar, page, status strip and the Ctrl+K palette, with the tray, the single
//! instance, placement and theme wired up. `run` is the hub's whole start-up.

use crate::icon::State;
use crate::logfile::FileLog;
use crate::palette::{Entry, Palette};
use crate::paths::HubPaths;
use crate::prefs::{Prefs, ThemeChoice};
use crate::status::{self, Item};
use crate::strip::{self, Action, Shortcut, StripState};
use crate::tabs::{Look, Mark, TabId, TabState, Tabs};
use crate::theme::{self, Rgb, Tokens};
use crate::{placement, platform, tray, window_title, AUMID, PRODUCT};
use eframe::egui::{self, Color32, Key, Modifiers};
use hideout_panels::Slot;
use hideout_panels::{PanelFeed, PanelHost, PanelKey, PanelState};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// A page in the sidebar. The hub binary supplies them; the shell names none (contract §2.2).
pub trait Page {
    /// A stable id, used by the palette and by pop-outs later.
    fn id(&self) -> &str;
    /// The sidebar label and the window title's page name.
    fn title(&self) -> &str;
    fn ui(&mut self, ui: &mut egui::Ui, tokens: &Tokens);
}

pub struct ShellOptions {
    pub paths: HubPaths,
    pub pages: Vec<Box<dyn Page>>,
    /// The hub's version, for the log.
    pub version: &'static str,
    /// Where module panels come from (the router implements it, H2.3). `page` panels join the
    /// sidebar after `pages` and draw in the page area.
    pub panels: Option<Box<dyn PanelFeed>>,
    /// Pop-out windows and module restarts (H2.11 and the supervisor). Without it, a popped-out
    /// tab shows its placeholder in the page area with Dock back.
    pub hooks: Option<Box<dyn TabHooks>>,
}

/// What the tab strip asks of the rest of the hub. Every method has a do-nothing default.
pub trait TabHooks {
    /// Open the tab's page in its own `Hideout · <title>` window (§2.5).
    fn pop_out(&mut self, _tab: &TabId, _title: &str) {}
    /// Close that window: the page is back in its tab.
    fn dock_back(&mut self, _tab: &TabId) {}
    /// "Start again" on a stopped module (§7.8).
    fn start_again(&mut self, _module: &str) {}
}

/// State the tray and the window both touch.
struct Shared {
    paths: HubPaths,
    prefs: Mutex<Prefs>,
    paused: AtomicBool,
    /// The window is closed to the tray: every tab reads `hidden` (§5.9).
    hidden: AtomicBool,
}

impl Shared {
    /// Remembers the window's place on its monitor and writes `hideout.toml`.
    fn save_placement(&self) {
        let Some(rect) = platform::main_window_rect() else {
            return;
        };
        let Ok(mut p) = self.prefs.lock() else { return };
        placement::remember(&mut p.window, rect, &platform::monitors());
        if let Err(e) = p.save(&self.paths.prefs_file()) {
            log::warn!("saving {}: {e}", self.paths.prefs_file().display());
        }
    }

    fn quit(&self) -> ! {
        self.save_placement();
        log::info!("quit from the tray");
        tray::remove();
        log::logger().flush();
        std::process::exit(0)
    }
}

/// Starts the hub and returns when it exits. A second hub on the same folder only brings the
/// first one's window up and returns `Ok`.
pub fn run(opts: ShellOptions) -> Result<(), String> {
    platform::init_process(AUMID);
    let paths = opts.paths;
    // The hub folder is current-user-only (+ SYSTEM, Administrators), contract §2.3: made so on
    // creation, fixed on start if an older run left it open. Logged once the log is open.
    let acl = hideout_acl::create_private_dir_all(&paths.root)
        .and_then(|()| hideout_acl::ensure_private_dir(&paths.root));
    match FileLog::open(&paths.logs_dir(), log::LevelFilter::Info) {
        Ok(l) => {
            let _ = l.install();
        }
        Err(e) => eprintln!("hideout: log {}: {e}", paths.logs_dir().display()),
    }
    match acl {
        Ok(true) => log::info!(
            "folder {} had open or inherited ACL entries; now current user, SYSTEM and Administrators only",
            paths.root.display()
        ),
        Ok(false) => {}
        Err(e) => log::warn!("folder {}: could not set the ACL: {e}", paths.root.display()),
    }
    let instance =
        match platform::claim_instance(&paths.instance_suffix()).map_err(|e| e.to_string())? {
            platform::Claim::Second => {
                log::info!(
                    "another hub runs on {}; asked it to show its window",
                    paths.root.display()
                );
                return Ok(());
            }
            platform::Claim::First(i, show) => (i, show),
        };
    log::info!(
        "{PRODUCT} {} started, pid {}, folder {}",
        opts.version,
        std::process::id(),
        paths.root.display()
    );
    let (prefs, err) = Prefs::load(&paths.prefs_file());
    if let Some(e) = err {
        log::warn!("preferences ignored (file kept as it is): {e}");
    }
    let shared = Arc::new(Shared {
        paths,
        prefs: Mutex::new(prefs),
        paused: AtomicBool::new(false),
        hidden: AtomicBool::new(false),
    });
    let pages = opts.pages;
    let feed = opts.panels;
    let hooks = opts.hooks;
    let first_title = pages
        .first()
        .map(|p| p.title().to_string())
        .unwrap_or_default();
    let glyph = crate::icon::rgba(64, State::Normal);
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(window_title(&first_title))
            .with_inner_size(placement::DEFAULT_SIZE)
            .with_min_inner_size(placement::MIN_SIZE)
            .with_icon(egui::IconData {
                rgba: glyph,
                width: 64,
                height: 64,
            }),
        ..Default::default()
    };
    let (_instance, show) = instance;
    // eframe's app on the hub's own loop: `wake::Gate` keeps a hidden window from polling.
    let event_loop = winit::event_loop::EventLoop::<eframe::UserEvent>::with_user_event()
        .build()
        .map_err(|e| e.to_string())?;
    let app = eframe::create_native(
        PRODUCT,
        options,
        Box::new(move |cc| {
            use raw_window_handle::{HasWindowHandle, RawWindowHandle};
            if let Ok(h) = cc.window_handle() {
                if let RawWindowHandle::Win32(w) = h.as_raw() {
                    platform::set_main_window(w.hwnd.get());
                }
            }
            let ctx = cc.egui_ctx.clone();
            if let Ok(p) = shared.prefs.lock() {
                if let Some((rect, _)) = placement::restore(&p.window, &platform::monitors()) {
                    platform::place_main_window(rect);
                }
                set_theme(&ctx, p.theme);
            }
            install_fonts(&ctx);
            let tray_shared = shared.clone();
            let tray_ctx = ctx.clone();
            // tray::install logs "tray icon registered" or "tray icon failed: <err>".
            let _ = tray::install(move |cmd| on_tray(cmd, &tray_shared, &tray_ctx));
            let show_ctx = ctx.clone();
            let show_shared = shared.clone();
            show.listen(move || {
                show_shared.hidden.store(false, Ordering::Release);
                platform::show_main_window();
                show_ctx.request_repaint();
            });
            let mut feed = feed;
            if let Some(f) = feed.as_mut() {
                let wake_ctx = ctx.clone();
                f.set_waker(Box::new(move || wake_ctx.request_repaint()));
            }
            Ok(Box::new(Hub::new(shared, pages, feed, hooks)))
        }),
        &event_loop,
    );
    let mut gate = crate::wake::Gate::new(app, platform::main_window_drawable);
    let result = event_loop.run_app(&mut gate);
    log::info!("window closed: {result:?}");
    log::logger().flush();
    result.map_err(|e| e.to_string())
}

fn on_tray(cmd: tray::Command, shared: &Shared, ctx: &egui::Context) {
    match cmd {
        tray::Command::Open => {
            shared.hidden.store(false, Ordering::Release);
            platform::show_main_window()
        }
        tray::Command::Pause(on) => {
            shared.paused.store(on, Ordering::Release);
            log::info!("{}", if on { "paused" } else { "resumed" });
            let (state, line) = if on {
                (State::Paused, "Paused")
            } else {
                (State::Normal, "No modules yet")
            };
            tray::set_state(state, line);
        }
        tray::Command::OpenLogs => {
            let dir = shared.paths.logs_dir();
            if let Err(e) = std::process::Command::new("explorer.exe").arg(&dir).spawn() {
                log::warn!("open {}: {e}", dir.display());
            }
        }
        tray::Command::Quit => shared.quit(),
    }
    // A hidden or minimised window draws nothing: it repaints when Windows shows it again (#935).
    if platform::main_window_on_screen() {
        ctx.request_repaint();
    }
}

fn set_theme(ctx: &egui::Context, choice: ThemeChoice) {
    ctx.set_visuals_of(egui::Theme::Light, visuals(&theme::LIGHT));
    ctx.set_visuals_of(egui::Theme::Dark, visuals(&theme::DARK));
    ctx.set_theme(match choice {
        ThemeChoice::System => egui::ThemePreference::System,
        ThemeChoice::Light => egui::ThemePreference::Light,
        ThemeChoice::Dark => egui::ThemePreference::Dark,
    });
}

fn c(rgb: Rgb) -> Color32 {
    Color32::from_rgb(rgb.0, rgb.1, rgb.2)
}

fn visuals(t: &Tokens) -> egui::Visuals {
    let mut v = if t.dark {
        egui::Visuals::dark()
    } else {
        egui::Visuals::light()
    };
    v.panel_fill = c(t.bg);
    v.window_fill = c(t.surface);
    v.extreme_bg_color = c(t.surface);
    v.faint_bg_color = c(t.chrome);
    v.window_stroke = egui::Stroke::new(1.0_f32, c(t.border));
    v.hyperlink_color = c(t.accent);
    v.error_fg_color = c(t.danger);
    v.selection.bg_fill = c(t.accent);
    v.selection.stroke = egui::Stroke::new(1.0_f32, c(t.on_accent));
    v.widgets.noninteractive.fg_stroke = egui::Stroke::new(1.0_f32, c(t.text));
    v.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0_f32, c(t.border));
    v.widgets.inactive.fg_stroke = egui::Stroke::new(1.0_f32, c(t.text));
    v.widgets.hovered.fg_stroke = egui::Stroke::new(1.0_f32, c(t.text));
    v.widgets.active.fg_stroke = egui::Stroke::new(1.0_f32, c(t.text));
    v
}

/// Windows' own UI fonts: the hub ships none (egui's bundled fonts aren't MIT or Apache-2.0).
/// The files are mapped read-only, not read: their pages are shared with every other process
/// using them and never count as the hub's private memory. Owned bytes would count twice, as
/// epaint copies an owned font when it parses it (#935).
fn install_fonts(ctx: &egui::Context) {
    let dir = std::env::var_os("WINDIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| "C:\\Windows".into())
        .join("Fonts");
    let mut fonts = egui::FontDefinitions::empty();
    let mut add = |name: &str, files: &[&str], family: egui::FontFamily| {
        for f in files {
            if let Some(bytes) = platform::map_file(&dir.join(f)) {
                fonts.font_data.insert(
                    name.to_owned(),
                    Arc::new(egui::FontData::from_static(bytes)),
                );
                fonts
                    .families
                    .entry(family)
                    .or_default()
                    .push(name.to_owned());
                return true;
            }
        }
        false
    };
    let ui = add(
        "ui",
        &["segoeui.ttf", "arial.ttf"],
        egui::FontFamily::Proportional,
    );
    let mono = add(
        "mono",
        &["consola.ttf", "cour.ttf"],
        egui::FontFamily::Monospace,
    );
    // Symbols (arrows, the middle dot) for both families.
    if add("symbols", &["seguisym.ttf"], egui::FontFamily::Proportional) {
        fonts
            .families
            .entry(egui::FontFamily::Monospace)
            .or_default()
            .push("symbols".into());
    }
    if !mono && ui {
        fonts
            .families
            .insert(egui::FontFamily::Monospace, vec!["ui".into()]);
    }
    if !ui && mono {
        fonts
            .families
            .insert(egui::FontFamily::Proportional, vec!["mono".into()]);
    }
    if !ui && !mono {
        log::error!("no UI font found in {}", dir.display());
        return;
    }
    ctx.set_fonts(fonts);
}

/// What a tab shows in the page area.
enum Target {
    Page(usize),
    Panel(PanelKey),
}

struct Hub {
    shared: Arc<Shared>,
    pages: Vec<Box<dyn Page>>,
    tabs: Tabs,
    strip: StripState,
    /// "Close <Title>?" is open for these tabs.
    asking: Option<Vec<TabId>>,
    panels: PanelHost,
    feed: Option<Box<dyn PanelFeed>>,
    hooks: Option<Box<dyn TabHooks>>,
    title: String,
    palette: Palette,
    /// From manifests and the registry (H2.2); empty until modules exist.
    entries: Vec<Entry>,
}

fn key_of(id: &TabId) -> Option<PanelKey> {
    match id {
        TabId::Panel { module, panel } => Some(PanelKey::new(module, panel)),
        TabId::Page { .. } => None,
    }
}

impl Hub {
    fn new(
        shared: Arc<Shared>,
        pages: Vec<Box<dyn Page>>,
        feed: Option<Box<dyn PanelFeed>>,
        hooks: Option<Box<dyn TabHooks>>,
    ) -> Self {
        let ids: Vec<String> = pages.iter().map(|p| p.id().to_string()).collect();
        // Hub pages that are gone are dropped; module tabs wait for their module ("Starting").
        let exists = |id: &TabId| match id {
            TabId::Page { page } => ids.contains(page),
            TabId::Panel { .. } => true,
        };
        let file = shared.paths.tabs_file();
        let tabs = match Tabs::load(&file, exists) {
            Ok(Some(t)) => t,
            Ok(None) => {
                // First start: the first page is open.
                let mut t = Tabs::default();
                if let Some(first) = ids.first() {
                    t.open(TabId::page(first));
                }
                t
            }
            Err(e) => {
                log::warn!("tabs ignored (file kept as it is): {e}");
                Tabs::default()
            }
        };
        Self {
            shared,
            pages,
            tabs,
            strip: StripState::default(),
            asking: None,
            panels: PanelHost::default(),
            feed,
            hooks,
            title: String::new(),
            palette: Palette::default(),
            entries: Vec::new(),
        }
    }

    fn target(&self, id: &TabId) -> Option<Target> {
        match id {
            TabId::Page { page } => self
                .pages
                .iter()
                .position(|p| p.id() == page)
                .map(Target::Page),
            TabId::Panel { .. } => key_of(id).map(Target::Panel),
        }
    }

    fn look(&self, i: usize) -> Look {
        let tab = &self.tabs.tabs()[i];
        match self.target(&tab.id) {
            Some(Target::Page(p)) => Look {
                title: self.pages[p].title().to_string(),
                state: TabState::Live,
                mark: Mark::None,
                popped: tab.popped,
            },
            Some(Target::Panel(key)) => match self.panels.panel(&key) {
                Some(p) => strip::panel_look(p, tab.popped),
                // Restored before its module registered again (§2.4): "Starting".
                None => Look {
                    title: tab.title.clone(),
                    state: TabState::Busy(hideout_panels::state_words("starting").into()),
                    mark: Mark::None,
                    popped: tab.popped,
                },
            },
            None => Look {
                title: tab.title.clone(),
                state: TabState::Live,
                mark: Mark::None,
                popped: tab.popped,
            },
        }
    }

    fn dirty(&self, id: &TabId) -> bool {
        key_of(id)
            .and_then(|k| self.panels.panel(&k))
            .is_some_and(|p| p.badge == hideout_panels::Badge::Dirty)
    }

    /// Buttons with `opens` (v1.4): the module's own page as a tab, scrolled to the node or to
    /// its Settings section. `action` buttons are already queued as `action.invoke`.
    fn go(&mut self, go: Vec<hideout_panels::Go>) {
        for g in go {
            if let hideout_panels::Go::Open { module, target } = g {
                if let Some(page) = self.panels.navigate(&module, &target) {
                    self.open(TabId::panel(&page.module, &page.panel));
                }
            }
        }
    }

    fn open(&mut self, id: TabId) {
        if let Some(k) = key_of(&id) {
            self.panels.reopen_panel(&k);
        }
        self.tabs.open(id);
    }

    fn closed(&mut self, ids: &[TabId]) {
        for id in ids {
            if let Some(k) = key_of(id) {
                self.panels.close_panel(&k);
            }
            let popped = self
                .tabs
                .position(id)
                .is_some_and(|i| self.tabs.tabs()[i].popped);
            if popped {
                if let Some(h) = self.hooks.as_mut() {
                    h.dock_back(id);
                }
            }
        }
        self.tabs.close_now(ids);
    }

    fn apply(&mut self, a: Action) {
        match a {
            Action::Activate(i) => self.tabs.activate(i),
            Action::Close(i) => {
                let Some(id) = self.tabs.tabs().get(i).map(|t| t.id.clone()) else {
                    return;
                };
                if self.dirty(&id) {
                    self.asking = Some(vec![id]);
                } else {
                    self.closed(&[id]);
                }
            }
            Action::CloseOthers(i) | Action::CloseRight(i) => {
                let right = matches!(a, Action::CloseRight(_));
                let ids: Vec<TabId> = self
                    .tabs
                    .tabs()
                    .iter()
                    .enumerate()
                    .filter(|(j, _)| if right { *j > i } else { *j != i })
                    .map(|(_, t)| t.id.clone())
                    .collect();
                self.tabs.activate(i);
                if ids.iter().any(|id| self.dirty(id)) {
                    self.asking = Some(ids);
                } else {
                    self.closed(&ids);
                }
            }
            Action::Move { from, to } => self.tabs.move_tab(from, to),
            Action::PopOut(i) => {
                let title = self.look(i).title;
                self.tabs.pop_out(i);
                if let (Some(h), Some(t)) = (self.hooks.as_mut(), self.tabs.tabs().get(i)) {
                    h.pop_out(&t.id, &title);
                }
            }
            Action::DockBack(i) => {
                if let (Some(h), Some(t)) = (self.hooks.as_mut(), self.tabs.tabs().get(i)) {
                    if t.popped {
                        h.dock_back(&t.id);
                    }
                }
                self.tabs.dock_back(i);
            }
            Action::Reopen => {
                let ids: Vec<String> = self.pages.iter().map(|p| p.id().to_string()).collect();
                let panels = &self.panels;
                let reopened = self.tabs.reopen_closed(|id| match id {
                    TabId::Page { page } => ids.contains(page),
                    TabId::Panel { .. } => key_of(id).is_some_and(|k| panels.panel(&k).is_some()),
                });
                if let Some(k) = reopened.as_ref().and_then(key_of) {
                    self.panels.reopen_panel(&k);
                }
            }
        }
    }

    fn shortcut(&mut self, s: Shortcut) {
        match s {
            Shortcut::Next => self.tabs.cycle(1),
            Shortcut::Prev => self.tabs.cycle(-1),
            Shortcut::Number(n) => self.tabs.select_number(n),
            Shortcut::Close => {
                if let Some(i) = self.tabs.active() {
                    self.apply(Action::Close(i));
                }
            }
            Shortcut::Reopen => self.apply(Action::Reopen),
            Shortcut::MoveLeft | Shortcut::MoveRight => {
                if let Some(i) = self.tabs.active() {
                    let to = if s == Shortcut::MoveLeft {
                        i.checked_sub(1)
                    } else {
                        Some(i + 1).filter(|&j| j < self.tabs.len())
                    };
                    if let Some(to) = to {
                        self.tabs.move_tab(i, to);
                    }
                }
            }
        }
    }

    fn status_items(&self) -> Vec<Item> {
        let mut items = Vec::new();
        if self.shared.paused.load(Ordering::Acquire) {
            items.push(Item::new("hub.paused", "Paused"));
        }
        items
    }

    fn sidebar(&mut self, ctx: &egui::Context, t: &Tokens) {
        egui::SidePanel::left("hideout.sidebar")
            .resizable(false)
            .exact_width(200.0)
            .frame(
                egui::Frame::new()
                    .fill(c(t.chrome))
                    .inner_margin(egui::Margin::symmetric(10, 14)),
            )
            .show(ctx, |ui| {
                ui.label(
                    egui::RichText::new(PRODUCT)
                        .size(20.0)
                        .strong()
                        .color(c(t.text)),
                );
                ui.add_space(14.0);
                let mut entries: Vec<(TabId, String, bool)> = self
                    .pages
                    .iter()
                    .map(|p| (TabId::page(p.id()), p.title().to_string(), false))
                    .collect();
                entries.extend(self.panels.in_slot(Slot::Page).map(|p| {
                    (
                        TabId::panel(&p.key.module, &p.key.panel),
                        p.title.clone(),
                        p.badge == hideout_panels::Badge::Attention,
                    )
                }));
                let active = self.tabs.active_id().cloned();
                for (target, title, attention) in entries {
                    let selected = active.as_ref() == Some(&target);
                    let text = egui::RichText::new(title)
                        .size(15.0)
                        .color(c(theme::nav_label(t, selected)));
                    let resp = ui.add_sized(
                        [ui.available_width(), 32.0],
                        egui::Button::selectable(selected, text),
                    );
                    if attention {
                        // The attention dot on the sidebar entry too; the tab's accessible name
                        // says "something new", so colour is not the only signal.
                        let at = egui::pos2(resp.rect.right() - 12.0, resp.rect.center().y);
                        let fill = if selected { t.on_accent } else { t.accent };
                        ui.painter().circle_filled(at, 4.0, c(fill));
                    }
                    if resp.clicked() {
                        self.open(target);
                    }
                }
            });
    }

    fn strip_bar(&mut self, ctx: &egui::Context, t: &Tokens) {
        for i in 0..self.tabs.len() {
            let title = self.look(i).title;
            self.tabs.set_title(i, &title);
        }
        let looks: Vec<Look> = (0..self.tabs.len()).map(|i| self.look(i)).collect();
        let mut actions = Vec::new();
        egui::TopBottomPanel::top("hideout.strip")
            .exact_height(strip::STRIP_H)
            .show_separator_line(false)
            .frame(egui::Frame::new().fill(c(t.chrome)))
            .show(ctx, |ui| {
                actions = strip::strip_ui(ui, &self.tabs, &looks, t, &mut self.strip);
            });
        for a in actions {
            self.apply(a);
        }
    }

    fn status_strip(&self, ctx: &egui::Context, t: &Tokens) {
        let items = self.status_items();
        egui::TopBottomPanel::bottom("hideout.status")
            .exact_height(28.0)
            .frame(
                egui::Frame::new()
                    .fill(c(t.chrome))
                    .inner_margin(egui::Margin::symmetric(12, 4)),
            )
            .show(ctx, |ui| {
                ui.horizontal_centered(|ui| {
                    let shown = status::visible(&items, &[]);
                    if shown.is_empty() {
                        ui.label(egui::RichText::new("Ready").color(c(t.text_muted)));
                    }
                    for (i, item) in shown.iter().enumerate() {
                        if i > 0 {
                            ui.label(egui::RichText::new("\u{b7}").color(c(t.text_muted)));
                        }
                        let col = if item.problem { c(t.danger) } else { c(t.text) };
                        ui.label(egui::RichText::new(&item.text).color(col));
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(egui::RichText::new("Ctrl+K  Commands").color(c(t.text_muted)));
                    });
                });
            });
    }

    /// The page area: the active tab's page, its placeholder when popped out, the stopped card,
    /// or "Nothing open".
    fn page_area(&mut self, ui: &mut egui::Ui, t: &Tokens) {
        let Some(i) = self.tabs.active() else {
            ui.vertical_centered(|ui| {
                ui.add_space(ui.available_height() / 3.0);
                ui.label(
                    egui::RichText::new("Nothing open")
                        .size(20.0)
                        .color(c(t.text)),
                );
                ui.add_space(6.0);
                ui.label(egui::RichText::new("Pick a page in the sidebar.").color(c(t.text_muted)));
                ui.add_space(12.0);
                ui.label(
                    egui::RichText::new("Ctrl+K  Commands     Ctrl+Shift+T  Reopen closed tab")
                        .color(c(t.text_muted)),
                );
            });
            return;
        };
        let look = self.look(i);
        let id = self.tabs.tabs()[i].id.clone();
        // The page area is the tab panel, named by its tab.
        let name = look.accessible_name();
        ui.ctx().accesskit_node_builder(ui.unique_id(), |n| {
            n.set_role(egui::accesskit::Role::TabPanel);
            n.set_label(name);
        });
        if look.popped {
            let mut dock = false;
            card(ui, t, |ui| {
                ui.label(
                    egui::RichText::new(format!("{} is in its own window", look.title))
                        .size(16.0)
                        .color(c(t.text)),
                );
                ui.add_space(8.0);
                dock = ui.button("Dock back").clicked();
            });
            if dock {
                self.apply(Action::DockBack(i));
            }
            return;
        }
        match self.target(&id) {
            Some(Target::Page(p)) => self.pages[p].ui(ui, t),
            Some(Target::Panel(key)) => {
                match &look.state {
                    TabState::Stopped(words) => {
                        let panel = self.panels.panel(&key);
                        let details = panel.and_then(|p| p.state.details()).unwrap_or_default();
                        let module_name = panel
                            .map(|p| p.module_name.clone())
                            .unwrap_or_else(|| look.title.clone());
                        let quarantined = panel.map(|p| &p.state) == Some(&PanelState::Quarantined);
                        let mut again = false;
                        card(ui, t, |ui| {
                            // The panel host's banner below names the state; the card holds
                            // the ways out.
                            ui.label(
                                egui::RichText::new(format!("{module_name} isn't running"))
                                    .size(16.0)
                                    .color(c(t.text)),
                            );
                            ui.label(
                                egui::RichText::new(format!(
                                    "{words}. The last page it drew stays below."
                                ))
                                .color(c(t.text_muted)),
                            );
                            ui.add_space(8.0);
                            ui.horizontal(|ui| {
                                again = ui.button("Start again").clicked();
                                if ui.button("Copy diagnostics").clicked() {
                                    ui.ctx().copy_text(details.clone());
                                }
                            });
                        });
                        if again {
                            if quarantined {
                                self.panels.release(&key.module);
                            }
                            if let Some(h) = self.hooks.as_mut() {
                                h.start_again(&key.module);
                            }
                        }
                        ui.add_space(12.0);
                    }
                    TabState::Busy(words) => {
                        ui.label(
                            egui::RichText::new(format!(
                                "{words}\u{2026} Your place on this page is kept."
                            ))
                            .color(c(t.text_muted)),
                        );
                        ui.add_space(8.0);
                    }
                    TabState::Off | TabState::Live => {}
                }
                if self.panels.panel(&key).is_some() {
                    let report = hideout_panels::panel_ui(&mut self.panels, ui, &key);
                    // Keys and Settings, drawn by the hub after the module's tree (4.7).
                    hideout_panels::page_sections_ui(&mut self.panels, ui, &key);
                    self.go(report.go);
                }
            }
            None => {}
        }
    }

    /// "Close <Title>?" for unsaved tabs. Keep open is the default (Enter and Esc).
    fn ask_ui(&mut self, ctx: &egui::Context, t: &Tokens) {
        let Some(ids) = self.asking.clone() else {
            return;
        };
        let unsaved: Vec<String> = ids
            .iter()
            .filter(|id| self.dirty(id))
            .filter_map(|id| self.tabs.position(id).map(|i| self.look(i).title))
            .collect();
        if unsaved.is_empty() {
            self.asking = None;
            self.closed(&ids);
            return;
        }
        let (head, body) = if unsaved.len() == 1 && ids.len() == 1 {
            (
                format!("Close {}?", unsaved[0]),
                format!("Your changes in {} aren't saved.", unsaved[0]),
            )
        } else {
            (
                format!("Close {} tabs?", ids.len()),
                format!("Your changes in {} aren't saved.", unsaved.join(", ")),
            )
        };
        let mut choice = None;
        let modal = egui::Modal::new(egui::Id::new("hideout.ask")).show(ctx, |ui| {
            ui.set_width(360.0);
            ui.label(egui::RichText::new(head).size(16.0).color(c(t.text)));
            ui.add_space(6.0);
            ui.label(egui::RichText::new(body).color(c(t.text)));
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                let keep = ui.button("Keep open");
                if !ui.ctx().memory(|m| m.focused().is_some()) {
                    keep.request_focus();
                }
                if keep.clicked() {
                    choice = Some(false);
                }
                if ui.button("Close anyway").clicked() {
                    choice = Some(true);
                }
            });
        });
        let esc = ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape));
        if modal.should_close() || esc {
            choice.get_or_insert(false);
        }
        match choice {
            Some(true) => {
                self.asking = None;
                self.closed(&ids);
            }
            Some(false) => self.asking = None,
            None => {}
        }
    }

    fn palette_ui(&mut self, ctx: &egui::Context, t: &Tokens) {
        if ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::K)) {
            self.palette.toggle();
        }
        if !self.palette.open {
            return;
        }
        let count = self.palette.matches(&self.entries).len();
        let (up, down, enter, esc) = ctx.input_mut(|i| {
            (
                i.consume_key(Modifiers::NONE, Key::ArrowUp),
                i.consume_key(Modifiers::NONE, Key::ArrowDown),
                i.consume_key(Modifiers::NONE, Key::Enter),
                i.consume_key(Modifiers::NONE, Key::Escape),
            )
        });
        if up {
            self.palette.step(-1, count);
        }
        if down {
            self.palette.step(1, count);
        }
        if enter {
            if let Some(target) = self.palette.chosen(&self.entries).map(|e| e.target.clone()) {
                if self.pages.iter().any(|p| p.id() == target) {
                    self.open(TabId::page(target));
                }
                self.palette.close();
                return;
            }
        }
        let modal = egui::Modal::new(egui::Id::new("hideout.palette")).show(ctx, |ui| {
            ui.set_width(480.0);
            let edit = ui.add(
                egui::TextEdit::singleline(&mut self.palette.query)
                    .hint_text("Type a command")
                    .desired_width(f32::INFINITY),
            );
            edit.request_focus();
            if edit.changed() {
                self.palette.selected = 0;
            }
            ui.add_space(8.0);
            let matches = self.palette.matches(&self.entries);
            if matches.is_empty() {
                let msg = if self.entries.is_empty() {
                    "No commands yet. Modules add their actions and pages here."
                } else {
                    "Nothing matches."
                };
                ui.label(egui::RichText::new(msg).color(c(t.text_muted)));
            }
            for (i, e) in matches.iter().enumerate() {
                let selected = i == self.palette.selected;
                let shown = match &e.unavailable {
                    Some(why) => format!("{}  ({why})", e.label),
                    None => e.label.clone(),
                };
                let text = egui::RichText::new(shown).color(c(theme::palette_label(
                    t,
                    selected,
                    e.unavailable.is_some(),
                )));
                ui.add(egui::Button::selectable(selected, text));
            }
        });
        if esc || modal.should_close() {
            self.palette.close();
        }
    }

    /// The panels on screen now, for `ui.visibility`: none when the window is minimised or in
    /// the tray, else the active tab, plus pop-outs that are shown.
    fn on_screen(&self, ctx: &egui::Context) -> Vec<PanelKey> {
        let minimised = ctx.input(|i| i.viewport().minimized == Some(true));
        let shown = !minimised && !self.shared.hidden.load(Ordering::Acquire);
        self.tabs
            .on_screen(shown)
            .into_iter()
            .filter_map(key_of)
            .collect()
    }

    /// One frame of the hub window, without the platform calls (tests draw it headless).
    fn frame(&mut self, ctx: &egui::Context) {
        let t = theme::tokens(ctx.theme() == egui::Theme::Dark);
        if let Some(feed) = self.feed.as_mut() {
            let now = std::time::Instant::now();
            // Event-driven: the feed's waker brought this frame; a held-back patch asks for one
            // more repaint when its slot comes (no timer loop).
            let (_, next) = self.panels.pump(feed.as_mut(), now);
            if let Some(at) = next {
                ctx.request_repaint_after(at.saturating_duration_since(now));
            }
        }
        if self.asking.is_none() && !self.palette.open {
            for s in strip::shortcuts(ctx) {
                self.shortcut(s);
            }
        }
        self.sidebar(ctx, &t);
        self.status_strip(ctx, &t);
        self.strip_bar(ctx, &t);
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(c(t.bg))
                    .inner_margin(egui::Margin::same(24)),
            )
            .show(ctx, |ui| self.page_area(ui, &t));
        self.ask_ui(ctx, &t);
        self.palette_ui(ctx, &t);
        // Input from the panel and a change of what is on screen (ui.visibility, 5.9) go out on
        // this frame, not the next one.
        let shown = self.on_screen(ctx);
        self.panels.set_visible(&shown);
        if let Some(feed) = self.feed.as_mut() {
            self.panels.send_out(feed.as_mut());
        }
        if self.tabs.take_changed() {
            let f = self.shared.paths.tabs_file();
            if let Err(e) = self.tabs.save(&f) {
                log::warn!("saving {}: {e}", f.display());
            }
        }
        let title = window_title(
            &self
                .tabs
                .active()
                .map(|i| self.look(i).title)
                .unwrap_or_default(),
        );
        if title != self.title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.title = title;
        }
    }
}

/// A card on the page: surface fill, border, padding.
fn card(ui: &mut egui::Ui, t: &Tokens, add: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new()
        .fill(c(t.surface))
        .stroke(egui::Stroke::new(1.0_f32, c(t.border)))
        .corner_radius(egui::CornerRadius::same(6))
        .inner_margin(egui::Margin::same(16))
        .show(ui, add);
}

impl eframe::App for Hub {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Closing the window keeps the hub in the tray; Quit is in the tray menu.
        if ctx.input(|i| i.viewport().close_requested()) {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.shared.save_placement();
            self.shared.hidden.store(true, Ordering::Release);
            platform::hide_main_window();
            log::info!("window closed to the tray");
        }
        self.frame(ctx);
    }
}

#[cfg(test)]
#[path = "shell_tests.rs"]
mod tests;
