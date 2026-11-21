//! Sound's level source (the plan: review/ux-plan-sound-effect.md, slice 2): how loud the PC is
//! playing, read from the default output device's own meter.
//!
//! What it reads, and what it never does:
//! - One number per read: the output device's peak, with whether there is a device, whether
//!   master is muted and how many sessions play ([`foxglove_panel::sessions::output`], asked of
//!   the audio sessions' thread the knob already runs). No samples, no capture stream, no
//!   loopback, and no input device (microphone) is ever opened.
//! - Nothing is kept: the newest level replaces the last, in memory only. Nothing is written
//!   to disk, sent to the hub or published as a fact.
//!
//! When it reads (no polling while there is nothing to follow):
//! - Only while a running scene's look is Sound and no gate holds the scene (away, paused,
//!   another app holding the lights): the same listener rule as Temperature
//!   ([`super::temperature_feed::Listener`]). Otherwise there is no thread at all.
//! - On a timer only while sound plays ([`PLAYING`]) or just went quiet ([`QUIET`]). With
//!   nothing playing, muted, no device or a failed read there is no timer: the thread sleeps
//!   until the audio sessions' thread says something changed ([`edge`]), and then reads once.
//! - Never more often than [`PLAYING`], whatever the events do.
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use foxglove_panel::sessions::Output;

/// Between reads while sound plays: 20 a second, the most the lights take.
pub const PLAYING: Duration = Duration::from_millis(50);
/// Between reads once it has been quiet for [`QUIET_AFTER_MS`].
pub const QUIET: Duration = Duration::from_millis(250);
/// Below the quiet line this long: Quiet.
pub const QUIET_AFTER_MS: i64 = 2_000;
/// This many failed reads in a row: "can't read", and no more tries until something changes.
pub const FAILS: u8 = 3;

/// Where the level comes from: the device's meter, or a test's.
pub trait LevelSource: Send {
    fn read(&self) -> Result<Output, String>;
}

/// The default output device's meter, through the audio sessions' thread.
pub struct DeviceMeter;

impl LevelSource for DeviceMeter {
    fn read(&self) -> Result<Output, String> {
        foxglove_panel::sessions::output()
    }
}

/// What the source says, for the status line and the card.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Status {
    /// Sound plays and the lights follow it.
    Playing,
    /// Something plays, below the quiet line.
    Quiet,
    /// No app is playing anything.
    #[default]
    Nothing,
    /// Master is muted, or at 0.
    Muted,
    /// There is no output device.
    NoDevice,
    /// The meter can't be read.
    CantRead,
}

impl Status {
    /// The words after "Running: Sound · ".
    pub fn words(self) -> &'static str {
        match self {
            Status::Playing => "Playing",
            Status::Quiet => "Quiet",
            Status::Nothing => "Nothing is playing",
            Status::Muted => "Your sound is muted",
            Status::NoDevice => "No speakers or headset are connected right now.",
            Status::CantRead => {
                "Foxglove can't read the sound level right now. It tries again when your sound \
                 device changes, or press Stop, then Start."
            }
        }
    }

    /// The lights follow the level (otherwise they rest at the floor).
    pub fn follows(self) -> bool {
        matches!(self, Status::Playing | Status::Quiet)
    }

    /// The card's word for the level bar: never a number.
    pub fn level_word(self, level: f32) -> &'static str {
        match self {
            Status::Playing if level > 0.0 => "Playing",
            Status::Muted => "Muted",
            Status::NoDevice | Status::CantRead => "No level",
            _ => "Quiet",
        }
    }
}

/// The keyboard's line in game mode, in place of Playing or Quiet.
pub const GAME_WORDS: &str = "Keyboard lights hold steady while you play";

/// The meter's state machine: pure, over the reads as they come and a clock in milliseconds.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Feed {
    status: Status,
    quiet_since: Option<i64>,
    fails: u8,
}

