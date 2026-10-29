//! Foxglove (#618): bringing a window to the front without faking input.
//!
//! On Windows, winit's `focus_window` (what egui's `ViewportCommand::Focus` runs) gets around
//! the foreground lock by pressing and releasing a fake Alt key with `SendInput` before
//! `SetForegroundWindow` (winit 0.30 `platform_impl/windows/window.rs`, `force_window_active`).
//! Windows counts that as the user's input (`GetLastInputInfo`), so every start (Settings opens
//! and focuses itself on its first frame), every tray panel opening and every second launch
//! looked like someone at the PC, which resets idle timers, screen savers and the lab's idle
//! gate. The app never injects input: on Windows, [`bring_to_front`] calls `SetForegroundWindow`
//! alone. When Windows refuses (the app wasn't given the foreground: not started by the
//! foreground app, no click on its icon, no second launch's `AllowSetForegroundWindow`), the
//! window still shows, behind, as Windows decides.
//!
//! Elsewhere winit's focus fakes no input, so egui's command is used as it is. A source test
//! keeps any other `ViewportCommand::Focus`, and the input-injection APIs, out of the app.

/// Brings `viewport`'s window (`hwnd` on Windows: its native handle) to the front and gives it
/// the keyboard focus, as far as Windows allows. With no handle known on Windows it does nothing.
pub fn bring_to_front(ctx: &egui::Context, viewport: egui::ViewportId, hwnd: Option<isize>) {
    #[cfg(windows)]
    {
        let _ = (ctx, viewport);
        if let Some(hwnd) = hwnd {
            use windows::Win32::Foundation::HWND;
            use windows::Win32::UI::WindowsAndMessaging::SetForegroundWindow;
            // SAFETY: a plain call on a window handle; a stale one just fails.
            let _ = unsafe { SetForegroundWindow(HWND(hwnd as *mut core::ffi::c_void)) };
        }
    }
    #[cfg(not(windows))]
    {
        let _ = hwnd;
        ctx.send_viewport_cmd_to(viewport, egui::ViewportCommand::Focus);
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    /// What would make the app inject input or fake the user's presence (or, for the last two,
    /// reach winit's fake Alt press on Windows).
    const BANNED: [&str; 8] = [
        "SendInput",
        "keybd_event",
        "mouse_event",
        "SetCursorPos",
        "SetThreadExecutionState",
        "SC_MONITORPOWER",
        "ViewportCommand::Focus",
        "focus_window(",
    ];

    fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                rust_files(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }

    /// #618: no code of the app's, nor of the PC-side crates, injects input, and nothing but
    /// [`super::bring_to_front`] (off Windows) asks egui for the focus. foxglove-lab is the test
    /// harness, which types on purpose and records each injection.
    #[test]
    fn the_app_injects_no_input() {
        let app = Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut files = Vec::new();
        rust_files(&app.join("src"), &mut files);
        for krate in std::fs::read_dir(app.join("../crates")).unwrap().flatten() {
            if krate.file_name() != "foxglove-lab" && krate.path().join("src").is_dir() {
                rust_files(&krate.path().join("src"), &mut files);
            }
        }
        assert!(files.len() > 100, "found the sources: {}", files.len());
        let me = Path::new("src/foxglove/focus.rs");
        let mut found = Vec::new();
        for file in files.iter().filter(|f| !f.ends_with(me)) {
            let text = std::fs::read_to_string(file).unwrap();
            for (n, line) in text.lines().enumerate() {
                for banned in BANNED.iter().filter(|b| line.contains(*b)) {
                    found.push(format!("{}:{}: {banned}", file.display(), n + 1));
                }
            }
        }
        assert!(
            found.is_empty(),
            "input injection or a winit focus: {found:#?}"
        );
    }
}
