//! When frames are drawn (#484). A scene renders only while it runs and someone can see it:
//! the gates are the scene itself, away (session locked or display off, the app's `away`) and
//! Pause Foxglove (the app's `pause`). Each change of a gate is an event the app passes to
//! [`Scheduler::set`]; nothing here watches for them.
//!
//! - No scene, or any gate closed: no thread, no timer, no wake-ups.
//! - A scene running: one thread waits the frame interval between frames (the shortest device
//!   interval, never faster than 30 fps) and stops at once when a gate closes.
//! - A scene that knows when it has nothing new ([`Scheduler::paced`]: Screen glow on a still
//!   screen, #1085) says so after each frame ([`Pace`]). The thread then sleeps until a
//!   [`Waker`] wakes it (a new picture, a control event) or until the time it named, with no
//!   wake-ups at the frame interval meanwhile.
//!
//! Scene time comes from the clock, not from counting frames, so a late or skipped frame never
//! puts the lights out of step with a wallpaper on the same epoch.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, TryRecvError};
use std::sync::{Arc, Mutex, Weak};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::clock::{Clock, SceneTime};

/// The fastest any device is sent frames.
pub const MAX_FPS: u32 = 30;

/// The shortest interval at [`MAX_FPS`].
pub const MIN_INTERVAL: Duration = Duration::from_micros(1_000_000_u64.div_ceil(MAX_FPS as u64));

/// A device's frame interval: its own shortest update interval (a LampArray's
/// `MinUpdateInterval`, a measured case board's), and never faster than [`MAX_FPS`].
pub fn device_interval(min_update: Option<Duration>) -> Duration {
    min_update.unwrap_or_default().max(MIN_INTERVAL)
}

/// Which devices are due a frame, each at its own interval.
#[derive(Clone, Debug, Default)]
pub struct Pacing {
    devices: Vec<(String, Duration, Option<SceneTime>)>,
}

impl Pacing {
    /// Each device's id and its own shortest update interval, if known.
    pub fn new(devices: impl IntoIterator<Item = (String, Option<Duration>)>) -> Self {
        Pacing {
            devices: devices
                .into_iter()
                .map(|(id, min)| (id, device_interval(min), None))
                .collect(),
        }
    }

    /// How long the scheduler waits between frames: the fastest device's interval.
    pub fn tick(&self) -> Duration {
        self.devices
            .iter()
            .map(|(_, i, _)| *i)
            .min()
            .unwrap_or(MIN_INTERVAL)
    }

    /// The slowest device's interval: within it every device has had a frame.
    pub fn longest(&self) -> Duration {
        self.devices
            .iter()
            .map(|(_, i, _)| *i)
            .max()
            .unwrap_or(MIN_INTERVAL)
    }

    /// The devices whose interval has passed since their last frame at `t` (all of them the
    /// first time); they count as sent.
    pub fn due(&mut self, t: SceneTime) -> Vec<String> {
        let mut due = Vec::new();
        for (id, interval, last) in &mut self.devices {
            // A tenth of slack, so a timer that wakes a little early doesn't skip a frame.
            let wait = (interval.as_millis() as i64 * 9 / 10).max(1);
            if last.is_none_or(|l| t.ms() - l.ms() >= wait || t < l) {
                *last = Some(t);
                due.push(id.clone());
            }
        }
        due
    }
}

/// One of the conditions a scene renders under.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gate {
    /// A scene is running (on: started, off: stopped).
    Scene,
    /// The session is locked or the display is off.
    Away,
    /// Foxglove is paused.
    Paused,
    /// The scene can't draw for a reason of its own: Screen glow while a game is full screen
    /// or the screen can't be read (#496).
    Held,
}

/// The gates' state.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Gates {
    pub scene: bool,
    pub away: bool,
    pub paused: bool,
    pub held: bool,
}

impl Gates {
    /// Frames are drawn only with a scene, someone present, Foxglove not paused and the scene
    /// not held.
    pub fn open(&self) -> bool {
        self.scene && !self.away && !self.paused && !self.held
    }

    fn set(&mut self, gate: Gate, on: bool) {
        match gate {
            Gate::Scene => self.scene = on,
            Gate::Away => self.away = on,
            Gate::Paused => self.paused = on,
            Gate::Held => self.held = on,
        }
    }
}

