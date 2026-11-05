//! The PC's lights end to end (#492): per-LED frames (SET_FRAME, D7 0C / D7 8C, HELLO bit 14) and
//! the firmware's own effects (SET_EFFECT, D7 0F / D7 8F, HELLO bit 19), as cases with a clear
//! pass, fail or blocked, for the test after a batch flash. `foxglove-lab health` runs the quick
//! set after the self-report; `foxglove-lab lights` runs them alone (`--watchdog` also waits out
//! the keyboard's frame timeout for its hand-back).
//!
//! Each case checks the reply's status and owner, and, with HELLO bit 24 (RGB_SHOWN), reads the
//! left's LEDs back from RGB_STATE (D7 83): the PC-drawn flag, and the brightest LED's hue while
//! the LEDs are lit. The cases:
//! - Frame: a whole red frame shows; the PC owns the lights (owner 3); the right draws it.
//! - Lost last chunk: a frame whose SHOW report never came, then a whole blue one: the blue shows.
//! - Frame stream: frames one in flight, every reply ok and owned; the round trips.
//! - Frame release: owner back to the keyboard (0), the LEDs no longer the PC's.
//! - Watchdog (`lights --watchdog`): no frame for the keyboard's timeout, then its D7 8C event.
//! - Effect: a still red breathe runs (owner 3, kind 4); a bad one is refused (status 5) and the
//!   running one goes on; a frame takes the lights from it; release hands them back.
//!
//! Out of scope: which key each LED sits under (needs eyes: #90 row G, the LampArray sitting), the
//! app's frame rate cap (app tests), typing while frames stream (the resilience suite's typing
//! cases), and the right's 10 fps coalescing (the zmk-sim host-frames case).
//!
//! A HID LampArray host (Windows Dynamic Lighting, owner 1 over USB or 2 over Bluetooth) wins
//! over the host link (host_link.h, D7 0C): while it has the lights, SHOW and EFFECT answer 1
//! (busy). That isn't a fault of the cases: they are BLOCKED, "host holds lights (Windows Dynamic
//! Lighting)" (#753). The owner is read first with a report that changes nothing
//! ([`settings::frame_owner`]); a busy reply mid-run blocks the rest the same way.
//!
//! Every case is bounded: each request waits at most the link's 500 ms, the stream sends a fixed
//! count, the watchdog waits the reply's timeout plus 5 s. Both releases run at the end whatever
//! happened (a drop guard); a lab killed mid-run leaves the lights to the keyboard's timeout.

use crate::health::{row_line, Outcome, BLOCKED, FAIL, PASS};
use foxglove_link::health::{HealthRow, Level};
use foxglove_link::lamps::LampParts;
use foxglove_link::lights_owner::Owner;
use foxglove_link::protocol::{
    self, capabilities, Effect, EffectKind, EffectReply, FrameReply, FrameRight, REPORT,
};
use foxglove_link::{settings, Transport};
use std::time::{Duration, Instant};

/// The rows' group.
pub const GROUP: &str = "lights";

const RED: [u8; 3] = [200, 0, 0];
const GREEN: [u8; 3] = [0, 200, 0];
const BLUE: [u8; 3] = [0, 0, 200];
/// A round trip slower than this in the stream is a warning (the link's own timeout is 500 ms).
const SLOW_MS: u128 = 400;

/// Which cases run, and how long the LEDs get to draw before a read-back.
#[derive(Clone, Copy, Debug)]
pub struct Options {
    /// Frames sent one in flight in the stream case; 0 skips it.
    pub stream: u16,
    /// Wait out the keyboard's frame timeout (30 s on the shipped firmware) for its hand-back.
    pub watchdog: bool,
    /// Before each read-back: the left draws on ZMK's low-priority queue.
    pub settle: Duration,
}

impl Options {
    /// `foxglove-lab health`: everything but the watchdog's wait.
    pub const QUICK: Options = Options {
        stream: 30,
        watchdog: false,
        settle: Duration::from_millis(150),
    };
}

/// The keyboard's own reports while a case waits: the next one within the timeout, `None` when
/// none came (`foxglove-lab lights` feeds it from `Link::subscribe_reports`).
pub type Events<'a> = &'a dyn Fn(Duration) -> Option<[u8; REPORT]>;

/// What the cases found: their rows, and what couldn't be tested and why.
#[derive(Clone, Debug, Default)]
pub struct Cases {
    pub rows: Vec<HealthRow>,
    pub blocked: Vec<String>,
}

impl Cases {
    /// One reason for the whole set (no keyboard, the app holds it).
    pub fn blocked_by(why: impl Into<String>) -> Cases {
        Cases {
            rows: Vec::new(),
            blocked: vec![why.into()],
        }
    }

    /// Exit code: 1 when a case failed, else 2 when something couldn't be tested, else 0.
    pub fn code(&self) -> u8 {
        if foxglove_link::health::worst(&self.rows) == Level::Fail {
            FAIL
        } else if !self.blocked.is_empty() {
            BLOCKED
        } else {
            PASS
        }
    }

    /// One line per row as `health` prints them, one per untested set, then the result.
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
            BLOCKED => format!("BLOCKED: {}", self.blocked.join("; ")),
            _ => format!("PASS: nothing failed, {warns} warning(s)"),
        });
        out
    }

    /// As JSON, in `health --json`'s shape, with `blocked` a list.
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
        v
    }

    /// Into `health`'s result: the rows as they are, each untested set as a warning, so a release
    /// build without SET_EFFECT (dev builds only) still passes the post-flash check. Failures
    /// fail it.
    pub fn fold_into(self, out: &mut Outcome) {
        out.rows.extend(self.rows);
        out.rows.extend(
            self.blocked
                .into_iter()
                .map(|why| HealthRow::new(GROUP, "Not tested", Level::Warn, why)),
        );
    }
}

