//! Screen glow's reader with fake captures of recorded frames: no real screen is read, and no
//! window opens.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use foxglove_scene::screen::ScreenImage;

use super::*;

/// A screen that "changes" every time it is asked, showing one recorded frame; or fails.
struct Fake {
    frame: ScreenImage,
    reads: Arc<AtomicU32>,
    fail: bool,
}

impl Capture for Fake {
    fn next(&mut self, _wait: Duration) -> Result<Option<ScreenImage>, String> {
        if self.fail {
            return Err("access lost".into());
        }
        self.reads.fetch_add(1, Ordering::SeqCst);
        Ok(Some(self.frame.clone()))
    }
}

struct Rig {
    reader: Reader,
    opens: Arc<AtomicU32>,
    reads: Arc<AtomicU32>,
    fails: Arc<AtomicU32>,
    feed: Arc<Feed>,
}

fn rig(fail: bool) -> Rig {
    let opens = Arc::new(AtomicU32::new(0));
    let reads = Arc::new(AtomicU32::new(0));
    let fails = Arc::new(AtomicU32::new(0));
    let feed = Arc::new(Feed::default());
    let (o, r) = (opens.clone(), reads.clone());
    let open: Opener = Arc::new(move || {
        o.fetch_add(1, Ordering::SeqCst);
        let mut frame = ScreenImage::filled(r"\\.\DISPLAY1", 64, 36, [200, 30, 30]);
        frame.paint((0, 18, 64, 36), [30, 30, 200]);
        let screens: Vec<Box<dyn Capture>> = vec![Box::new(Fake {
            frame,
            reads: r.clone(),
            fail,
        })];
        Ok(screens)
    });
    let f = fails.clone();
    let reader = Reader::new(
        open,
        feed.clone(),
        Arc::new(move || {
            f.fetch_add(1, Ordering::SeqCst);
        }),
        false,
    );
    Rig {
        reader,
        opens,
        reads,
        fails,
        feed,
    }
}

