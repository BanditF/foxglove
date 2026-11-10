// SPDX-License-Identifier: MIT
//! The hub's winit loop around eframe's application (#935).
//!
//! Why: eframe 0.33 turns every due repaint into `ControlFlow::Poll` plus `request_redraw`, and
//! only a later event that answers `EventResult::Wait` puts the loop back to `Wait`
//! (`check_redraw_requests` in eframe's `native/run.rs`). Closing the window to the tray sends
//! `CancelClose`, and `send_viewport_cmd` always asks for a repaint; the window is hidden in the
//! same frame. Windows sends no `WM_PAINT` to a hidden window, so `RedrawRequested` never comes,
//! nothing answers `Wait`, and winit 0.30 polls without waiting (`wait_for_messages_impl`
//! returns at once for `Poll`): the main thread spun at about 1–1.6 k context switches a second.
//!
//! The fix: while the window is hidden, a `Poll` left over at `about_to_wait` becomes a wait.
//! Showing the window paints it (Windows sends `WM_PAINT` on show), so no frame is lost.
//!
//! The tray flyout (#991) is an immediate viewport: eframe draws it inside the main window's
//! frame, and asks the main window for a frame whenever the flyout needs one. While the main
//! window is hidden that frame would never come, so while the flyout is up a due frame is run
//! here directly (`hidden_pass`). With the flyout closed nothing changes: the hidden hub waits.
//! Those hidden frames are bounded (#1000): a frame the flyout asks for right away counts towards
//! [`HIDDEN_STREAK`], and a run of that many with no event between them stops them until the next
//! event (input, a timer it asked for, a wake). An idle flyout asks for nothing, so it waits.
//!
//! A tray click also kicks one hidden frame (#1019): with the window hidden, egui's repaint
//! request for the click can stay inside egui (it already holds an older request that eframe
//! can't answer for a hidden window, and calls eframe again only for a sooner one), so eframe's
//! flow stays `Wait` and no `Poll` would ever start the flyout's frames. The kick runs the first
//! one; from there the flyout's own requests reach eframe again.
//!
//! It also counts loop wakes and logs one line per 10 minutes, so a live check can prove the
//! hidden hub sleeps. The report timer is the only wake of a fully idle hub. The line names what
//! woke the loop ([`Cause`]), so a reading says whether the wakes are ours (a repaint one of our
//! wakers asked for, a timer a frame asked for) or the OS's (a message to a window of the thread
//! that winit handled itself) (#1000). `HIDEOUT_WAKE_REPORT_S` shortens the period for the lab's
//! footprint case (`scripts/lab/hideout_footprint.py`), which reads the lines from the log.
//!
//! Each turn starts from `Wait` (#1000, the P0 spin): winit keeps the control flow from one turn
//! to the next, and eframe sets it only while it has a repaint due (`check_redraw_requests`).
//! Without the reset, a turn eframe left alone handed the gate its own previous answer back,
//! the wait until the last wake report, already in the past, and `settle` kept it: winit got
//! `WaitUntil(past)` on every turn and spun at millions of `timer` wakes a second, hidden and
//! shown. A guard behind it ([`guard`]) never hands winit a deadline that has passed.

use eframe::UserEvent;
use std::time::{Duration, Instant};
use winit::application::ApplicationHandler;
use winit::event::{DeviceEvent, DeviceId, StartCause, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow};
use winit::window::WindowId;

/// The most hidden frames run back to back (each asking for the next one at once) before the
/// gate waits for an event: about half a second of frames at 60 Hz. Settling (an animation, a
/// placing) takes a few; anything longer is a loop, and a loop must not spin the hidden hub.
pub const HIDDEN_STREAK: u32 = 30;

/// The most turns in a row a drawable window may turn on a deadline that has passed without a
/// frame (`RedrawRequested`) between them (#1000): a deadline that passed while the turn ran is
/// normal once, a few times at most; more is a loop that no frame will clear.
pub const PAST_TURNS: u32 = 3;

