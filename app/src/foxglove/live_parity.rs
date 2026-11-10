//! The live feed's art against the keyboard's own report (#1053). A restart or a reconnect can
//! lose the in-memory state a published fact depends on (#1044: `Snapshot.sent`) while the
//! keyboard keeps playing; the wallpaper then draws its placeholder and nothing else says so.
//!
//! - **Health row** (`feed_art`, subsystem [`SUBSYSTEM`]): computed when the health view is read,
//!   from the keyboard's art status then (the art service's snapshot) and the `art` the feed last
//!   published. Warn when the keyboard shows a piece and the feed has `art: null`. No timer.
//! - **Log line**: the feed's hub notes what it publishes ([`Gap::note`], on each of its own
//!   events); a step into "`art: null` while the keyboard shows art" logs one line, once per
//!   step into it, not per message.
//! - No row while the feed is off (nothing published to compare) or the keyboard shows no piece.

use std::sync::Mutex;
use std::time::Instant;

use foxglove_link::protocol::{ArtStatus, RIGHT_STATES};

use super::art_words::{showing, Showing};
use super::health::{HealthRow, Level};
use super::live_wire as wire;

/// The row's subsystem in the health view (`feed_art`).
pub const SUBSYSTEM: &str = "live_feed.feed_art";
/// The row's name on the health page.
pub const ITEM: &str = "Live wallpaper art";

/// What the right screen shows by the keyboard's own report when it shows a piece ("playing
/// pushed set, set -1"); `None` with no keyboard, the relay not up (the right's fields are then
/// stale) or the right between pieces (receiving, saving, a refused set).
pub fn keyboard_shows(connected: bool, status: Option<ArtStatus>) -> Option<String> {
    let st = status.filter(|st| connected && st.link == 2)?;
    if matches!(showing(&st), Showing::Other(_)) {
        return None;
    }
    let state = RIGHT_STATES
        .get(usize::from(st.right_state))
        .copied()
        .unwrap_or("?");
    Some(format!("{state}, set {}", st.right_set))
}

/// The keyboard's side now, from the art service.
pub fn keyboard_now() -> Option<String> {
    let s = super::art_service::handle()?.snapshot();
    keyboard_shows(s.connected, s.status)
}

/// The `art` the feed published last, as far as the row needs it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Published {
    pub name: String,
    pub frames: bool,
    pub missing: Option<&'static str>,
}

impl Published {
    pub fn of(art: &wire::Art) -> Published {
        Published {
            name: art.name.clone(),
            frames: art.nva.is_some(),
            missing: art.missing,
        }
    }
}

/// The feed's side: what it published, and since when it has had no art while the keyboard
/// showed one (its hub's view).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Feed {
    pub art: Option<Published>,
    pub gap_since: Option<Instant>,
}

/// `None` while the feed is off.
static FEED: Mutex<Option<Feed>> = Mutex::new(None);

