use super::OverlayHost;
use crate::device_discovery::DiscoveredDevice;
use crate::overlay_window::OverlayApp;
use crate::settings::Settings;
use crate::ui_wake::UiWake;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

struct EframeHost<'a> {
    ctx: &'a egui::Context,
    #[cfg(target_os = "windows")]
    hwnd: Option<isize>,
}

impl OverlayHost for EframeHost<'_> {
    fn set_passthrough(&mut self, enabled: bool) {
        self.ctx
            .send_viewport_cmd(egui::ViewportCommand::MousePassthrough(enabled));
    }

    fn request_close(&mut self) {
        self.ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }

    #[cfg(target_os = "windows")]
    fn set_hit_region(&mut self, region: Option<[i32; 5]>) {
        if let Some(hwnd) = self.hwnd {
            set_window_region(hwnd, region);
        }
    }
}

/// Foxglove (#405): clips the overlay window to a rounded rectangle (window pixels), or gives
/// it the whole window back. Outside a window's region Windows sends the mouse to whatever is
/// underneath, so the tray panel takes only the clicks that land on it.
#[cfg(target_os = "windows")]
fn set_window_region(hwnd: isize, region: Option<[i32; 5]>) {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::Graphics::Gdi::{CreateRoundRectRgn, DeleteObject, SetWindowRgn, HGDIOBJ};

    let hwnd = HWND(hwnd as *mut core::ffi::c_void);
    let Some([left, top, right, bottom, corner]) = region else {
        // SAFETY: `hwnd` is the overlay window, alive while its frames run; a null region
        // removes the clip and there is nothing to free.
        unsafe {
            let _ = SetWindowRgn(hwnd, None, true);
        }
        return;
    };
    // SAFETY: plain GDI calls on a region this function owns. On success SetWindowRgn takes
    // the region over (the system frees it); on failure it's still ours, so it's freed here.
    unsafe {
        let rgn = CreateRoundRectRgn(left, top, right, bottom, corner, corner);
        if rgn.is_invalid() {
            eprintln!("Foxglove: can't make the tray panel's window region");
            return;
        }
        if SetWindowRgn(hwnd, Some(rgn), true) == 0 {
            eprintln!("Foxglove: can't set the tray panel's window region");
            let _ = DeleteObject(HGDIOBJ(rgn.0));
        }
    }
}

struct EframeApp {
    app: OverlayApp,
    // Undecorated transparent windows don't reliably honor `with_maximized`, so we
    // size to the monitor explicitly once known. Linux never WM-maximizes at all,
    // since Mutter drops always-on-top on a maximized window.
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    sized_to_monitor: bool,
    // winit's always-on-top request is sent before the window is mapped, which
    // EWMH WMs like Mutter ignore, so re-assert it for a few frames after mapping.
    #[cfg(target_os = "linux")]
    x11_above_ticks: u32,
    // The overlay window and the display it was last moved to. Starts as the
    // main display, where Windows maximizes the window on launch.
    #[cfg(target_os = "windows")]
    hwnd: Option<isize>,
    #[cfg(target_os = "windows")]
    placed_on_monitor: String,
    // The area the overlay covers, re-read from Windows now and then so a changed scale,
    // resolution or display layout moves it again; how many tries the current area has had.
    #[cfg(target_os = "windows")]
    monitor_rect: Option<[i32; 4]>,
    #[cfg(target_os = "windows")]
    placement_tries: u32,
    #[cfg(target_os = "windows")]
    displays_checked: Option<std::time::Instant>,
}

impl eframe::App for EframeApp {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        self.app.clear_color().to_array()
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();

