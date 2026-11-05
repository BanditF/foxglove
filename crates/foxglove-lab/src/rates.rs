//! What each light can take (#486): a bounded burst of writes to every light the lab reaches,
//! one in flight, and the sustained accepted rate and write latency (p50/p95) of each, as the
//! scene catalog's data (`crates/devices/light-rates.toml`, `foxglove_devices::pc_lighting::
//! rates`). `foxglove-lab rates` runs it; `--out` writes the measured entries into that file, so
//! scenes' intervals change with data, not code.
//!
//! The lights, by class, never by vendor:
//! - the keyboard's frames (SET_FRAME, D7 0C / D7 8C, HELLO bit 14), on the bus the link uses
//!   (run it on USB and on BLE for both entries; the slower one counts). The right half's own
//!   rate (its 10 fps coalescing over BLE) is the zmk-sim host-frames case, not seen from here;
//! - every catalog board with a driver (the OMEN case's TracerLED, 59-byte reports per zone),
//!   through the same driver and calibration the app uses;
//! - every Windows Dynamic Lighting (LampArray) device Windows lets this process drive. A
//!   LampArray's latency is the `SetColorsForIndices` call: Windows queues the colours.
//!
//! Every burst is bounded: at most `--writes` writes and `--seconds` (never over 2 minutes) per
//! light, and five failed writes in a row end it. Each keyboard request waits at most the link's
//! 500 ms. The user's lights come back after each burst whatever happened: the keyboard's frames
//! are released (its own lights again), a case board gets the colours saved in Foxglove's
//! settings (white where none were), a LampArray is let go (Windows' own lighting again).
//! Light-to-eye latency needs a camera: a #90 row if the numbers look off.

use crate::health::{row_line, Outcome, BLOCKED, FAIL, PASS};
use foxglove_devices::pc_lighting::rates::{LightRate, Measured, Rates};
use foxglove_devices::pc_lighting::{self as pcl, LightingDevice};
use foxglove_devices::Rgb;
use foxglove_link::health::{HealthRow, Level};
use foxglove_link::protocol::capabilities;
use foxglove_link::{settings, Transport};
use std::time::{Duration, Instant};

/// The rows' group.
pub const GROUP: &str = "rates";

/// The longest burst on one light (#486: at most 2 minutes per device).
pub const MAX_TIME: Duration = Duration::from_secs(120);
/// Failed writes in a row that end a burst: the light isn't taking them.
pub const MAX_ERRORS_IN_A_ROW: u32 = 5;

/// How much one burst may do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Budget {
    pub writes: u32,
    pub time: Duration,
}

impl Budget {
    /// `foxglove-lab rates`' default: 400 writes or 10 s per light, whichever ends first.
    pub const DEFAULT: Budget = Budget {
        writes: 400,
        time: Duration::from_secs(10),
    };

    /// Within the limits: at least one write, at most [`MAX_TIME`].
    pub fn bounded(writes: u32, time: Duration) -> Budget {
        Budget {
            writes: writes.max(1),
            time: time.min(MAX_TIME),
        }
    }
}

/// Which light a burst measured, as the rates file names it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Light {
    /// What the rows call it ("Keyboard frames (USB)", the board's name).
    pub name: String,
    /// `keyboard`, a catalog protocol's id or `lamp-array`.
    pub class: String,
    /// `vvvv:pppp`, for a model's own entry.
    pub device: Option<String>,
    /// `usb` or `ble`.
    pub transport: Option<String>,
    /// The firmware's build, when the device says.
    pub build: Option<String>,
}

/// One light the burst writes to.
pub trait Writer {
    /// The `i`th write of the burst (colours alternate, so each write changes the lights); `Err`
    /// when the light refused it or the write failed.
    fn write(&mut self, i: u32) -> Result<(), String>;
    /// The user's lights back. Runs once after the burst, whatever happened.
    fn restore(&mut self) -> Result<(), String>;
}

/// Why a burst ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stop {
    Writes,
    Time,
    Errors,
}

