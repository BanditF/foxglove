//! Idle diagnostics (#100).
//!
//! - `FOXGLOVE_TRACE_REPAINT=1` prints, every 5 s, how many frames ran, what egui says asked
//!   for them (file:line), and who called [`crate::ui_wake::UiWake`] from a background thread.
//! - `FOXGLOVE_DISABLE=providers,screens,art,status,keepalive` leaves those parts out, so an
//!   idle-CPU probe can bisect which one keeps the process awake.

use std::collections::HashMap;
use std::panic::Location;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

struct Trace {
    since: Instant,
    frames: u32,
    causes: HashMap<String, u32>,
    wakes: HashMap<String, u32>,
}

static TRACE: OnceLock<Option<Mutex<Trace>>> = OnceLock::new();

fn trace() -> Option<&'static Mutex<Trace>> {
    TRACE
        .get_or_init(|| {
            std::env::var_os("FOXGLOVE_TRACE_REPAINT").map(|_| {
                Mutex::new(Trace {
                    since: Instant::now(),
                    frames: 0,
                    causes: HashMap::new(),
                    wakes: HashMap::new(),
                })
            })
        })
        .as_ref()
}

/// Whether `FOXGLOVE_DISABLE` names `part`.
pub fn disabled(part: &str) -> bool {
    static PARTS: OnceLock<Vec<String>> = OnceLock::new();
    PARTS
        .get_or_init(|| {
            std::env::var("FOXGLOVE_DISABLE")
                .map(|v| v.split(',').map(|p| p.trim().to_lowercase()).collect())
                .unwrap_or_default()
        })
        .iter()
        .any(|p| p == part)
}

/// Counts a background wake-up by its caller.
pub fn wake(at: &'static Location<'static>) {
    if let Some(t) = trace() {
        let mut t = t.lock().unwrap_or_else(|e| e.into_inner());
        *t.wakes.entry(format!("{}:{}", at.file(), at.line())).or_default() += 1;
    }
}

/// Called once per frame.
pub fn frame(ctx: &egui::Context) {
    let Some(trace) = trace() else {
        return;
    };
    let mut t = trace.lock().unwrap_or_else(|e| e.into_inner());
    t.frames += 1;
    for cause in ctx.repaint_causes() {
        *t.causes.entry(cause.to_string()).or_default() += 1;
    }
    if t.since.elapsed() >= Duration::from_secs(5) {
        let sorted = |m: &mut HashMap<String, u32>| {
            let mut v: Vec<_> = m.drain().collect();
            v.sort_by_key(|c| std::cmp::Reverse(c.1));
            v
        };
        let (causes, wakes) = (sorted(&mut t.causes), sorted(&mut t.wakes));
        eprintln!("repaint trace: {} frames in 5 s; causes: {causes:?}; wakes: {wakes:?}", t.frames);
        t.frames = 0;
        t.since = Instant::now();
    }
}