fn feed_now() -> Option<Feed> {
    FEED.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// The hub's record of the art it publishes, and the one log line per step into a gap.
#[derive(Debug, Default)]
pub struct Gap {
    last: Option<Feed>,
}

impl Gap {
    /// What the hub publishes now (`art`), against what the keyboard showed at its last read
    /// (`keyboard`). Returns the log line on a step into a gap, else `None`.
    pub fn step(
        &mut self,
        art: Option<&wire::Art>,
        keyboard: Option<&str>,
        now: Instant,
    ) -> Option<String> {
        let was_gap = self.last.as_ref().is_some_and(|f| f.gap_since.is_some());
        let gap = art.is_none() && keyboard.is_some();
        let feed = Feed {
            art: art.map(Published::of),
            gap_since: match (gap, &self.last) {
                (false, _) => None,
                (
                    true,
                    Some(Feed {
                        gap_since: Some(t), ..
                    }),
                ) => Some(*t),
                (true, _) => Some(now),
            },
        };
        if self.last.as_ref() != Some(&feed) {
            *FEED.lock().unwrap_or_else(|e| e.into_inner()) = Some(feed.clone());
            self.last = Some(feed);
        }
        (gap && !was_gap).then(|| format!("live feed: no art for {}", keyboard.unwrap_or("?")))
    }

    /// As [`Gap::step`], logging its line.
    pub fn note(&mut self, art: Option<&wire::Art>, keyboard: Option<&str>) {
        if let Some(line) = self.step(art, keyboard, Instant::now()) {
            eprintln!(
                "Foxglove: {line} (the keyboard shows it; the wallpaper draws its placeholder)"
            );
        }
    }
}

impl Drop for Gap {
    /// The hub ended: the feed is off, nothing to compare.
    fn drop(&mut self) {
        *FEED.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
}

fn ago(at: Instant, now: Instant) -> String {
    let s = now.saturating_duration_since(at).as_secs();
    match s {
        0..=59 => format!("{s} s"),
        60..=3599 => format!("{} min", s / 60),
        _ => format!("{} h", s / 3600),
    }
}

/// The `feed_art` row: the keyboard's side now against the feed's.
pub fn row(feed: Option<&Feed>, keyboard: Option<&str>, now: Instant) -> Option<HealthRow> {
    let feed = feed?;
    let keyboard = keyboard?;
    let (level, detail) = match &feed.art {
        None => {
            let mut d = format!(
                "the keyboard shows art ({keyboard}) but the live feed has none: the wallpaper shows its placeholder"
            );
            if let Some(t) = feed.gap_since {
                d.push_str(&format!(", for {}", ago(t, now)));
            }
            (Level::Warn, d)
        }
        Some(a) if a.frames => (Level::Ok, format!("{}, with its frames", a.name)),
        Some(a) => (
            Level::Ok,
            format!(
                "{}, without its frames ({})",
                a.name,
                a.missing.unwrap_or("unknown")
            ),
        ),
    };
    Some(HealthRow::new(SUBSYSTEM, ITEM, level, detail))
}

/// The row now (part of `live_feed::health_rows`).
pub fn rows_now() -> Vec<HealthRow> {
    row(
        feed_now().as_ref(),
        keyboard_now().as_deref(),
        Instant::now(),
    )
    .into_iter()
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use foxglove_link::protocol::right_state;
    use std::time::Duration;

    fn status(state: u8, set: i8) -> ArtStatus {
        ArtStatus {
            link: 2,
            right_state: state,
            right_set: set,
            right_set_count: 6,
            ..ArtStatus::default()
        }
    }

    fn art(frames: bool) -> wire::Art {
        wire::Art {
            name: "fury".into(),
            from: wire::FROM_FILE,
            nva: frames.then(|| "AAAA".into()),
            missing: (!frames).then_some(wire::MISSING_PUSHED_UNKNOWN),
        }
    }

    #[test]
    fn the_keyboard_shows_art_only_with_a_piece_on_screen() {
        let pushed = Some(status(right_state::PLAYING, -1));
        assert_eq!(
            keyboard_shows(true, pushed).as_deref(),
            Some("playing pushed set, set -1")
        );
        assert_eq!(
            keyboard_shows(true, Some(status(right_state::BUILT_IN, 2))).as_deref(),
            Some("built-in, set 2")
        );
        assert_eq!(keyboard_shows(false, pushed), None, "no keyboard");
        assert_eq!(
            keyboard_shows(
                true,
                Some(ArtStatus {
                    link: 0,
                    ..status(right_state::PLAYING, -1)
                })
            ),
            None,
            "the relay is down: the right's fields are stale"
        );
        assert_eq!(
            keyboard_shows(true, Some(status(right_state::RECEIVING, -1))),
            None
        );
        assert_eq!(keyboard_shows(true, None), None);
    }

    /// #1053: the keyboard plays a piece, the feed says `art: null` (the NO FRAMES restart):
    /// Warn with the reason. With art, frames or not, Ok; with the feed off or nothing on the
    /// keyboard, no row.
    #[test]
    fn the_row_warns_when_the_keyboard_shows_art_and_the_feed_has_none() {
        let now = Instant::now();
        let kb = Some("playing pushed set, set -1");
        let gap = Feed {
            art: None,
            gap_since: Some(now - Duration::from_secs(90)),
        };
        let r = row(Some(&gap), kb, now).unwrap();
        assert_eq!(
            (r.subsystem.as_str(), r.item.as_str(), r.level),
            (SUBSYSTEM, ITEM, Level::Warn)
        );
        assert_eq!(
            r.detail,
            "the keyboard shows art (playing pushed set, set -1) but the live feed has none: the \
             wallpaper shows its placeholder, for 1 min"
        );
        // The hub hasn't seen the gap yet (a read between the keyboard's change and the hub's):
        // still Warn, without a "for".
        let fresh = Feed {
            art: None,
            gap_since: None,
        };
        assert!(!row(Some(&fresh), kb, now).unwrap().detail.contains(", for"));
        let with = Feed {
            art: Some(Published::of(&art(true))),
            gap_since: None,
        };
        let r = row(Some(&with), kb, now).unwrap();
        assert_eq!(
            (r.level, r.detail.as_str()),
            (Level::Ok, "fury, with its frames")
        );
        let without = Feed {
            art: Some(Published::of(&art(false))),
            gap_since: None,
        };
        assert_eq!(
            row(Some(&without), kb, now).unwrap().detail,
            "fury, without its frames (pushed_unknown)"
        );
        assert_eq!(row(None, kb, now), None, "the feed is off");
        assert_eq!(row(Some(&gap), None, now), None, "nothing on the keyboard");
    }

    /// #1053: one log line per step into the gap, not one per message; leaving it and coming
    /// back logs again. The gap keeps the time it began.
    #[test]
    fn the_gap_logs_once_per_step_into_it() {
        let mut g = Gap::default();
        let t0 = Instant::now();
        let kb = Some("playing pushed set, set -1");
        let with = art(true);
        assert_eq!(g.step(Some(&with), kb, t0), None);
        assert_eq!(
            g.step(None, kb, t0).as_deref(),
            Some("live feed: no art for playing pushed set, set -1")
        );
        for i in 1..5 {
            assert_eq!(
                g.step(None, kb, t0 + Duration::from_secs(i)),
                None,
                "once, not per message"
            );
        }
        assert_eq!(g.last.as_ref().unwrap().gap_since, Some(t0));
        // No keyboard art: no gap, nothing logged.
        assert_eq!(g.step(None, None, t0), None);
        assert_eq!(g.last.as_ref().unwrap().gap_since, None);
        assert!(
            g.step(None, kb, t0).is_some(),
            "a new step into it logs again"
        );
        assert_eq!(g.step(Some(&with), kb, t0), None);
        assert!(g.step(None, kb, t0).is_some());
    }
}
