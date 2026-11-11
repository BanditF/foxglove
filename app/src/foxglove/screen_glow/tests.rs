//! Screen glow's reader with fake captures of recorded frames: no real screen is read, and no
//! window opens.

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use foxglove_scene::screen::ScreenImage;

use super::*;

/// What a fake screen does each time it is asked.
#[derive(Clone, Copy, PartialEq)]
enum Mode {
    /// It "changed": one recorded frame.
    Pictures,
    /// Access lost.
    Fails,
    /// Only the pointer moved (a mouse moving all the time).
    Pointer,
    /// Blocked in the wait for the whole of it (a still screen).
    Still,
}

struct Fake {
    frame: ScreenImage,
    reads: Arc<AtomicU32>,
    mode: Mode,
}

impl Capture for Fake {
    fn next(&mut self, wait: Duration) -> Result<Got, String> {
        match self.mode {
            Mode::Fails => Err("access lost".into()),
            Mode::Still => {
                std::thread::sleep(wait);
                Ok(Got::Unchanged)
            }
            Mode::Pointer => {
                self.reads.fetch_add(1, Ordering::SeqCst);
                Ok(Got::Pointer)
            }
            Mode::Pictures => {
                self.reads.fetch_add(1, Ordering::SeqCst);
                Ok(Got::Picture(self.frame.clone()))
            }
        }
    }
}

struct Rig {
    reader: Reader,
    opens: Arc<AtomicU32>,
    reads: Arc<AtomicU32>,
    fails: Arc<AtomicU32>,
    /// The generation of the last failure reported.
    failed_run: Arc<AtomicU64>,
    feed: Arc<Feed>,
}

fn rig(fail: bool) -> Rig {
    rig_with(if fail { Mode::Fails } else { Mode::Pictures })
}

fn rig_with(mode: Mode) -> Rig {
    let opens = Arc::new(AtomicU32::new(0));
    let reads = Arc::new(AtomicU32::new(0));
    let fails = Arc::new(AtomicU32::new(0));
    let failed_run = Arc::new(AtomicU64::new(0));
    let feed = Arc::new(Feed::default());
    let (o, r) = (opens.clone(), reads.clone());
    let open: Opener = Arc::new(move || {
        o.fetch_add(1, Ordering::SeqCst);
        let mut frame = ScreenImage::filled(r"\\.\DISPLAY1", 64, 36, [200, 30, 30]);
        frame.paint((0, 18, 64, 36), [30, 30, 200]);
        let screens: Vec<Box<dyn Capture>> = vec![Box::new(Fake {
            frame,
            reads: r.clone(),
            mode,
        })];
        Ok(screens)
    });
    let (f, run) = (fails.clone(), failed_run.clone());
    let reader = Reader::new(
        open,
        feed.clone(),
        Arc::new(move |generation| {
            run.store(generation, Ordering::SeqCst);
            f.fetch_add(1, Ordering::SeqCst);
        }),
        false,
    );
    Rig {
        reader,
        opens,
        reads,
        fails,
        failed_run,
        feed,
    }
}

