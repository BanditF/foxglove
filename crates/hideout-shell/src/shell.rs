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
use crate::{flyout, placement, platform, popout, tray, window_title, AUMID, PRODUCT};
use eframe::egui::{self, Color32, Key, Modifiers};
use hideout_contract::config::{PageWindow, WindowState};
use hideout_panels::Slot;
use hideout_panels::{PanelFeed, PanelHost, PanelKey, PanelState, TrayAction, TrayLayout};
use hideout_supervisor::words::{self, Recovery};
use std::collections::{BTreeMap, HashMap};
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
    /// Module restarts (the supervisor), and word of pop-outs opening and docking (H2.11).
    pub hooks: Option<Box<dyn TabHooks>>,
    /// Started in the tray (sign-in): the main window hides after its first frame, and saved
    /// pop-outs wait for the first time the user opens Hideout (§2.5, spec v1.6).
    pub start_in_tray: bool,
}

/// What the tab strip asks of the rest of the hub. Every method has a do-nothing default.
pub trait TabHooks {
    /// The tab's page opened in its own `Hideout · <title>` window (§2.5); the shell draws it.
    fn pop_out(&mut self, _tab: &TabId, _title: &str) {}
    /// That window closed: the page is back in its tab (or closed with it).
    fn dock_back(&mut self, _tab: &TabId) {}
    /// "Start again" on a stopped module (§7.8).
    fn start_again(&mut self, _module: &str) {}
    /// "Start with default settings" (the recovery ladder's second step).
    fn start_with_defaults(&mut self, _module: &str) {}
    /// "Go back to <previous>" (the third step).
    fn go_back(&mut self, _module: &str) {}
    /// "Turn off <Module>" (the last step).
    fn turn_off(&mut self, _module: &str) {}
}

/// Modules to open in the window, from "Open <Module>" on a notification (any thread).
static OPEN_REQUESTS: Mutex<Vec<String>> = Mutex::new(Vec::new());
/// Wakes the window for them.
static OPEN_WAKER: Mutex<Option<Box<dyn Fn() + Send>>> = Mutex::new(None);

/// Shows the hub window with the module's page (a notification's "Open <Module>"). Any thread.
pub fn open_module(module: &str) {
    OPEN_REQUESTS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push(module.to_string());
    platform::show_main_window();
    if let Some(w) = OPEN_WAKER
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
    {
        w();
    }
}

/// Applies a pending tray change on the UI thread (`wake::Gate` runs it on every wake).
fn tray_sync() {
    if let Some((state, line, tip)) = crate::marks::take_change() {
        tray::set_state(state, &line, &tip);
    }
}

/// When the crash bar's countdown next needs a frame: once a second while it counts, never at
/// "Restarting…".
fn countdown_repaint(in_s: u64) -> Option<std::time::Duration> {
    (in_s > 0).then_some(std::time::Duration::from_secs(1))
}

/// State the tray and the window both touch.
struct Shared {
    paths: HubPaths,
    prefs: Mutex<Prefs>,
    paused: AtomicBool,
    /// The window is closed to the tray: every tab reads `hidden` (§5.9).
    hidden: AtomicBool,
    /// A left click on the tray icon (the icon's rectangle and when), for the next frame.
    flyout_click: Mutex<Option<(placement::Rect, std::time::Instant)>>,
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
        flyout_click: Mutex::new(None),
    });
    let pages = opts.pages;
    let feed = opts.panels;
    let hooks = opts.hooks;
    let start_in_tray = opts.start_in_tray;
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
            // Facts that change the tray wake the loop once; `tray_sync` applies them.
            let marks_ctx = ctx.clone();
            crate::marks::set_waker(Box::new(move || marks_ctx.request_repaint()));
            tray_sync();
            let open_ctx = ctx.clone();
            *OPEN_WAKER.lock().unwrap_or_else(|e| e.into_inner()) =
                Some(Box::new(move || open_ctx.request_repaint()));
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
            // A pop-out's move or resize ended, or its window menu was used.
            let pop_ctx = ctx.clone();
            platform::set_popout_waker(Box::new(move || pop_ctx.request_repaint()));
            let mut hub = Hub::new(shared, pages, feed, hooks);
            hub.start_hidden = if start_in_tray { 2 } else { 0 };
            hub.popouts_held = start_in_tray;
            // Choose file for file settings: the Windows picker (tests leave it out).
            hub.panels.picker = Some(hideout_panels::model::Picker(Box::new(platform::pick_file)));
            hub.capture = start_capture(&hub.shared.paths, &ctx);
            Ok(Box::new(hub))
        }),
        &event_loop,
    );
    let mut gate = crate::wake::Gate::new(
        app,
        platform::main_window_drawable,
        side_windows_live,
        platform::main_window_id,
        tray_sync,
    );
    let result = event_loop.run_app(&mut gate);
    log::info!("window closed: {result:?}");
    log::logger().flush();
    result.map_err(|e| e.to_string())
}

/// The lab capture pipe (#1002), only on a lab folder with `HIDEOUT_LAB_CAPTURE=1`.
fn start_capture(
    paths: &crate::paths::HubPaths,
    ctx: &egui::Context,
) -> Option<Arc<crate::capture::Slot>> {
    use crate::capture;
    if !capture::enabled(paths, |k| std::env::var_os(k)) {
        return None;
    }
    let name = capture::pipe_name(paths);
    let slot = Arc::new(capture::Slot::default());
    let wake_ctx = ctx.clone();
    let on_screen = || capture::OnScreen {
        main: platform::main_window_on_screen(),
        flyout: flyout::live(),
        popouts: POPOUTS_SHOWN.load(Ordering::Acquire),
    };
    match capture::serve(&name, slot.clone(), on_screen, move || {
        wake_ctx.request_repaint()
    }) {
        Ok(()) => {
            log::info!("capture: lab capture on, {name}");
            Some(slot)
        }
        Err(e) => {
            log::warn!("capture: {name}: {e}");
            None
        }
    }
}

