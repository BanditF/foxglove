//! Foxglove (#346): coming back after the GPU resets.
//!
//! The overlay draws with OpenGL (eframe `glow`, the only backend that gives it a transparent
//! window, emilk/egui#4451). When the NVIDIA driver resets the GPU (a TDR) or is updated, it
//! logs "The GPU has been disconnected and this application may become unresponsive" for this
//! process, and the context is gone: eframe's context isn't created robust, so nothing in GL
//! reports the loss, and egui_glow can't rebuild a painter in place. The driver library loaded
//! in this process is also the old one after an update.
//!
//! So recovery is a fresh process: the left panel and the art are handed back as on Quit, the
//! single-instance guard is released, the same exe starts again, and this one ends. Every GL
//! object, texture cache and driver library is new in the copy; the keyboard types throughout.
//!
//! What starts it (events only, no timer):
//! - the Windows event log: the NVIDIA OpenGL driver's event naming this process, or Windows'
//!   generic "display driver stopped responding and has recovered" (Display 4101), through a
//!   subscription the log calls back on;
//! - eframe's window loop ending in an error after the window was up (a failed swap or paint).
//!
//! A restart budget (3 in 10 minutes, passed to the copy in `FOXGLOVE_GPU_RESETS`) stops a
//! loop if the GPU keeps failing.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// The copy's record of earlier restarts: unix seconds, comma-separated.
const ENV: &str = "FOXGLOVE_GPU_RESETS";
/// At most this many restarts within [`WINDOW_SECS`].
const BUDGET: usize = 3;
const WINDOW_SECS: u64 = 600;
/// A copy started after a reset whose window then fails to open tries again once after this
/// one-shot delay, in case the driver is still coming back.
const SETTLE: Duration = Duration::from_secs(3);

/// Set once the first frame is drawn: a window-loop error after this is a lost device, not a
/// failure to start.
static WINDOW_UP: AtomicBool = AtomicBool::new(false);
/// One restart per process, whichever event comes first.
static RESTARTING: AtomicBool = AtomicBool::new(false);

/// Called from each drawn frame (no wake-up of its own).
pub fn frame_drawn() {
    WINDOW_UP.store(true, Ordering::Relaxed);
}

/// The earlier restarts this process was started with.
fn history() -> Vec<u64> {
    parse_history(std::env::var(ENV).ok().as_deref())
}

fn parse_history(value: Option<&str>) -> Vec<u64> {
    value
        .unwrap_or_default()
        .split(',')
        .filter_map(|t| t.trim().parse().ok())
        .collect()
}

fn format_history(history: &[u64]) -> String {
    history.iter().map(u64::to_string).collect::<Vec<_>>().join(",")
}

/// The history to hand to the next copy, or `None` when the budget is spent.
fn next_history(previous: &[u64], now: u64) -> Option<Vec<u64>> {
    let mut recent: Vec<u64> = previous.iter().copied().filter(|&t| t <= now && now - t < WINDOW_SECS).collect();
    if recent.len() >= BUDGET {
        return None;
    }
    recent.push(now);
    Some(recent)
}

/// What to do when the window loop ends in an error: restart after this delay, or `None` to
/// report it as a failure to start.
fn failure_plan(window_was_up: bool, after_reset: bool) -> Option<Duration> {
    if window_was_up {
        Some(Duration::ZERO)
    } else if after_reset {
        Some(SETTLE)
    } else {
        None
    }
}

/// From `startup::finish` when eframe returns an error. Restarts (and doesn't return) when the
/// error is a lost window and the budget allows; otherwise returns so the error is reported.
pub fn window_failed(error: &str) {
    let Some(delay) = failure_plan(WINDOW_UP.load(Ordering::Relaxed), !history().is_empty()) else {
        return;
    };
    // A one-shot wait on a path that is ending anyway; no UI or link waits on this thread.
    std::thread::sleep(delay);
    restart(&format!("the window loop ended: {error}"));
}