impl Feed {
    /// Takes one read at `t_ms`: the status, and the peak the lights follow (`None`: they rest
    /// at the floor).
    pub fn note(&mut self, read: &Result<Output, String>, t_ms: i64) -> (Status, Option<f32>) {
        let out = match read {
            Ok(o) if !o.device => {
                self.fails = 0;
                self.quiet_since = None;
                self.status = Status::NoDevice;
                return (self.status, None);
            }
            Ok(Output {
                peak: Some(peak), ..
            }) if peak.is_finite() => {
                self.fails = 0;
                *read.as_ref().expect("matched Ok")
            }
            // An error, or a device whose meter didn't answer.
            _ => {
                self.fails = self.fails.saturating_add(1).min(FAILS);
                if self.fails >= FAILS {
                    self.quiet_since = None;
                    self.status = Status::CantRead;
                }
                // Until then the status stands, and the lights rest: never a made-up level.
                return (self.status, None);
            }
        };
        let peak = out.peak.unwrap_or(0.0).clamp(0.0, 1.0);
        let level = super::sound::level(peak);
        if out.muted {
            self.quiet_since = None;
            self.status = Status::Muted;
            return (self.status, None);
        }
        if level > 0.0 {
            self.quiet_since = None;
            self.status = Status::Playing;
            return (self.status, Some(peak));
        }
        if out.active == 0 {
            self.quiet_since = None;
            self.status = Status::Nothing;
            return (self.status, None);
        }
        let since = *self.quiet_since.get_or_insert(t_ms);
        self.status = if t_ms - since >= QUIET_AFTER_MS {
            Status::Quiet
        } else {
            Status::Playing
        };
        (self.status, Some(peak))
    }

    /// When to read next without being told: only while it follows sound, or a failed read
    /// still has tries left. `None`: wait for an [`edge`].
    pub fn next(&self) -> Option<Duration> {
        if self.fails > 0 {
            return (self.fails < FAILS).then_some(QUIET);
        }
        match self.status {
            Status::Playing => Some(PLAYING),
            Status::Quiet => Some(QUIET),
            Status::Nothing | Status::Muted | Status::NoDevice | Status::CantRead => None,
        }
    }
}

/// What the lights and the page read: the newest status and peak. A level only, replaced by
/// each read.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Now {
    pub status: Status,
    /// The peak the lights follow; `None`: they rest at the floor.
    pub peak: Option<f32>,
}

// ---- the hand-offs: the scenes' wish, the reader thread, the newest level ----

struct Hub {
    /// The scenes that want the level, by their number (one per running scene).
    wants: Vec<u64>,
    /// The reader thread's ear, while it runs.
    wake: Option<Sender<()>>,
}

static HUB: Mutex<Hub> = Mutex::new(Hub {
    wants: Vec::new(),
    wake: None,
});
static NOW: Mutex<Now> = Mutex::new(Now {
    status: Status::Nothing,
    peak: None,
});
type Wake = Arc<dyn Fn() + Send + Sync>;
static ON_STATE: Mutex<Option<Wake>> = Mutex::new(None);
/// The keyboard is on its game layer: its lights hold the floor.
static GAME: AtomicBool = AtomicBool::new(false);

fn hub() -> std::sync::MutexGuard<'static, Hub> {
    HUB.lock().unwrap_or_else(|e| e.into_inner())
}

/// The newest status and peak.
pub fn latest() -> Now {
    *NOW.lock().unwrap_or_else(|e| e.into_inner())
}

/// The Scenes page's ear for a change of the status (an event, never polled).
pub fn on_state_change(wake: impl Fn() + Send + Sync + 'static) {
    *ON_STATE.lock().unwrap_or_else(|e| e.into_inner()) = Some(Arc::new(wake));
}

/// The keyboard's game layer came or went (`keyboard_facts::layers`): the keyboard's lights
/// hold steady meanwhile. Wakes the frames only on a change.
pub fn set_game(on: bool) {
    if GAME.swap(on, Ordering::Relaxed) != on {
        super::scene_run::nudge();
        state_changed();
    }
}

pub fn game() -> bool {
    GAME.load(Ordering::Relaxed)
}

fn state_changed() {
    let wake = ON_STATE.lock().unwrap_or_else(|e| e.into_inner()).clone();
    if let Some(wake) = wake {
        wake();
    }
}

fn publish(now: Now) {
    let before = std::mem::replace(&mut *NOW.lock().unwrap_or_else(|e| e.into_inner()), now);
    if before != now {
        // The frames sleep while the level is still.
        super::scene_run::nudge();
    }
    if before.status != now.status {
        state_changed();
    }
}