fn on_tray(cmd: tray::Command, shared: &Shared, ctx: &egui::Context) {
    match cmd {
        tray::Command::Flyout(icon) => {
            // The flyout is drawn by the main window's frame: ask for one even while the window
            // is hidden (`wake::Gate` runs it while the flyout is wanted, #935).
            if let Ok(mut c) = shared.flyout_click.lock() {
                *c = Some((icon, std::time::Instant::now()));
            }
            flyout::set_live(true);
            ctx.request_repaint();
            return;
        }
        tray::Command::Open => {
            shared.hidden.store(false, Ordering::Release);
            platform::show_main_window()
        }
        tray::Command::Pause(on) => {
            shared.paused.store(on, Ordering::Release);
            log::info!("{}", if on { "paused" } else { "resumed" });
            crate::marks::note_paused(on);
            tray_sync();
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
    // Symbols (arrows, the middle dot), then Windows' icon font for the published icons it
    // draws best (the microphone), for both families.
    let symbols = add("symbols", &["seguisym.ttf"], egui::FontFamily::Proportional);
    let icons = add(
        "icons",
        &["SegoeIcons.ttf", "segmdl2.ttf"],
        egui::FontFamily::Proportional,
    );
    for (name, found) in [("symbols", symbols), ("icons", icons)] {
        if found {
            fonts
                .families
                .entry(egui::FontFamily::Monospace)
                .or_default()
                .push(name.into());
        }
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
    /// The tray flyout while it is open.
    flyout: Option<FlyoutState>,
    /// When the flyout last closed because it lost focus (a tray click's toggle).
    flyout_closed_by_focus: Option<std::time::Instant>,
    /// The tray panel's tile layout, saved in `hideout.toml` `[tray]`.
    tray_layout: TrayLayout,
    /// The stopped card whose main button has had the focus (module page, ladder step): the
    /// button takes it once when the card appears.
    card_focus: HashMap<PanelKey, &'static str>,
    /// The page the focus rule last saw: opening another page (and coming back) focuses the
    /// card's main button again.
    card_target: Option<TabId>,
    /// Lab captures (#1002): `None` unless the lab switch is on.
    capture: Option<Arc<crate::capture::Slot>>,
    /// The pop-out windows that are open (H2.11).
    popouts: Vec<Popout>,
    /// `hideout.toml`'s `[windows]`: per page, popped out, Keep on top, places (§2.5).
    windows: BTreeMap<String, PageWindow>,
    /// Started in the tray: pop-outs wait until Hideout is first shown.
    popouts_held: bool,
    /// Started in the tray: frames until the main window hides (eframe shows it after the
    /// first one), 0 once done.
    start_hidden: u8,
    /// The active tab last frame: a placeholder that becomes active brings its window forward.
    last_active: Option<TabId>,
    /// The page area's size (points): a new pop-out's size.
    page_size: egui::Vec2,
    /// Close page with unsaved work: "Close <page>?" shows in that pop-out.
    popout_asking: Option<TabId>,
}

/// The open tray flyout.
struct FlyoutState {
    /// The tray icon it opened from (physical pixels).
    icon: placement::Rect,
    /// Edit tiles is open.
    editing: bool,
    /// It had the focus once: losing it now closes the flyout.
    had_focus: bool,
    /// The height (points) it was last placed with; `None` before it is first shown.
    placed: Option<f32>,
    /// Frames that couldn't find its window yet.
    misses: u8,
    /// When it was first shown (placed), for the focus hand-off check (#1000).
    shown_at: Option<std::time::Instant>,
}

/// The flyout's viewport.
fn flyout_id() -> egui::ViewportId {
    egui::ViewportId::from_hash_of("hideout.tray.flyout")
}

/// The page that Update Hideout opens: the hub has no updater of its own yet.
pub const RELEASES_URL: &str = "https://github.com/BanditF/foxglove/releases";

fn layout_from(p: &crate::prefs::TrayPrefs) -> TrayLayout {
    TrayLayout {
        order: p.order.clone(),
        hidden: p.hidden.iter().cloned().collect(),
        shown: p.shown.iter().cloned().collect(),
    }
}

fn prefs_from(l: &TrayLayout) -> crate::prefs::TrayPrefs {
    crate::prefs::TrayPrefs {
        order: l.order.clone(),
        hidden: l.hidden.iter().cloned().collect(),
        shown: l.shown.iter().cloned().collect(),
    }
}

/// What one flyout frame saw.
struct FlyoutFrame {
    focused: Option<bool>,
    close: bool,
    report: hideout_panels::tray::TrayReport,
    /// The panel's height in points (the grid plus its margins).
    height: f32,
}

/// The flyout's margin, in points.
const FLYOUT_MARGIN: i8 = 14;

/// The flyout's frame: the surface with a border.
fn flyout_frame(t: &Tokens) -> egui::Frame {
    egui::Frame::new()
        .fill(c(t.surface))
        .stroke(egui::Stroke::new(1.0_f32, c(t.border)))
        .inner_margin(egui::Margin::same(FLYOUT_MARGIN))
}

/// The flyout's contents (the tray panel) inside its frame. Returns the report and the height
/// the flyout needs, frame included. Window-free (the review renders it).
fn flyout_panel(
    ui: &mut egui::Ui,
    panels: &mut PanelHost,
    layout: &mut TrayLayout,
    editing: &mut bool,
    t: &Tokens,
) -> (hideout_panels::tray::TrayReport, f32) {
    // Tracks (a meter's bar) are `extreme_bg_color`, the hub's surface: on the flyout's own
    // surface they vanish, so here they take the page colour (the same contrast, reversed).
    ui.visuals_mut().extreme_bg_color = c(t.bg);
    let now = std::time::Instant::now();
    // Measured on a child: a panel's own rect is the whole window.
    let r = ui.vertical(|ui| hideout_panels::tray_panel_ui(panels, ui, layout, editing, now));
    let frame = 2.0 * (FLYOUT_MARGIN as f32 + 1.0);
    (r.inner, r.response.rect.height() + frame)
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
        let (tray_layout, windows) = shared
            .prefs
            .lock()
            .map(|p| {
                let (windows, errors) = popout::load(&p.rest);
                for e in errors {
                    log::warn!("{e}; that page opens at the fallback place");
                }
                (layout_from(&p.tray), windows)
            })
            .unwrap_or_default();
        Self {
            shared,
            pages,
            tray_layout,
            tabs,
            strip: StripState::default(),
            asking: None,
            panels: PanelHost::default(),
            feed,
            hooks,
            title: String::new(),
            palette: Palette::default(),
            entries: Vec::new(),
            flyout: None,
            flyout_closed_by_focus: None,
            card_focus: HashMap::new(),
            card_target: None,
            capture: None,
            popouts: Vec::new(),
            windows,
            popouts_held: false,
            start_hidden: 0,
            last_active: None,
            page_size: egui::vec2(popout::FIRST_MIN.0 as f32, popout::FIRST_MIN.1 as f32),
            popout_asking: None,
        }
    }

    fn close_flyout(&mut self, ctx: &egui::Context, by_focus: bool) {
        if self.flyout.take().is_some() {
            self.panels.set_tray_open(false);
            if by_focus {
                self.flyout_closed_by_focus = Some(std::time::Instant::now());
            }
            // One more frame drops its window (it isn't shown in that frame).
            ctx.request_repaint();
        }
    }

    /// Shows Hideout's own window (from the flyout).
    fn show_main(&self) {
        self.shared.hidden.store(false, Ordering::Release);
        platform::show_main_window();
    }

    fn save_tray_layout(&self) {
        let Ok(mut p) = self.shared.prefs.lock() else {
            return;
        };
        p.tray = prefs_from(&self.tray_layout);
        if let Err(e) = p.save(&self.shared.paths.prefs_file()) {
            log::warn!("saving {}: {e}", self.shared.paths.prefs_file().display());
        }
    }

    /// The tray flyout (#991): opened and closed by tray clicks, drawn as its own window from
    /// this frame, closed on focus loss, Esc, or anything that leaves it.
    fn flyout_ui(
        &mut self,
        ctx: &egui::Context,
        t: &Tokens,
        mut capture: Option<std::sync::mpsc::Sender<crate::capture::Shot>>,
    ) {
        let click = self
            .shared
            .flyout_click
            .lock()
            .ok()
            .and_then(|mut c| c.take());
        if let Some((icon, at)) = click {
            match flyout::on_click(self.flyout.is_some(), self.flyout_closed_by_focus, at) {
                flyout::Click::Open => {
                    self.flyout = Some(FlyoutState {
                        icon,
                        editing: false,
                        had_focus: false,
                        placed: None,
                        misses: 0,
                        shown_at: None,
                    });
                    self.panels.set_tray_open(true);
                }
                flyout::Click::Close => self.close_flyout(ctx, false),
                flyout::Click::Nothing => {}
            }
        }
        let Some(fly) = self.flyout.as_mut() else {
            flyout::set_live(false);
            platform::forget_flyout();
            return;
        };
        flyout::set_live(true);
        let title = window_title("Tray");
        let builder = egui::ViewportBuilder::default()
            .with_title(title.clone())
            .with_decorations(false)
            .with_resizable(false)
            .with_taskbar(false)
            .with_always_on_top()
            // Shown once placed against the taskbar (no flash at a default spot).
            .with_visible(false)
            .with_inner_size([flyout::WIDTH, flyout::FIRST_HEIGHT]);
        let (panels, layout) = (&mut self.panels, &mut self.tray_layout);
        let editing = &mut fly.editing;
        let frame = ctx.show_viewport_immediate(flyout_id(), builder, |ctx, class| {
            let focused = ctx.input(|i| i.viewport().focused);
            let close = ctx.input_mut(|i| {
                i.consume_key(Modifiers::NONE, Key::Escape) || i.viewport().close_requested()
            });
            if let Some(tx) = capture.take() {
                crate::capture::queue_readback(ctx, crate::capture::Window::Flyout, tx);
            }
            let draw = |ui: &mut egui::Ui| flyout_panel(ui, panels, layout, editing, t);
            let (report, height) = if class == egui::ViewportClass::Embedded {
                // No second window (tests, or a backend without viewports): an area on top.
                egui::Area::new(egui::Id::new("hideout.tray.flyout"))
                    .order(egui::Order::Foreground)
                    .show(ctx, |ui| flyout_frame(t).show(ui, draw).inner)
                    .inner
            } else {
                egui::CentralPanel::default()
                    .frame(flyout_frame(t))
                    .show(ctx, draw)
                    .inner
            };
            FlyoutFrame {
                focused,
                close,
                report,
                height,
            }
        });
        if frame.focused == Some(true) {
            fly.had_focus = true;
        }
        // A whole physical pixel of change, not a rounding: placing resizes, and a resize brings
        // a frame, so a sub-pixel difference would place it again every frame (#1000).
        if flyout::needs_place(fly.placed, frame.height, ctx.pixels_per_point()) {
            let first = fly.placed.is_none();
            if platform::place_flyout(&title, fly.icon, (flyout::WIDTH, frame.height), first) {
                fly.placed = Some(frame.height);
            } else if fly.misses < 3 {
                fly.misses += 1;
                ctx.request_repaint();
            } else if first {
                // Its window can't be found to place: show it where Windows put it.
                ctx.send_viewport_cmd_to(flyout_id(), egui::ViewportCommand::Visible(true));
                fly.placed = Some(frame.height);
            }
            if first && fly.placed.is_some() {
                fly.shown_at = Some(std::time::Instant::now());
            }
        }
        if let Some(d) = frame.report.repaint_in {
            ctx.request_repaint_after(d);
        }
        let lost = fly.had_focus && frame.focused == Some(false);
        // Shown but never focused (Windows refused the hand-off): it could never close on focus
        // loss, so it closes after the grace; one wake to look, only while it is up (#1000).
        let unfocused = match flyout::focus_check(
            fly.had_focus,
            frame.focused,
            fly.shown_at,
            std::time::Instant::now(),
        ) {
            flyout::FocusCheck::Fine => false,
            flyout::FocusCheck::LookIn(d) => {
                ctx.request_repaint_after(d);
                false
            }
            flyout::FocusCheck::Close => {
                log::info!("tray flyout closed: it never got the focus");
                true
            }
        };
        let mut leave = frame.close || lost || unfocused;
        for a in frame.report.actions {
            match a {
                TrayAction::Open { module, target } => {
                    if let Some(page) = self.panels.navigate(&module, &target) {
                        self.open(TabId::panel(&page.module, &page.panel));
                    }
                    self.show_main();
                    leave = true;
                }
                TrayAction::OpenHub => {
                    self.show_main();
                    leave = true;
                }
                TrayAction::UpdateHub => {
                    log::info!("update hideout: opening {RELEASES_URL}");
                    platform::open_url(RELEASES_URL);
                    leave = true;
                }
                TrayAction::LayoutChanged => self.save_tray_layout(),
            }
        }
        if leave {
            self.close_flyout(ctx, lost && !frame.close);
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
                Some(p) => {
                    let mut look = strip::panel_look(p, tab.popped);
                    // A failure from launching reads "Couldn't start <Module>" (#983).
                    if self.launch_failed(&key.module) {
                        look.state = TabState::Stopped(words::couldnt_start_title(&p.module_name));
                    }
                    // Restarting: "Restarting… (2 of 5)", which changes once per crash and so
                    // needs no repaint (the live countdown is the crash bar's).
                    if self.panels.module_state(&key.module) == Some("restarting") {
                        let crash = self
                            .panels
                            .fact(&format!("module.{}.restart", key.module))
                            .and_then(|f| f.get("crash"))
                            .and_then(|v| v.as_u64());
                        if let Some(k) = crash {
                            look.state = TabState::Busy(words::restarting_row(0, k as usize));
                        }
                    }
                    look
                }
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

    /// `install-failed` that came from launching (`module.<id>.failed` is `launch`).
    fn launch_failed(&self, module: &str) -> bool {
        self.panels.module_state(module) == Some("install-failed")
            && self
                .panels
                .fact(&format!("module.{module}.failed"))
                .and_then(|v| v.as_str())
                == Some("launch")
    }

    /// The module's log (`logs\<id>.log`), in the app Windows opens `.log` files with.
    fn open_log(&self, module: &str) {
        open_module_log(&self.shared.paths.root, module);
    }

    /// "Open <Module>" from a notification: the module's page as the active tab.
    fn take_open_requests(&mut self) {
        let asked = std::mem::take(&mut *OPEN_REQUESTS.lock().unwrap_or_else(|e| e.into_inner()));
        for module in asked {
            self.shared.hidden.store(false, Ordering::Release);
            let page = self
                .panels
                .in_slot(Slot::Page)
                .find(|p| p.key.module == module)
                .map(|p| p.key.clone());
            match page {
                Some(k) => self.open(TabId::panel(&k.module, &k.panel)),
                None => log::info!("open {module}: it has no page yet"),
            }
        }
    }

    /// The crash bar while the module restarts (docs/hideout-supervisor-ux.md): amber, two
    /// lines, Open log. The countdown comes from `module.<id>.restart` and the moment it
    /// arrived, repainted once a second only while the window shows it. The first line is a
    /// polite live region whose name stays the same for the whole crash, so a screen reader says
    /// it once and never reads the countdown.
    fn crash_bar_ui(
        &mut self,
        ui: &mut egui::Ui,
        t: &Tokens,
        key: &PanelKey,
        name: &str,
        popout: bool,
    ) {
        let now = std::time::Instant::now();
        let Some(r) = self.panels.restart(&key.module, now) else {
            return;
        };
        let (line1, line2) = words::crash_bar(name, r.in_s, r.crash as usize);
        let mut open_log = false;
        ui.push_id(("hideout.crash-bar", &key.module), |ui| {
            egui::Frame::new()
                .fill(c(theme::warn_fill(t)))
                .stroke(egui::Stroke::new(1.0_f32, c(t.warn)))
                .corner_radius(egui::CornerRadius::same(6))
                .inner_margin(egui::Margin::symmetric(14, 10))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    let first = ui.label(egui::RichText::new(&line1).strong().color(c(t.text)));
                    let said = words::crash_announcement(name);
                    ui.ctx().accesskit_node_builder(first.id, |n| {
                        n.set_live(egui::accesskit::Live::Polite);
                        n.set_label(said);
                    });
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(&line2).color(c(t.text)));
                        ui.label(egui::RichText::new("\u{b7}").color(c(t.text)));
                        open_log = ui.link(words::OPEN_LOG).clicked();
                    });
                });
        });
        if open_log {
            self.open_log(&key.module);
        }
        let minimised = ui.ctx().input(|i| i.viewport().minimized == Some(true));
        // A pop-out stays on screen while the main window is in the tray.
        if !minimised && (popout || !self.shared.hidden.load(Ordering::Acquire)) {
            if let Some(d) = countdown_repaint(r.in_s) {
                ui.ctx().request_repaint_after(d);
            }
        }
        ui.add_space(12.0);
    }

    /// Whether a card's main button takes the focus this frame: when the card appears (or its
    /// page is opened again), and on Esc, which it consumes so Esc does nothing. Never while the
    /// close dialog, the palette or the flyout is up: their own Esc and focus come first.
    fn card_takes_focus(&mut self, ui: &egui::Ui, key: &PanelKey, card: &'static str) -> bool {
        if self.asking.is_some() || self.palette.open || self.flyout.is_some() {
            return false;
        }
        let fresh = self.card_focus.get(key) != Some(&card);
        self.card_focus.insert(key.clone(), card);
        let esc = ui
            .ctx()
            .input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape));
        fresh || esc
    }

    /// The stopped card (crash-stopped): one main button for the ladder's step from
    /// `module.<id>.recovery`, then Copy diagnostics, always last. The main button takes the
    /// focus when the card appears; Esc does nothing.
    fn stopped_card_ui(&mut self, ui: &mut egui::Ui, t: &Tokens, key: &PanelKey, name: &str) {
        let module = &key.module;
        let fact = |k: &str| {
            self.panels
                .fact(&format!("module.{module}.{k}"))
                .and_then(|v| v.as_str())
                .map(String::from)
        };
        let step = Recovery::from_id(fact("recovery").as_deref());
        let previous = fact("previous");
        let main = step.button(name, previous.as_deref());
        let line = step.line(name, previous.as_deref());
        // Copied, never drawn: the ids belong here (Never shown).
        let diagnostics = format!(
            "Module: {module}\nState: {}\nRecovery: {}",
            self.panels.module_state(module).unwrap_or("unknown"),
            step.id()
        );
        let focus = self.card_takes_focus(ui, key, step.id());
        let mut pressed = false;
        card(ui, t, |ui| {
            ui.label(
                egui::RichText::new(format!("\u{26A0} {}", words::STOPPED))
                    .size(16.0)
                    .strong()
                    .color(c(t.danger)),
            );
            ui.add_space(4.0);
            ui.label(egui::RichText::new(words::stopped_body(name)).color(c(t.text)));
            if let Some(l) = &line {
                ui.label(egui::RichText::new(l).color(c(t.text_muted)));
            }
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                pressed = main_button(ui, t, &main, focus).clicked();
                if ui.button(words::COPY_DIAGNOSTICS).clicked() {
                    ui.ctx().copy_text(diagnostics.clone());
                }
            });
        });
        if pressed {
            if let Some(h) = self.hooks.as_mut() {
                match (step, previous.is_some()) {
                    (Recovery::StartAgain, _) => h.start_again(module),
                    (Recovery::Defaults, _) => h.start_with_defaults(module),
                    (Recovery::GoBack, true) => h.go_back(module),
                    (Recovery::GoBack, false) | (Recovery::Last, _) => h.turn_off(module),
                }
            }
        }
        ui.add_space(12.0);
    }

    /// `install-failed` from launching: "Couldn't start <Module>", its reason, **Try again** (the
    /// main button, with the stopped card's focus and Esc rule) · Open log.
    fn couldnt_start_ui(&mut self, ui: &mut egui::Ui, t: &Tokens, key: &PanelKey, name: &str) {
        let (title, files, buttons) = words::couldnt_start(name);
        // The supervisor's reason is one of its own plain lines (words.rs).
        let body = self
            .panels
            .fact(&format!("module.{}.reason", key.module))
            .and_then(|v| v.as_str())
            .map(String::from)
            .unwrap_or(files);
        let focus = self.card_takes_focus(ui, key, "couldnt-start");
        let (mut again, mut log) = (false, false);
        card(ui, t, |ui| {
            ui.label(
                egui::RichText::new(format!("\u{26A0} {title}"))
                    .size(16.0)
                    .strong()
                    .color(c(t.danger)),
            );
            ui.add_space(4.0);
            ui.label(egui::RichText::new(body).color(c(t.text)));
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                again = main_button(ui, t, &buttons[0], focus).clicked();
                log = ui.button(&buttons[1]).clicked();
            });
        });
        if again {
            if let Some(h) = self.hooks.as_mut() {
                h.start_again(&key.module);
            }
        }
        if log {
            self.open_log(&key.module);
        }
        ui.add_space(12.0);
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
                if let Some(k) = self.popout_index(id) {
                    let p = self.popouts.remove(k);
                    self.save_place(&p);
                }
                if self.popout_asking.as_ref() == Some(id) {
                    self.popout_asking = None;
                }
                self.set_popped_out(id, false);
                if let Some(h) = self.hooks.as_mut() {
                    h.dock_back(id);
                }
            }
        }
        set_popouts_open(!self.popouts.is_empty());
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
            Action::PopOut(i) => self.pop_out(i),
            Action::DockBack(i) => self.dock(i, false),
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
            Shortcut::PopOut => {
                if let Some(i) = self.tabs.active() {
                    if self.tabs.tabs()[i].popped {
                        self.dock(i, false);
                    } else {
                        self.pop_out(i);
                    }
                }
            }
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
                // (target, title, attention dot, the red mark's accessible name when stopped)
                let mut entries: Vec<(TabId, String, bool, Option<String>)> = self
                    .pages
                    .iter()
                    .map(|p| (TabId::page(p.id()), p.title().to_string(), false, None))
                    .collect();
                entries.extend(self.panels.in_slot(Slot::Page).map(|p| {
                    let mut look = strip::panel_look(p, false);
                    if self.launch_failed(&p.key.module) {
                        look.state = TabState::Stopped(words::couldnt_start_title(&p.module_name));
                    }
                    let stopped = matches!(look.state, TabState::Stopped(_));
                    (
                        TabId::panel(&p.key.module, &p.key.panel),
                        p.title.clone(),
                        p.badge == hideout_panels::Badge::Attention,
                        stopped.then(|| look.accessible_name()),
                    )
                }));
                let active = self.tabs.active_id().cloned();
                for (target, title, attention, stopped) in entries {
                    let selected = active.as_ref() == Some(&target);
                    let text = egui::RichText::new(title)
                        .size(15.0)
                        .color(c(theme::nav_label(t, selected)));
                    let resp = ui.add_sized(
                        [ui.available_width(), 32.0],
                        egui::Button::selectable(selected, text),
                    );
                    if let Some(said) = stopped {
                        // The red warning mark, as on its tab (docs/hideout-supervisor-ux.md, S
                        // and T); its words are in the entry's accessible name.
                        let at = egui::pos2(resp.rect.right() - 14.0, resp.rect.center().y);
                        ui.painter().text(
                            at,
                            egui::Align2::CENTER_CENTER,
                            "\u{26A0}",
                            egui::FontId::proportional(14.0),
                            c(theme::nav_mark(t, selected)),
                        );
                        ui.ctx()
                            .accesskit_node_builder(resp.id, |n| n.set_label(said));
                    } else if attention {
                        // The attention dot on the sidebar entry too; the tab's accessible name
                        // says "something new", so colour is not the only signal.
                        let at = egui::pos2(resp.rect.right() - 12.0, resp.rect.center().y);
                        let fill = if selected { t.on_accent } else { t.accent };
                        ui.painter().circle_filled(at, 4.0, c(fill));
                    }
                    let out = self
                        .tabs
                        .position(&target)
                        .is_some_and(|i| self.tabs.tabs()[i].popped);
                    if out {
                        // In its own window: a muted ↗ at the entry's right.
                        let at = egui::pos2(resp.rect.right() - 30.0, resp.rect.center().y);
                        ui.painter().text(
                            at,
                            egui::Align2::CENTER_CENTER,
                            "\u{2197}",
                            egui::FontId::proportional(12.0),
                            c(t.text_muted),
                        );
                    }
                    if resp.clicked() {
                        if out {
                            self.focus_popout(&target);
                        } else {
                            self.open(target);
                        }
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
        if self.card_target.as_ref() != Some(&id) {
            self.card_target = Some(id.clone());
            self.card_focus.clear();
        }
        // The page area is the tab panel, named by its tab.
        let name = look.accessible_name();
        ui.ctx().accesskit_node_builder(ui.unique_id(), |n| {
            n.set_role(egui::accesskit::Role::TabPanel);
            n.set_label(name);
        });
        if look.popped {
            // The placeholder: the page is in its own window (Show it brings it forward).
            let (mut show, mut dock) = (false, false);
            ui.vertical_centered(|ui| {
                ui.add_space(ui.available_height() / 3.0);
                ui.label(
                    egui::RichText::new("Nothing open")
                        .size(20.0)
                        .color(c(t.text)),
                );
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new(format!("{} is in its own window.", look.title))
                        .color(c(t.text_muted)),
                );
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    // Centred: the two buttons' width, give or take.
                    ui.add_space((ui.available_width() - 180.0).max(0.0) / 2.0);
                    show = main_button(ui, t, "Show it", false).clicked();
                    ui.label(egui::RichText::new("\u{b7}").color(c(t.text_muted)));
                    dock = ui.button("Dock back").clicked();
                });
            });
            if show {
                self.focus_popout(&id);
            }
            if dock {
                self.apply(Action::DockBack(i));
            }
            return;
        }
        self.page_size = ui.available_size();
        self.page_body(ui, t, &id, &look, false);
    }

    /// A page's contents, in the page area or its pop-out (`popout`): the hub page, or the
    /// module's panel with the supervisor's surfaces (crash bar, stopped card, Couldn't start)
    /// and the state line. Under one id per page, so scroll, open sections and typed text are the
    /// same widgets in either place (§5.6).
    fn page_body(&mut self, ui: &mut egui::Ui, t: &Tokens, id: &TabId, look: &Look, popout: bool) {
        ui.push_id(("hideout.page", id), |ui| {
            self.page_body_inner(ui, t, id, look, popout)
        });
    }

    fn page_body_inner(
        &mut self,
        ui: &mut egui::Ui,
        t: &Tokens,
        id: &TabId,
        look: &Look,
        popout: bool,
    ) {
        match self.target(id) {
            Some(Target::Page(p)) => self.pages[p].ui(ui, t),
            Some(Target::Panel(key)) => {
                let name = self
                    .panels
                    .panel(&key)
                    .map(|p| p.module_name.clone())
                    .unwrap_or_else(|| look.title.clone());
                let fact_state = self.panels.module_state(&key.module).map(String::from);
                let stopped = matches!(
                    fact_state.as_deref(),
                    Some("crash-stopped" | "crash-looped")
                );
                // The supervisor's own surfaces (docs/hideout-supervisor-ux.md) replace the
                // generic ones, and the greyed page below drops its state line.
                let own = match fact_state.as_deref() {
                    Some("restarting") => {
                        self.crash_bar_ui(ui, t, &key, &name, popout);
                        true
                    }
                    _ if stopped => {
                        self.stopped_card_ui(ui, t, &key, &name);
                        true
                    }
                    Some("install-failed") if self.launch_failed(&key.module) => {
                        self.couldnt_start_ui(ui, t, &key, &name);
                        true
                    }
                    _ => false,
                };
                // A card that comes back focuses its main button again.
                if !own || fact_state.as_deref() == Some("restarting") {
                    self.card_focus.remove(&key);
                }
                match &look.state {
                    _ if own => {}
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
                    let report = if own {
                        hideout_panels::panel_ui_quiet(&mut self.panels, ui, &key)
                    } else {
                        hideout_panels::panel_ui(&mut self.panels, ui, &key)
                    };
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
        let (entries, targets) = self.palette_entries();
        let count = self.palette.matches(&entries).len();
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
            if let Some(target) = self.palette.chosen(&entries).map(|e| e.target.clone()) {
                if let Some((_, id)) = targets.iter().find(|(t, _)| *t == target) {
                    // "Pop out <page>": works for a page that isn't open as a tab yet.
                    let id = id.clone();
                    if !self.tabs.is_open(&id) {
                        self.open(id.clone());
                    }
                    if let Some(i) = self.tabs.position(&id) {
                        self.pop_out(i);
                    }
                } else if self.pages.iter().any(|p| p.id() == target) {
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
            let matches = self.palette.matches(&entries);
            if matches.is_empty() {
                let msg = if entries.is_empty() {
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
        // A lab capture (#1002) waiting for this frame; nothing while none is asked for.
        let job = self.capture.as_ref().and_then(|s| s.take());
        if let Some(feed) = self.feed.as_mut() {
            let now = std::time::Instant::now();
            // Event-driven: the feed's waker brought this frame; a held-back patch asks for one
            // more repaint when its slot comes (no timer loop).
            let (_, next) = self.panels.pump(feed.as_mut(), now);
            if let Some(at) = next {
                ctx.request_repaint_after(at.saturating_duration_since(now));
            }
        }
        self.take_open_requests();
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
        let fly_shot = job.as_ref().filter(|j| j.flyout).map(|j| j.shots.clone());
        let pop_shot = job
            .as_ref()
            .filter(|j| j.popouts > 0)
            .map(|j| j.shots.clone());
        self.flyout_ui(ctx, &t, fly_shot);
        if let Some(j) = job.filter(|j| j.main) {
            crate::capture::queue_readback(ctx, crate::capture::Window::Main, j.shots);
        }
        self.popouts_ui(ctx, &t, pop_shot);
        // A placeholder tab that became active (a click, Ctrl+number) brings its window forward.
        let active = self.tabs.active_id().cloned();
        if active != self.last_active {
            if let Some(a) = active.as_ref() {
                if self
                    .tabs
                    .position(a)
                    .is_some_and(|i| self.tabs.tabs()[i].popped)
                {
                    self.focus_popout(a);
                }
            }
            self.last_active = active;
        }
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

/// Pop-out windows (H2.11, contract §2.5, `docs/hideout-popouts-ux.md`): each popped-out page is
/// the same panel drawn in an immediate viewport of the hub, so its view state (kept by node id,
/// §5.6) moves with it and survives a module restart or hot swap.
impl Hub {
    /// The open pop-out of `id`, if any.
    fn popout_index(&self, id: &TabId) -> Option<usize> {
        self.popouts.iter().position(|p| &p.id == id)
    }

    /// Opens `id`'s window (it is popped out in the strip already). `focus` brings it forward
    /// with the focus (the user popped it out); a restore shows it without the focus.
    fn start_popout(&mut self, id: TabId, focus: bool) {
        if self.popout_index(&id).is_some() {
            return;
        }
        let vid = egui::ViewportId::from_hash_of(("hideout.popout", popout::page_key(&id)));
        self.popouts.push(Popout {
            id,
            vid,
            hwnd: None,
            misses: 0,
            focus,
            shown: false,
            minimized: false,
            last_rect: None,
            unsaved: false,
            placed: None,
        });
        set_popouts_open(true);
    }

    /// Writes `[windows]` into `hideout.toml` (with the rest of the file as it was).
    fn save_windows(&self) {
        let Ok(mut p) = self.shared.prefs.lock() else {
            return;
        };
        popout::store(&mut p.rest, &self.windows);
        if let Err(e) = p.save(&self.shared.paths.prefs_file()) {
            log::warn!("saving {}: {e}", self.shared.paths.prefs_file().display());
        }
    }

    /// Records whether `id` is popped out, for the next start.
    fn set_popped_out(&mut self, id: &TabId, on: bool) {
        let key = popout::page_key(id);
        let entry = self.windows.entry(key).or_default();
        if entry.popped_out != on {
            entry.popped_out = on;
            self.save_windows();
        }
    }

    /// Saves where the pop-out is for the current screen layout (§2.5): when a move or resize
    /// ended, and when it closes. Nothing while it is minimised (its place didn't change).
    fn save_place(&mut self, p: &Popout) {
        let Some(h) = p.hwnd else { return };
        let Some(wp) = platform::window_place(h) else {
            return;
        };
        if wp.minimized {
            return;
        }
        let (rect, state) = if wp.maximized {
            (wp.normal, WindowState::Maximized)
        } else if wp.actual != wp.normal {
            (wp.actual, WindowState::Snapped)
        } else {
            (wp.normal, WindowState::Normal)
        };
        let mons = platform::monitors();
        let layout = popout::layout_id(&mons);
        let entry = self.windows.entry(popout::page_key(&p.id)).or_default();
        popout::remember(entry, &mons, &layout, rect, state, &popout::now_utc());
        self.save_windows();
    }

    /// Pop out (the tab menu, Ctrl+Shift+O, the ↗ button, Ctrl+K): the page leaves a placeholder
    /// and opens in its window next to Hideout, with the focus. A page out already comes forward.
    fn pop_out(&mut self, i: usize) {
        let Some(id) = self.tabs.tabs().get(i).map(|t| t.id.clone()) else {
            return;
        };
        if self.tabs.tabs()[i].popped {
            self.focus_popout(&id);
            return;
        }
        let title = self.look(i).title;
        self.tabs.pop_out(i);
        self.set_popped_out(&id, true);
        self.start_popout(id.clone(), true);
        log::info!("{title} opened in its own window");
        if let Some(h) = self.hooks.as_mut() {
            h.pop_out(&id, &title);
        }
    }

    /// Dock back (`quiet` false: Dock back, Ctrl+Shift+O, the window menu): the page lands at its
    /// placeholder's place as the active tab and Hideout comes forward. `quiet` (×, Ctrl+W,
    /// Alt+F4): the page goes back to its tab without activating it, Hideout left as it is.
    /// Neither is `closed` and neither asks (§2.5, spec v1.6).
    fn dock(&mut self, i: usize, quiet: bool) {
        let Some(id) = self.tabs.tabs().get(i).map(|t| t.id.clone()) else {
            return;
        };
        if let Some(k) = self.popout_index(&id) {
            let p = self.popouts.remove(k);
            self.save_place(&p);
        }
        set_popouts_open(!self.popouts.is_empty());
        if self.popout_asking.as_ref() == Some(&id) {
            self.popout_asking = None;
        }
        let was = self.tabs.tabs()[i].popped;
        if quiet {
            self.tabs.dock_quiet(i);
        } else {
            self.tabs.dock_back(i);
            self.last_active = Some(id.clone());
            self.show_main();
        }
        self.set_popped_out(&id, false);
        if was {
            if let Some(h) = self.hooks.as_mut() {
                h.dock_back(&id);
            }
        }
    }

    /// Brings `id`'s window forward (its placeholder tab, its sidebar entry, Show it).
    fn focus_popout(&self, id: &TabId) {
        if let Some(h) = self.popout_index(id).and_then(|k| self.popouts[k].hwnd) {
            platform::focus_window(h);
        }
    }

    /// Keep on top for `id`: toggled, remembered per page, applied to its window.
    fn toggle_keep_on_top(&mut self, id: &TabId) {
        let entry = self.windows.entry(popout::page_key(id)).or_default();
        entry.keep_on_top = !entry.keep_on_top;
        let on = entry.keep_on_top;
        self.save_windows();
        if let Some(h) = self.popout_index(id).and_then(|k| self.popouts[k].hwnd) {
            platform::set_topmost(h, on);
        }
    }

    fn keep_on_top(&self, id: &TabId) -> bool {
        self.windows
            .get(&popout::page_key(id))
            .is_some_and(|w| w.keep_on_top)
    }

    /// Every frame: opens the windows of popped-out pages (unless they wait for the first open of
    /// Hideout), draws each, and applies what the user did in them.
    fn popouts_ui(
        &mut self,
        ctx: &egui::Context,
        t: &Tokens,
        capture: Option<std::sync::mpsc::Sender<crate::capture::Shot>>,
    ) {
        // Started in the tray at sign-in: pop-outs wait for the first time Hideout is shown.
        if self.popouts_held && !self.shared.hidden.load(Ordering::Acquire) {
            self.popouts_held = false;
        }
        if !self.popouts_held {
            let popped: Vec<TabId> = self
                .tabs
                .tabs()
                .iter()
                .filter(|t| t.popped)
                .map(|t| t.id.clone())
                .collect();
            for id in popped {
                self.start_popout(id, false);
            }
        }
        // A tab that closed or docked another way takes its window with it.
        let tabs = &self.tabs;
        self.popouts
            .retain(|p| tabs.position(&p.id).is_some_and(|i| tabs.tabs()[i].popped));
        for (h, cmd) in platform::take_menu_commands() {
            let Some(id) = self
                .popouts
                .iter()
                .find(|p| p.hwnd == Some(h))
                .map(|p| p.id.clone())
            else {
                continue;
            };
            match cmd {
                platform::MenuCommand::KeepOnTop => self.toggle_keep_on_top(&id),
                platform::MenuCommand::DockBack => {
                    if let Some(i) = self.tabs.position(&id) {
                        self.dock(i, false);
                    }
                }
            }
        }
        let mut list = std::mem::take(&mut self.popouts);
        let mut done: Vec<(TabId, Vec<PopCmd>)> = Vec::new();
        let taken: Vec<isize> = list.iter().filter_map(|p| p.hwnd).collect();
        let placed: Vec<placement::Rect> = list.iter().filter_map(|p| p.placed).collect();
        // Pop-outs on screen, numbered from 1 for a lab capture (as `POPOUTS_SHOWN` counts them).
        let mut shown_n: u8 = 0;
        for p in list.iter_mut() {
            let Some(i) = self.tabs.position(&p.id) else {
                continue;
            };
            let title = window_title(&self.look(i).title);
            let builder = egui::ViewportBuilder::default()
                .with_title(title.clone())
                .with_icon(egui::IconData {
                    rgba: crate::icon::rgba(64, State::Normal),
                    width: 64,
                    height: 64,
                })
                .with_min_inner_size([popout::MIN.0 as f32, popout::MIN.1 as f32])
                .with_inner_size([
                    self.page_size.x.max(popout::FIRST_MIN.0 as f32),
                    self.page_size.y.max(popout::FIRST_MIN.1 as f32),
                ])
                // Shown once placed (no flash at a default spot).
                .with_visible(false);
            let frame = ctx.show_viewport_immediate(p.vid, builder, |vctx, class| {
                if class == egui::ViewportClass::Embedded {
                    // No second window (tests, or a backend without viewports): the placeholder
                    // in the page area stays the way to the page.
                    return PopFrame::default();
                }
                let out = self.popout_window_ui(vctx, t, i);
                if let Some(tx) = capture.as_ref().filter(|_| p.shown && !p.minimized) {
                    shown_n = shown_n.saturating_add(1);
                    let w = crate::capture::Window::Popout(shown_n);
                    crate::capture::queue_readback(vctx, w, tx.clone());
                }
                out
            });
            self.popout_platform(ctx, p, &title, &taken, &placed, &frame);
            if frame.drawn {
                // A window still being placed counts as on screen, so popping out the active tab
                // stays `visible` (§5.9); minimised is `hidden`.
                let on = !p.minimized && (p.shown || p.hwnd.is_none());
                self.tabs.set_popout_shown(i, on);
            }
            if !frame.cmds.is_empty() {
                done.push((p.id.clone(), frame.cmds));
            }
        }
        self.popouts = list;
        for (id, cmds) in done {
            for cmd in cmds {
                self.popout_cmd(&id, cmd);
            }
        }
        set_popouts_open(!self.popouts.is_empty());
        let on = self
            .popouts
            .iter()
            .filter(|p| p.shown && !p.minimized)
            .count();
        POPOUTS_SHOWN.store(on.min(u8::MAX as usize) as u8, Ordering::Release);
    }

    /// Applies one thing the user did in `id`'s pop-out.
    fn popout_cmd(&mut self, id: &TabId, cmd: PopCmd) {
        let Some(i) = self.tabs.position(id) else {
            return;
        };
        match cmd {
            PopCmd::DockBack => self.dock(i, false),
            PopCmd::Close => self.dock(i, true),
            PopCmd::ToggleTop => self.toggle_keep_on_top(id),
            PopCmd::ShowHub => self.show_main(),
            PopCmd::Palette => {
                self.show_main();
                if !self.palette.open {
                    self.palette.toggle();
                }
            }
            PopCmd::ClosePage => {
                if self.dirty(id) {
                    self.popout_asking = Some(id.clone());
                } else {
                    self.closed(std::slice::from_ref(id));
                }
            }
            PopCmd::CloseAnyway => {
                self.popout_asking = None;
                self.closed(std::slice::from_ref(id));
            }
            PopCmd::KeepOpen => self.popout_asking = None,
        }
    }

    /// The window side of one pop-out: find its window once, place and show it, keep on top,
    /// and save its place when a move or resize ends.
    fn popout_platform(
        &mut self,
        ctx: &egui::Context,
        p: &mut Popout,
        title: &str,
        taken: &[isize],
        placed: &[placement::Rect],
        frame: &PopFrame,
    ) {
        if !frame.drawn {
            return;
        }
        p.minimized = frame.minimized;
        if let Some(h) = p.hwnd.filter(|&h| !platform::window_alive(h)) {
            // egui made the window again (a GPU reset): find and place it again.
            log::info!("pop-out window {h:#x} is gone; placing its new one");
            p.hwnd = None;
            p.shown = false;
            p.misses = 0;
        }
        let Some(h) = p.hwnd else {
            match platform::find_popout(title, taken) {
                Some(h) => {
                    p.hwnd = Some(h);
                    let mons = platform::monitors();
                    let layout = popout::layout_id(&mons);
                    let key = popout::page_key(&p.id);
                    let first = (self.page_size.x as f64, self.page_size.y as f64);
                    let anchor = platform::main_window_any_rect();
                    let at = popout::restore(
                        self.windows.get(&key),
                        &mons,
                        &layout,
                        anchor,
                        first,
                        placed,
                    );
                    let mut maximized = false;
                    if let Some(at) = at {
                        platform::place_window(h, at.rect);
                        p.placed = Some(at.rect);
                        maximized = at.state == WindowState::Maximized;
                    }
                    let top = self.keep_on_top(&p.id);
                    platform::install_popout_hooks(h, top);
                    if top {
                        platform::set_topmost(h, true);
                    }
                    platform::show_window(h, maximized, p.focus);
                    p.focus = false;
                    p.shown = true;
                    // The first frame at its real size.
                    ctx.request_repaint_of(p.vid);
                }
                None if p.misses < 3 => {
                    p.misses += 1;
                    ctx.request_repaint();
                }
                None if !p.shown => {
                    // Its window can't be found to place: show it where Windows put it.
                    log::warn!("pop-out {title:?}: window not found; shown unplaced");
                    ctx.send_viewport_cmd_to(p.vid, egui::ViewportCommand::Visible(true));
                    p.shown = true;
                }
                None => {}
            }
            return;
        };
        // A move or resize is saved once it ends (never during a drag, never on a timer).
        if frame.rect.is_some() && frame.rect != p.last_rect {
            if p.last_rect.is_some() {
                p.unsaved = true;
            }
            p.last_rect = frame.rect;
        }
        let ended = platform::take_size_move_ended(h);
        if (p.unsaved || ended) && !platform::in_size_move(h) {
            p.unsaved = false;
            self.save_place(p);
        }
    }

    /// One pop-out window: its title row (title, Keep on top, Dock back, More) and the page,
    /// drawn exactly as in the page area. Returns what the user did. Window-free: the review
    /// and the tests draw it on their own context.
    fn popout_window_ui(&mut self, ctx: &egui::Context, t: &Tokens, i: usize) -> PopFrame {
        let mut out = PopFrame {
            drawn: true,
            ..Default::default()
        };
        let Some(id) = self.tabs.tabs().get(i).map(|t| t.id.clone()) else {
            return out;
        };
        let look = self.look(i);
        let (close, minimized, rect) = ctx.input(|inp| {
            let v = inp.viewport();
            (v.close_requested(), v.minimized == Some(true), v.outer_rect)
        });
        out.minimized = minimized;
        out.rect = rect;
        let asking = self.popout_asking.as_ref() == Some(&id);
        if close {
            // × (Alt+F4): back to its tab quietly; the window goes with the next frame.
            out.cmds.push(PopCmd::Close);
        }
        if !asking {
            let cs = Modifiers::COMMAND | Modifiers::SHIFT;
            let (dock, w, k) = ctx.input_mut(|inp| {
                (
                    inp.consume_key(cs, Key::O),
                    inp.consume_key(Modifiers::COMMAND, Key::W),
                    inp.consume_key(Modifiers::COMMAND, Key::K),
                )
            });
            if dock {
                out.cmds.push(PopCmd::DockBack);
            }
            if w {
                out.cmds.push(PopCmd::Close);
            }
            if k {
                out.cmds.push(PopCmd::Palette);
            }
        }
        let top = self.keep_on_top(&id);
        egui::TopBottomPanel::top("hideout.popout.title")
            .exact_height(POPOUT_TITLE_H)
            .show_separator_line(false)
            .frame(
                egui::Frame::new()
                    .fill(c(t.chrome))
                    .inner_margin(egui::Margin::symmetric(12, 0)),
            )
            .show(ctx, |ui| {
                popout_title_row(ui, t, &look.title, top, &mut out.cmds);
            });
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(c(t.bg))
                    .inner_margin(egui::Margin::same(24)),
            )
            .show(ctx, |ui| self.page_body(ui, t, &id, &look, true));
        if asking {
            if let Some(cmd) = close_page_ask(ctx, t, &look.title) {
                out.cmds.push(cmd);
            }
        }
        out
    }

    /// "Pop out <page>" for the palette: every page (the hub's and the modules') that isn't out.
    fn palette_entries(&self) -> (Vec<Entry>, Vec<(String, TabId)>) {
        let mut entries = self.entries.clone();
        let mut targets = Vec::new();
        let mut pages: Vec<(TabId, String)> = self
            .pages
            .iter()
            .map(|p| (TabId::page(p.id()), p.title().to_string()))
            .collect();
        pages.extend(
            self.panels
                .in_slot(Slot::Page)
                .map(|p| (TabId::panel(&p.key.module, &p.key.panel), p.title.clone())),
        );
        for (id, title) in pages {
            let out = self
                .tabs
                .position(&id)
                .is_some_and(|i| self.tabs.tabs()[i].popped);
            if out {
                continue;
            }
            let target = format!("{POPOUT_TARGET}{}", popout::page_key(&id));
            entries.push(Entry::new(&target, format!("Pop out {title}")));
            targets.push((target, id));
        }
        (entries, targets)
    }
}

/// A palette entry's target for "Pop out <page>" (opaque to the palette).
const POPOUT_TARGET: &str = "hub.pop-out:";

/// The pop-out's title row height (points): the plan's one row of 36 px.
const POPOUT_TITLE_H: f32 = 36.0;

/// One open pop-out window.
struct Popout {
    id: TabId,
    vid: egui::ViewportId,
    /// Its window, once found by its title (eframe gives an immediate viewport no handle).
    hwnd: Option<isize>,
    /// Frames that couldn't find its window yet.
    misses: u8,
    /// Bring it forward with the focus when it is first shown (popped out by the user).
    focus: bool,
    /// Placed and shown.
    shown: bool,
    minimized: bool,
    /// The outer rectangle (points) egui reported last, to notice a move or resize.
    last_rect: Option<egui::Rect>,
    /// A move or resize to save once its loop ends.
    unsaved: bool,
    /// Where it was placed (physical pixels): a new one cascades off it.
    placed: Option<placement::Rect>,
}

/// What the user did in a pop-out this frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PopCmd {
    DockBack,
    /// ×, Ctrl+W, Alt+F4: back to its tab quietly.
    Close,
    ToggleTop,
    ShowHub,
    Palette,
    ClosePage,
    CloseAnyway,
    KeepOpen,
}

#[derive(Default)]
struct PopFrame {
    /// Drawn in its own window (not embedded).
    drawn: bool,
    minimized: bool,
    rect: Option<egui::Rect>,
    cmds: Vec<PopCmd>,
}

/// Any pop-out window is open: `wake::Gate` runs the hidden main window's frames for them, as
/// for the flyout (an immediate viewport is drawn from its parent's frame). False with none, so
/// the hidden, idle hub keeps waiting (#935, #1000).
static POPOUTS_OPEN: AtomicBool = AtomicBool::new(false);

/// Pop-out windows on screen (placed, not minimised), for a lab capture's answer.
static POPOUTS_SHOWN: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

fn set_popouts_open(on: bool) {
    POPOUTS_OPEN.store(on, Ordering::Release);
}

fn popouts_open() -> bool {
    POPOUTS_OPEN.load(Ordering::Acquire)
}

/// The windows drawn from the main window's frame: the tray flyout and the pop-outs.
fn side_windows_live() -> bool {
    flyout::live() || popouts_open()
}

/// The pop-out's title row: the page's title, then Keep on top, Dock back and More (…). A
/// toolbar for screen readers.
fn popout_title_row(ui: &mut egui::Ui, t: &Tokens, title: &str, top: bool, cmds: &mut Vec<PopCmd>) {
    ui.ctx().accesskit_node_builder(ui.unique_id(), |n| {
        n.set_role(egui::accesskit::Role::Toolbar);
        n.set_label("Window");
    });
    ui.horizontal_centered(|ui| {
        ui.label(
            egui::RichText::new(title)
                .size(14.0)
                .strong()
                .color(c(t.text)),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let more = ui.menu_button(egui::RichText::new("\u{2026}").size(16.0), |ui| {
                popout_more_menu(ui, t, top, cmds);
            });
            more.response
                .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "More"));
            // The arrow a size up: at the label's size it reads as a tick.
            let dock = ui.add(egui::Button::new((
                egui::RichText::new("\u{2199}").size(17.0).color(c(t.text)),
                egui::RichText::new("Dock back").color(c(t.text)),
            )));
            if dock.on_hover_text("Dock back (Ctrl+Shift+O)").clicked() {
                cmds.push(PopCmd::DockBack);
            }
            if keep_on_top_toggle(ui, t, top).clicked() {
                cmds.push(PopCmd::ToggleTop);
            }
        });
    });
}

/// Keep on top: a pin that fills orange when on. Its accessible name says the state.
fn keep_on_top_toggle(ui: &mut egui::Ui, t: &Tokens, on: bool) -> egui::Response {
    let (glyph, fg) = if on {
        ("\u{e841}", c(t.on_accent))
    } else {
        ("\u{e718}", c(t.text))
    };
    let mut b = egui::Button::new(egui::RichText::new(glyph).size(14.0).color(fg));
    if on {
        b = b.fill(c(t.accent));
    }
    let r = ui.add(b);
    let name = if on { "Keep on top, on" } else { "Keep on top" };
    r.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Checkbox, true, on, name));
    r.on_hover_text(if on { "Keep on top: on" } else { "Keep on top" })
}

/// More: Dock back, Keep on top, Show Hideout, Close page.
fn popout_more_menu(ui: &mut egui::Ui, t: &Tokens, top: bool, cmds: &mut Vec<PopCmd>) {
    let mut item = |ui: &mut egui::Ui, b: egui::Button, cmd: PopCmd| {
        if ui.add(b).clicked() {
            cmds.push(cmd);
            ui.close();
        }
    };
    let text = |s: &str| egui::RichText::new(s).color(c(t.text));
    item(
        ui,
        egui::Button::new(text("Dock back")).shortcut_text("Ctrl+Shift+O"),
        PopCmd::DockBack,
    );
    item(
        ui,
        egui::Button::selectable(top, text("Keep on top")),
        PopCmd::ToggleTop,
    );
    item(ui, egui::Button::new(text("Show Hideout")), PopCmd::ShowHub);
    ui.separator();
    item(ui, egui::Button::new(text("Close page")), PopCmd::ClosePage);
}

/// "Close <page>?" in the pop-out (Close page with unsaved work). Keep open is the default.
fn close_page_ask(ctx: &egui::Context, t: &Tokens, title: &str) -> Option<PopCmd> {
    let mut choice = None;
    let modal = egui::Modal::new(egui::Id::new("hideout.popout.ask")).show(ctx, |ui| {
        ui.set_width(340.0);
        ui.label(
            egui::RichText::new(format!("Close {title}?"))
                .size(16.0)
                .color(c(t.text)),
        );
        ui.add_space(6.0);
        ui.label(
            egui::RichText::new(format!("Your changes in {title} aren't saved.")).color(c(t.text)),
        );
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            let keep = ui.button("Keep open");
            if !ui.ctx().memory(|m| m.focused().is_some()) {
                keep.request_focus();
            }
            if keep.clicked() {
                choice = Some(PopCmd::KeepOpen);
            }
            if ui.button("Close anyway").clicked() {
                choice = Some(PopCmd::CloseAnyway);
            }
        });
    });
    let esc = ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape));
    if choice.is_none() && (modal.should_close() || esc) {
        choice = Some(PopCmd::KeepOpen);
    }
    choice
}