/// How often a firing [`guard`] is logged, at most.
pub const GUARD_LOG_EVERY: Duration = Duration::from_secs(3600);

/// How often the wake count is logged.
pub const REPORT_EVERY: Duration = Duration::from_secs(600);

/// The shortest report period `HIDEOUT_WAKE_REPORT_S` may ask for: each report is a wake.
pub const REPORT_MIN: Duration = Duration::from_secs(5);

/// The report period from `HIDEOUT_WAKE_REPORT_S` (whole seconds), kept within
/// [`REPORT_MIN`]..=[`REPORT_EVERY`]; [`REPORT_EVERY`] when it is unset or not a number.
pub fn report_every(env: Option<&str>) -> Duration {
    env.and_then(|v| v.trim().parse::<u64>().ok())
        .map(Duration::from_secs)
        .map_or(REPORT_EVERY, |d| d.clamp(REPORT_MIN, REPORT_EVERY))
}

/// What woke the loop, for the report line (#1000). One per wake, the first that applies:
/// a window event, a repaint request (eframe's user event: our wakers, the show signal, tray
/// commands, egui), an AccessKit request (a screen reader or a UI Automation client walking the
/// window), a device event, the timer the loop waited for (a frame's `request_repaint_after` or
/// the report itself), a `Poll`, else `Other`: a message winit handled itself or one for another
/// window of the thread (the tray icon's, IME, COM), which reaches no handler here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cause {
    Window,
    Repaint,
    Accesskit,
    Device,
    Timer,
    Poll,
    Other,
}

impl Cause {
    /// In the report line's order.
    const ALL: [Cause; 7] = [
        Cause::Repaint,
        Cause::Timer,
        Cause::Window,
        Cause::Accesskit,
        Cause::Device,
        Cause::Poll,
        Cause::Other,
    ];

    fn name(self) -> &'static str {
        match self {
            Cause::Window => "window",
            Cause::Repaint => "repaint",
            Cause::Accesskit => "accesskit",
            Cause::Device => "device",
            Cause::Timer => "timer",
            Cause::Poll => "poll",
            Cause::Other => "other",
        }
    }
}

/// What one wake dispatched, set by the handlers between `new_events` and `about_to_wait`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Seen {
    pub window: bool,
    pub repaint: bool,
    pub accesskit: bool,
    pub device: bool,
}

/// The cause of a wake that started with `start` and dispatched `seen`.
pub fn cause_of(start: StartCause, seen: Seen) -> Cause {
    if seen.window {
        Cause::Window
    } else if seen.repaint {
        Cause::Repaint
    } else if seen.accesskit {
        Cause::Accesskit
    } else if seen.device {
        Cause::Device
    } else {
        match start {
            StartCause::ResumeTimeReached { .. } => Cause::Timer,
            StartCause::Poll => Cause::Poll,
            _ => Cause::Other,
        }
    }
}

/// The control flow to wait with, given eframe's and whether the window can be painted.
/// A hidden window never gets the frame a `Poll` waits for, so it waits instead; every wait
/// ends by the next wake report at the latest.
pub fn settle(flow: ControlFlow, drawable: bool, report_at: Instant) -> ControlFlow {
    match flow {
        ControlFlow::Poll if drawable => ControlFlow::Poll,
        ControlFlow::Poll | ControlFlow::Wait => ControlFlow::WaitUntil(report_at),
        ControlFlow::WaitUntil(t) => ControlFlow::WaitUntil(t.min(report_at)),
    }
}

/// Why [`guard`] changed a turn's flow.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stale {
    /// A passed deadline while no frame can run (hidden, minimised, not drawable).
    NoFrame,
    /// A drawable window turned [`PAST_TURNS`] times on passed deadlines without a frame.
    NoRedraw,
}

impl Stale {
    pub fn name(self) -> &'static str {
        match self {
            Stale::NoFrame => "a passed deadline with no frame able to run",
            Stale::NoRedraw => "passed deadlines with no frame between them",
        }
    }
}