/// Why nothing about the PC's lights can be tested: a LampArray host holds them.
pub const HOST_HOLDS: &str = "host holds lights (Windows Dynamic Lighting)";

/// The blocked reason when a LampArray host has the lights.
fn held_reason(what: &str, owner: u8) -> String {
    let via = match Owner::from_byte(owner) {
        Owner::LampArrayUsb => "its LampArray over USB",
        Owner::LampArrayBle => "its LampArray over Bluetooth",
        _ => "a LampArray",
    };
    format!("{what} not tested: {HOST_HOLDS}; owner {owner}, {via}")
}

/// `Some(reason)` when a LampArray host (Windows Dynamic Lighting) has the keyboard's lights
/// now, read with a report that changes nothing; `None` when it doesn't, or the keyboard has no
/// SET_FRAME (HELLO bit 14) or didn't answer. For cases that need the keyboard's own lights
/// (its idle, on/off and colours in RGB_STATE): they can't show while Windows draws.
pub fn lamp_array_holds(t: &impl Transport, caps: u32) -> Option<String> {
    if caps & capabilities::FRAMES == 0 {
        return None;
    }
    settings::frame_owner(t)
        .ok()
        .filter(|o| o.lamp_array())
        .map(|o| {
            format!(
                "{HOST_HOLDS}: the keyboard's own lights don't show (owner {})",
                o.byte()
            )
        })
}

/// Runs the cases on `t`; `caps` are HELLO's. Events are needed only for the watchdog.
pub fn run(t: &impl Transport, caps: u32, events: Option<Events<'_>>, opts: &Options) -> Cases {
    let mut r = Run {
        t,
        caps,
        opts: *opts,
        cases: Cases::default(),
        held: false,
    };
    let frames = caps & capabilities::FRAMES != 0;
    let effects = caps & capabilities::EFFECTS != 0;
    if !frames {
        r.cases
            .blocked
            .push("SET_FRAME not tested: no HELLO bit 14 (FRAMES)".into());
    }
    if !effects {
        r.cases
            .blocked
            .push("SET_EFFECT not tested: no HELLO bit 19 (EFFECTS, dev builds only)".into());
    }
    if !frames && !effects {
        return r.cases;
    }
    // Windows Dynamic Lighting has the lights: every case would be refused busy.
    if frames {
        if let Ok(o) = settings::frame_owner(t) {
            if o.lamp_array() {
                r.cases
                    .blocked
                    .push(held_reason("SET_FRAME and SET_EFFECT", o.byte()));
                return r.cases;
            }
        }
    }
    let _release = Release { t, frames, effects };
    if caps & capabilities::RGB_SHOWN == 0 {
        r.row(
            "Read-back",
            Level::Warn,
            "no LED read-back on this firmware (no HELLO bit 24): replies only",
        );
    }
    if frames {
        r.frames(events);
    }
    if effects {
        r.effects(frames);
    }
    r.cases
}

/// Both releases, whatever the cases did: the user's own lights come back.
struct Release<'a, T: Transport> {
    t: &'a T,
    frames: bool,
    effects: bool,
}

impl<T: Transport> Drop for Release<'_, T> {
    fn drop(&mut self) {
        if self.effects {
            let _ = settings::release_effect(self.t);
        }
        if self.frames {
            let _ = settings::release_frame(self.t);
        }
    }
}

struct Run<'a, T: Transport> {
    t: &'a T,
    caps: u32,
    opts: Options,
    cases: Cases,
    /// A busy reply said a LampArray host took the lights mid-run: the rest is blocked.
    held: bool,
}

fn status_text(s: u8) -> &'static str {
    match s {
        0 => "ok",
        1 => "busy: a LampArray host has the lights",
        4 => "not built in",
        5 => "bad request",
        _ => "unknown",
    }
}

fn right_text(r: FrameRight) -> &'static str {
    match r {
        FrameRight::Draws => "the right draws it",
        FrameRight::NoLink => "no link to the right half",
        FrameRight::Unsupported => "the right's firmware can't draw it",
    }
}

/// Within 8 degrees, round the wheel.
fn hue_near(h: u16, want: u16) -> bool {
    let d = (i32::from(h) - i32::from(want)).rem_euclid(360);
    d <= 8 || d >= 352
}

fn breathe(c: [u8; 3]) -> Effect {
    Effect {
        kind: EffectKind::Breathe,
        colours: vec![c, c],
        clock_ms: 0,
        period_ms: 0,
        angle_deg: 0.0,
        width_mm: 0.0,
        origin_mm: [0, 0],
    }
}