/// What a burst saw.
#[derive(Clone, Debug, PartialEq)]
pub struct Sample {
    /// The accepted writes' latencies.
    pub latencies: Vec<Duration>,
    pub writes: u32,
    pub errors: u32,
    pub elapsed: Duration,
    pub cpu: Option<Duration>,
    pub stop: Stop,
    /// The last failed write's message.
    pub last_error: Option<String>,
}

/// One burst on `w` within `budget`. `cpu` is the process's CPU time so far, when known.
pub fn burst(w: &mut dyn Writer, budget: Budget, cpu: &dyn Fn() -> Option<Duration>) -> Sample {
    let cpu_start = cpu();
    let start = Instant::now();
    let deadline = start + budget.time;
    let mut s = Sample {
        latencies: Vec::with_capacity(budget.writes.min(4096) as usize),
        writes: 0,
        errors: 0,
        elapsed: Duration::ZERO,
        cpu: None,
        stop: Stop::Writes,
        last_error: None,
    };
    let mut in_a_row = 0;
    while s.writes < budget.writes {
        if Instant::now() >= deadline {
            s.stop = Stop::Time;
            break;
        }
        let t = Instant::now();
        let r = w.write(s.writes);
        let took = t.elapsed();
        s.writes += 1;
        match r {
            Ok(()) => {
                in_a_row = 0;
                s.latencies.push(took);
            }
            Err(e) => {
                s.errors += 1;
                in_a_row += 1;
                s.last_error = Some(e);
                if in_a_row >= MAX_ERRORS_IN_A_ROW {
                    s.stop = Stop::Errors;
                    break;
                }
            }
        }
    }
    s.elapsed = start.elapsed();
    s.cpu = cpu_start.zip(cpu()).map(|(a, b)| b.saturating_sub(a));
    s
}

/// The `q` quantile (0-1) of sorted values, nearest rank.
fn quantile(sorted: &[Duration], q: f64) -> Duration {
    if sorted.is_empty() {
        return Duration::ZERO;
    }
    let rank = (q * sorted.len() as f64).ceil() as usize;
    sorted[rank.clamp(1, sorted.len()) - 1]
}

fn ms(d: Duration) -> f64 {
    (d.as_secs_f64() * 10_000.0).round() / 10.0
}

impl Sample {
    /// As the rates file keeps it.
    pub fn measured(&self, at: &str, build: Option<String>) -> Measured {
        let mut sorted = self.latencies.clone();
        sorted.sort();
        let secs = self.elapsed.as_secs_f64();
        let ok = self.latencies.len() as f64;
        Measured {
            rate_hz: if secs > 0.0 {
                (ok / secs * 10.0).round() / 10.0
            } else {
                0.0
            },
            p50_ms: ms(quantile(&sorted, 0.5)),
            p95_ms: ms(quantile(&sorted, 0.95)),
            writes: self.writes,
            errors: self.errors,
            cpu_ms: self.cpu.map(|c| c.as_millis() as u64),
            seconds: (secs * 100.0).round() / 100.0,
            at: at.into(),
            build,
        }
    }
}

/// What the run found: a row per light, what couldn't be measured, and the rates file's
/// entries for the lights that gave one.
#[derive(Clone, Debug, Default)]
pub struct Report {
    pub rows: Vec<HealthRow>,
    pub blocked: Vec<String>,
    pub rates: Vec<LightRate>,
}

impl Report {
    /// Exit code: 1 when a light failed, else 2 when something couldn't be measured, else 0.
    pub fn code(&self) -> u8 {
        if foxglove_link::health::worst(&self.rows) == Level::Fail {
            FAIL
        } else if !self.blocked.is_empty() || self.rows.is_empty() {
            BLOCKED
        } else {
            PASS
        }
    }

    /// One line per row as `health` prints them, one per light not measured, then the result.
    pub fn lines(&self) -> Vec<String> {
        let mut out: Vec<String> = self.rows.iter().map(row_line).collect();
        out.extend(
            self.blocked
                .iter()
                .map(|why| format!("BLOCK {GROUP:<10} {why}")),
        );
        let fails = self.rows.iter().filter(|r| r.level == Level::Fail).count();
        let warns = self.rows.iter().filter(|r| r.level == Level::Warn).count();
        out.push(match self.code() {
            FAIL => format!("FAIL: {fails} failed, {warns} warning(s)"),
            BLOCKED if self.blocked.is_empty() => "BLOCKED: no light to measure".into(),
            BLOCKED => format!("BLOCKED: {}", self.blocked.join("; ")),
            _ => format!("PASS: nothing failed, {warns} warning(s)"),
        });
        out
    }