/// Whether any scene wants the level now.
pub fn wanted() -> bool {
    !hub().wants.is_empty()
}

/// One scene's wish: follow the sound, or not. The first wish starts the reader thread; it
/// ends itself when the last one goes.
pub fn want(scene: u64, on: bool) {
    want_from(scene, on, || Box::new(DeviceMeter));
}

fn want_from(scene: u64, on: bool, source: impl FnOnce() -> Box<dyn LevelSource>) {
    let mut h = hub();
    h.wants.retain(|s| *s != scene);
    if on {
        h.wants.push(scene);
    }
    match (&h.wake, h.wants.is_empty()) {
        (None, false) => {
            let (tx, rx) = channel();
            let source = source();
            let spawned = std::thread::Builder::new()
                .name("sound-level".into())
                .spawn(move || run(source, rx));
            match spawned {
                Ok(_) => {
                    h.wake = Some(tx);
                    watch();
                }
                Err(e) => eprintln!("Foxglove: sound: no reader thread: {e}"),
            }
        }
        // It looks again: a new wish reads now (Start is the user's retry), a last wish gone
        // ends it.
        (Some(tx), _) => {
            let _ = tx.send(());
        }
        (None, true) => {}
    }
}

/// Something the level depends on changed (a session came, went or changed state, master, the
/// device): a sleeping reader reads once. From the audio sessions' thread: only a send.
pub fn edge() {
    if let Some(tx) = &hub().wake {
        let _ = tx.send(());
    }
}

/// Hears the audio sessions' thread, once.
fn watch() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| foxglove_panel::sessions::watch_output(edge));
}

/// The reader thread: reads while a scene wants it, at the pace [`Feed::next`] says.
fn run(source: Box<dyn LevelSource>, rx: Receiver<()>) {
    let start = Instant::now();
    let mut meter = Meter::new(source);
    loop {
        {
            let mut h = hub();
            if h.wants.is_empty() {
                h.wake = None;
                drop(h);
                // Nothing follows: the level is forgotten.
                publish(Now::default());
                return;
            }
        }
        let read_at = Instant::now();
        let next = meter.step(start.elapsed().as_millis() as i64);
        publish(meter.now());
        // Never sooner than the fastest pace, whatever wakes it.
        let earliest = read_at + PLAYING;
        match next {
            // On a timer: an event changes nothing, the next read is coming (but a wish that
            // went ends it now).
            Some(every) => {
                let due = read_at + every;
                loop {
                    match rx.recv_timeout(due.saturating_duration_since(Instant::now())) {
                        Ok(()) if !wanted() => break,
                        Ok(()) => {}
                        Err(RecvTimeoutError::Timeout) => break,
                        Err(RecvTimeoutError::Disconnected) => return,
                    }
                }
            }
            // No timer: asleep until something changes.
            None => {
                if rx.recv().is_err() {
                    return;
                }
                while rx.try_recv().is_ok() {}
                if wanted() {
                    std::thread::sleep(earliest.saturating_duration_since(Instant::now()));
                    while rx.try_recv().is_ok() {}
                }
            }
        }
    }
}

/// One source and its state machine: what the thread steps, and what tests step with a fake
/// source and clock.
pub struct Meter {
    source: Box<dyn LevelSource>,
    feed: Feed,
    now: Now,
}

impl Meter {
    pub fn new(source: Box<dyn LevelSource>) -> Meter {
        Meter {
            source,
            feed: Feed::default(),
            now: Now::default(),
        }
    }

    /// Reads once at `t_ms`; returns when to read again without being told.
    pub fn step(&mut self, t_ms: i64) -> Option<Duration> {
        let (status, peak) = self.feed.note(&self.source.read(), t_ms);
        self.now = Now { status, peak };
        self.feed.next()
    }

    pub fn now(&self) -> Now {
        self.now
    }
}