/// What the frame thread does after a frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pace {
    /// The next frame after the tick (the frame interval); a wake doesn't bring it sooner.
    Tick,
    /// The next frame after this long, or sooner when woken.
    After(Duration),
    /// Nothing new: no frame until woken.
    Idle,
}

type Render = Arc<Mutex<Box<dyn FnMut(SceneTime) -> Pace + Send>>>;

enum Msg {
    Wake,
    Stop,
}

struct Run {
    stop: mpsc::Sender<Msg>,
    thread: JoinHandle<()>,
}

/// What the frame thread shares with its [`Waker`]s.
#[derive(Default)]
struct Shared {
    /// The running frame thread's channel; `None` while there is none.
    tx: Mutex<Option<mpsc::Sender<Msg>>>,
    /// Frames drawn so far (each is one wake of the frame thread).
    frames: AtomicU64,
}

impl Shared {
    fn tx(&self) -> std::sync::MutexGuard<'_, Option<mpsc::Sender<Msg>>> {
        self.tx.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// Wakes a paced scene's frame thread for a frame now, from any thread: a new picture, an
/// output that wants its next frame in full. Does nothing while no frame thread runs, or once
/// the scheduler is gone.
#[derive(Clone, Default)]
pub struct Waker(Weak<Shared>);

impl std::fmt::Debug for Waker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Waker")
    }
}

impl Waker {
    pub fn wake(&self) {
        if let Some(shared) = self.0.upgrade() {
            if let Some(tx) = shared.tx().as_ref() {
                let _ = tx.send(Msg::Wake);
            }
        }
    }
}

struct State {
    gates: Gates,
    run: Option<Run>,
}

/// Runs a render callback on the scene clock while the gates are open.
pub struct Scheduler {
    clock: Clock,
    tick: Duration,
    render: Render,
    state: Mutex<State>,
    shared: Arc<Shared>,
}

impl Scheduler {
    /// `render` draws and sends one frame at a scene time. It runs on the scheduler's own
    /// thread and must not call [`Scheduler::set`].
    pub fn new(
        clock: Clock,
        tick: Duration,
        mut render: impl FnMut(SceneTime) + Send + 'static,
    ) -> Self {
        Self::paced(clock, tick, move |t| {
            render(t);
            Pace::Tick
        })
    }

    /// As [`Scheduler::new`], but `render` says after each frame when the next is due
    /// ([`Pace`]); [`Scheduler::waker`] brings one sooner.
    pub fn paced(
        clock: Clock,
        tick: Duration,
        render: impl FnMut(SceneTime) -> Pace + Send + 'static,
    ) -> Self {
        Scheduler {
            clock,
            tick: tick.max(MIN_INTERVAL),
            render: Arc::new(Mutex::new(Box::new(render))),
            state: Mutex::new(State {
                gates: Gates::default(),
                run: None,
            }),
            shared: Arc::default(),
        }
    }

    /// Wakes the frame thread for a frame now (see [`Waker`]).
    pub fn waker(&self) -> Waker {
        Waker(Arc::downgrade(&self.shared))
    }

    /// Frames drawn so far: each is one wake of the frame thread.
    pub fn frames(&self) -> u64 {
        self.shared.frames.load(Ordering::Relaxed)
    }

    pub fn clock(&self) -> Clock {
        self.clock
    }

    pub fn gates(&self) -> Gates {
        self.lock().gates
    }

    /// Whether the frame thread exists now.
    pub fn running(&self) -> bool {
        self.lock().run.is_some()
    }

