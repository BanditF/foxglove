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
//! It also counts loop wakes and logs one line per 10 minutes, so a live check can prove the
//! hidden hub sleeps. The report timer is the only wake of a fully idle hub.

use eframe::UserEvent;
use std::time::{Duration, Instant};
use winit::application::ApplicationHandler;
use winit::event::{DeviceEvent, DeviceId, StartCause, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow};
use winit::window::WindowId;

/// How often the wake count is logged.
pub const REPORT_EVERY: Duration = Duration::from_secs(600);

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

/// Counts event-loop wakes and says, once per [`REPORT_EVERY`], how many there were.
pub struct WakeCount {
    since: Instant,
    wakes: u64,
}

impl WakeCount {
    pub fn new(now: Instant) -> Self {
        Self {
            since: now,
            wakes: 0,
        }
    }

    /// When the next report is due.
    pub fn report_at(&self) -> Instant {
        self.since + REPORT_EVERY
    }

    /// One wake; the log line when a report is due.
    pub fn wake(&mut self, now: Instant) -> Option<String> {
        self.wakes += 1;
        let span = now.saturating_duration_since(self.since);
        if span < REPORT_EVERY {
            return None;
        }
        let line = format!(
            "event loop: {} wakes in {} min",
            self.wakes,
            (span.as_secs() + 30) / 60
        );
        self.since = now;
        self.wakes = 0;
        Some(line)
    }
}

/// eframe's application with the hidden-window wait and the wake count.
pub struct Gate<A> {
    app: A,
    wakes: WakeCount,
    drawable: fn() -> bool,
}

impl<A> Gate<A> {
    pub fn new(app: A, drawable: fn() -> bool) -> Self {
        Self {
            app,
            wakes: WakeCount::new(Instant::now()),
            drawable,
        }
    }
}

impl<A: ApplicationHandler<UserEvent>> ApplicationHandler<UserEvent> for Gate<A> {
    fn new_events(&mut self, event_loop: &ActiveEventLoop, cause: StartCause) {
        if let Some(line) = self.wakes.wake(Instant::now()) {
            log::info!("{line}");
        }
        self.app.new_events(event_loop, cause);
    }

    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        self.app.resumed(event_loop);
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        self.app.user_event(event_loop, event);
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        self.app.window_event(event_loop, id, event);
    }

    fn device_event(&mut self, event_loop: &ActiveEventLoop, id: DeviceId, event: DeviceEvent) {
        self.app.device_event(event_loop, id, event);
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        self.app.about_to_wait(event_loop);
        let flow = settle(
            event_loop.control_flow(),
            (self.drawable)(),
            self.wakes.report_at(),
        );
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
    fn wakes_are_reported_once_per_ten_minutes() {
        let t0 = Instant::now();
        let mut w = WakeCount::new(t0);
        assert_eq!(w.report_at(), t0 + REPORT_EVERY);
        for i in 1..=41u64 {
            assert_eq!(w.wake(t0 + Duration::from_secs(i)), None);
        }
        assert_eq!(
            w.wake(t0 + REPORT_EVERY).as_deref(),
            Some("event loop: 42 wakes in 10 min")
        );
        // The count starts again from the report.
        assert_eq!(w.report_at(), t0 + 2 * REPORT_EVERY);
        assert_eq!(w.wake(t0 + REPORT_EVERY + Duration::from_secs(1)), None);
        assert_eq!(
            w.wake(t0 + 3 * REPORT_EVERY).as_deref(),
            Some("event loop: 2 wakes in 20 min")
        );
    }
}