        // Re-assert always-on-top now that the window is mapped (see field docs).
        #[cfg(target_os = "linux")]
        if self.x11_above_ticks > 0 {
            self.x11_above_ticks -= 1;
            ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(
                egui::WindowLevel::AlwaysOnTop,
            ));
            ctx.request_repaint();
        }

        #[cfg(any(target_os = "macos", target_os = "linux"))]
        if !self.sized_to_monitor {
            if let Some(monitor_size) = ctx.input(|i| i.viewport().monitor_size) {
                ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(egui::pos2(0.0, 0.0)));
                ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(monitor_size));
                self.sized_to_monitor = true;
            }
        }

        #[cfg(target_os = "windows")]
        self.keep_on_monitor(&ctx);

        let mut host = EframeHost {
            ctx: &ctx,
            #[cfg(target_os = "windows")]
            hwnd: self.hwnd,
        };
        self.app.ui(&ctx, &mut host);
        crate::foxglove::repaint_trace::frame(&ctx);
        crate::foxglove::gpu_reset::frame_drawn(); // Foxglove (#346)
    }
}

#[cfg(target_os = "windows")]
fn window_hwnd(cc: &eframe::CreationContext<'_>) -> Option<isize> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    match cc.window_handle().map(|h| h.as_raw()) {
        Ok(RawWindowHandle::Win32(handle)) => Some(handle.hwnd.get()),
        _ => None,
    }
}

#[cfg(target_os = "windows")]
impl EframeApp {
    /// Keeps the overlay window covering the chosen display's work area. The area is read
    /// from Windows when the choice changes and at most twice a second otherwise, so a
    /// changed scale, resolution or display layout is picked up while running.
    fn keep_on_monitor(&mut self, ctx: &egui::Context) {
        use std::time::{Duration, Instant};

        let Some(hwnd) = self.hwnd else {
            return;
        };
        let chosen_changed = self.placed_on_monitor != self.app.monitor();
        let due = self
            .displays_checked
            .is_none_or(|at| at.elapsed() >= Duration::from_millis(500));
        if !chosen_changed && !due {
            return;
        }
        self.displays_checked = Some(Instant::now());
        self.placed_on_monitor = self.app.monitor().to_string();

        let Some([left, top, right, bottom]) =
            crate::monitors::find(&crate::monitors::list(), &self.placed_on_monitor)
                .map(|m| m.work_area)
        else {
            return;
        };
        // One pixel short of the bottom: a borderless window covering a whole display counts as
        // fullscreen, and Windows/the GPU driver then present it directly, bypassing DWM, so its
        // transparent pixels turn black (here on a display with no taskbar).
        let target = [left, top, right, bottom - 1];
        if Some(target) != self.monitor_rect {
            self.monitor_rect = Some(target);
            self.placement_tries = 0;
        }
        // A few tries per area, so a window manager that insists on another size isn't fought
        // forever.
        if window_rect(hwnd) != Some(target) && self.placement_tries < 3 {
            self.placement_tries += 1;
            place_window(hwnd, target);
            ctx.request_repaint();
        }
    }
}