    /// One gate changed. Opening the last gate starts the frame thread (the first frame at
    /// once); closing any stops it before this returns, waiting at most for a frame already
    /// being drawn. Call it from the app's own worker, not from a system callback.
    pub fn set(&self, gate: Gate, on: bool) {
        let mut state = self.lock();
        state.gates.set(gate, on);
        match (state.gates.open(), state.run.is_some()) {
            // No thread to be had: no frames, and the next gate change tries again.
            (true, false) => state.run = self.spawn(),
            (false, true) => {
                if let Some(run) = state.run.take() {
                    self.stop(run);
                }
            }
            _ => {}
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn spawn(&self) -> Option<Run> {
        let (stop, wait) = mpsc::channel::<Msg>();
        let (clock, tick, render) = (self.clock, self.tick, self.render.clone());
        let shared = self.shared.clone();
        // Wakes go to this thread from now on (one before its first frame changes nothing:
        // that frame comes at once).
        *self.shared.tx() = Some(stop.clone());
        let thread = std::thread::Builder::new()
            .name("scene-frames".into())
            .spawn(move || loop {
                shared.frames.fetch_add(1, Ordering::Relaxed);
                let pace = (render.lock().unwrap_or_else(|e| e.into_inner()))(clock.now());
                if !wait_for_next(&wait, pace, tick) {
                    break;
                }
            });
        match thread {
            Ok(thread) => Some(Run { stop, thread }),
            Err(e) => {
                *self.shared.tx() = None;
                eprintln!("Foxglove: no thread for scene frames: {e}");
                None
            }
        }
    }

    fn stop(&self, run: Run) {
        *self.shared.tx() = None;
        let _ = run.stop.send(Msg::Stop);
        let _ = run.thread.join();
    }
}

/// Waits for the next frame as `pace` says; false when the thread should end (stopped, or the
/// scheduler is gone). Wakes that queued up meanwhile count as one.
fn wait_for_next(rx: &mpsc::Receiver<Msg>, pace: Pace, tick: Duration) -> bool {
    let (deadline, early) = match pace {
        Pace::Tick => (Some(Instant::now() + tick), false),
        Pace::After(d) => (Some(Instant::now() + d), true),
        Pace::Idle => (None, true),
    };
    loop {
        let got = match deadline {
            Some(at) => rx.recv_timeout(at.saturating_duration_since(Instant::now())),
            None => rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
        };
        match got {
            Ok(Msg::Wake) if early => break,
            // Ticking anyway: the frame comes at the tick.
            Ok(Msg::Wake) => {}
            Err(RecvTimeoutError::Timeout) => break,
            Ok(Msg::Stop) | Err(RecvTimeoutError::Disconnected) => return false,
        }
    }
    loop {
        match rx.try_recv() {
            Ok(Msg::Wake) => {}
            Ok(Msg::Stop) | Err(TryRecvError::Disconnected) => return false,
            Err(TryRecvError::Empty) => return true,
        }
    }
}

impl Drop for Scheduler {
    fn drop(&mut self) {
        let run = self.lock().run.take();
        if let Some(run) = run {
            self.stop(run);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// A scheduler whose frames count up and report on a channel.
    fn counting() -> (Scheduler, Arc<AtomicU32>, mpsc::Receiver<SceneTime>) {
        let frames = Arc::new(AtomicU32::new(0));
        let (tx, rx) = mpsc::channel();
        let seen = frames.clone();
        let s = Scheduler::new(Clock::starting_now(), MIN_INTERVAL, move |t| {
            seen.fetch_add(1, Ordering::SeqCst);
            let _ = tx.send(t);
        });
        (s, frames, rx)
    }

    /// Waits (bounded) for `n` more frames.
    fn frames_arrive(rx: &mpsc::Receiver<SceneTime>, n: usize) {
        for _ in 0..n {
            rx.recv_timeout(Duration::from_secs(5))
                .expect("a frame while the scene runs");
        }
    }

    #[test]
    fn no_scene_means_no_thread_and_no_frames() {
        let (s, frames, _rx) = counting();
        s.set(Gate::Away, true);
        s.set(Gate::Away, false);
        s.set(Gate::Paused, true);
        s.set(Gate::Paused, false);
        assert!(!s.running());
        assert_eq!(frames.load(Ordering::SeqCst), 0, "zero scene wake-ups");
    }

    #[test]
    fn display_off_lock_and_pause_each_stop_rendering() {
        let (s, frames, rx) = counting();
        s.set(Gate::Scene, true);
        assert!(s.running());
        frames_arrive(&rx, 2);
        // Away stands for both the lock and the display going off (the app's `away`).
        for (gate, closed) in [
            (Gate::Away, true),
            (Gate::Paused, true),
            (Gate::Held, true),
            (Gate::Scene, false),
        ] {
            s.set(gate, closed);
            assert!(!s.running(), "{gate:?} stops the thread");
            // The thread is joined: nothing can draw any more.
            while rx.try_recv().is_ok() {}
            let after = frames.load(Ordering::SeqCst);
            std::thread::sleep(MIN_INTERVAL * 3);
            assert_eq!(frames.load(Ordering::SeqCst), after, "{gate:?}");
            assert!(rx.try_recv().is_err());
            // Opening it again resumes.
            s.set(gate, !closed);
            assert!(s.running(), "{gate:?} reopened");
            frames_arrive(&rx, 1);
        }
        // Two gates closed: reopening one isn't enough.
        s.set(Gate::Away, true);
        s.set(Gate::Paused, true);
        s.set(Gate::Away, false);
        assert!(!s.running());
        s.set(Gate::Paused, false);
        assert!(s.running());
        drop(s);
    }

    /// #1085: a paced scene with nothing new sleeps (no frame at the interval) until woken, and
    /// one wake is one frame however many queue up.
    #[test]
    fn an_idle_scene_sleeps_until_woken() {
        let (tx, rx) = mpsc::channel();
        let s = Scheduler::paced(Clock::starting_now(), MIN_INTERVAL, move |t| {
            let _ = tx.send(t);
            Pace::Idle
        });
        let waker = s.waker();
        waker.wake(); // no frame thread yet: nothing
        s.set(Gate::Scene, true);
        frames_arrive(&rx, 1);
        std::thread::sleep(MIN_INTERVAL * 6);
        assert!(rx.try_recv().is_err(), "no frame while idle");
        assert_eq!(s.frames(), 1, "one wake: the first frame");
        waker.wake();
        frames_arrive(&rx, 1);
        std::thread::sleep(MIN_INTERVAL * 3);
        assert_eq!(s.frames(), 2);
        // Closed and reopened: the new thread is woken the same way.
        s.set(Gate::Held, true);
        waker.wake();
        s.set(Gate::Held, false);
        frames_arrive(&rx, 1);
        waker.wake();
        frames_arrive(&rx, 1);
        drop(s);
        waker.wake(); // the scheduler is gone: nothing
    }

    #[test]
    fn a_paced_scene_names_its_next_frame() {
        let (tx, rx) = mpsc::channel();
        let mut n = 0;
        let s = Scheduler::paced(Clock::starting_now(), MIN_INTERVAL, move |_| {
            n += 1;
            let _ = tx.send(Instant::now());
            if n == 1 {
                Pace::After(Duration::from_millis(150))
            } else {
                Pace::Idle
            }
        });
        s.set(Gate::Scene, true);
        let first = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let second = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(
            second - first >= Duration::from_millis(140),
            "{:?}",
            second - first
        );
        std::thread::sleep(MIN_INTERVAL * 3);
        assert_eq!(s.frames(), 2);
    }

    #[test]
    fn frames_carry_clock_time() {
        let (s, _, rx) = counting();
        s.set(Gate::Scene, true);
        let t = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(t.ms() >= 0 && t.ms() < 5000, "{t:?}");
        assert_eq!(
            s.gates(),
            Gates {
                scene: true,
                away: false,
                paused: false,
                held: false
            }
        );
    }

    #[test]
    fn each_device_at_its_own_rate_never_above_30_fps() {
        assert_eq!(device_interval(None), MIN_INTERVAL);
        assert_eq!(
            device_interval(Some(Duration::from_millis(5))),
            MIN_INTERVAL,
            "a 200 Hz LampArray still gets 30 fps"
        );
        let slow = Duration::from_millis(100);
        assert_eq!(device_interval(Some(slow)), slow);
        let mut p = Pacing::new([
            ("keys".to_string(), None),
            ("right half".to_string(), Some(slow)),
        ]);
        assert_eq!(p.tick(), MIN_INTERVAL);
        assert_eq!(p.due(SceneTime(0)), ["keys", "right half"]);
        assert_eq!(p.due(SceneTime(34)), ["keys"]);
        assert_eq!(p.due(SceneTime(67)), ["keys"]);
        assert_eq!(p.due(SceneTime(101)), ["keys", "right half"]);
        assert_eq!(Pacing::default().tick(), MIN_INTERVAL);
    }
}