/// A card's main button: the accent fill; takes the focus when `focus`.
fn main_button(ui: &mut egui::Ui, t: &Tokens, text: &str, focus: bool) -> egui::Response {
    let b = ui
        .add(egui::Button::new(egui::RichText::new(text).color(c(t.on_accent))).fill(c(t.accent)));
    if focus {
        b.request_focus();
    }
    b
}

/// Opens a module's log, `logs\<id>.log` under the hub folder, in the app Windows opens `.log`
/// files with (the crash bar, Couldn't start and the rollback notification's Open log).
pub fn open_module_log(home: &std::path::Path, module: &str) {
    let file = home.join("logs").join(format!("{module}.log"));
    if let Err(e) = std::process::Command::new("explorer.exe")
        .arg(&file)
        .spawn()
    {
        log::warn!("open {}: {e}", file.display());
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
        if self.start_hidden > 0 {
            // Started in the tray (sign-in): eframe shows the window after its first frame, so
            // it hides on the second; tabs read `hidden` from the start.
            self.shared.hidden.store(true, Ordering::Release);
            self.start_hidden -= 1;
            if self.start_hidden == 0 {
                platform::hide_main_window();
                log::info!("started in the tray; saved pop-outs wait for Hideout to open");
            } else {
                ctx.request_repaint();
            }
        }
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