/// The message for a failure that is reported rather than restarted.
pub fn failure_note() -> Option<String> {
    let n = history().len();
    (n > 0).then(|| {
        format!(
            "The graphics driver was reset and Foxglove restarted itself {n} time(s) in the last \
             few minutes, so it stopped trying. Open Foxglove again from the Start menu once the \
             display has settled."
        )
    })
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}

/// Starts a fresh copy and ends this one. Returns only when it can't (budget spent, a restart
/// already under way, or the copy failed to start).
fn restart(reason: &str) {
    if RESTARTING.swap(true, Ordering::SeqCst) {
        return;
    }
    eprintln!("Foxglove: GPU reset ({reason})");
    let Some(next) = next_history(&history(), now()) else {
        eprintln!("Foxglove: restarted {BUDGET} times in {} minutes after GPU resets; not again", WINDOW_SECS / 60);
        return;
    };
    imp::restart(&format_history(&next));
}

/// Watches for the GPU being reset under this process. Windows only; elsewhere a no-op.
pub fn start() {
    imp::start();
}

/// The event-log query: the NVIDIA OpenGL driver's event for this process (it is logged from
/// inside the process, so `Execution/@ProcessID` is ours) and Windows' generic TDR event.
#[cfg_attr(not(windows), allow(dead_code))]
fn query(pid: u32) -> String {
    format!(
        "<QueryList>\
         <Query Id=\"0\" Path=\"Application\"><Select Path=\"Application\">\
         *[System[Provider[@Name='NVIDIA OpenGL Driver'] and Execution[@ProcessID='{pid}']]]\
         </Select></Query>\
         <Query Id=\"1\" Path=\"System\"><Select Path=\"System\">\
         *[System[Provider[@Name='Display'] and EventID=4101]]\
         </Select></Query>\
         </QueryList>"
    )
}

#[cfg(windows)]
mod imp {
    use std::sync::OnceLock;

    use windows::core::HSTRING;
    use windows::Win32::System::EventLog::{
        EvtSubscribe, EvtSubscribeActionDeliver, EvtSubscribeToFutureEvents, EVT_HANDLE,
        EVT_SUBSCRIBE_NOTIFY_ACTION,
    };
    use windows::Win32::System::Threading::{GetCurrentProcess, TerminateProcess};

    /// The subscription, kept for the process's life (the log calls back on its own threads).
    struct Subscription(#[allow(dead_code)] EVT_HANDLE);
    // SAFETY: an event-log handle is a plain kernel-side value, usable from any thread.
    unsafe impl Send for Subscription {}
    unsafe impl Sync for Subscription {}
    static SUBSCRIPTION: OnceLock<Subscription> = OnceLock::new();

    pub(super) fn subscribe(query: &str, callback: windows::Win32::System::EventLog::EVT_SUBSCRIBE_CALLBACK) -> windows::core::Result<EVT_HANDLE> {
        // SAFETY: a structured XML query takes no channel path; no signal event, bookmark or
        // context pointer is passed, and `callback` is a plain function that lives forever.
        unsafe {
            EvtSubscribe(
                None,
                None,
                None,
                &HSTRING::from(query),
                None,
                None,
                callback,
                EvtSubscribeToFutureEvents.0,
            )
        }
    }

    unsafe extern "system" fn on_event(action: EVT_SUBSCRIBE_NOTIFY_ACTION, _context: *const core::ffi::c_void, _event: EVT_HANDLE) -> u32 {
        if action == EvtSubscribeActionDeliver {
            super::restart("the graphics driver reported it");
        } else {
            eprintln!("Foxglove: the GPU-reset watch got an error from the event log");
        }
        0
    }

    pub fn start() {
        match subscribe(&super::query(std::process::id()), Some(on_event)) {
            Ok(handle) => {
                let _ = SUBSCRIPTION.set(Subscription(handle));
            }
            Err(e) => eprintln!("Foxglove: can't watch for GPU resets: {e}"),
        }
    }

    pub fn restart(history: &str) {
        // Hand the keyboard's screens back first, as Quit does (each bounded), so the copy finds
        // them free.
        crate::foxglove::screens::stop();
        crate::foxglove::art_service::stop();
        crate::foxglove::single_instance::release();
        let exe = match std::env::current_exe() {
            Ok(exe) => exe,
            Err(e) => {
                eprintln!("Foxglove: can't restart after the GPU reset: {e}");
                return;
            }
        };
        // No stdio handed down: the copy sets up its own.
        let null = std::process::Stdio::null;
        let spawned = std::process::Command::new(exe).env(super::ENV, history).stdin(null()).stdout(null()).stderr(null()).spawn();
        if let Err(e) = spawned {
            eprintln!("Foxglove: can't restart after the GPU reset: {e}");
            return;
        }
        eprintln!("Foxglove: started a fresh copy after the GPU reset");
        // Ends without unloading DLLs: after a reset the GL driver may hold a lock on the UI
        // thread, and a normal exit would wait on it in its detach.
        // SAFETY: the pseudo-handle for this process needs no closing.
        unsafe {
            let _ = TerminateProcess(GetCurrentProcess(), 0);
        }
    }
}

#[cfg(not(windows))]
mod imp {
    pub fn start() {}
    pub fn restart(_history: &str) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_round_trips_and_ignores_junk() {
        assert_eq!(parse_history(None), Vec::<u64>::new());
        assert_eq!(parse_history(Some("")), Vec::<u64>::new());
        assert_eq!(parse_history(Some("10, 20,x,30")), vec![10, 20, 30]);
        assert_eq!(format_history(&[10, 20, 30]), "10,20,30");
        assert_eq!(parse_history(Some(&format_history(&[1, 2]))), vec![1, 2]);
    }