impl<T: Transport> Run<'_, T> {
    fn row(&mut self, item: &str, level: Level, detail: impl Into<String>) {
        self.cases
            .rows
            .push(HealthRow::new(GROUP, item, level, detail));
    }

    /// The board's LEDs, as it says (LAMP_PARTS, HELLO bit 22), else the Corne's 42.
    fn lamps(&self) -> usize {
        if self.caps & capabilities::LAMP_PARTS != 0 {
            if let Ok(p) = settings::lamp_parts(self.t) {
                return p.total();
            }
        }
        LampParts::old_firmware().total()
    }

    /// A LampArray host took the lights: blocked, once, from `what` on.
    fn hold(&mut self, what: &str, owner: u8) {
        if !self.held {
            self.held = true;
            self.cases.blocked.push(held_reason(what, owner));
        }
    }

    /// Whether a LampArray host has the lights now (read without changing anything): if so,
    /// blocked from `item` on. For a result a LampArray taking over would also explain.
    fn windows_took(&mut self, item: &str) -> bool {
        match settings::frame_owner(self.t) {
            Ok(o) if o.lamp_array() && self.caps & capabilities::FRAMES != 0 => {
                self.hold(&format!("{item:?} on"), o.byte());
                true
            }
            _ => false,
        }
    }

    /// A frame's reply as a row: ok, the PC's, and the right drawing it. Whether the PC holds
    /// the lights.
    fn frame_row(&mut self, item: &str, reply: anyhow::Result<FrameReply>, what: &str) -> bool {
        match reply {
            Err(e) => {
                self.row(item, Level::Fail, format!("no reply to SET_FRAME: {e:#}"));
                false
            }
            Ok(r) if r.busy() => {
                self.hold(&format!("SET_FRAME from {item:?} on"), r.owner);
                false
            }
            Ok(r) if r.status != 0 => {
                self.row(
                    item,
                    Level::Fail,
                    format!("SET_FRAME status {} ({})", r.status, status_text(r.status)),
                );
                false
            }
            Ok(r) if !r.held() => {
                self.row(
                    item,
                    Level::Fail,
                    format!("SET_FRAME ok but owner {} (want 3, the PC's)", r.owner),
                );
                false
            }
            Ok(r) => {
                let level = match r.right {
                    FrameRight::Draws => Level::Ok,
                    FrameRight::NoLink => Level::Warn,
                    FrameRight::Unsupported => Level::Fail,
                };
                self.row(
                    item,
                    level,
                    format!("{what}; owner 3 (the PC's), {}", right_text(r.right)),
                );
                true
            }
        }
    }

    /// The left's LEDs as RGB_STATE reports them: the PC's or not, and the colour while lit.
    fn read_back(&mut self, item: &str, want_host: bool, want: Option<(u16, &str)>) {
        if self.held || self.caps & capabilities::RGB_SHOWN == 0 {
            return;
        }
        std::thread::sleep(self.opts.settle);
        let left = match settings::rgb_output(self.t) {
            Ok((_, out)) => out.left,
            Err(e) => return self.row(item, Level::Fail, format!("RGB_STATE: {e:#}")),
        };
        let Some(left) = left else {
            return self.row(
                item,
                Level::Fail,
                "RGB_STATE says nothing about the left's LEDs",
            );
        };
        // Windows took the lights after the hand-back: the LEDs are its, not the PC's.
        if left.host && !want_host && self.windows_took(item) {
            return;
        }
        if left.host != want_host {
            let detail = if want_host {
                "the left's LEDs aren't the PC's (RGB_STATE host flag off)"
            } else {
                "the left's LEDs are still the PC's (RGB_STATE host flag on)"
            };
            return self.row(item, Level::Fail, detail);
        }
        let Some((hue, name)) = want else {
            return self.row(item, Level::Ok, "the left's LEDs are the keyboard's own");
        };
        match (left.lit, left.hue) {
            (true, Some((h, s))) if hue_near(h, hue) && s >= 90 => {
                self.row(item, Level::Ok, format!("the left shows {name} (hue {h})"))
            }
            (true, Some((h, s))) => self.row(
                item,
                Level::Fail,
                format!("the left shows hue {h} saturation {s}, not {name} (hue {hue})"),
            ),
            (true, None) => self.row(
                item,
                Level::Warn,
                format!("the PC's and lit, but no colour reported: {name} not confirmed"),
            ),
            (false, _) => self.row(
                item,
                Level::Ok,
                format!("the PC's; dark (idle or unpowered), so {name} not confirmed"),
            ),
        }
    }

    fn frames(&mut self, events: Option<Events<'_>>) {
        let n = self.lamps();
        let reply = settings::set_frame(self.t, 0, &vec![RED; n]);
        let timeout_s = reply.as_ref().map_or(0, |r| r.timeout_s);
        let what = format!("{n} LEDs red, the keyboard's timeout {timeout_s} s");
        if self.frame_row("Frame", reply, &what) {
            self.read_back("Frame read-back", true, Some((0, "red")));
            self.lost_chunk(n);
            self.stream(n);
        }
        if self.held {
            return;
        }
        self.frame_release("Frame release");
        if self.opts.watchdog {
            match events {
                Some(events) => self.watchdog(n, events),
                None => self
                    .cases
                    .blocked
                    .push("frame watchdog not tested: no event feed".into()),
            }
        }
    }

    /// A frame whose last report (SHOW) was lost, then a whole one: the whole one shows.
    fn lost_chunk(&mut self, n: usize) {
        if self.held {
            return;
        }
        let green = protocol::frame(0, &vec![GREEN; n]);
        let Some((_, chunks)) = green.split_last().filter(|(_, c)| !c.is_empty()) else {
            return self.row(
                "Lost last chunk",
                Level::Ok,
                "one report per frame on this board: nothing to lose",
            );
        };
        for c in chunks {
            if let Err(e) = self.t.send(c) {
                return self.row("Lost last chunk", Level::Fail, format!("send: {e:#}"));
            }
        }
        let what = format!(
            "{} of a green frame's {} reports, no SHOW; then a whole blue frame",
            chunks.len(),
            green.len()
        );
        let reply = settings::set_frame(self.t, 0, &vec![BLUE; n]);
        if self.frame_row("Lost last chunk", reply, &what) {
            self.read_back("Lost last chunk read-back", true, Some((240, "blue")));
        }
    }

    /// Frames one in flight, as the app's scenes send them: every reply ok and the PC's.
    fn stream(&mut self, n: usize) {
        let count = self.opts.stream;
        if count == 0 || self.held {
            return;
        }
        let red = vec![RED; n];
        let blue = vec![BLUE; n];
        let mut times = Vec::with_capacity(usize::from(count));
        for i in 0..count {
            let start = Instant::now();
            let frame = if i % 2 == 0 { &blue } else { &red };
            let bad = match settings::set_frame(self.t, 0, frame) {
                Err(e) => Some(format!("no reply: {e:#}")),
                Ok(r) if r.busy() => {
                    return self.hold("SET_FRAME from \"Frame stream\" on", r.owner)
                }
                Ok(r) if r.status != 0 => {
                    Some(format!("status {} ({})", r.status, status_text(r.status)))
                }
                Ok(r) if !r.held() => Some(format!("owner {} (want 3)", r.owner)),
                Ok(_) => None,
            };
            if let Some(bad) = bad {
                return self.row(
                    "Frame stream",
                    Level::Fail,
                    format!("frame {} of {count}: {bad}", i + 1),
                );
            }
            times.push(start.elapsed().as_millis());
        }
        let max = times.iter().copied().max().unwrap_or(0);
        let mean = times.iter().sum::<u128>() / times.len().max(1) as u128;
        let level = if max > SLOW_MS {
            Level::Warn
        } else {
            Level::Ok
        };
        self.row(
            "Frame stream",
            level,
            format!("{count} frames one in flight, all the PC's: mean {mean} ms, max {max} ms"),
        );
    }

    /// RELEASE: owner back to the keyboard, the LEDs its own.
    fn frame_release(&mut self, item: &str) {
        if self.held {
            return;
        }
        match settings::release_frame(self.t) {
            Err(e) => self.row(item, Level::Fail, format!("no reply to RELEASE: {e:#}")),
            Ok(r) if r.status != 0 => self.row(
                item,
                Level::Fail,
                format!("RELEASE status {} ({})", r.status, status_text(r.status)),
            ),
            Ok(r) if r.held() => self.row(
                item,
                Level::Fail,
                "RELEASE ok but the PC still owns the lights (owner 3)",
            ),
            Ok(r) if Owner::from_byte(r.owner).lamp_array() => {
                self.hold(&format!("{item:?} on"), r.owner)
            }
            Ok(r) => {
                self.row(
                    item,
                    Level::Ok,
                    format!("owner {} (the keyboard's) after RELEASE", r.owner),
                );
                self.read_back(&format!("{item} read-back"), false, None);
            }
        }
    }

    /// One frame, then nothing: the keyboard takes its lights back after its timeout and says so
    /// (D7 8C, event 1). Waits the timeout plus 5 s at most.
    fn watchdog(&mut self, n: usize, events: Events<'_>) {
        const ITEM: &str = "Watchdog";
        // What came before the case (bounded: the keyboard doesn't stream on its own).
        for _ in 0..256 {
            if events(Duration::ZERO).is_none() {
                break;
            }
        }
        let reply = settings::set_frame(self.t, 0, &vec![RED; n]);
        let timeout_s = match reply {
            Ok(r) if r.status == 0 && r.held() => r.timeout_s,
            other => {
                self.frame_row(ITEM, other, "the watchdog's frame");
                return;
            }
        };
        if timeout_s == 0 {
            return self.row(
                ITEM,
                Level::Warn,
                "timeout 0: this keyboard never takes its lights back on its own",
            );
        }
        let start = Instant::now();
        let deadline = start + Duration::from_secs(u64::from(timeout_s) + 5);
        let event = loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match events(left) {
                Some(r) if protocol::is_frame_event(&r) => break Some(FrameReply::parse(&r)),
                Some(_) if !left.is_zero() => continue,
                _ => break None,
            }
        };
        let after = start.elapsed();
        let Some(event) = event else {
            return self.row(
                ITEM,
                Level::Fail,
                format!(
                    "no hand-back event within {} s (timeout {timeout_s} s)",
                    u64::from(timeout_s) + 5
                ),
            );
        };
        let early = Duration::from_secs(u64::from(timeout_s.saturating_sub(1)));
        if after < early {
            return self.row(
                ITEM,
                Level::Fail,
                format!(
                    "handed back after {:.1} s, before its {timeout_s} s timeout",
                    after.as_secs_f32()
                ),
            );
        }
        if event.held() {
            return self.row(
                ITEM,
                Level::Fail,
                "the hand-back event still says owner 3 (the PC's)",
            );
        }
        self.row(
            ITEM,
            Level::Ok,
            format!(
                "handed back after {:.1} s (timeout {timeout_s} s), D7 8C event, owner {}",
                after.as_secs_f32(),
                event.owner
            ),
        );
        self.read_back("Watchdog read-back", false, None);
    }

    /// An effect's reply as a row: ok, the PC's, the kind sent, the right drawing it.
    fn effect_row(&mut self, item: &str, reply: anyhow::Result<EffectReply>, what: &str) -> bool {
        match reply {
            Err(e) => {
                self.row(item, Level::Fail, format!("no reply to SET_EFFECT: {e:#}"));
                false
            }
            Ok(r) if r.busy() => {
                self.hold(&format!("SET_EFFECT from {item:?} on"), r.owner);
                false
            }
            Ok(r) if r.status != 0 => {
                self.row(
                    item,
                    Level::Fail,
                    format!("SET_EFFECT status {} ({})", r.status, status_text(r.status)),
                );
                false
            }
            Ok(r) if !r.running() => {
                self.row(
                    item,
                    Level::Fail,
                    format!("SET_EFFECT ok but owner {} (want 3, the PC's)", r.owner),
                );
                false
            }
            Ok(r) if r.kind != EffectKind::Breathe as u8 => {
                self.row(
                    item,
                    Level::Fail,
                    format!("SET_EFFECT runs kind {} (want 4, breathe)", r.kind),
                );
                false
            }
            Ok(r) => {
                let level = match r.right {
                    FrameRight::Draws => Level::Ok,
                    FrameRight::NoLink => Level::Warn,
                    FrameRight::Unsupported => Level::Fail,
                };
                self.row(
                    item,
                    level,
                    format!("{what}; owner 3, kind 4, {}", right_text(r.right)),
                );
                true
            }
        }
    }

    fn effects(&mut self, frames: bool) {
        if self.held {
            return;
        }
        let reply = settings::set_effect(self.t, &breathe(RED));
        if self.effect_row("Effect", reply, "a still red breathe") {
            self.read_back("Effect read-back", true, Some((0, "red")));
            self.bad_effect();
            if frames {
                self.frame_over_effect();
            }
        }
        if self.held {
            return;
        }
        // Released whether it started or not; a second one so the release has one to end.
        let reply = settings::set_effect(self.t, &breathe(BLUE));
        if self.effect_row("Effect again", reply, "a still blue breathe") {
            self.read_back("Effect again read-back", true, Some((240, "blue")));
        }
        if self.held {
            return;
        }
        match settings::release_effect(self.t) {
            Err(e) => self.row(
                "Effect release",
                Level::Fail,
                format!("no reply to RELEASE: {e:#}"),
            ),
            Ok(r) if r.status != 0 => self.row(
                "Effect release",
                Level::Fail,
                format!("RELEASE status {} ({})", r.status, status_text(r.status)),
            ),
            Ok(r) if r.running() || r.kind != 0 => self.row(
                "Effect release",
                Level::Fail,
                format!(
                    "RELEASE ok but the effect runs on (owner {}, kind {})",
                    r.owner, r.kind
                ),
            ),
            Ok(_) => {
                self.row(
                    "Effect release",
                    Level::Ok,
                    "owner 0 (the keyboard's), no effect after RELEASE",
                );
                self.read_back("Effect release read-back", false, None);
            }
        }
    }

    /// An effect the keyboard can't draw (kind 0): refused with status 5, the running one on.
    fn bad_effect(&mut self) {
        const ITEM: &str = "Bad effect refused";
        if self.held {
            return;
        }
        let Some(mut report) = protocol::effect(&breathe(GREEN)) else {
            return self.row(ITEM, Level::Fail, "the probe doesn't fit the wire");
        };
        report[3] = 0;
        let reply = self
            .t
            .request(
                &report,
                protocol::SETTINGS,
                protocol::settings::EFFECT,
                Duration::from_millis(500),
            )
            .map(|r| EffectReply::parse(&r));
        match reply {
            Err(e) => self.row(ITEM, Level::Fail, format!("no reply: {e:#}")),
            Ok(r) if r.busy() => self.hold(&format!("{ITEM:?} on"), r.owner),
            Ok(r) if r.status != 5 => self.row(
                ITEM,
                Level::Fail,
                format!(
                    "an effect of kind 0 got status {} ({}), want 5",
                    r.status,
                    status_text(r.status)
                ),
            ),
            Ok(r) if !r.running() && self.windows_took(ITEM) => {}
            Ok(r) if !r.running() || r.kind != EffectKind::Breathe as u8 => self.row(
                ITEM,
                Level::Fail,
                format!(
                    "refused, but the running effect stopped (owner {}, kind {})",
                    r.owner, r.kind
                ),
            ),
            Ok(_) => self.row(
                ITEM,
                Level::Ok,
                "kind 0 refused with status 5; the breathe runs on",
            ),
        }
    }

    /// A frame takes the lights from a running effect: the effect stops, quietly.
    fn frame_over_effect(&mut self) {
        const ITEM: &str = "Frame over effect";
        if self.held {
            return;
        }
        let n = self.lamps();
        let reply = settings::set_frame(self.t, 0, &vec![BLUE; n]);
        if !self.frame_row(ITEM, reply, "a blue frame over the running breathe") {
            return;
        }
        self.read_back("Frame over effect read-back", true, Some((240, "blue")));
        // The effect's state, read through a RELEASE (the reply says whether one still runs).
        match settings::release_effect(self.t) {
            Err(e) => self.row(ITEM, Level::Fail, format!("SET_EFFECT RELEASE: {e:#}")),
            Ok(r) if r.running() => self.row(
                ITEM,
                Level::Fail,
                format!("the effect still runs under the frame (kind {})", r.kind),
            ),
            Ok(_) => {}
        }
        self.frame_release("Frame over effect release");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// The keyboard's side of SET_FRAME, SET_EFFECT and RGB_STATE, as host_frames.c and
    /// host_effects.c behave, with knobs for the broken ones.
    struct Kb {
        staging: Vec<[u8; 3]>,
        frames_owned: bool,
        /// Kind and first colour.
        effect: Option<(u8, [u8; 3])>,
        shown: [u8; 3],
        lit: bool,
        timeout_s: u8,
        right: u8,
        /// A RELEASE that answers ok and changes nothing.
        release_keeps: bool,
        /// The LEDs show a hue this far off.
        hue_off: u16,
        /// SET_FRAME answers this status.
        frame_status: u8,
        /// A HID LampArray's owner byte (1 USB, 2 Bluetooth) once it has the lights; 0 none.
        lamp_array: u8,
        /// The LampArray takes the lights at this request (counted from 1); `None`: from the
        /// start, if `lamp_array` is set.
        takes_at: Option<usize>,
        requests: usize,
    }

    impl Default for Kb {
        fn default() -> Self {
            Kb {
                staging: vec![[0; 3]; 256],
                frames_owned: false,
                effect: None,
                shown: [0; 3],
                lit: true,
                timeout_s: 30,
                right: 1,
                release_keeps: false,
                hue_off: 0,
                frame_status: 0,
                lamp_array: 0,
                takes_at: None,
                requests: 0,
            }
        }
    }

    #[derive(Default)]
    struct Fake {
        kb: RefCell<Kb>,
        sent: RefCell<usize>,
    }

    fn hue_of(c: [u8; 3]) -> u16 {
        match c {
            [_, 0, 0] => 0,
            [0, _, 0] => 120,
            _ => 240,
        }
    }

    fn reply(bytes: &[u8]) -> [u8; REPORT] {
        let mut r = [0; REPORT];
        r[..bytes.len()].copy_from_slice(bytes);
        r
    }

    impl Fake {
        fn with(kb: Kb) -> Fake {
            Fake {
                kb: RefCell::new(kb),
                sent: RefCell::new(0),
            }
        }

        fn stage(kb: &mut Kb, r: &[u8]) {
            let (start, n) = (usize::from(r[3]), usize::from(r[4]));
            for i in 0..n {
                kb.staging[start + i] = [r[5 + 3 * i], r[6 + 3 * i], r[7 + 3 * i]];
            }
        }

        /// D7 8C's owner: the frames'; D7 8F's: the effect's (each module answers for itself).
        fn owner(held: bool) -> u8 {
            if held {
                3
            } else {
                0
            }
        }
    }

    impl Transport for Fake {
        fn send(&self, r: &[u8]) -> anyhow::Result<()> {
            *self.sent.borrow_mut() += 1;
            assert_eq!(
                &r[..2],
                &[0xd7, 0x0c],
                "only frame chunks go without a reply"
            );
            assert_eq!(r[2] & protocol::frame::SHOW, 0);
            Fake::stage(&mut self.kb.borrow_mut(), r);
            Ok(())
        }

        fn request(
            &self,
            r: &[u8],
            proto: u8,
            cmd: u8,
            _: Duration,
        ) -> anyhow::Result<[u8; REPORT]> {
            *self.sent.borrow_mut() += 1;
            assert_eq!(proto, 0xd7);
            let mut kb = self.kb.borrow_mut();
            kb.requests += 1;
            // host_frames.c: a LampArray wins; frames and an effect stop, SHOW and EFFECT are
            // refused busy with its owner, RELEASE answers ok and leaves it the lights.
            let windows = kb.lamp_array != 0 && kb.takes_at.is_none_or(|n| kb.requests >= n);
            if windows {
                kb.frames_owned = false;
                kb.effect = None;
            }
            let lamp_owner = if windows { kb.lamp_array } else { 0 };
            let release = r.get(2).is_some_and(|f| f & protocol::frame::RELEASE != 0);
            match cmd {
                0x0c if !release
                    && (r.len() < 5 || r[4] > 9 || r.len() < 5 + 3 * usize::from(r[4])) =>
                {
                    let owner = lamp_owner.max(Fake::owner(kb.frames_owned));
                    Ok(reply(&[0xd7, 0x8c, 5, owner, kb.timeout_s, kb.right, 0]))
                }
                0x0c if windows && !release => Ok(reply(&[
                    0xd7,
                    0x8c,
                    1,
                    lamp_owner,
                    kb.timeout_s,
                    kb.right,
                    0,
                ])),
                // Only an effect it could draw is refused busy; a bad one is still bad (5).
                0x0f if windows && r[2] == 2 && (1..=4).contains(&r[3]) => {
                    Ok(reply(&[0xd7, 0x8f, 1, lamp_owner, 0, kb.right, 0]))
                }
                0x0c => {
                    if r[2] & protocol::frame::RELEASE != 0 {
                        if !kb.release_keeps {
                            kb.frames_owned = false;
                        }
                    } else if kb.frame_status == 0 {
                        Fake::stage(&mut kb, r);
                        kb.effect = None;
                        kb.frames_owned = true;
                        kb.shown = kb.staging[0];
                    }
                    let status = if r[2] & protocol::frame::RELEASE != 0 {
                        0
                    } else {
                        kb.frame_status
                    };
                    Ok(reply(&[
                        0xd7,
                        0x8c,
                        status,
                        lamp_owner.max(Fake::owner(kb.frames_owned)),
                        kb.timeout_s,
                        kb.right,
                        0,
                    ]))
                }
                0x0f => {
                    let mut status = 0;
                    match r[2] {
                        0 => {
                            if !kb.release_keeps {
                                kb.effect = None;
                            }
                        }
                        2 if (1..=4).contains(&r[3]) => {
                            kb.frames_owned = false;
                            let c = [r[19], r[20], r[21]];
                            kb.effect = Some((r[3], c));
                            kb.shown = c;
                        }
                        _ => status = 5,
                    }
                    let kind = kb.effect.map_or(0, |(k, _)| k);
                    Ok(reply(&[
                        0xd7,
                        0x8f,
                        status,
                        Fake::owner(kb.effect.is_some()),
                        kind,
                        kb.right,
                        0,
                    ]))
                }
                0x03 => {
                    let host = kb.frames_owned || kb.effect.is_some() || windows;
                    let mut out = reply(&[0xd7, 0x83]);
                    out[9] = 0xff; // no right half heard from
                    let drawn = kb.lit;
                    out[17] = 1
                        | (u8::from(kb.lit) << 1)
                        | (u8::from(host) << 3)
                        | (u8::from(drawn) << 6);
                    let hue = if host {
                        (hue_of(kb.shown) + kb.hue_off) % 360
                    } else {
                        30
                    };
                    out[18..20].copy_from_slice(&hue.to_le_bytes());
                    out[20] = 100;
                    out[21] = 78;
                    out[22] = 100;
                    Ok(out)
                }
                other => panic!("unexpected request D7 {other:02x}"),
            }
        }
    }

    const ALL: u32 = capabilities::FRAMES | capabilities::EFFECTS | capabilities::RGB_SHOWN;
    const FAST: Options = Options {
        stream: 6,
        watchdog: false,
        settle: Duration::ZERO,
    };

    fn released(f: &Fake) -> bool {
        let kb = f.kb.borrow();
        !kb.frames_owned && kb.effect.is_none()
    }

    #[test]
    fn a_healthy_keyboard_passes_and_gets_its_lights_back() {
        let f = Fake::default();
        let out = run(&f, ALL, None, &FAST);
        let lines = out.lines();
        assert_eq!(out.code(), PASS, "{lines:#?}");
        assert!(out.rows.iter().all(|r| r.level == Level::Ok), "{lines:#?}");
        for want in [
            "PASS lights     Frame: 42 LEDs red, the keyboard's timeout 30 s; owner 3 (the PC's), the right draws it",
            "PASS lights     Frame read-back: the left shows red (hue 0)",
            "PASS lights     Lost last chunk: 4 of a green frame's 5 reports, no SHOW; then a whole blue frame; owner 3 (the PC's), the right draws it",
            "PASS lights     Lost last chunk read-back: the left shows blue (hue 240)",
            "PASS lights     Frame release: owner 0 (the keyboard's) after RELEASE",
            "PASS lights     Frame release read-back: the left's LEDs are the keyboard's own",
            "PASS lights     Effect: a still red breathe; owner 3, kind 4, the right draws it",
            "PASS lights     Bad effect refused: kind 0 refused with status 5; the breathe runs on",
            "PASS lights     Frame over effect: a blue frame over the running breathe; owner 3 (the PC's), the right draws it",
            "PASS lights     Effect release: owner 0 (the keyboard's), no effect after RELEASE",
        ] {
            assert!(lines.iter().any(|l| l == want), "missing {want:?} in {lines:#?}");
        }
        assert!(lines
            .iter()
            .any(|l| l.starts_with("PASS lights     Frame stream: 6 frames one in flight")));
        assert_eq!(lines.last().unwrap(), "PASS: nothing failed, 0 warning(s)");
        assert!(released(&f));
        assert_eq!(out.json()["result"], "pass");
    }

    #[test]
    fn a_release_that_keeps_the_lights_fails() {
        let f = Fake::with(Kb {
            release_keeps: true,
            ..Kb::default()
        });
        let out = run(&f, ALL, None, &FAST);
        assert_eq!(out.code(), FAIL);
        let lines = out.lines();
        assert!(lines.iter().any(|l| l
            == "FAIL lights     Frame release: RELEASE ok but the PC still owns the lights (owner 3)"),
            "{lines:#?}");
        assert!(lines
            .iter()
            .any(|l| l
                .starts_with("FAIL lights     Effect release: RELEASE ok but the effect runs on")));
        assert!(lines.last().unwrap().starts_with("FAIL: "));
    }

    #[test]
    fn a_wrong_colour_or_a_refused_frame_fails() {
        let f = Fake::with(Kb {
            hue_off: 120,
            ..Kb::default()
        });
        let out = run(&f, ALL, None, &FAST);
        assert_eq!(out.code(), FAIL);
        assert!(out.lines().iter().any(|l| l
            == "FAIL lights     Frame read-back: the left shows hue 120 saturation 100, not red (hue 0)"));

        let f = Fake::with(Kb {
            frame_status: 5,
            ..Kb::default()
        });
        let out = run(&f, ALL, None, &FAST);
        assert_eq!(out.code(), FAIL);
        assert!(out
            .lines()
            .iter()
            .any(|l| l == "FAIL lights     Frame: SET_FRAME status 5 (bad request)"));
        assert!(released(&f));
    }

    #[test]
    fn the_right_half_decides_warn_or_fail() {
        let f = Fake::with(Kb {
            right: 0,
            ..Kb::default()
        });
        let out = run(&f, ALL, None, &FAST);
        assert_eq!(out.code(), PASS, "{:#?}", out.lines());
        assert!(out
            .lines()
            .iter()
            .any(|l| l.starts_with("WARN lights     Frame: ")
                && l.ends_with("no link to the right half")));
        let f = Fake::with(Kb {
            right: 2,
            ..Kb::default()
        });
        assert_eq!(run(&f, ALL, None, &FAST).code(), FAIL);
    }

    #[test]
    fn a_dark_keyboard_passes_without_the_colour() {
        let f = Fake::with(Kb {
            lit: false,
            ..Kb::default()
        });
        let out = run(&f, ALL, None, &FAST);
        assert_eq!(out.code(), PASS, "{:#?}", out.lines());
        assert!(out.lines().iter().any(|l| l
            == "PASS lights     Frame read-back: the PC's; dark (idle or unpowered), so red not confirmed"));
    }

    #[test]
    fn a_missing_capability_blocks() {
        let f = Fake::default();
        let out = run(&f, 0, None, &FAST);
        assert_eq!(out.code(), BLOCKED);
        assert!(out.rows.is_empty());
        assert_eq!(*f.sent.borrow(), 0, "nothing sent without the bits");
        assert!(out
            .lines()
            .last()
            .unwrap()
            .starts_with("BLOCKED: SET_FRAME not tested: no HELLO bit 14"));

        // Frames without effects (a release build): the frames run and pass, effects block.
        let out = run(
            &f,
            capabilities::FRAMES | capabilities::RGB_SHOWN,
            None,
            &FAST,
        );
        assert_eq!(out.code(), BLOCKED, "{:#?}", out.lines());
        assert!(out.rows.iter().all(|r| r.level == Level::Ok));
        assert!(out.lines().iter().any(|l| {
            l
            == "BLOCK lights     SET_EFFECT not tested: no HELLO bit 19 (EFFECTS, dev builds only)"
        }));
        assert_eq!(out.json()["result"], "blocked");

        // Something that failed outranks something untested.
        let f = Fake::with(Kb {
            release_keeps: true,
            ..Kb::default()
        });
        assert_eq!(run(&f, capabilities::FRAMES, None, &FAST).code(), FAIL);
    }

    #[test]
    fn without_read_back_the_replies_still_decide() {
        let f = Fake::default();
        let out = run(
            &f,
            capabilities::FRAMES | capabilities::EFFECTS,
            None,
            &FAST,
        );
        assert_eq!(out.code(), PASS, "{:#?}", out.lines());
        assert!(out.lines().iter().any(|l| l
            == "WARN lights     Read-back: no LED read-back on this firmware (no HELLO bit 24): replies only"));
        assert!(!out.lines().iter().any(|l| l.contains("read-back:")));
    }

    #[test]
    fn the_watchdog_needs_its_hand_back_event() {
        let opts = Options {
            watchdog: true,
            ..FAST
        };
        let f = Fake::with(Kb {
            timeout_s: 1,
            ..Kb::default()
        });
        // The keyboard's timeout fires: it hands back and says so.
        let events = |_: Duration| -> Option<[u8; REPORT]> {
            let mut kb = f.kb.borrow_mut();
            if !kb.frames_owned {
                return None; // nothing before the case's frame
            }
            kb.frames_owned = false;
            Some(reply(&[0xd7, 0x8c, 0, 0, 1, 1, 1]))
        };
        let out = run(&f, ALL, Some(&events), &opts);
        assert_eq!(out.code(), PASS, "{:#?}", out.lines());
        assert!(out.lines().iter().any(|l| l
            .starts_with("PASS lights     Watchdog: handed back after ")
            && l.ends_with("(timeout 1 s), D7 8C event, owner 0")));
        assert!(out
            .lines()
            .iter()
            .any(|l| l
                == "PASS lights     Watchdog read-back: the left's LEDs are the keyboard's own"));

        // No event: a failure, not a hang.
        let f = Fake::with(Kb {
            timeout_s: 1,
            ..Kb::default()
        });
        let none = |_: Duration| -> Option<[u8; REPORT]> { None };
        let out = run(&f, ALL, Some(&none), &opts);
        assert_eq!(out.code(), FAIL);
        assert!(out
            .lines()
            .iter()
            .any(|l| l == "FAIL lights     Watchdog: no hand-back event within 6 s (timeout 1 s)"));
        assert!(released(&f), "the drop guard releases what the case left");

        // A keyboard without a timeout: a warning. No feed: blocked.
        let f = Fake::with(Kb {
            timeout_s: 0,
            ..Kb::default()
        });
        let out = run(&f, ALL, Some(&none), &opts);
        assert_eq!(out.code(), PASS);
        assert!(out
            .lines()
            .iter()
            .any(|l| l.starts_with("WARN lights     Watchdog: timeout 0")));
        let out = run(&Fake::default(), ALL, None, &opts);
        assert_eq!(out.code(), BLOCKED);
    }

    /// #753: Windows Dynamic Lighting has the lights (the left_lamparray build): every case is
    /// BLOCKED with that reason, not FAIL on "status 1 (unknown)", and nothing else is sent.
    #[test]
    fn windows_dynamic_lighting_holding_the_lights_blocks() {
        for (owner, via) in [(1, "USB"), (2, "Bluetooth")] {
            let f = Fake::with(Kb {
                lamp_array: owner,
                ..Kb::default()
            });
            let opts = Options {
                watchdog: true,
                ..FAST
            };
            let none = |_: Duration| -> Option<[u8; REPORT]> { None };
            let out = run(&f, ALL, Some(&none), &opts);
            let lines = out.lines();
            assert_eq!(out.code(), BLOCKED, "{lines:#?}");
            assert!(out.rows.is_empty(), "{lines:#?}");
            assert_eq!(
                lines.last().unwrap(),
                &format!(
                    "BLOCKED: SET_FRAME and SET_EFFECT not tested: host holds lights (Windows \
                     Dynamic Lighting); owner {owner}, its LampArray over {via}"
                )
            );
            assert_eq!(*f.sent.borrow(), 1, "only the owner read: {lines:#?}");
            assert_eq!(out.json()["result"], "blocked");
            // Health: a warning, not a failure.
            let mut health = Outcome {
                rows: Vec::new(),
                blocked: None,
            };
            out.fold_into(&mut health);
            assert_eq!(health.code(), PASS, "{:#?}", health.lines());
        }
        assert_eq!(
            lamp_array_holds(
                &Fake::with(Kb {
                    lamp_array: 1,
                    ..Kb::default()
                }),
                ALL
            )
            .as_deref(),
            Some("host holds lights (Windows Dynamic Lighting): the keyboard's own lights don't show (owner 1)")
        );
        assert_eq!(lamp_array_holds(&Fake::default(), ALL), None);
        assert_eq!(
            lamp_array_holds(&Fake::default(), 0),
            None,
            "no SET_FRAME, no read"
        );
    }

    /// Windows takes the lights mid-run: the busy reply blocks the rest; nothing fails.
    #[test]
    fn a_lamp_array_taking_the_lights_mid_run_blocks_the_rest() {
        let healthy = Fake::default();
        run(&healthy, ALL, None, &FAST);
        let total = healthy.kb.borrow().requests;
        // At every request after the owner read, up to the cases' last (the drop guard's two
        // releases come after them).
        for at in 2..=total - 2 {
            let f = Fake::with(Kb {
                lamp_array: 2,
                takes_at: Some(at),
                ..Kb::default()
            });
            let out = run(&f, ALL, None, &FAST);
            let lines = out.lines();
            assert_eq!(out.code(), BLOCKED, "at {at}: {lines:#?}");
            assert!(
                out.rows.iter().all(|r| r.level != Level::Fail),
                "at {at}: {lines:#?}"
            );
            assert_eq!(out.blocked.len(), 1, "at {at}: {lines:#?}");
            assert!(
                out.blocked[0].contains("host holds lights (Windows Dynamic Lighting); owner 2"),
                "at {at}: {lines:#?}"
            );
        }
    }

    #[test]
    fn health_takes_the_rows_and_warns_for_what_it_couldnt_test() {
        let f = Fake::default();
        let cases = run(
            &f,
            capabilities::FRAMES | capabilities::RGB_SHOWN,
            None,
            &FAST,
        );
        let mut health = Outcome {
            rows: Vec::new(),
            blocked: None,
        };
        cases.fold_into(&mut health);
        assert_eq!(health.code(), PASS, "{:#?}", health.lines());
        assert!(health.lines().iter().any(|l| l
            == "WARN lights     Not tested: SET_EFFECT not tested: no HELLO bit 19 (EFFECTS, dev builds only)"));

        let f = Fake::with(Kb {
            frame_status: 5,
            ..Kb::default()
        });
        let mut health = Outcome {
            rows: Vec::new(),
            blocked: None,
        };
        run(&f, ALL, None, &FAST).fold_into(&mut health);
        assert_eq!(health.code(), FAIL);
    }
}
