//! The left screen's host panel, shared by Foxglove and the standalone hostpanel:
//! - [`render`]: the layout, drawn onto a `digitalis` panel image
//! - [`providers`]: clock, volume, media and the foreground program, read from Windows
//! - [`Sources`]: keeps a [`render::View`] up to date from the providers
//! - [`panel_lock`]: only one app draws the panel at a time

pub mod changes;
pub mod mic;
pub mod panel_lock;
pub mod providers;
pub mod render;

use chrono::Timelike;
use std::time::{Duration, Instant};

use render::View;

/// Fills a [`View`] when a provider announces a change.
pub struct Sources {
    media: providers::Media,
}

impl Sources {
    /// Call on the thread that will use it (COM is set up there).
    pub fn new() -> Self {
        providers::init_thread();
        Sources { media: providers::Media::new() }
    }

    /// Update time-based content and return its next deadline, using the same time sample.
    pub fn update_clock(view: &mut View) -> Instant {
        let start = Instant::now();
        let now = chrono::Local::now();
        view.time = now.format("%H:%M").to_string();
        view.date = now.format("%a %d %b").to_string();
        start + until_minute(now.second(), now.nanosecond())
    }

    /// `hint_for` gives the hint for a lower-case exe name while that program is in front.
    pub fn update(&mut self, view: &mut View, hint_for: impl Fn(&str) -> Option<String>) -> Instant {
        let clock_at = Self::update_clock(view);
        view.volume = providers::volume();
        match self.media.now_playing() {
            Some((title, artist, playing)) => {
                view.title = Some(title);
                view.artist = (!artist.is_empty()).then_some(artist);
                view.playing = playing;
            }
            None => {
                (view.title, view.artist) = (None, None);
                view.playing = false;
            }
        }
        view.hint = providers::foreground_exe().and_then(|exe| hint_for(&exe.to_lowercase()));
        clock_at
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

fn until_minute(second: u32, nanosecond: u32) -> Duration {
    Duration::from_secs(60 - u64::from(second)) - Duration::from_nanos(u64::from(nanosecond))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn clock_waits_for_a_minute_boundary_including_fractional_seconds() {
        assert_eq!(until_minute(0, 0), Duration::from_secs(60));
        assert_eq!(until_minute(59, 999_000_000), Duration::from_millis(1));
        assert_eq!(until_minute(12, 250_000_000), Duration::from_millis(47_750));
    }
}