/// Defence in depth behind [`settle`] (#1000): winit never gets `WaitUntil(t)` with `t <= now`.
/// With no frame able to run, a passed deadline becomes the wait until the report. A drawable
/// window turns at once (`Poll`) for at most [`PAST_TURNS`] such turns since its last frame
/// (`past_turns`), then waits until the report too. Any other flow passes unchanged.
pub fn guard(
    flow: ControlFlow,
    now: Instant,
    drawable: bool,
    past_turns: u32,
    report_at: Instant,
) -> (ControlFlow, Option<Stale>) {
    match flow {
        ControlFlow::WaitUntil(t) if t <= now => {
            if !drawable {
                (ControlFlow::WaitUntil(report_at), Some(Stale::NoFrame))
            } else if past_turns >= PAST_TURNS {
                (ControlFlow::WaitUntil(report_at), Some(Stale::NoRedraw))
            } else {
                (ControlFlow::Poll, None)
            }
        }
        other => (other, None),
    }
}

/// Whether the hidden main window must run a frame now: eframe has one due (`Poll`), the window
/// is hidden (so `RedrawRequested` won't come), and a window drawn from its frame is up (the
/// tray flyout). Never true with the flyout closed: the hidden, idle hub keeps waiting (#935).
/// `kicked`: a tray click asked for the frame ([`crate::flyout::kick`]), whatever eframe's flow
/// says (#1019): egui may not have passed its repaint request on.
pub fn hidden_pass(flow: ControlFlow, drawable: bool, side_live: bool, kicked: bool) -> bool {
    (flow == ControlFlow::Poll || kicked) && !drawable && side_live
}

/// Whether the side window still gets hidden frames: it is up, and fewer than [`HIDDEN_STREAK`]
/// hidden frames ran back to back since the last event (`streak`).
pub fn side_frames(side_live: bool, streak: u32) -> bool {
    side_live && streak < HIDDEN_STREAK
}

/// The streak after a wake for `cause`: only a wake that a `Poll` brought (no event, no timer)
/// continues it; anything else is an event and starts it again.
pub fn next_streak(streak: u32, cause: StartCause) -> u32 {
    match cause {
        StartCause::Poll => streak,
        _ => 0,
    }
}

/// Counts event-loop wakes by [`Cause`] and says, once per period, how many there were:
/// `event loop: 1182 wakes in 600 s (repaint 3, timer 1, window 0, accesskit 0, device 0,
/// poll 0, other 1178)`.
pub struct WakeCount {
    since: Instant,
    every: Duration,
    wakes: [u64; 7],
}

impl WakeCount {
    pub fn new(now: Instant, every: Duration) -> Self {
        Self {
            since: now,
            every,
            wakes: [0; 7],
        }
    }

    /// When the next report is due.
    pub fn report_at(&self) -> Instant {
        self.since + self.every
    }

    /// One wake for `cause`; the log line when a report is due.
    pub fn wake(&mut self, now: Instant, cause: Cause) -> Option<String> {
        let i = Cause::ALL.iter().position(|c| *c == cause).unwrap_or(6);
        self.wakes[i] += 1;
        let span = now.saturating_duration_since(self.since);
        if span < self.every {
            return None;
        }
        let parts: Vec<String> = Cause::ALL
            .iter()
            .zip(self.wakes)
            .map(|(c, n)| format!("{} {n}", c.name()))
            .collect();
        let line = format!(
            "event loop: {} wakes in {} s ({})",
            self.wakes.iter().sum::<u64>(),
            span.as_secs(),
            parts.join(", ")
        );
        self.since = now;
        self.wakes = [0; 7];
        Some(line)
    }
}