    /// As JSON: the rows, the blocked list and the measured entries (the rates file's shape).
    pub fn json(&self) -> serde_json::Value {
        let mut v = Outcome {
            rows: self.rows.clone(),
            blocked: None,
        }
        .json();
        v["result"] = match self.code() {
            PASS => "pass",
            FAIL => "fail",
            _ => "blocked",
        }
        .into();
        v["blocked"] = self.blocked.clone().into();
        v["rates"] = serde_json::to_value(&self.rates).unwrap_or_default();
        v
    }

    /// The measured entries put into `rates` (a measurement replaces the same light's entry and
    /// its class's unmeasured default).
    pub fn apply(&self, rates: &mut Rates) {
        for r in &self.rates {
            rates.put(r.clone());
        }
    }

    fn row(&mut self, item: &str, level: Level, detail: impl Into<String>) {
        self.rows.push(HealthRow::new(GROUP, item, level, detail));
    }

    /// One light's burst, then its lights back; its row and, when it gave one, its entry.
    pub fn measure(
        &mut self,
        light: &Light,
        w: &mut dyn Writer,
        budget: Budget,
        at: &str,
        cpu: &dyn Fn() -> Option<Duration>,
    ) {
        let sample = burst(w, budget, cpu);
        let restored = w.restore();
        let m = sample.measured(at, light.build.clone());
        let interval = m.interval_ms();
        let cpu_text = m.cpu_ms.map_or(String::new(), |c| format!(", CPU {c} ms"));
        let stop = match sample.stop {
            Stop::Writes => "",
            Stop::Time => " (time's up)",
            Stop::Errors => " (stopped: 5 failed in a row)",
        };
        let what = format!(
            "{} writes in {:.1} s{stop}: {} a second accepted, p50 {} ms, p95 {} ms, {} error(s){cpu_text}",
            m.writes, m.seconds, m.rate_hz, m.p50_ms, m.p95_ms, m.errors
        );
        let last = sample
            .last_error
            .as_deref()
            .map_or(String::new(), |e| format!("; last error: {e}"));
        match interval {
            None => self.row(
                &light.name,
                Level::Fail,
                format!("{what}{last}: too many failed to give a rate"),
            ),
            Some(ms) => {
                let level = if m.errors > 0 { Level::Warn } else { Level::Ok };
                let detail = if m.errors > 0 {
                    format!("{what} -> interval {ms} ms{last}")
                } else {
                    format!("{what} -> interval {ms} ms")
                };
                self.row(&light.name, level, detail);
                self.rates.push(LightRate {
                    class: light.class.clone(),
                    device: light.device.clone(),
                    transport: light.transport.clone(),
                    interval_ms: ms,
                    measured: Some(m),
                });
            }
        }
        if let Err(e) = restored {
            self.row(
                &format!("{} restore", light.name),
                Level::Fail,
                format!("the user's lights didn't go back: {e}"),
            );
        }
    }
}

const RED: [u8; 3] = [200, 0, 0];
const BLUE: [u8; 3] = [0, 0, 200];

fn colour(i: u32) -> [u8; 3] {
    if i.is_multiple_of(2) {
        BLUE
    } else {
        RED
    }
}

/// The keyboard's frames: whole frames of every LED, one in flight, each answered ok and the
/// PC's; RELEASE gives the keyboard its own lights back.
pub struct KeyboardFrames<'a, T: Transport> {
    t: &'a T,
    lamps: usize,
}