/// Waits (bounded) until `n` failures in all were reported.
fn fails_arrive(r: &Rig, n: u32) {
    let until = Instant::now() + Duration::from_secs(5);
    while r.fails.load(Ordering::SeqCst) < n {
        assert!(Instant::now() < until, "the failure is reported");
        std::thread::sleep(Duration::from_millis(5));
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

/// Nothing is read for a while after `reading()` went false: the stopped threads end by
/// themselves (a frame already being taken may finish, but none is handed on).
fn no_reads(r: &Rig, why: &str) {
    assert!(!r.reader.reading(), "{why}: still reading");
    std::thread::sleep(MIN_GAP);
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
    fails_arrive(&r, 1);
    // The control thread hears it (with the run's generation) and holds; the next display
    // change lifts it.
    let run = r.failed_run.load(Ordering::SeqCst);
    assert_eq!(Some(run), r.reader.generation());
    r.reader.failed(run);
    assert_eq!(r.reader.gates().hold(), Some(Hold::Failed));
    assert!(!r.reader.reading());
    assert_eq!(r.fails.load(Ordering::SeqCst), 1);
    assert!(
        r.reader.gates().scene_held(),
        "the lights get their colours back"
    );
    r.reader.reopen();
    assert!(r.reader.reading(), "tried again");
    // The capture thread opens the screen again (and, this fake, fails again: once more).
    let until = Instant::now() + Duration::from_secs(5);
    while r.fails.load(Ordering::SeqCst) < 2 {
        assert!(Instant::now() < until, "opened again");
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(r.opens.load(Ordering::SeqCst), 2);
}

/// #496 review 1-3: a window that goes full screen while already in front holds it (the
/// full-screen event, no foreground change); a display change or a desktop switch restarts a
/// running capture, and a failure from the replaced run, arriving after, is ignored; in the
/// other order the failure holds and the change lifts it. One test: the full-screen fake is
/// shared.
#[test]
fn events_hold_and_reopen_in_either_order() {
    let mut r = rig(false);
    r.reader.set(|g| {
        g.on = true;
        g.allowed = true;
    });
    reads_arrive(&r, 1);
    // The game was in front windowed; now it goes full screen (Alt+Enter): only the
    // full-screen event says so.
    FAKE_FULL_SCREEN.store(1, Ordering::SeqCst);
    r.reader.event(Event::FullScreen);
    assert_eq!(r.reader.gates().hold(), Some(Hold::Game));
    assert!(r.reader.gates().scene_held());
    no_reads(&r, "a game went full screen in front");
    FAKE_FULL_SCREEN.store(2, Ordering::SeqCst);
    r.reader.event(Event::FullScreen);
    assert_eq!(r.reader.gates().hold(), Some(Hold::Presenting));
    FAKE_FULL_SCREEN.store(0, Ordering::SeqCst);
    r.reader.event(Event::FullScreen);
    assert!(r.reader.reading(), "left full screen");
    reads_arrive(&r, 1);

    // The display changes first; the old run's failure (access lost) comes after.
    let old = r.reader.generation().unwrap();
    r.reader.event(Event::DisplayChanged);
    let new = r.reader.generation().unwrap();
    assert_ne!(old, new, "a running capture is opened again");
    r.reader.event(Event::Failed(old));
    assert_eq!(
        r.reader.gates().hold(),
        None,
        "the replaced run's failure is ignored"
    );
    assert!(r.reader.reading());
    reads_arrive(&r, 1);

    // The other order: the failure first, then the change lifts it.
    r.reader.event(Event::Failed(new));
    assert_eq!(r.reader.gates().hold(), Some(Hold::Failed));
    assert!(!r.reader.reading());
    r.reader.event(Event::DisplayChanged);
    assert!(r.reader.reading());
    reads_arrive(&r, 1);

    // A UAC prompt or Ctrl+Alt+Del: access lost, then the desktop switches back.
    let run = r.reader.generation().unwrap();
    r.reader.event(Event::Failed(run));
    assert_eq!(r.reader.gates().hold(), Some(Hold::Failed));
    r.reader.event(Event::DesktopSwitched);
    assert!(r.reader.reading(), "opened again once");
    assert_eq!(r.reader.gates().hold(), None);
    reads_arrive(&r, 1);
    assert!(r.opens.load(Ordering::SeqCst) >= 4);
}

/// #496 review 5: a pointer that moves all the time wakes a screen's thread at most 20 times a
/// second, as pictures do.
#[test]
fn a_moving_pointer_wakes_it_at_most_twenty_times_a_second() {
    let mut r = rig_with(Mode::Pointer);
    r.reader.set(|g| {
        g.on = true;
        g.allowed = true;
    });
    reads_arrive(&r, 1);
    let start = r.reads.load(Ordering::SeqCst);
    std::thread::sleep(Duration::from_millis(1000));
    let n = r.reads.load(Ordering::SeqCst) - start;
    assert!(n <= 21, "{n} wake-ups in a second");
    assert!(r.feed.latest().is_empty(), "a pointer is no picture");
}

/// #496 review 6: a gate doesn't wait for a capture blocked in its wait for the screen; the
/// lights come back at once.
#[test]
fn a_gate_never_waits_for_a_blocked_capture() {
    let mut r = rig_with(Mode::Still);
    r.reader.set(|g| {
        g.on = true;
        g.allowed = true;
    });
    // The capture is inside its 1 s wait for a change.
    std::thread::sleep(Duration::from_millis(100));
    let at = Instant::now();
    r.reader.set(|g| g.paused = true);
    assert!(
        at.elapsed() < Duration::from_millis(200),
        "the gate waited {:?}",
        at.elapsed()
    );
    assert!(!r.reader.reading());
    // Back again at once too: the new run waits for the old one, not this thread.
    let at = Instant::now();
    r.reader.set(|g| g.paused = false);
    assert!(at.elapsed() < Duration::from_millis(200));
    assert!(r.reader.reading());
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
    assert!(Gates {
        presenting: true,
        ..on
    }
    .scene_held());
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
    assert_eq!(Hold::Presenting.words(None), "Paused during a presentation");
    assert_eq!(
        ASK_POINTS[1],
        "It pauses while the PC is locked or the display is off, and during full-screen games (borderless ones keep glowing)."
    );
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
