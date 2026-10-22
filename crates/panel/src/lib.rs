//! The left screen's host panel, shared by Foxglove and the standalone hostpanel:
//! - [`render`]: the layout, drawn onto a `digitalis` panel image
//! - [`providers`]: clock, volume, media and the foreground program, read from Windows
//! - [`Sources`]: keeps a [`render::View`] up to date from the providers
//! - [`panel_lock`]: only one app draws the panel at a time

pub mod panel_lock;
pub mod providers;
pub mod render;

use std::time::{Duration, Instant};

use render::View;

/// Fills a [`View`]: the clock every call, the rest at most once a second.
pub struct Sources {
    media: providers::Media,
    slow_at: Option<Instant>,
}

impl Sources {
    /// Call on the thread that will use it (COM is set up there).
    pub fn new() -> Self {
        providers::init_thread();
        Sources { media: providers::Media::new(), slow_at: None }
    }

    /// `hint_for` gives the hint for a lower-case exe name while that program is in front.
    pub fn update(&mut self, view: &mut View, hint_for: impl Fn(&str) -> Option<String>) {
        let now = chrono::Local::now();
        view.time = now.format("%H:%M").to_string();
        view.date = now.format("%a %d %b").to_string();
        if self.slow_at.is_some_and(|at| at.elapsed() < Duration::from_secs(1)) {
            return;
        }
        self.slow_at = Some(Instant::now());
        view.volume = providers::volume();
        match self.media.now_playing() {
            Some((title, artist, playing)) => {
                view.title = Some(title);
                view.artist = (!artist.is_empty()).then_some(artist);
                view.playing = playing;
            }
            None => (view.title, view.artist) = (None, None),
        }
        view.hint = providers::foreground_exe().and_then(|exe| hint_for(&exe.to_lowercase()));
    }
}

/// Local time as `YYYY-MM-DD HH:MM:SS`, for logs.
pub fn local_timestamp() -> String {
    chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

impl Default for Sources {
    fn default() -> Self {
        Self::new()
    }
}