impl<'a, T: Transport> KeyboardFrames<'a, T> {
    /// `None` without SET_FRAME (HELLO bit 14). The LED count is the board's (LAMP_PARTS, bit
    /// 22), else the Corne's 42.
    pub fn new(t: &'a T, caps: u32) -> Option<Self> {
        if caps & capabilities::FRAMES == 0 {
            return None;
        }
        let lamps = (caps & capabilities::LAMP_PARTS != 0)
            .then(|| settings::lamp_parts(t).ok())
            .flatten()
            .unwrap_or_else(foxglove_link::lamps::LampParts::old_firmware)
            .total();
        Some(KeyboardFrames { t, lamps })
    }
}

impl<T: Transport> Writer for KeyboardFrames<'_, T> {
    fn write(&mut self, i: u32) -> Result<(), String> {
        match settings::set_frame(self.t, 0, &vec![colour(i); self.lamps]) {
            Err(e) => Err(format!("no reply: {e:#}")),
            Ok(r) if r.status != 0 => Err(format!("status {}", r.status)),
            Ok(r) if !r.held() => Err(format!("owner {} (want 3)", r.owner)),
            Ok(_) => Ok(()),
        }
    }

    fn restore(&mut self) -> Result<(), String> {
        match settings::release_frame(self.t) {
            Err(e) => Err(format!("no reply to RELEASE: {e:#}")),
            Ok(r) if r.status != 0 => Err(format!("RELEASE status {}", r.status)),
            Ok(r) if r.held() => Err("RELEASE ok but the PC still owns the lights".into()),
            Ok(_) => Ok(()),
        }
    }
}

/// A light through its Foxglove driver (a case board's, a LampArray's): every zone one colour a
/// write. `restore`: the user's colours per zone, or nothing to put back (a LampArray, which
/// Windows lights again once it is let go).
pub struct DriverWriter {
    device: Box<dyn LightingDevice>,
    restore: Option<Vec<(usize, Rgb)>>,
}

impl DriverWriter {
    pub fn new(device: Box<dyn LightingDevice>, restore: Option<Vec<(usize, Rgb)>>) -> Self {
        DriverWriter { device, restore }
    }
}

impl Writer for DriverWriter {
    fn write(&mut self, i: u32) -> Result<(), String> {
        let [r, g, b] = colour(i);
        self.device
            .set_all(Rgb { r, g, b })
            .map_err(|e| e.to_string())
    }

    fn restore(&mut self) -> Result<(), String> {
        match &self.restore {
            Some(colours) => self.device.set_zones(colours).map_err(|e| e.to_string()),
            None => Ok(()),
        }
    }
}

/// The colours the user saved for a device in Foxglove's settings (`[pc_lighting.devices."key"]
/// colors`, zone id to `#rrggbb`), per zone in `zones`' order; white where none was saved.
pub fn saved_colours(store: &str, key: &str, zones: &[pcl::ZoneSpec]) -> Vec<(usize, Rgb)> {
    let table: toml::Table = store.parse().unwrap_or_default();
    let colors = table
        .get("pc_lighting")
        .and_then(|v| v.get("devices"))
        .and_then(|v| v.get(key))
        .and_then(|v| v.get("colors"));
    let saved: Vec<Option<Rgb>> = zones
        .iter()
        .map(|z| {
            let hex = colors?.get(z.id.to_string())?.as_str()?;
            let v = u32::from_str_radix(hex.trim_start_matches('#'), 16).ok()?;
            (hex.trim_start_matches('#').len() == 6).then_some(Rgb {
                r: (v >> 16) as u8,
                g: (v >> 8) as u8,
                b: v as u8,
            })
        })
        .collect();
    pcl::restore_colors(&vec![None; zones.len()], &saved)
}

