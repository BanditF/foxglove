//! Hideout's hub (#932, design Phase 2 H2.1): the tray, the window frame (sidebar, status strip,
//! command palette), theme and placement, from the shell Foxglove shares (`hub_shell`). For now
//! it starts empty, with only the Modules page. Lab only: never launched on a user desktop yet.
//!
//! One per user (its own single-instance names) and its own AUMID, so it never groups or hands
//! off with Foxglove. Its files live under `HIDEOUT_CONFIG_DIR` (tests, the lab), else
//! `HIDEOUT_HOME`, else `%LOCALAPPDATA%\Hideout` (spec §2.3).

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod config;
#[cfg(windows)]
mod tray;
mod ui;

use std::path::PathBuf;
use std::sync::Arc;

use hub_shell::placement::{self, Memory, NativeWindow, DEFAULT_SIZE, MIN_SIZE};

pub const APP: &str = "Hideout";
/// Hideout's AppUserModelID: its windows group under it, never under Foxglove's.
pub const AUMID: &str = "BanditF.Hideout";
const VERSION: &str = env!("CARGO_PKG_VERSION");

fn log(line: &str) {
    eprintln!("{line}");
}

fn main() -> eframe::Result {
    if !hub_shell::single_instance::claim(APP, config::override_dir().as_deref(), log) {
        return Ok(());
    }
    if let Err(e) = hub_shell::aumid::set(AUMID) {
        log(&format!("Hideout: no AUMID ({e}); Windows groups by path"));
    }
    let dir = config::dir();
    let prefs = dir.as_deref().map(config::Prefs::load).unwrap_or_default();

    let icon = eframe::icon_data::from_png_bytes(hub_shell::tray_glyph::png(
        hub_shell::tray_glyph::Glyph::Connected,
        false,
        32,
    ))
    .unwrap_or_default();
    let mut viewport = egui::ViewportBuilder::default()
        .with_title(APP)
        .with_inner_size(DEFAULT_SIZE)
        .with_min_inner_size(MIN_SIZE)
        .with_icon(Arc::new(icon));
    if let Some([left, top, right, bottom]) = first_place(&prefs.window) {
        viewport = viewport
            .with_position([left, top])
            .with_inner_size([right - left, bottom - top]);
    }
    let options = eframe::NativeOptions {
        renderer: eframe::Renderer::Glow,
        viewport,
        ..Default::default()
    };
    eframe::run_native(
        APP,
        options,
        Box::new(move |cc| {
            ui::apply_theme(&cc.egui_ctx, prefs.theme);
            let ctx = cc.egui_ctx.clone();
            let show: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                ctx.request_repaint();
            });
            hub_shell::single_instance::on_show({
                let show = show.clone();
                move || show()
            });
            hub_shell::taskbar::watch(APP, {
                let ctx = cc.egui_ctx.clone();
                move || ctx.request_repaint()
            });
            Ok(Box::new(Hideout {
                hub: ui::Hub::default(),
                memory: Memory::new(prefs.window.clone()),
                prefs,
                dir,
                focused: true,
                #[cfg(windows)]
                tray: tray::Tray::new(show),
            }))
        }),
    )
}

/// Where the window opens, in points for the builder: the remembered place (or a compact one
/// centred on the display under the mouse), converted with its display's scaling.
fn first_place(saved: &placement::Saved) -> Option<[f32; 4]> {
    let monitors = hub_shell::monitors::list();
    let fallback = hub_shell::monitors::cursor()
        .map(|p| placement::monitor_at(p, &monitors))
        .unwrap_or_default();
    let rect = placement::restore(
        saved,
        &monitors,
        [DEFAULT_SIZE.x, DEFAULT_SIZE.y],
        &fallback,
    )?;
    let id = placement::monitor_of(rect, &monitors);
    let s = monitors
        .iter()
        .find(|m| m.id == id)
        .map_or(1.0, hub_shell::monitors::Monitor::scale);
    Some(rect.map(|v| v as f32 / s))
}

/// The root window as `placement` sees it: read from egui's viewport info. The builder already
/// placed it, so moving and showing are nothing here.
struct Viewport<'a>(&'a egui::Context);

impl NativeWindow for Viewport<'_> {
    fn rect(&self) -> Option<[i32; 4]> {
        self.0.input(|i| {
            let v = i.viewport();
            if v.minimized == Some(true) || v.maximized == Some(true) {
                return None;
            }
            let r = v.outer_rect?;
            let s = v.native_pixels_per_point.unwrap_or(1.0);
            Some([r.left(), r.top(), r.right(), r.bottom()].map(|p| (p * s).round() as i32))
        })
    }

    fn dpi(&self) -> u32 {
        let s = self
            .0
            .input(|i| i.viewport().native_pixels_per_point.unwrap_or(1.0));
        (s * 96.0).round() as u32
    }

    fn move_to(&mut self, _: [i32; 4]) {}

    fn show(&mut self) {}
}

struct Hideout {
    hub: ui::Hub,
    prefs: config::Prefs,
    dir: Option<PathBuf>,
    memory: Memory,
    focused: bool,
    #[cfg(windows)]
    tray: tray::Tray,
}

impl Hideout {
    fn save_place(&mut self, closing: bool) {
        let monitors = hub_shell::monitors::list();
        let saved = if closing {
            self.memory.closed(&monitors)
        } else {
            self.memory.due_save(&monitors)
        };
        let (Some(saved), Some(dir)) = (saved, &self.dir) else {
            return;
        };
        self.prefs.window = saved;
        if let Err(e) = self.prefs.save(dir) {
            log(&format!("Hideout: can't save hideout.toml ({e})"));
        }
    }
}

impl eframe::App for Hideout {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        #[cfg(windows)]
        self.tray.frame();
        // Placed by the builder; this only notes where it is now.
        self.memory.frame(&mut Viewport(&ctx), &[], "");
        let focused = ctx.input(|i| i.viewport().focused.unwrap_or(true));
        if self.focused && !focused {
            self.save_place(false);
        }
        self.focused = focused;
        // Closing hides the window; the hub lives in the tray (Quit is in its menu).
        if ctx.input(|i| i.viewport().close_requested()) {
            self.save_place(true);
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        }
        self.hub.draw(ui, VERSION);
    }
}
