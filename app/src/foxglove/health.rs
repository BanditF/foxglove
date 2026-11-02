//! Keyboard health (#705): the keyboard's self-report (D7 13, read and judged by
//! `foxglove_link::health`) and the rows Foxglove's own features add, for the App page's
//! "Keyboard health" card, `health.read` in the local API, Copy diagnostics and the log.
//!
//! - A read happens on events only: each keyboard connect (the devices bus), the card's
//!   "Check now", and `health.read`. One read at a time; nothing polls.
//! - The log gets one line per connect (the summary) and one whenever the set of failed rows
//!   changes.
//! - Features register a function that returns their rows ([`register`]); the card and the API
//!   show them under the keyboard's. The row type is `foxglove_link::health::HealthRow` (the art
//!   keys #701, LampArray, lighting zones and colour-aware effects use the same one).

use std::sync::Mutex;
use std::time::SystemTime;

use foxglove_devices::Device as _;
use foxglove_link::health::{self as kh, Thresholds};
pub use foxglove_link::health::{HealthRow, Level};

/// A feature's rows, read when the card draws or a caller asks. It must not block: return what the
/// feature already knows.
pub type Provider = fn() -> Vec<HealthRow>;

static PROVIDERS: Mutex<Vec<(&'static str, Provider)>> = Mutex::new(Vec::new());

/// Adds a feature's rows to the health view, once per `name` (a second call replaces the first).
pub fn register(name: &'static str, rows: Provider) {
    let mut p = PROVIDERS.lock().unwrap_or_else(|e| e.into_inner());
    p.retain(|(n, _)| *n != name);
    p.push((name, rows));
}

/// Every registered feature's rows now.
pub fn feature_rows() -> Vec<HealthRow> {
    let providers = PROVIDERS.lock().unwrap_or_else(|e| e.into_inner()).clone();
    providers.into_iter().flat_map(|(_, rows)| rows()).collect()
}

/// The keyboard's last read.
#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    /// The keyboard's rows from the last read; empty before one.
    pub rows: Vec<HealthRow>,
    pub read_at: Option<SystemTime>,
    /// A read is running now.
    pub reading: bool,
}

static LAST: Mutex<Snapshot> = Mutex::new(Snapshot {
    rows: Vec::new(),
    read_at: None,
    reading: false,
});
/// One read of the keyboard at a time: the replies of two would interleave.
static ONE_READ: Mutex<()> = Mutex::new(());

/// The keyboard's last read.
pub fn snapshot() -> Snapshot {
    LAST.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// The keyboard's rows, then the features'.
pub fn all_rows() -> Vec<HealthRow> {
    let mut rows = snapshot().rows;
    rows.extend(feature_rows());
    rows
}

/// Why a read ran, for its log line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Why {
    Connect,
    Asked,
}

/// What the log says for a new read, given the failed rows of the one before. `None`: nothing new
/// to say (not a connect, and the failed set is the same).
pub fn log_line(why: Why, before: &[String], rows: &[HealthRow]) -> Option<String> {
    let failed = kh::failed_lines(rows);
    match why {
        Why::Connect => Some(format!("Foxglove: on connect, {}", kh::summary(rows))),
        Why::Asked if failed != before => Some(if failed.is_empty() {
            "Foxglove: keyboard health: nothing failed now".to_string()
        } else {
            format!("Foxglove: keyboard health failed: {}", failed.join("; "))
        }),
        Why::Asked => None,
    }
}

/// The thresholds in use: the shipped data (`crates/foxglove-link/data/health.toml`).
fn thresholds() -> Thresholds {
    Thresholds::default()
}

/// Reads both halves now over the keyboard's link and keeps the rows. `None` without a keyboard
/// with Foxglove's host link. Blocks for the read (the right's chunk 0 waits up to 8 s): call it
/// from a worker or an API call's own thread, never the UI.
pub fn read_now(why: Why) -> Option<Vec<HealthRow>> {
    let device = super::devices::device()?;
    let link = device.link().clone();
    let capabilities = link
        .state()
        .connected
        .and_then(|c| c.hello)
        .map_or(0, |h| h.capabilities);
    let _one = ONE_READ.lock().unwrap_or_else(|e| e.into_inner());
    set_reading(true);
    let report = kh::read_both(link.as_ref(), capabilities);
    let rows = kh::rows(&report, &thresholds());
    let before = {
        let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
        let before = kh::failed_lines(&last.rows);
        last.rows = rows.clone();
        last.read_at = Some(SystemTime::now());
        last.reading = false;
        before
    };
    if let Some(line) = log_line(why, &before, &rows) {
        eprintln!("{line}");
    }
    Some(rows)
}