    #[test]
    fn three_restarts_in_ten_minutes_then_none() {
        let now = 10_000;
        let one = next_history(&[], now).unwrap();
        let two = next_history(&one, now + 5).unwrap();
        let three = next_history(&two, now + 10).unwrap();
        assert_eq!(three, vec![now, now + 5, now + 10]);
        assert_eq!(next_history(&three, now + 15), None, "a fourth within ten minutes is refused");
    }

    #[test]
    fn old_restarts_drop_out_of_the_budget() {
        let now = 10_000;
        let three = vec![now - 700, now - 650, now - 100];
        assert_eq!(next_history(&three, now), Some(vec![now - 100, now]));
        // A clock that went backwards doesn't count as recent.
        assert_eq!(next_history(&[now + 50, now + 60, now + 70], now), Some(vec![now]));
    }

    #[test]
    fn a_lost_window_restarts_and_a_failed_start_is_reported() {
        assert_eq!(failure_plan(true, false), Some(Duration::ZERO));
        assert_eq!(failure_plan(true, true), Some(Duration::ZERO));
        assert_eq!(failure_plan(false, true), Some(SETTLE), "a copy after a reset tries again once settled");
        assert_eq!(failure_plan(false, false), None, "a first start that fails is reported as before");
    }

    #[test]
    fn the_query_names_this_process_and_both_channels() {
        let q = query(4242);
        assert!(q.contains("Execution[@ProcessID='4242']"));
        assert!(q.contains("Provider[@Name='NVIDIA OpenGL Driver']"));
        assert!(q.contains("Path=\"Application\"") && q.contains("Path=\"System\""));
        assert!(q.contains("EventID=4101"));
    }

    /// The event log accepts the query: a real subscription (no window), closed again. Nothing
    /// matches a made-up process id, so the callback never runs.
    #[cfg(windows)]
    #[test]
    fn the_event_log_accepts_the_query() {
        use windows::Win32::System::EventLog::{EvtClose, EVT_HANDLE, EVT_SUBSCRIBE_NOTIFY_ACTION};
        unsafe extern "system" fn never(_: EVT_SUBSCRIBE_NOTIFY_ACTION, _: *const core::ffi::c_void, _: EVT_HANDLE) -> u32 {
            0
        }
        let handle = imp::subscribe(&query(u32::MAX - 7), Some(never)).expect("query accepted");
        // SAFETY: the handle came from EvtSubscribe above and is closed once.
        unsafe { EvtClose(handle).expect("closed") };
    }
}