/// Waits (bounded) until `n` more pictures were read.
fn reads_arrive(r: &Rig, n: u32) {
    let start = r.reads.load(Ordering::SeqCst);
    let until = Instant::now() + Duration::from_secs(5);
    while r.reads.load(Ordering::SeqCst) < start + n {
        assert!(Instant::now() < until, "no pictures while reading");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Nothing is read for a while after `reading()` went false: the threads are joined.
fn no_reads(r: &Rig, why: &str) {
    assert!(!r.reader.reading(), "{why}: still reading");
    let before = r.reads.load(Ordering::SeqCst);
    std::thread::sleep(MIN_GAP * 4);
    assert_eq!(
        r.reads.load(Ordering::SeqCst),
        before,
        "{why}: read a picture"
    );
    assert!(r.feed.latest().is_empty(), "{why}: kept what it read");
}

#[test]
fn nothing_is_read_without_the_yes() {
    let mut r = rig(false);
    r.reader.set(|g| g.on = true);
    assert!(!r.reader.reading());
    std::thread::sleep(MIN_GAP * 2);
    assert_eq!(
        r.opens.load(Ordering::SeqCst),
        0,
        "the screen was never opened"
    );
    assert_eq!(r.reads.load(Ordering::SeqCst), 0);
    // The yes: reading starts.
    r.reader.set(|g| g.allowed = true);
    assert!(r.reader.reading());
    reads_arrive(&r, 2);
    // Stop allowing: it stops at once.
    r.reader.set(|g| g.allowed = false);
    no_reads(&r, "Stop allowing");
}

#[test]
fn each_gate_stops_capture_and_lifting_it_resumes() {
    type Flip = fn(&mut Gates, bool);
    let gates: [(&str, Flip, Hold); 6] = [
        ("Pause Foxglove", |g, on| g.paused = on, Hold::Paused),
        ("display off", |g, on| g.display_off = on, Hold::DisplayOff),
        ("PC locked", |g, on| g.locked = on, Hold::Locked),
        ("full-screen game", |g, on| g.game = on, Hold::Game),
        (
            "another app has the lights",
            |g, on| g.yielded = on,
            Hold::Yielded,
        ),
        ("capture failed", |g, on| g.failed = on, Hold::Failed),
    ];
    let mut r = rig(false);
    r.reader.set(|g| {
        g.on = true;
        g.allowed = true;
    });
    reads_arrive(&r, 1);
    for (why, flip, hold) in gates {
        r.reader.set(|g| flip(g, true));
        assert_eq!(r.reader.gates().hold(), Some(hold), "{why}");
        no_reads(&r, why);
        r.reader.set(|g| flip(g, false));
        assert!(r.reader.reading(), "{why} lifted");
        reads_arrive(&r, 1);
    }
    // Two at once: lifting one isn't enough.
    r.reader.set(|g| {
        g.locked = true;
        g.game = true;
    });
    r.reader.set(|g| g.locked = false);
    no_reads(&r, "still a game");
    r.reader.set(|g| g.game = false);
    reads_arrive(&r, 1);
    // Another look: Screen glow ends, so does reading.
    r.reader.set(|g| g.on = false);
    no_reads(&r, "another look");
}

#[test]
fn edges_reach_the_feed_and_go_with_the_reading() {
    let mut r = rig(false);
    r.reader.set(|g| {
        g.on = true;
        g.allowed = true;
    });
    reads_arrive(&r, 1);
    let until = Instant::now() + Duration::from_secs(5);
    while r.feed.latest().is_empty() {
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(5));
    }
    let e = &r.feed.latest()[0];
    assert_eq!(e.monitor, r"\\.\DISPLAY1");
    // Red on top, blue below: only the colours are kept, not the picture.
    assert!(e.top[0].0[0] > 150 && e.bottom[0].0[2] > 150, "{e:?}");
    drop(r.reader);
    assert!(r.feed.latest().is_empty(), "nothing kept after the scene");
}

#[test]
fn at_most_twenty_pictures_a_second() {
    let mut r = rig(false);
    r.reader.set(|g| {
        g.on = true;
        g.allowed = true;
    });
    reads_arrive(&r, 1);
    let start = r.reads.load(Ordering::SeqCst);
    std::thread::sleep(Duration::from_millis(1000));
    let n = r.reads.load(Ordering::SeqCst) - start;
    // A screen that changes all the time is still read at most 20 times a second (+1 for timing).
    assert!(n <= 21, "{n} pictures in a second");
    assert!(n >= 5, "still reads: {n}");
}

#[test]
fn a_failed_capture_says_so_once_and_waits_for_the_display() {
    let mut r = rig(true);
    r.reader.set(|g| {
        g.on = true;
        g.allowed = true;
    });
    let until = Instant::now() + Duration::from_secs(5);
    while r.fails.load(Ordering::SeqCst) == 0 {
        assert!(Instant::now() < until, "the failure is reported");
        std::thread::sleep(Duration::from_millis(5));
    }
    // The control thread hears it and holds; the next display change lifts it.
    r.reader.set(|g| g.failed = true);
    assert_eq!(r.reader.gates().hold(), Some(Hold::Failed));
    assert!(!r.reader.reading());
    assert_eq!(r.fails.load(Ordering::SeqCst), 1);
    assert!(
        r.reader.gates().scene_held(),
        "the lights get their colours back"
    );
    r.reader.set(|g| g.failed = false);
    assert!(r.reader.reading(), "tried again");
    // The capture thread opens the screen again (and, this fake, fails again: once more).
    let until = Instant::now() + Duration::from_secs(5);
    while r.fails.load(Ordering::SeqCst) < 2 {
        assert!(Instant::now() < until, "opened again");
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(r.opens.load(Ordering::SeqCst), 2);
}

#[test]
fn only_a_game_or_a_failure_holds_the_scene_itself() {
    let on = Gates {
        on: true,
        allowed: true,
        ..Gates::default()
    };
    assert!(on.reads() && !on.scene_held());
    assert!(Gates { game: true, ..on }.scene_held());
    assert!(Gates { failed: true, ..on }.scene_held());
    // The scene's own gates already handle these.
    for g in [
        Gates { paused: true, ..on },
        Gates { locked: true, ..on },
        Gates {
            display_off: true,
            ..on
        },
        Gates {
            yielded: true,
            ..on
        },
    ] {
        assert!(!g.reads() && !g.scene_held(), "{g:?}");
    }
    assert!(!Gates {
        on: false,
        game: true,
        ..on
    }
    .scene_held());
}

#[test]
fn tests_never_open_the_real_screen() {
    assert!(opener()().is_err());
}

#[test]
fn the_words_are_the_designs() {
    assert_eq!(Hold::Game.words(None), "Paused while a game is full screen");
    assert_eq!(Hold::Locked.words(None), "Paused while the PC is locked");
    assert_eq!(
        Hold::DisplayOff.words(None),
        "Paused while the display is off"
    );
    assert_eq!(
        Hold::Paused.words(None),
        "Paused: Foxglove is paused. Resume it from the tray."
    );
    assert_eq!(
        Hold::Yielded.words(Some("Windows Dynamic Lighting")),
        "Paused for Windows Dynamic Lighting"
    );
    assert_eq!(
        Hold::Failed.words(None),
        "Can't read the screen right now. Foxglove tries again when the display changes."
    );
    assert_eq!(tooltip_status(true), "Connected · Screen glow on");
    assert_eq!(tooltip_status(false), "Disconnected · Screen glow on");
    assert_eq!(
        diagnostics_line(true, "held (full-screen game)", 0.0),
        "Screen glow: allowed · held (full-screen game) · 0 updates/s"
    );
    assert_eq!(
        diagnostics_line(false, "stopped", 0.0),
        "Screen glow: not allowed · stopped · 0 updates/s"
    );
    assert_eq!(
        [Smoothing::Quick, Smoothing::Even, Smoothing::Calm].map(smoothing_label),
        ["Quick", "Even", "Calm"]
    );
    assert_eq!(RUNNING, "Running: Screen glow · reading your screen");
    assert_eq!(
        NEEDS_YES,
        "Screen glow didn't start: it needs your yes. Pick it and press Start."
    );
    // The smoothing default follows Windows' animation effects (on under test).
    assert_eq!(smoothing_for(None), Smoothing::Even);
    assert_eq!(smoothing_for(Some(Smoothing::Quick)), Smoothing::Quick);
}