fn set_reading(on: bool) {
    LAST.lock().unwrap_or_else(|e| e.into_inner()).reading = on;
}

/// "Check now": reads on a thread of its own and repaints when it's in. A click while a read runs
/// does nothing more.
pub fn request_read(ctx: &egui::Context) {
    if snapshot().reading {
        return;
    }
    set_reading(true);
    let ctx = ctx.clone();
    let _ = std::thread::Builder::new()
        .name("health-read".into())
        .spawn(move || {
            if read_now(Why::Asked).is_none() {
                set_reading(false);
            }
            ctx.request_repaint();
        });
}

/// Reads once on each keyboard connect (the devices bus): the log's line, and fresh rows for the
/// card. Started once with the app (main.rs).
pub fn start() {
    if super::repaint_trace::disabled("health") {
        return;
    }
    let events = super::services::First::new(super::devices::bus().subscribe(false));
    let _ = super::services::spawn_supervised("health", move || {
        let events = events.take_or(|| super::devices::bus().subscribe(false));
        for e in events {
            if e.event == foxglove_devices::DeviceEvent::Connected
                && super::devices::device()
                    .is_some_and(|d| d.info().id.to_string() == e.device.to_string())
            {
                relay_settle();
                read_now(Why::Connect);
            }
        }
    });
}

/// How long a connect's read waits for the relay to the right half: it comes up ~20 s after the
/// split link (discovery), and a read before then would only say "no relay link".
const RELAY_SETTLE: std::time::Duration = std::time::Duration::from_secs(45);

/// Waits (on the link's events, `art::wait_relay_ready`) until the relay is up or
/// [`RELAY_SETTLE`] passed, on a keyboard with the relay; at once otherwise.
fn relay_settle() {
    let Some(device) = super::devices::device() else {
        return;
    };
    let link = device.link().clone();
    let relay = link
        .state()
        .connected
        .and_then(|c| c.hello)
        .is_some_and(|h| h.capabilities & foxglove_link::protocol::capabilities::ART_RELAY != 0);
    if relay {
        let _ = foxglove_link::art::wait_relay_ready(&link, RELAY_SETTLE);
    }
}

/// Copy diagnostics' lines: every row, the worst first.
pub fn diagnostics_lines(rows: &[HealthRow]) -> Vec<String> {
    let mut rows: Vec<&HealthRow> = rows.iter().collect();
    rows.sort_by_key(|r| std::cmp::Reverse(r.level));
    rows.into_iter()
        .map(|r| format!("{:<8}{}", r.level.word(), r.line()))
        .collect()
}

/// The card's rows, grouped: failed first, then warnings, then OK, each in the order they came.
pub fn ordered(rows: &[HealthRow]) -> Vec<HealthRow> {
    let mut out = rows.to_vec();
    out.sort_by_key(|r| std::cmp::Reverse(r.level)); // stable: keeps the order within a level
    out
}

/// What the card draws: the rows and the state of the last read.
#[derive(Clone, Debug, Default)]
pub struct View {
    pub rows: Vec<HealthRow>,
    /// "Checked 10:42" or "Not checked yet".
    pub when: String,
    pub reading: bool,
    pub keyboard: bool,
}

/// The card's view now.
pub fn view() -> View {
    let s = snapshot();
    let mut rows = s.rows;
    rows.extend(feature_rows());
    View {
        rows: ordered(&rows),
        when: s.read_at.map_or("Not checked yet".into(), |t| {
            let secs = t.elapsed().map_or(0, |d| d.as_secs());
            if secs < 60 {
                "Checked just now".into()
            } else {
                format!("Checked {} ago", kh::duration_words(secs as u32))
            }
        }),
        reading: s.reading,
        keyboard: super::devices::host_link().is_some(),
    }
}

/// The App page's "Keyboard health" card: one line per feature, OK / warning / failed and why.
pub fn group(ui: &mut egui::Ui) {
    let v = view();
    if draw(ui, &v) {
        request_read(ui.ctx());
    }
}