/// Every catalog board with a driver on this PC, opened as the app opens it: the bundled
/// catalog with the user's own entries (`lighting-catalog.local.toml` beside the settings),
/// each with the user's saved colours to put back.
pub fn case_boards() -> Result<Vec<(Light, DriverWriter)>, String> {
    let store = crate::host::store_path();
    let read = |name: &str| {
        store
            .as_ref()
            .and_then(|p| std::fs::read_to_string(p.with_file_name(name)).ok())
            .unwrap_or_default()
    };
    let local = pcl::Catalog::parse(&read("lighting-catalog.local.toml")).unwrap_or_default();
    let catalog = pcl::Catalog::bundled().with_local(&local);
    let settings = read("foxglove.toml");
    let hid = pcl::hid::enumerate().map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for f in pcl::discover(&catalog, &hid) {
        let pcl::Control::Driver(protocol) = f.control else {
            continue;
        };
        let writer = pcl::hid::PathWriter::new(&f.path).map_err(|e| e.to_string())?;
        let device = pcl::open(&f, Box::new(writer)).map_err(|e| format!("{}: {e}", f.name))?;
        let restore = saved_colours(&settings, &f.key, &f.zones);
        out.push((
            Light {
                name: f.name.clone(),
                class: protocol.id().into(),
                device: Some(format!("{:04x}:{:04x}", f.vid, f.pid)),
                transport: Some("usb".into()),
                build: None,
            },
            DriverWriter::new(device, Some(restore)),
        ));
    }
    Ok(out)
}

/// Every Windows Dynamic Lighting device, through `Windows.Devices.Lights` as the app drives
/// them. One Windows doesn't let this process drive now (not in front, not the background
/// controller the user picked, or switched off in Settings) is a reason in the second list.
#[cfg(windows)]
pub fn lamp_arrays() -> (Vec<(Light, DriverWriter)>, Vec<String>) {
    lamps::all()
}

#[cfg(not(windows))]
pub fn lamp_arrays() -> (Vec<(Light, DriverWriter)>, Vec<String>) {
    (
        Vec::new(),
        vec!["LampArrays not measured: Windows only".into()],
    )
}

#[cfg(windows)]
mod lamps {
    use super::{DriverWriter, Light};
    use foxglove_devices::pc_lighting::{self as pcl, LampHandle};
    use foxglove_devices::{DeviceError, Rgb};
    use windows::Devices::Enumeration::DeviceInformation;
    use windows::Devices::Lights::LampArray;
    use windows::UI::Color;

    struct Handle {
        array: LampArray,
        count: usize,
    }

    fn c(rgb: Rgb) -> Color {
        Color {
            A: 255,
            R: rgb.r,
            G: rgb.g,
            B: rgb.b,
        }
    }

    impl LampHandle for Handle {
        fn lamp_count(&self) -> usize {
            self.count
        }

        fn set_all(&mut self, color: Rgb) -> foxglove_devices::Result<()> {
            self.array
                .SetColor(c(color))
                .map_err(|e| DeviceError::Backend(e.to_string()))
        }

        fn set_lamps(&mut self, lamps: &[(usize, Rgb)]) -> foxglove_devices::Result<()> {
            let colours: Vec<Color> = lamps.iter().map(|(_, rgb)| c(*rgb)).collect();
            let indices: Vec<i32> = lamps.iter().map(|(i, _)| *i as i32).collect();
            self.array
                .SetColorsForIndices(&colours, &indices)
                .map_err(|e| DeviceError::Backend(e.to_string()))
        }
    }