// Covers `area` (physical pixels) with the overlay window, like the maximized window the
// overlay starts as on the main display.
#[cfg(target_os = "windows")]
fn place_window(hwnd: isize, [left, top, right, bottom]: [i32; 4]) {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{
        SetWindowPos, ShowWindow, SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER, SW_SHOWNOACTIVATE,
    };

    let hwnd = HWND(hwnd as *mut core::ffi::c_void);
    unsafe {
        // A maximized window snaps back to its own display, so un-maximize first.
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        // Two steps: arriving on a display with different scaling makes Windows rescale the
        // window (WM_DPICHANGED), so first move it there, then give it the exact size.
        let _ = SetWindowPos(
            hwnd,
            None,
            left,
            top,
            0,
            0,
            SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
        );
        let _ = SetWindowPos(
            hwnd,
            None,
            left,
            top,
            right - left,
            bottom - top,
            SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
}

/// The window's outer rectangle in physical pixels: left, top, right, bottom.
#[cfg(target_os = "windows")]
fn window_rect(hwnd: isize) -> Option<[i32; 4]> {
    use windows::Win32::Foundation::{HWND, RECT};
    use windows::Win32::UI::WindowsAndMessaging::GetWindowRect;

    let mut r = RECT::default();
    unsafe { GetWindowRect(HWND(hwnd as *mut core::ffi::c_void), &mut r) }.ok()?;
    Some([r.left, r.top, r.right, r.bottom])
}

// Keep the overlay visible when switching Spaces or using fullscreen apps.
#[cfg(target_os = "macos")]
fn show_on_all_spaces(cc: &eframe::CreationContext<'_>) {
    use objc2_app_kit::{NSView, NSWindowCollectionBehavior};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    let Ok(handle) = cc.window_handle() else {
        return;
    };
    if let RawWindowHandle::AppKit(handle) = handle.as_raw() {
        let view = unsafe { handle.ns_view.cast::<NSView>().as_ref() };
        if let Some(window) = view.window() {
            window.setCollectionBehavior(
                NSWindowCollectionBehavior::CanJoinAllSpaces
                    | NSWindowCollectionBehavior::FullScreenAuxiliary,
            );
        }
    }
}

// DWM only composites a window's per-pixel alpha when asked to via
// `DwmEnableBlurBehindWindow`; otherwise the overlay can render opaque (black)
// on some systems. See https://github.com/srwi/keypeek/issues/16
#[cfg(target_os = "windows")]
fn enable_dwm_per_pixel_alpha(cc: &eframe::CreationContext<'_>) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows::core::BOOL;
    use windows::Win32::Foundation::HWND;
    use windows::Win32::Graphics::Dwm::{
        DwmEnableBlurBehindWindow, DWM_BB_BLURREGION, DWM_BB_ENABLE, DWM_BLURBEHIND,
    };
    use windows::Win32::Graphics::Gdi::{CreateRectRgn, DeleteObject, HGDIOBJ};

    let Ok(RawWindowHandle::Win32(handle)) = cc.window_handle().map(|h| h.as_raw()) else {
        return;
    };
    let hwnd = HWND(handle.hwnd.get() as *mut core::ffi::c_void);

    // A region covering the whole window makes DWM honor the window's alpha.
    let region = unsafe { CreateRectRgn(0, 0, -1, -1) };
    let blur_behind = DWM_BLURBEHIND {
        dwFlags: DWM_BB_ENABLE | DWM_BB_BLURREGION,
        fEnable: BOOL(1),
        hRgnBlur: region,
        ..Default::default()
    };
    unsafe {
        let _ = DwmEnableBlurBehindWindow(hwnd, &blur_behind);
        let _ = DeleteObject(HGDIOBJ(region.0));
    }
}

// `force_x11` (Linux only) makes winit use XWayland instead of native Wayland,
// since Mutter honors always-on-top for XWayland clients but not native ones.
pub fn run(
    settings: Settings,
    devices: Vec<DiscoveredDevice>,
    #[cfg_attr(not(target_os = "linux"), allow(unused_variables))] force_x11: bool,
) -> Result<(), eframe::Error> {
    #[cfg(target_os = "linux")]
    if force_x11 {
        match run_inner(settings.clone(), devices.clone(), true) {
            Ok(()) => return Ok(()),
            Err(e) => {
                eprintln!(
                    "Foxglove: XWayland/X11 backend unavailable ({e}); \
                     retrying on Wayland (overlay will not stay always-on-top)."
                );
            }
        }
    }
    run_inner(settings, devices, false)
}

fn run_inner(
    settings: Settings,
    devices: Vec<DiscoveredDevice>,
    #[cfg_attr(not(target_os = "linux"), allow(unused_variables))] force_x11: bool,
) -> Result<(), eframe::Error> {
    #[allow(unused_mut)]
    let mut viewport = egui::ViewportBuilder::default()
        .with_decorations(false)
        .with_taskbar(false)
        .with_maximized(true)
        .with_transparent(true)
        .with_has_shadow(false)
        .with_always_on_top()
        // Foxglove: the window icon (Alt+Tab, Linux), the same art as the exe's.
        .with_icon(std::sync::Arc::new(
            eframe::icon_data::from_png_bytes(include_bytes!("../../resources/icon.png")).unwrap_or_default(),
        ));

    #[cfg(target_os = "linux")]
    {
        viewport = viewport.with_window_type(egui::X11WindowType::Utility);
    }

    #[allow(unused_mut)]
    let mut options = eframe::NativeOptions {
        renderer: eframe::Renderer::Glow, // Glow is required for a transparent background (https://github.com/emilk/egui/issues/4451)
        viewport,
        ..Default::default()
    };

    // Hide from the macOS dock so the app only appears as a tray icon.
    #[cfg(target_os = "macos")]
    {
        options.event_loop_builder = Some(Box::new(|builder| {
            use winit::platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS};
            builder.with_activation_policy(ActivationPolicy::Accessory);
        }));
    }

    // Force XWayland so always-on-top is honored on GNOME (see `run`).
    #[cfg(target_os = "linux")]
    if force_x11 {
        options.event_loop_builder = Some(Box::new(|builder| {
            use winit::platform::x11::EventLoopBuilderExtX11;
            builder.with_x11();
        }));
    }

    eframe::run_native(
        "Foxglove",
        options,
        Box::new(move |cc| {
            #[cfg(target_os = "macos")]
            show_on_all_spaces(cc);
            #[cfg(target_os = "windows")]
            enable_dwm_per_pixel_alpha(cc);
            #[cfg(target_os = "windows")]
            let hwnd = window_hwnd(cc);

            egui_extras::install_image_loaders(&cc.egui_ctx);
            // Foxglove (#371): the GPU for "Copy diagnostics".
            if let Some(gl) = &cc.gl {
                use eframe::glow::HasContext;
                // SAFETY: plain string queries on the context eframe just made current.
                let (renderer, version) = unsafe { (gl.get_parameter_string(eframe::glow::RENDERER), gl.get_parameter_string(eframe::glow::VERSION)) };
                crate::foxglove::diagnostics::set_gpu(&renderer, &version);
            }

            let ui_wake = UiWake::from_ctx(&cc.egui_ctx);
            let settings_requested = Arc::new(AtomicBool::new(false));
            let tray_icon = crate::tray::create_tray_icon({
                let settings_requested = settings_requested.clone();
                let ui_wake = ui_wake.clone();
                Arc::new(move || {
                    settings_requested.store(true, Ordering::Relaxed);
                    ui_wake.request_repaint();
                })
            });
            // Foxglove (#345): a second launch opens the settings here and focuses this window.
            crate::foxglove::single_instance::on_show({
                let settings_requested = settings_requested.clone();
                let ctx = cc.egui_ctx.clone();
                let ui_wake = ui_wake.clone();
                move || {
                    settings_requested.store(true, Ordering::Relaxed);
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                    ui_wake.request_repaint();
                }
            });

            let mut fonts = egui::FontDefinitions::default();
            super::add_phosphor_to_fonts(&mut fonts);
            crate::foxglove::theme::add_fonts(&mut fonts); // Foxglove (#384)
            cc.egui_ctx.set_fonts(fonts);
            crate::foxglove::theme::apply(&cc.egui_ctx);

            let app = OverlayApp::new(tray_icon, settings_requested, ui_wake, settings, devices);
            Ok(Box::new(EframeApp {
                app,
                #[cfg(any(target_os = "macos", target_os = "linux"))]
                sized_to_monitor: false,
                #[cfg(target_os = "linux")]
                x11_above_ticks: 10,
                #[cfg(target_os = "windows")]
                hwnd,
                #[cfg(target_os = "windows")]
                placed_on_monitor: String::new(),
                #[cfg(target_os = "windows")]
                monitor_rect: None,
                #[cfg(target_os = "windows")]
                placement_tries: 0,
                #[cfg(target_os = "windows")]
                displays_checked: None,
            }))
        }),
    )
}