/// eframe's application with the hidden-window wait and the wake count.
pub struct Gate<A> {
    app: A,
    wakes: WakeCount,
    drawable: fn() -> bool,
    /// A window drawn from the main window's frame is up (the tray flyout).
    side_live: fn() -> bool,
    /// Takes a tray click's request for one hidden frame ([`hidden_pass`], #1019).
    side_kick: fn() -> bool,
    /// The main window, to run its frame while it is hidden.
    root: fn() -> Option<WindowId>,
    /// Applies a pending tray change (the icon is the UI thread's). Runs on every wake, so a
    /// fact that changes the tray shows while the window is hidden, without a frame.
    tray_sync: fn(),
    /// Hidden frames run back to back since the last event ([`HIDDEN_STREAK`]).
    streak: u32,
    /// How this wake started and what it dispatched, for its [`Cause`].
    start: StartCause,
    seen: Seen,
    /// Turns on a passed deadline since the last frame ([`PAST_TURNS`]).
    past_turns: u32,
    /// When a firing [`guard`] was last logged ([`GUARD_LOG_EVERY`]).
    guard_logged: Option<Instant>,
}

impl<A> Gate<A> {
    pub fn new(
        app: A,
        drawable: fn() -> bool,
        side_live: fn() -> bool,
        side_kick: fn() -> bool,
        root: fn() -> Option<WindowId>,
        tray_sync: fn(),
    ) -> Self {
        Self {
            app,
            wakes: WakeCount::new(
                Instant::now(),
                report_every(std::env::var("HIDEOUT_WAKE_REPORT_S").ok().as_deref()),
            ),
            drawable,
            side_live,
            side_kick,
            root,
            tray_sync,
            streak: 0,
            start: StartCause::Init,
            seen: Seen::default(),
            past_turns: 0,
            guard_logged: None,
        }
    }
}