    pub(super) fn all() -> (Vec<(Light, DriverWriter)>, Vec<String>) {
        let mut out = Vec::new();
        let mut blocked = Vec::new();
        let list = LampArray::GetDeviceSelector()
            .and_then(|s| DeviceInformation::FindAllAsyncAqsFilter(&s)?.join());
        let list = match list {
            Ok(l) => l,
            Err(e) => {
                blocked.push(format!("LampArrays not listed: {e}"));
                return (out, blocked);
            }
        };
        for info in &list {
            let Ok(id) = info.Id() else { continue };
            let Ok(array) = LampArray::FromIdAsync(&id).and_then(|op| op.join()) else {
                continue;
            };
            let vid = array.HardwareVendorId().unwrap_or(0);
            let pid = array.HardwareProductId().unwrap_or(0);
            let model = format!("{vid:04x}:{pid:04x}");
            let name = format!("LampArray {model}");
            let enabled = array.IsEnabled().unwrap_or(true);
            // Windows before 11 22H2 has no IsAvailable: there, any app may.
            let available = array.IsAvailable().unwrap_or(true);
            if !enabled || !available {
                blocked.push(format!(
                    "{name} not measured: Windows doesn't let foxglove-lab drive it now ({})",
                    if enabled {
                        "not in front and not the background controller"
                    } else {
                        "switched off in Settings > Dynamic Lighting"
                    }
                ));
                continue;
            }
            let count = array.LampCount().unwrap_or(0).max(0) as usize;
            if count == 0 {
                blocked.push(format!("{name} not measured: no lamps"));
                continue;
            }
            let lamps = vec![pcl::Lamp::default(); count];
            let zones = pcl::lamp_array::lamp_zones(&lamps);
            let handle = Box::new(Handle { array, count });
            match pcl::LampArrayDriver::new(name.clone(), &zones, handle) {
                Ok(d) => out.push((
                    Light {
                        name,
                        class: pcl::rates::LAMP_ARRAY.into(),
                        device: Some(model),
                        transport: None,
                        build: None,
                    },
                    DriverWriter::new(Box::new(d), None),
                )),
                Err(e) => blocked.push(format!("{name} not measured: {e}")),
            }
        }
        (out, blocked)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use foxglove_link::protocol::{self, REPORT};
    use std::cell::RefCell;

    /// A light that takes `ok` writes, then fails the rest; each write takes `each`.
    struct Fake {
        ok: u32,
        each: Duration,
        written: u32,
        restored: u32,
        restore_fails: bool,
    }

    impl Fake {
        fn new(ok: u32) -> Fake {
            Fake {
                ok,
                each: Duration::ZERO,
                written: 0,
                restored: 0,
                restore_fails: false,
            }
        }
    }

    impl Writer for Fake {
        fn write(&mut self, _: u32) -> Result<(), String> {
            self.written += 1;
            std::thread::sleep(self.each);
            if self.written <= self.ok {
                Ok(())
            } else {
                Err("refused".into())
            }
        }

        fn restore(&mut self) -> Result<(), String> {
            self.restored += 1;
            if self.restore_fails {
                Err("unplugged".into())
            } else {
                Ok(())
            }
        }
    }

    fn no_cpu() -> Option<Duration> {
        None
    }

    fn light(class: &str) -> Light {
        Light {
            name: format!("{class} light"),
            class: class.into(),
            ..Light::default()
        }
    }

    #[test]
    fn a_burst_stops_at_its_writes_its_time_or_its_errors() {
        let mut f = Fake::new(u32::MAX);
        let s = burst(&mut f, Budget::bounded(50, Duration::from_secs(5)), &no_cpu);
        assert_eq!((s.writes, s.errors, s.stop), (50, 0, Stop::Writes));
        assert_eq!(s.latencies.len(), 50);

        let mut slow = Fake::new(u32::MAX);
        slow.each = Duration::from_millis(5);
        let s = burst(
            &mut slow,
            Budget::bounded(10_000, Duration::from_millis(40)),
            &no_cpu,
        );
        assert_eq!(s.stop, Stop::Time);
        assert!(s.writes < 100, "{} writes in 40 ms", s.writes);

        let mut dead = Fake::new(3);
        let s = burst(&mut dead, Budget::DEFAULT, &no_cpu);
        assert_eq!(s.stop, Stop::Errors);
        assert_eq!(
            (s.writes, s.errors),
            (3 + MAX_ERRORS_IN_A_ROW, MAX_ERRORS_IN_A_ROW)
        );
        assert_eq!(s.last_error.as_deref(), Some("refused"));

        assert_eq!(Budget::bounded(0, Duration::from_secs(600)).time, MAX_TIME);
        assert_eq!(Budget::bounded(0, Duration::ZERO).writes, 1);
    }

    #[test]
    fn the_numbers_come_from_the_accepted_writes() {
        let s = Sample {
            latencies: (1..=20).map(Duration::from_millis).collect(),
            writes: 21,
            errors: 1,
            elapsed: Duration::from_secs(2),
            cpu: Some(Duration::from_millis(7)),
            stop: Stop::Writes,
            last_error: None,
        };
        let m = s.measured("2026-11-05T00:00:00Z", Some("0.3.0".into()));
        assert_eq!(m.rate_hz, 10.0, "20 accepted in 2 s");
        assert_eq!((m.p50_ms, m.p95_ms), (10.0, 19.0));
        assert_eq!((m.writes, m.errors, m.cpu_ms), (21, 1, Some(7)));
        assert_eq!(m.seconds, 2.0);
        assert_eq!(m.build.as_deref(), Some("0.3.0"));
        // 100 ms a write at 10 a second, with headroom.
        assert_eq!(m.interval_ms(), Some(150));
        assert_eq!(quantile(&[], 0.5), Duration::ZERO);
    }

    #[test]
    fn each_light_gets_a_row_an_entry_and_its_lights_back() {
        let mut report = Report::default();
        let mut good = Fake::new(u32::MAX);
        report.measure(
            &Light {
                device: Some("103c:84fd".into()),
                transport: Some("usb".into()),
                ..light("omen-tracer")
            },
            &mut good,
            Budget::bounded(20, Duration::from_secs(5)),
            "now",
            &no_cpu,
        );
        let mut dead = Fake::new(0);
        dead.restore_fails = true;
        report.measure(
            &light("keyboard"),
            &mut dead,
            Budget::DEFAULT,
            "now",
            &no_cpu,
        );
        assert_eq!((good.restored, dead.restored), (1, 1));

        let lines = report.lines();
        assert!(
            lines[0].starts_with("PASS rates      omen-tracer light: 20 writes in "),
            "{lines:#?}"
        );
        assert!(lines[0].contains("-> interval 34 ms"), "{lines:#?}");
        assert!(lines[1].starts_with("FAIL rates      keyboard light: 5 writes in "));
        assert!(lines[1].ends_with("last error: refused: too many failed to give a rate"));
        assert_eq!(
            lines[2],
            "FAIL rates      keyboard light restore: the user's lights didn't go back: unplugged"
        );
        assert_eq!(report.code(), FAIL);
        assert_eq!(report.rates.len(), 1, "only a light that gave a rate");
        assert_eq!(report.rates[0].device.as_deref(), Some("103c:84fd"));

        // Into the rates file: the case board's default is replaced by its measurement.
        let mut rates = Rates::bundled();
        report.apply(&mut rates);
        assert_eq!(
            rates.interval("omen-tracer", Some("103c:84fd")),
            Some(Duration::from_millis(34))
        );
        let json = report.json();
        assert_eq!(json["result"], "fail");
        assert_eq!(json["rates"][0]["class"], "omen-tracer");
        assert_eq!(json["rates"][0]["measured"]["writes"], 20);
    }

    #[test]
    fn nothing_to_measure_is_blocked() {
        let r = Report {
            blocked: vec!["no keyboard".into()],
            ..Report::default()
        };
        assert_eq!(r.code(), BLOCKED);
        assert_eq!(r.lines().last().unwrap(), "BLOCKED: no keyboard");
        assert_eq!(Report::default().code(), BLOCKED);
        assert_eq!(
            Report::default().lines().last().unwrap(),
            "BLOCKED: no light to measure"
        );
    }

    /// The keyboard's side of SET_FRAME as host_frames.c answers it.
    #[derive(Default)]
    struct Kb {
        owned: RefCell<bool>,
        frames: RefCell<u32>,
        refuse_after: Option<u32>,
    }

    impl Transport for Kb {
        fn send(&self, r: &[u8]) -> anyhow::Result<()> {
            assert_eq!(
                &r[..2],
                &[0xd7, 0x0c],
                "only frame chunks go without a reply"
            );
            Ok(())
        }

        fn request(
            &self,
            r: &[u8],
            proto: u8,
            cmd: u8,
            _: Duration,
        ) -> anyhow::Result<[u8; REPORT]> {
            assert_eq!((proto, cmd), (0xd7, 0x0c));
            let mut status = 0;
            if r[2] & protocol::frame::RELEASE != 0 {
                *self.owned.borrow_mut() = false;
            } else {
                let mut n = self.frames.borrow_mut();
                *n += 1;
                if self.refuse_after.is_some_and(|a| *n > a) {
                    status = 5;
                } else {
                    *self.owned.borrow_mut() = true;
                }
            }
            let mut out = [0; REPORT];
            let owner = if *self.owned.borrow() { 3 } else { 0 };
            out[..7].copy_from_slice(&[0xd7, 0x8c, status, owner, 30, 1, 0]);
            Ok(out)
        }
    }

    #[test]
    fn keyboard_frames_need_the_bit_and_are_released() {
        let kb = Kb::default();
        assert!(KeyboardFrames::new(&kb, 0).is_none(), "no HELLO bit 14");
        let mut w = KeyboardFrames::new(&kb, capabilities::FRAMES).unwrap();
        let mut report = Report::default();
        report.measure(
            &light("keyboard"),
            &mut w,
            Budget::bounded(12, Duration::from_secs(5)),
            "now",
            &no_cpu,
        );
        assert_eq!(report.code(), PASS, "{:#?}", report.lines());
        assert_eq!(*kb.frames.borrow(), 12);
        assert!(!*kb.owned.borrow(), "the keyboard has its lights back");

        let kb = Kb {
            refuse_after: Some(2),
            ..Kb::default()
        };
        let mut w = KeyboardFrames::new(&kb, capabilities::FRAMES).unwrap();
        assert_eq!(w.write(0), Ok(()));
        assert_eq!(w.write(1), Ok(()));
        assert_eq!(w.write(2), Err("status 5".into()));
        assert_eq!(w.restore(), Ok(()));
    }

    /// A driver that keeps what it was told.
    struct Board {
        zones: usize,
        last: std::sync::Arc<std::sync::Mutex<Vec<Rgb>>>,
    }

    impl LightingDevice for Board {
        fn name(&self) -> &str {
            "board"
        }
        fn zones(&self) -> Vec<String> {
            (0..self.zones).map(|i| i.to_string()).collect()
        }
        fn colors(&self) -> Vec<Option<Rgb>> {
            vec![None; self.zones]
        }
        fn set_zones(&mut self, colors: &[(usize, Rgb)]) -> foxglove_devices::Result<()> {
            let mut last = self.last.lock().unwrap();
            for (i, c) in colors {
                last[*i] = *c;
            }
            Ok(())
        }
    }

    fn zone(id: u8) -> pcl::ZoneSpec {
        pcl::ZoneSpec {
            id,
            name: format!("zone {id}"),
            position: None,
            role: Default::default(),
        }
    }

    #[test]
    fn a_case_board_gets_the_users_saved_colours_back() {
        let store = r##"
            [pc_lighting.devices."103c:84fd"]
            match_keyboard = false
            [pc_lighting.devices."103c:84fd".colors]
            1 = "#ff8800"
            5 = "bad"
        "##;
        let zones = [zone(1), zone(2), zone(5)];
        let white = pcl::WHITE;
        let orange = Rgb {
            r: 0xff,
            g: 0x88,
            b: 0,
        };
        let restore = saved_colours(store, "103c:84fd", &zones);
        assert_eq!(restore, vec![(0, orange), (1, white), (2, white)]);
        assert_eq!(
            saved_colours("not toml [", "103c:84fd", &zones),
            vec![(0, white), (1, white), (2, white)],
            "nothing readable: white, never dark"
        );

        let last = std::sync::Arc::new(std::sync::Mutex::new(vec![pcl::BLACK; 3]));
        let board = Board {
            zones: 3,
            last: last.clone(),
        };
        let mut w = DriverWriter::new(Box::new(board), Some(restore));
        let mut report = Report::default();
        report.measure(
            &light("omen-tracer"),
            &mut w,
            Budget::bounded(7, Duration::from_secs(5)),
            "now",
            &no_cpu,
        );
        assert_eq!(report.code(), PASS);
        assert_eq!(*last.lock().unwrap(), vec![orange, white, white]);
        // A LampArray has nothing to put back: Windows lights it again once it's let go.
        let mut lamp = DriverWriter::new(
            Box::new(Board {
                zones: 1,
                last: std::sync::Arc::new(std::sync::Mutex::new(vec![pcl::BLACK])),
            }),
            None,
        );
        assert_eq!(lamp.restore(), Ok(()));
    }
}