/// The scene's listener for Sound: wants the level exactly while the look is Sound and nothing
/// holds the scene; the scene's end ends its wish, however it ends.
pub fn listener() -> super::temperature_feed::Listener {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let id = NEXT.fetch_add(1, Ordering::SeqCst);
    super::temperature_feed::Listener::new(Box::new(move |on| want(id, on)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    fn out(peak: f32, active: usize) -> Result<Output, String> {
        Ok(Output {
            device: true,
            peak: Some(peak),
            muted: false,
            active,
        })
    }

    #[test]
    fn the_status_lines_are_the_plans() {
        assert_eq!(Status::Playing.words(), "Playing");
        assert_eq!(Status::Quiet.words(), "Quiet");
        assert_eq!(Status::Nothing.words(), "Nothing is playing");
        assert_eq!(Status::Muted.words(), "Your sound is muted");
        assert_eq!(
            Status::NoDevice.words(),
            "No speakers or headset are connected right now."
        );
        assert!(Status::CantRead
            .words()
            .starts_with("Foxglove can't read the sound level right now."));
        assert_eq!(GAME_WORDS, "Keyboard lights hold steady while you play");
        // Never a word for the microphone, recording or listening in a status.
        for s in [
            Status::Playing,
            Status::Quiet,
            Status::Nothing,
            Status::Muted,
            Status::NoDevice,
            Status::CantRead,
        ] {
            let w = s.words().to_lowercase();
            for bad in [
                "listen",
                "record",
                "captur",
                "microphone",
                "loopback",
                "wasapi",
            ] {
                assert!(!w.contains(bad), "{w}");
            }
        }
    }

    #[test]
    fn playing_reads_on_the_fast_timer_and_quiet_comes_after_two_seconds() {
        let mut f = Feed::default();
        assert_eq!(f.note(&out(0.5, 1), 0), (Status::Playing, Some(0.5)));
        assert_eq!(f.next(), Some(PLAYING));
        // Below the quiet line, something still plays: Playing for 2 s, then Quiet.
        assert_eq!(f.note(&out(0.0, 1), 100).0, Status::Playing);
        assert_eq!(f.note(&out(0.001, 1), 2_000).0, Status::Playing);
        assert_eq!(f.note(&out(0.0, 1), 2_100), (Status::Quiet, Some(0.0)));
        assert_eq!(f.next(), Some(QUIET));
        // Sound again: Playing at once, and the 2 s start over.
        assert_eq!(f.note(&out(0.4, 1), 2_350).0, Status::Playing);
        assert_eq!(f.note(&out(0.0, 1), 2_400).0, Status::Playing);
        assert_eq!(f.note(&out(0.0, 1), 4_399).0, Status::Playing);
        assert_eq!(f.note(&out(0.0, 1), 4_400).0, Status::Quiet);
    }

    #[test]
    fn nothing_muted_and_no_device_have_no_timer_and_no_level() {
        let mut f = Feed::default();
        assert_eq!(f.note(&out(0.0, 0), 0), (Status::Nothing, None));
        assert_eq!(f.next(), None);
        // Muted: the meter still moves (it is before the volume), the lights don't.
        let muted = Ok(Output {
            device: true,
            peak: Some(0.8),
            muted: true,
            active: 2,
        });
        assert_eq!(f.note(&muted, 10), (Status::Muted, None));
        assert_eq!(f.next(), None);
        let none = Ok(Output::default());
        assert_eq!(f.note(&none, 20), (Status::NoDevice, None));
        assert_eq!(f.next(), None);
        // A level with no session said to play is still a level: the lights follow it.
        assert_eq!(f.note(&out(0.5, 0), 30), (Status::Playing, Some(0.5)));
    }

    #[test]
    fn three_failed_reads_say_cant_read_and_then_one_try_per_event() {
        let mut f = Feed::default();
        f.note(&out(0.5, 1), 0);
        let err: Result<Output, String> = Err("no".into());
        // Two failures: the status stands, the lights rest, and it tries again shortly.
        assert_eq!(f.note(&err, 50), (Status::Playing, None));
        assert_eq!(f.next(), Some(QUIET));
        assert_eq!(f.note(&err, 300), (Status::Playing, None));
        assert_eq!(f.next(), Some(QUIET));
        assert_eq!(f.note(&err, 550), (Status::CantRead, None));
        // No timer now: one read per event, and a failed one schedules nothing.
        assert_eq!(f.next(), None);
        assert_eq!(f.note(&err, 9_000), (Status::CantRead, None));
        assert_eq!(f.next(), None);
        // A device whose meter doesn't answer counts as a failed read too.
        let silent = Ok(Output {
            device: true,
            peak: None,
            muted: false,
            active: 1,
        });
        assert_eq!(f.note(&silent, 9_100), (Status::CantRead, None));
        // One good read ends it.
        assert_eq!(f.note(&out(0.5, 1), 9_200), (Status::Playing, Some(0.5)));
        assert_eq!(f.next(), Some(PLAYING));
    }

    #[test]
    fn a_peak_that_is_not_a_number_is_a_failed_read_never_a_level() {
        let mut f = Feed::default();
        for bad in [f32::NAN, f32::INFINITY] {
            assert_eq!(f.note(&out(bad, 1), 0).1, None);
        }
        // Out of range is clamped.
        let mut f = Feed::default();
        assert_eq!(f.note(&out(7.0, 1), 0), (Status::Playing, Some(1.0)));
    }

    struct Fake(Arc<AtomicUsize>, Arc<Mutex<Result<Output, String>>>);

    impl LevelSource for Fake {
        fn read(&self) -> Result<Output, String> {
            self.0.fetch_add(1, Ordering::SeqCst);
            self.1.lock().unwrap().clone()
        }
    }

    #[test]
    fn the_meter_reads_once_per_step_and_asks_for_no_timer_when_nothing_plays() {
        let reads = Arc::new(AtomicUsize::new(0));
        let says = Arc::new(Mutex::new(out(0.0, 0)));
        let mut m = Meter::new(Box::new(Fake(reads.clone(), says.clone())));
        assert_eq!(m.step(0), None);
        assert_eq!(reads.load(Ordering::SeqCst), 1);
        assert_eq!(m.now().status, Status::Nothing);
        *says.lock().unwrap() = out(0.3, 1);
        assert_eq!(m.step(40), Some(PLAYING));
        assert_eq!(reads.load(Ordering::SeqCst), 2);
        assert_eq!(m.now().peak, Some(0.3));
    }

    /// The reader thread exists only while a scene wants the level, reads on events when
    /// nothing plays, and forgets the level when the wish ends. One test owns the statics.
    #[test]
    fn the_reader_thread_lives_only_while_wanted_and_sleeps_when_nothing_plays() {
        let wait = |what: &str, ok: &dyn Fn() -> bool| {
            let until = Instant::now() + Duration::from_secs(5);
            while !ok() {
                assert!(Instant::now() < until, "{what}");
                std::thread::sleep(Duration::from_millis(5));
            }
        };
        let reads = Arc::new(AtomicUsize::new(0));
        let says = Arc::new(Mutex::new(out(0.0, 0)));
        assert!(!wanted());
        assert!(hub().wake.is_none(), "no thread before a wish");
        let (r, s) = (reads.clone(), says.clone());
        want_from(7_001, true, move || Box::new(Fake(r, s)));
        wait("the first read", &|| reads.load(Ordering::SeqCst) == 1);
        // Nothing plays: no timer, so no second read however long it waits.
        std::thread::sleep(PLAYING * 6);
        assert_eq!(reads.load(Ordering::SeqCst), 1);
        assert_eq!(latest().status, Status::Nothing);
        // An event reads once.
        *says.lock().unwrap() = Ok(Output {
            device: true,
            peak: Some(0.9),
            muted: true,
            active: 1,
        });
        edge();
        wait("the event's read", &|| reads.load(Ordering::SeqCst) == 2);
        assert_eq!(latest().status, Status::Muted);
        std::thread::sleep(PLAYING * 6);
        assert_eq!(reads.load(Ordering::SeqCst), 2, "muted: no timer");
        // Playing: the timer runs.
        *says.lock().unwrap() = out(0.5, 1);
        edge();
        wait("the timer's reads", &|| reads.load(Ordering::SeqCst) >= 5);
        assert_eq!(latest().peak, Some(0.5));
        // The wish ends: the thread ends, the level is forgotten, the reads stop.
        want_from(7_001, false, || {
            unreachable!("no thread for a wish that ends")
        });
        wait("the thread's end", &|| hub().wake.is_none());
        assert_eq!(latest(), Now::default());
        let after = reads.load(Ordering::SeqCst);
        std::thread::sleep(PLAYING * 6);
        assert_eq!(reads.load(Ordering::SeqCst), after);
    }
}