impl<A: ApplicationHandler<UserEvent>> ApplicationHandler<UserEvent> for Gate<A> {
    fn new_events(&mut self, event_loop: &ActiveEventLoop, cause: StartCause) {
        self.start = cause;
        self.seen = Seen::default();
        self.streak = next_streak(self.streak, cause);
        // eframe decides this turn's flow afresh: it sets one only while it has a repaint due,
        // so whatever the gate set last turn (a wait until a deadline now passed) must not
        // stand in for its answer (#1000).
        event_loop.set_control_flow(ControlFlow::Wait);
        self.app.new_events(event_loop, cause);
    }

    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        self.app.resumed(event_loop);
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::RequestRepaint { .. } => self.seen.repaint = true,
            _ => self.seen.accesskit = true,
        }
        self.app.user_event(event_loop, event);
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        self.seen.window = true;
        if matches!(event, WindowEvent::RedrawRequested) {
            self.past_turns = 0;
        }
        self.app.window_event(event_loop, id, event);
    }

    fn device_event(&mut self, event_loop: &ActiveEventLoop, id: DeviceId, event: DeviceEvent) {
        self.seen.device = true;
        self.app.device_event(event_loop, id, event);
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        (self.tray_sync)();
        // The wake ends here: count it by what it dispatched (the report may be due).
        let cause = cause_of(self.start, self.seen);
        if let Some(line) = self.wakes.wake(Instant::now(), cause) {
            log::info!("{line}");
        }
        self.app.about_to_wait(event_loop);
        let drawable = (self.drawable)();
        // A kick is a tray click, an event: it starts the streak again.
        let kicked = (self.side_kick)();
        if kicked {
            self.streak = 0;
        }
        let side = side_frames((self.side_live)(), self.streak);
        if hidden_pass(event_loop.control_flow(), drawable, side, kicked) {
            if let Some(root) = (self.root)() {
                // eframe's own frame path, as Windows would run it for a shown window.
                self.streak += 1;
                self.past_turns = 0;
                self.app
                    .window_event(event_loop, root, WindowEvent::RedrawRequested);
            }
        }
        // While the flyout is up, a frame it still wants keeps the loop turning (as a shown
        // window's would); it stops asking once it settles, closing it drops the flag, and a
        // run of HIDDEN_STREAK frames with no event waits for the next event (#1000).
        let side = side_frames((self.side_live)(), self.streak);
        let flow = settle(
            event_loop.control_flow(),
            drawable || side,
            self.wakes.report_at(),
        );
        let now = Instant::now();
        let past = matches!(flow, ControlFlow::WaitUntil(t) if t <= now);
        let (flow, stale) = guard(
            flow,
            now,
            drawable || side,
            self.past_turns,
            self.wakes.report_at(),
        );
        if past && stale.is_none() {
            self.past_turns += 1;
        }
        if let Some(why) = stale {
            if self
                .guard_logged
                .is_none_or(|t| now.saturating_duration_since(t) >= GUARD_LOG_EVERY)
            {
                self.guard_logged = Some(now);
                log::warn!("event loop: wait guard fired: {} (#1000)", why.name());
            }
        }
        event_loop.set_control_flow(flow);
    }

    fn suspended(&mut self, event_loop: &ActiveEventLoop) {
        self.app.suspended(event_loop);
    }

    fn exiting(&mut self, event_loop: &ActiveEventLoop) {
        self.app.exiting(event_loop);
    }

    fn memory_warning(&mut self, event_loop: &ActiveEventLoop) {
        self.app.memory_warning(event_loop);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ControlFlow::*;

    #[test]
    fn a_hidden_window_never_polls() {
        let now = Instant::now();
        let report = now + REPORT_EVERY;
        assert_eq!(
            settle(ControlFlow::Poll, false, report),
            ControlFlow::WaitUntil(report)
        );
        // A shown window keeps eframe's Poll: its RedrawRequested comes and answers Wait.
        assert_eq!(settle(ControlFlow::Poll, true, report), ControlFlow::Poll);
        for drawable in [false, true] {
            assert_eq!(
                settle(ControlFlow::Wait, drawable, report),
                ControlFlow::WaitUntil(report)
            );
            let soon = now + Duration::from_millis(16);
            assert_eq!(
                settle(ControlFlow::WaitUntil(soon), drawable, report),
                ControlFlow::WaitUntil(soon)
            );
            let late = report + Duration::from_secs(5);
            assert_eq!(
                settle(ControlFlow::WaitUntil(late), drawable, report),
                ControlFlow::WaitUntil(report)
            );
        }
    }

    #[test]
    fn a_hidden_window_runs_frames_only_while_the_flyout_is_up() {
        let soon = Instant::now() + Duration::from_millis(16);
        // Flyout closed: never, whatever eframe wants (the #935 wait stands).
        for flow in [Poll, Wait, WaitUntil(soon)] {
            assert!(!hidden_pass(flow, false, false, false));
            assert!(!hidden_pass(flow, true, false, false));
        }
        // Flyout up: only a due frame (Poll) of a hidden window.
        assert!(hidden_pass(Poll, false, true, false));
        assert!(!hidden_pass(Poll, true, true, false));
        assert!(!hidden_pass(Wait, false, true, false));
        assert!(!hidden_pass(WaitUntil(soon), false, true, false));
        // A tray click's kick (#1019): a frame of the hidden window whatever eframe's flow, while
        // the flyout is up; nothing for a shown window (it paints) or with the flyout closed.
        for flow in [Poll, Wait, WaitUntil(soon)] {
            assert!(hidden_pass(flow, false, true, true));
            assert!(!hidden_pass(flow, true, true, true));
            assert!(!hidden_pass(flow, false, false, true));
        }
    }

    #[test]
    fn hidden_frames_stop_after_a_streak_and_start_again_on_an_event() {
        let soon = Instant::now() + Duration::from_millis(16);
        // Flyout closed: no hidden frames, whatever the streak.
        assert!(!side_frames(false, 0));
        // Flyout up: frames until HIDDEN_STREAK back to back, then the loop waits.
        assert!(side_frames(true, 0));
        assert!(side_frames(true, HIDDEN_STREAK - 1));
        assert!(!side_frames(true, HIDDEN_STREAK));
        assert!(!hidden_pass(
            Poll,
            false,
            side_frames(true, HIDDEN_STREAK),
            false
        ));
        assert_eq!(
            settle(Poll, side_frames(true, HIDDEN_STREAK), soon),
            WaitUntil(soon)
        );
        // Only a Poll wake continues the streak; an event, a timer or a wake starts it again.
        assert_eq!(next_streak(7, StartCause::Poll), 7);
        assert_eq!(
            next_streak(
                HIDDEN_STREAK,
                StartCause::ResumeTimeReached {
                    start: soon,
                    requested_resume: soon,
                }
            ),
            0
        );
        assert_eq!(
            next_streak(
                HIDDEN_STREAK,
                StartCause::WaitCancelled {
                    start: soon,
                    requested_resume: None,
                }
            ),
            0
        );
    }

    #[test]
    fn wakes_are_reported_once_per_ten_minutes_by_cause() {
        let t0 = Instant::now();
        let mut w = WakeCount::new(t0, REPORT_EVERY);
        assert_eq!(w.report_at(), t0 + REPORT_EVERY);
        for i in 1..=40u64 {
            let cause = if i % 2 == 0 {
                Cause::Other
            } else {
                Cause::Repaint
            };
            assert_eq!(w.wake(t0 + Duration::from_secs(i), cause), None);
        }
        assert_eq!(w.wake(t0 + Duration::from_secs(41), Cause::Window), None);
        assert_eq!(
            w.wake(t0 + REPORT_EVERY, Cause::Timer).as_deref(),
            Some(
                "event loop: 42 wakes in 600 s (repaint 20, timer 1, window 1, accesskit 0, \
                 device 0, poll 0, other 20)"
            )
        );
        // The count starts again from the report.
        assert_eq!(w.report_at(), t0 + 2 * REPORT_EVERY);
        assert_eq!(
            w.wake(t0 + REPORT_EVERY + Duration::from_secs(1), Cause::Accesskit),
            None
        );
        assert_eq!(
            w.wake(t0 + 3 * REPORT_EVERY, Cause::Timer).as_deref(),
            Some(
                "event loop: 2 wakes in 1200 s (repaint 0, timer 1, window 0, accesskit 1, \
                 device 0, poll 0, other 0)"
            )
        );
    }

    #[test]
    fn the_report_period_comes_from_the_environment_within_bounds() {
        assert_eq!(report_every(None), REPORT_EVERY);
        assert_eq!(report_every(Some("15")), Duration::from_secs(15));
        assert_eq!(report_every(Some(" 30 ")), Duration::from_secs(30));
        // Never a spin, never longer than the default.
        assert_eq!(report_every(Some("0")), REPORT_MIN);
        assert_eq!(report_every(Some("99999")), REPORT_EVERY);
        assert_eq!(report_every(Some("soon")), REPORT_EVERY);
    }

    #[test]
    fn a_wake_is_named_by_what_it_dispatched() {
        let t = Instant::now();
        let timer = StartCause::ResumeTimeReached {
            start: t,
            requested_resume: t,
        };
        let cancelled = StartCause::WaitCancelled {
            start: t,
            requested_resume: Some(t),
        };
        let none = Seen::default();
        assert_eq!(cause_of(timer, none), Cause::Timer);
        assert_eq!(cause_of(StartCause::Poll, none), Cause::Poll);
        // A message winit handled itself, or one for another window of the thread.
        assert_eq!(cause_of(cancelled, none), Cause::Other);
        assert_eq!(cause_of(StartCause::Init, none), Cause::Other);
        let repaint = Seen {
            repaint: true,
            ..none
        };
        assert_eq!(cause_of(cancelled, repaint), Cause::Repaint);
        assert_eq!(cause_of(timer, repaint), Cause::Repaint);
        let ak = Seen {
            accesskit: true,
            ..none
        };
        assert_eq!(cause_of(cancelled, ak), Cause::Accesskit);
        let dev = Seen {
            device: true,
            ..none
        };
        assert_eq!(cause_of(cancelled, dev), Cause::Device);
        let all = Seen {
            window: true,
            repaint: true,
            accesskit: true,
            device: true,
        };
        assert_eq!(cause_of(StartCause::Poll, all), Cause::Window);
    }

    /// The footprint gate's case, window-free (#1000): a hidden hub, no flyout, a module feed
    /// that wakes it every 4 s. Whatever eframe leaves at `about_to_wait`, the loop waits for the
    /// next event or the report, so the wakes are the feed's events plus the reports: under 1/s.
    #[test]
    fn a_hidden_hub_without_the_flyout_wakes_only_for_events() {
        let t0 = Instant::now();
        let mut w = WakeCount::new(t0, Duration::from_secs(15));
        let mut lines = Vec::new();
        let end = t0 + Duration::from_secs(60);
        let mut next_event = t0 + Duration::from_secs(4);
        let mut now = t0;
        while now < end {
            // The loop's wait ends at the first of: the feed's next event, the report.
            let (at, cause) = if next_event < w.report_at() {
                next_event += Duration::from_secs(4);
                (next_event - Duration::from_secs(4), Cause::Repaint)
            } else {
                (w.report_at(), Cause::Timer)
            };
            now = at;
            lines.extend(w.wake(now, cause));
            // Hidden, no flyout: no hidden frame, and every flow is a wait until the report.
            let side = side_frames(false, 0);
            for flow in [Poll, Wait, WaitUntil(now + Duration::from_secs(3600))] {
                assert!(!hidden_pass(flow, false, side, false));
                assert_eq!(settle(flow, side, w.report_at()), WaitUntil(w.report_at()));
            }
        }
        assert_eq!(lines.len(), 4, "{lines:?}");
        let wakes: u64 = lines
            .iter()
            .map(|l| l.split_whitespace().nth(2).unwrap().parse::<u64>().unwrap())
            .sum();
        // 14 feed events (every 4 s up to 56 s) plus 4 reports, the last at 60 s.
        assert_eq!(wakes, 18, "{lines:?}");
        assert!(wakes as f64 / 60.0 <= 1.0);
    }

    #[test]
    fn the_report_is_always_ahead() {
        // The footprint script's period (HIDEOUT_WAKE_REPORT_S=15) and the default: after any
        // wake, the next report is in the future, so the wait until it never spins.
        for every in [report_every(Some("15")), REPORT_EVERY] {
            let t0 = Instant::now();
            let mut w = WakeCount::new(t0, every);
            for k in [0u32, 1, 7, 15, 16, 31, 600, 1201] {
                let now = t0 + Duration::from_secs(k as u64);
                w.wake(now, Cause::Timer);
                assert!(w.report_at() > now, "every {every:?}, at {k} s");
            }
        }
    }

    #[test]
    fn the_guard_never_hands_winit_a_passed_deadline() {
        let now = Instant::now();
        let report = now + Duration::from_secs(15);
        let past = now - Duration::from_millis(1);
        let soon = now + Duration::from_millis(16);
        // No frame can run: the wait until the report, named.
        assert_eq!(
            guard(WaitUntil(past), now, false, 0, report),
            (WaitUntil(report), Some(Stale::NoFrame))
        );
        assert_eq!(
            guard(WaitUntil(now), now, false, 0, report),
            (WaitUntil(report), Some(Stale::NoFrame))
        );
        // Drawable: a few turns at once, then the wait until the report.
        for n in 0..PAST_TURNS {
            assert_eq!(guard(WaitUntil(past), now, true, n, report), (Poll, None));
        }
        assert_eq!(
            guard(WaitUntil(past), now, true, PAST_TURNS, report),
            (WaitUntil(report), Some(Stale::NoRedraw))
        );
        // Anything else passes unchanged.
        for drawable in [false, true] {
            for flow in [Poll, Wait, WaitUntil(soon), WaitUntil(report)] {
                assert_eq!(guard(flow, now, drawable, PAST_TURNS, report), (flow, None));
            }
        }
    }

    /// The gate's per-turn decision against an eframe that keeps answering a passed deadline,
    /// for 1 s of fake time, hidden and shown: the turns stay a few, not a spin (#1000).
    #[test]
    fn a_passed_deadline_every_turn_does_not_spin() {
        for drawable in [false, true] {
            let t0 = Instant::now();
            let end = t0 + Duration::from_secs(1);
            let mut w = WakeCount::new(t0, report_every(Some("15")));
            let mut now = t0;
            let mut past_turns = 0;
            let mut turns = 0u32;
            while now < end && turns < 1_000_000 {
                turns += 1;
                w.wake(now, Cause::Timer);
                let flow = settle(
                    WaitUntil(now - Duration::from_millis(5)),
                    drawable,
                    w.report_at(),
                );
                let past = matches!(flow, WaitUntil(t) if t <= now);
                let (flow, stale) = guard(flow, now, drawable, past_turns, w.report_at());
                if past && stale.is_none() {
                    past_turns += 1;
                }
                now = match flow {
                    WaitUntil(t) => {
                        assert!(t > now, "a passed deadline reached winit");
                        t
                    }
                    // winit turns again at once: a microsecond of fake time.
                    _ => now + Duration::from_micros(1),
                };
            }
            assert!(
                turns <= 1 + PAST_TURNS,
                "drawable {drawable}: {turns} turns in 1 s"
            );
        }
    }

    /// eframe's `check_redraw_requests` in miniature: it sets the flow only while it has a
    /// repaint due; an answered frame (a shown window's) sets `Wait`.
    struct Eframe {
        next: Option<Instant>,
        drawable: bool,
        t0: Instant,
        turns: u64,
    }

    impl ApplicationHandler<UserEvent> for Eframe {
        fn new_events(&mut self, event_loop: &ActiveEventLoop, _: StartCause) {
            let now = Instant::now();
            match self.next {
                Some(t) if t <= now => {
                    self.next = None;
                    event_loop.set_control_flow(Poll);
                }
                Some(t) => event_loop.set_control_flow(WaitUntil(t)),
                None => {}
            }
        }

        fn resumed(&mut self, _: &ActiveEventLoop) {}

        fn window_event(&mut self, _: &ActiveEventLoop, _: WindowId, _: WindowEvent) {}

        fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
            self.turns += 1;
            if self.drawable && event_loop.control_flow() == Poll {
                // The shown window's frame ran and answered Wait.
                event_loop.set_control_flow(Wait);
            }
            if self.t0.elapsed() >= Duration::from_secs(1) || self.turns > 1_000_000 {
                event_loop.exit();
            }
        }
    }

    fn hidden() -> bool {
        false
    }
    fn shown() -> bool {
        true
    }
    fn no_side() -> bool {
        false
    }
    fn no_root() -> Option<WindowId> {
        None
    }
    fn no_tray() {}

    /// The P0 of #1000 on a real winit loop (its hidden message window only, no window of ours):
    /// eframe asks for one repaint and then nothing, the report comes every 200 ms. On cde0b451
    /// the gate handed winit its own passed report deadline back on every turn: a million turns
    /// before the second report. Now: the reports and the repaint, under 20 turns a second.
    #[test]
    fn the_real_loop_sleeps_when_eframe_sets_no_flow() {
        use winit::event_loop::EventLoop;
        use winit::platform::run_on_demand::EventLoopExtRunOnDemand;
        use winit::platform::windows::EventLoopBuilderExtWindows;
        let mut event_loop = EventLoop::<UserEvent>::with_user_event()
            .with_any_thread(true)
            .build()
            .expect("event loop");
        for (drawable, drawn) in [(false, hidden as fn() -> bool), (true, shown)] {
            let t0 = Instant::now();
            let app = Eframe {
                next: Some(t0 + Duration::from_millis(10)),
                drawable,
                t0,
                turns: 0,
            };
            let mut gate = Gate::new(app, drawn, no_side, no_side, no_root, no_tray);
            gate.wakes = WakeCount::new(t0, Duration::from_millis(200));
            event_loop.run_app_on_demand(&mut gate).expect("run");
            let secs = t0.elapsed().as_secs_f64();
            let rate = gate.app.turns as f64 / secs;
            assert!(
                rate <= 20.0,
                "drawable {drawable}: {} turns in {secs:.2} s ({rate:.0}/s)",
                gate.app.turns
            );
        }
    }
}
