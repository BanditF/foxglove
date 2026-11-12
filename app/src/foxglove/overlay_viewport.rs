//! Foxglove (#1143, P2, H2.2 step 1): the one place that says which egui viewport is the overlay.
//!
//! Today the overlay is the root viewport (`platform/eframe_host.rs` builds it as the root
//! window), so [`overlay_viewport_id`] returns `ViewportId::ROOT`. Every wake, repaint request
//! and bring-to-front aimed at the overlay goes through it, so H2.2 can move the overlay to its
//! own viewport by changing this one function. No behaviour change today.
//!
//! A source test keeps any new `ViewportId::ROOT` out of the app, except where the root itself
//! is meant (see [`ALLOWED`]).

/// The viewport the overlay is drawn in.
pub fn overlay_viewport_id() -> egui::ViewportId {
    egui::ViewportId::ROOT
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    /// Files that may name `ViewportId::ROOT`, with the reason. Everything else asks
    /// [`overlay_viewport_id`].
    const ALLOWED: [(&str, &str); 4] = [
        ("src/foxglove/overlay_viewport.rs", "the seam itself"),
        (
            "src/foxglove/settings_window.rs",
            "tests only: Settings' parent viewport in a fed-by-hand frame",
        ),
        (
            "src/foxglove/theme_preview.rs",
            "tests only: native pixels per point of the test frame's root",
        ),
        (
            "src/foxglove/device_thumb_tests.rs",
            "tests only: reads the test frame's root repaint delay",
        ),
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

    #[test]
    fn no_new_root_viewport_outside_the_seam() {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = Vec::new();
        rust_files(&src, &mut files);
        assert!(files.len() > 100, "found the sources: {}", files.len());
        let needle = ["ViewportId", "::ROOT"].concat();
        let mut found = Vec::new();
        for file in &files {
            let rel = file
                .strip_prefix(src.parent().unwrap())
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            if ALLOWED.iter().any(|(p, _)| *p == rel) {
                continue;
            }
            let text = std::fs::read_to_string(file).unwrap();
            for (n, line) in text.lines().enumerate() {
                if line.contains(&needle) {
                    found.push(format!("{rel}:{}", n + 1));
                }
            }
        }
        assert!(
            found.is_empty(),
            "use overlay_viewport::overlay_viewport_id() (or add the file to ALLOWED with a reason): {found:#?}"
        );
    }

    /// Bringing the overlay to the front names the overlay's viewport, at every call site.
    #[test]
    fn bring_to_front_uses_the_seam() {
        let text = include_str!("tray_panel.rs");
        let calls = text.matches("focus::bring_to_front(").count();
        assert!(calls >= 3, "found the calls: {calls}");
        let seamed = text
            .split("focus::bring_to_front(")
            .skip(1)
            .filter(|rest| {
                let args: String = rest.chars().take(120).collect();
                args.contains("overlay_viewport_id()")
            })
            .count();
        assert_eq!(seamed, calls, "each bring_to_front asks the seam");
    }

    #[test]
    fn the_overlay_is_the_root_for_now() {
        assert_eq!(overlay_viewport_id(), egui::ViewportId::ROOT);
    }

    /// A wake from another thread asks for the overlay's viewport.
    #[test]
    fn wakes_ask_for_the_overlay_viewport() {
        use std::sync::{Arc, Mutex};
        let ctx = egui::Context::default();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = seen.clone();
        ctx.set_request_repaint_callback(move |info| sink.lock().unwrap().push(info.viewport_id));
        crate::ui_wake::UiWake::from_ctx(&ctx).request_repaint();
        assert_eq!(*seen.lock().unwrap(), vec![overlay_viewport_id()]);
    }
}
