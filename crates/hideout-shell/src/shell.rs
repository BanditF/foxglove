// SPDX-License-Identifier: MIT
//! The hub window: sidebar, page, status strip and the Ctrl+K palette, with the tray, the single
//! instance, placement and theme wired up. `run` is the hub's whole start-up.

use crate::icon::State;
use crate::logfile::FileLog;
use crate::palette::{Entry, Palette};
use crate::paths::HubPaths;
use crate::prefs::{Prefs, ThemeChoice};
use crate::status::{self, Item};
use crate::theme::{self, Rgb, Tokens};
use crate::{placement, platform, tray, window_title, AUMID, PRODUCT};
use eframe::egui::{self, Color32, Key, Modifiers};
use hideout_panels::Slot;
use hideout_panels::{PanelFeed, PanelHost, PanelKey};
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
}

/// State the tray and the window both touch.
struct Shared {
    paths: HubPaths,
    prefs: Mutex<Prefs>,
    paused: AtomicBool,
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
    match FileLog::open(&paths.logs_dir(), log::LevelFilter::Info) {
        Ok(l) => {
            let _ = l.install();
        }
        Err(e) => eprintln!("hideout: log {}: {e}", paths.logs_dir().display()),
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
    });
    let pages = opts.pages;
    let feed = opts.panels;
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
    let result = eframe::run_native(
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
            show.listen(move || {
                platform::show_main_window();
                show_ctx.request_repaint();
            });
            let mut feed = feed;
            if let Some(f) = feed.as_mut() {
                let wake_ctx = ctx.clone();
                f.set_waker(Box::new(move || wake_ctx.request_repaint()));
            }
            Ok(Box::new(Hub::new(shared, pages, feed)))
        }),
    );
    log::info!("window closed: {result:?}");
    log::logger().flush();
    result.map_err(|e| e.to_string())
}

fn on_tray(cmd: tray::Command, shared: &Shared, ctx: &egui::Context) {
    match cmd {
        tray::Command::Open => platform::show_main_window(),
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
    ctx.request_repaint();
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
fn install_fonts(ctx: &egui::Context) {
    let dir = std::env::var_os("WINDIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| "C:\\Windows".into())
        .join("Fonts");
    let mut fonts = egui::FontDefinitions::empty();
    let mut add = |name: &str, files: &[&str], family: egui::FontFamily| {
        for f in files {
            if let Ok(bytes) = std::fs::read(dir.join(f)) {
                fonts
                    .font_data
                    .insert(name.to_owned(), Arc::new(egui::FontData::from_owned(bytes)));
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

/// What the page area shows: one of the hub's own pages, or any registered module panel.
#[derive(Clone, PartialEq)]
enum Current {
    Page(usize),
    Panel(PanelKey),
}

struct Hub {
    shared: Arc<Shared>,
    pages: Vec<Box<dyn Page>>,
    current: Current,
    panels: PanelHost,
    feed: Option<Box<dyn PanelFeed>>,
    title: String,
    palette: Palette,
    /// From manifests and the registry (H2.2); empty until modules exist.
    entries: Vec<Entry>,
}

impl Hub {
    fn new(
        shared: Arc<Shared>,
        pages: Vec<Box<dyn Page>>,
        feed: Option<Box<dyn PanelFeed>>,
    ) -> Self {
        let title = window_title(pages.first().map(|p| p.title()).unwrap_or_default());
        Self {
            shared,
            pages,
            current: Current::Page(0),
            panels: PanelHost::default(),
            feed,
            title,
            palette: Palette::default(),
            entries: Vec::new(),
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
                let mut entries: Vec<(Current, String)> = self
                    .pages
                    .iter()
                    .enumerate()
                    .map(|(i, p)| (Current::Page(i), p.title().to_string()))
                    .collect();
                entries.extend(
                    self.panels
                        .in_slot(Slot::Page)
                        .map(|p| (Current::Panel(p.key.clone()), p.title.clone())),
                );
                for (target, title) in entries {
                    let selected = target == self.current;
                    let text = egui::RichText::new(title)
                        .size(15.0)
                        .color(c(theme::nav_label(t, selected)));
                    let resp = ui.add_sized(
                        [ui.available_width(), 32.0],
                        egui::Button::selectable(selected, text),
                    );
                    if resp.clicked() {
                        if let Current::Panel(key) = &target {
                            self.panels.reopen_panel(key);
                        }
                        self.current = target;
                    }
                }
            });
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
                if let Some(i) = self.pages.iter().position(|p| p.id() == target) {
                    self.current = Current::Page(i);
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
}

impl eframe::App for Hub {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let t = theme::tokens(ctx.theme() == egui::Theme::Dark);
        // Closing the window keeps the hub in the tray; Quit is in the tray menu.
        if ctx.input(|i| i.viewport().close_requested()) {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.shared.save_placement();
            platform::hide_main_window();
            log::info!("window closed to the tray");
        }
        if let Some(feed) = self.feed.as_mut() {
            let now = std::time::Instant::now();
            // Event-driven: the feed's waker brought this frame; a held-back patch asks for one
            // more repaint when its slot comes (no timer loop).
            let (_, next) = self.panels.pump(feed.as_mut(), now);
            if let Some(at) = next {
                ctx.request_repaint_after(at.saturating_duration_since(now));
            }
        }
        self.sidebar(ctx, &t);
        self.status_strip(ctx, &t);
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(c(t.bg))
                    .inner_margin(egui::Margin::same(24)),
            )
            .show(ctx, |ui| {
                match &self.current {
                    Current::Page(i) => {
                        if let Some(page) = self.pages.get_mut(*i) {
                            page.ui(ui, &t);
                        }
                    }
                    Current::Panel(key) => {
                        hideout_panels::panel_ui(&mut self.panels, ui, key);
                    }
                }
                // Input from the panel goes out on this frame, not the next one.
                // So does a change of what the content area shows (ui.visibility, 5.9).
                let shown = match &self.current {
                    Current::Panel(key) => Some(key),
                    Current::Page(_) => None,
                };
                self.panels.set_shown(shown);
                if let Some(feed) = self.feed.as_mut() {
                    for e in self.panels.take_events() {
                        feed.send(hideout_panels::Outbound::Event(e));
                    }
                    for (module, visibility) in self.panels.take_visibility() {
                        feed.send(hideout_panels::Outbound::Visibility { module, visibility });
                    }
                }
            });
        self.palette_ui(ctx, &t);
        let title = window_title(&match &self.current {
            Current::Page(i) => self
                .pages
                .get(*i)
                .map(|p| p.title().to_string())
                .unwrap_or_default(),
            Current::Panel(key) => self
                .panels
                .panel(key)
                .map(|p| p.title.clone())
                .unwrap_or_default(),
        });
        if title != self.title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.title = title;
        }
    }
}