/// Draws `v`; true when "Check now" was clicked.
pub fn draw(ui: &mut egui::Ui, v: &View) -> bool {
    use egui::RichText;
    let mut clicked = false;
    super::settings_ui::section(ui, "Keyboard health", |ui| {
        let p = super::theme::palette(ui);
        ui.horizontal_wrapped(|ui| {
            let button = ui.add_enabled(
                v.keyboard && !v.reading,
                egui::Button::new(if v.reading {
                    "Checking…"
                } else {
                    "Check now"
                }),
            );
            clicked = button
                .on_hover_text("Asks both halves how each part has done since they started")
                .clicked();
            ui.label(RichText::new(&v.when).color(p.muted));
        });
        if v.rows.is_empty() {
            ui.label(
                RichText::new(if v.keyboard {
                    "Not checked yet: Check now asks the keyboard."
                } else {
                    "No keyboard with Foxglove's firmware is connected."
                })
                .color(p.muted),
            );
            return;
        }
        ui.add_space(4.0);
        egui::Grid::new("keyboard-health")
            .num_columns(2)
            .spacing([12.0, 4.0])
            .show(ui, |ui| {
                for r in &v.rows {
                    let (word, colour) = match r.level {
                        Level::Ok => ("OK", p.ok),
                        Level::Warn => ("Warning", p.warn),
                        Level::Fail => ("Failed", p.danger),
                    };
                    ui.label(RichText::new(word).color(colour).strong());
                    ui.add(egui::Label::new(r.line()).wrap());
                    ui.end_row();
                }
            });
    });
    clicked
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    fn row(item: &str, level: Level, detail: &str) -> HealthRow {
        HealthRow::new("screens", item, level, detail)
    }

    /// What the review images show: a keyboard with a failing right screen and a slow relay.
    pub fn sample() -> View {
        View {
            rows: ordered(&[
                row("Left half", Level::Ok, "firmware 0.1.0 (dev) c068bcc, up 2 h 4 min, last reset: power on"),
                row("Right half", Level::Ok, "firmware 0.1.0 (dev) c068bcc, up 2 h 4 min, last reset: a software reset (a flash, the bootloader or a reset key)"),
                row("Right screen", Level::Fail, "5 art pushes failed (relay timeout or write failure)"),
                row("Relay to the right half", Level::Warn, "2 requests timed out"),
                row("Reports to the PC", Level::Ok, "no problems since start"),
                row("Left screen", Level::Ok, "started"),
                row("Right lights", Level::Ok, "21 LEDs ready"),
                row("Link between the halves", Level::Warn, "Left half saw it drop 3 times"),
                row("Stay awake", Level::Ok, "held awake: the PC is on"),
                HealthRow::new("screens.art_keys", "Art keys", Level::Ok, "Foxglove holds them: 12 presses, 12 stepped"),
            ]),
            when: "Checked just now".into(),
            reading: false,
            keyboard: true,
        }
    }

    pub fn sample_none() -> View {
        View::default()
    }

    #[test]
    fn failed_rows_come_first_and_keep_their_order() {
        let rows = ordered(&[
            row("a", Level::Ok, ""),
            row("b", Level::Fail, ""),
            row("c", Level::Warn, ""),
            row("d", Level::Fail, ""),
        ]);
        let items: Vec<&str> = rows.iter().map(|r| r.item.as_str()).collect();
        assert_eq!(items, ["b", "d", "c", "a"]);
    }

    #[test]
    fn the_log_speaks_on_connect_and_when_failed_changes() {
        let ok = vec![row("Right screen", Level::Ok, "no problems since start")];
        let bad = vec![row(
            "Right screen",
            Level::Fail,
            "5 art pushes failed (relay timeout)",
        )];
        let line = log_line(Why::Connect, &[], &ok).unwrap();
        assert!(
            line.starts_with("Foxglove: on connect, keyboard health OK"),
            "{line}"
        );
        assert_eq!(log_line(Why::Asked, &[], &ok), None, "nothing changed");
        let line = log_line(Why::Asked, &[], &bad).unwrap();
        assert_eq!(
            line,
            "Foxglove: keyboard health failed: Right screen: 5 art pushes failed (relay timeout)"
        );
        let before = kh::failed_lines(&bad);
        assert_eq!(
            log_line(Why::Asked, &before, &bad),
            None,
            "the same failure again"
        );
        assert_eq!(
            log_line(Why::Asked, &before, &ok).unwrap(),
            "Foxglove: keyboard health: nothing failed now"
        );
    }

    #[test]
    fn features_register_rows_once_by_name() {
        fn mine() -> Vec<HealthRow> {
            vec![HealthRow::new(
                "screens.art_keys",
                "Art keys",
                Level::Warn,
                "not held",
            )]
        }
        register("test-art-keys", mine);
        register("test-art-keys", mine);
        let rows: Vec<HealthRow> = feature_rows()
            .into_iter()
            .filter(|r| r.item == "Art keys")
            .collect();
        assert_eq!(rows.len(), 1);
    }

    #[test]
    fn diagnostics_put_the_worst_first() {
        let lines = diagnostics_lines(&[
            row("Left screen", Level::Ok, "started"),
            row(
                "Right screen",
                Level::Fail,
                "2 art pushes failed (relay timeout)",
            ),
        ]);
        assert_eq!(
            lines[0],
            "failed  Right screen: 2 art pushes failed (relay timeout)"
        );
        assert_eq!(lines[1], "OK      Left screen: started");
    }
}
