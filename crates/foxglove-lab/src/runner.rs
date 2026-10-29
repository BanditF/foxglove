//! Bounded execution. Unknown firmware features fail closed before any injection.
use crate::{
    behaviors::Catalog,
    host::Verdict,
    report::{CaseResult, Status, StepResult},
    screen,
    spec::{Case, Invoke, Step},
};
use anyhow::{bail, ensure, Result};
use foxglove_link::{
    art,
    link::{Link, Subscription},
    panel,
    protocol::{self, capabilities as caps, Hello, InvokeStatus},
    settings, Transport,
};
use std::{
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

/// A step that couldn't run because a precondition outside the test isn't met (reported as
/// BLOCKED, not FAIL).
#[derive(Debug)]
pub struct Blocked(pub String);
impl std::fmt::Display for Blocked {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for Blocked {}

const POSITION: u8 = 0x05;

/// Keymap positions wired to the right half (matrix columns 7-13 in the Eyelash Corne
/// transform; tests/zmk-sim/common/matrix.dtsi has the same map).
pub const RIGHT_POSITIONS: [u8; 23] = [
    6, 7, 8, 9, 10, 11, 12, 19, 20, 21, 22, 23, 24, 25, 26, 27, 35, 36, 37, 38, 39, 40, 41,
];
pub fn is_right(pos: u8) -> bool {
    RIGHT_POSITIONS.contains(&pos) || (45..=47).contains(&pos)
}

pub trait Device: Transport {
    fn layer(&self, timeout: Duration) -> Result<Option<u32>>;
    /// Unsolicited art-key events (D7 8D 01 <dir>, #302), from now on; `None` where the device
    /// can't deliver unsolicited reports.
    fn art_key_events(&self) -> Option<Events> {
        None
    }
    /// Art relay reports (D6 8x with the relay status: STATUS, SELECT, COMMIT, ...) as they
    /// arrive from now on: the left's relay events (HELLO bit 3) and the replies to every
    /// client's art commands, which Windows hands to each open handle of the HID collection
    /// (so the app's own push or select loop is heard too). `None` where the device can't
    /// deliver reports it didn't ask for.
    fn art_status_events(&self) -> Option<Events> {
        None
    }
}
/// Reports collected from the link until dropped.
pub struct Events {
    pub rx: std::sync::mpsc::Receiver<[u8; 32]>,
    _watch: Option<foxglove_link::link::EventSubscription>,
}
impl Events {
    /// Reports from a channel with no link behind it (offline tests' fake devices).
    pub fn from_channel(rx: std::sync::mpsc::Receiver<[u8; 32]>) -> Self {
        Self { rx, _watch: None }
    }
}
pub struct Hardware {
    pub link: Link,
    pub layers: Subscription,
    generation: u64,
}
impl Hardware {
    pub fn new(link: Link) -> Self {
        let generation = link.state().generation;
        let layers = link.subscribe(|r| r.first() == Some(&0xff));
        Self {
            link,
            layers,
            generation,
        }
    }
    fn check_connection(&self) -> Result<()> {
        let state = self.link.state();
        ensure!(
            state.connected.is_some() && state.generation == self.generation,
            "keyboard disconnected or reconnected; start a new run"
        );
        Ok(())
    }
}
impl Transport for Hardware {
    fn send(&self, r: &[u8]) -> Result<()> {
        self.check_connection()?;
        self.link.send(r)
    }
    fn request(&self, r: &[u8], p: u8, c: u8, t: Duration) -> Result<[u8; 32]> {
        self.check_connection()?;
        let result = self.link.request(r, p, c, t)?;
        self.check_connection()?;
        Ok(result)
    }
}
impl Device for Hardware {
    fn art_key_events(&self) -> Option<Events> {
        // Every incoming report: the link's dispatcher drops unsolicited replies before plain
        // subscribers see them.
        let (tx, rx) = std::sync::mpsc::channel();
        let watch = self.link.subscribe_reports(move |_, r| {
            if r.starts_with(&[protocol::SETTINGS, 0x80 | ART_KEYS, 1]) {
                let _ = tx.send(*r);
            }
        });
        Some(Events {
            rx,
            _watch: Some(watch),
        })
    }
    fn art_status_events(&self) -> Option<Events> {
        let (tx, rx) = std::sync::mpsc::channel();
        let watch = self.link.subscribe_reports(move |_, r| {
            if is_art_status(r) {
                let _ = tx.send(*r);
            }
        });
        Some(Events {
            rx,
            _watch: Some(watch),
        })
    }
    fn layer(&self, timeout: Duration) -> Result<Option<u32>> {
        self.check_connection()?;
        match self.layers.recv_timeout(timeout) {
            Ok(r) => {
                let n = usize::from(r[1]);
                ensure!((1..=4).contains(&n), "invalid layer mask size");
                let mask = |at| {
                    r[at..at + n]
                        .iter()
                        .rev()
                        .fold(0u32, |m, b| m << 8 | u32::from(*b))
                };
                Ok(Some(mask(2) | mask(2 + n)))
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }
}

/// Whether `r` is an art relay reply that carries the relay status (art_command in host_link.c:
/// every reply but SLOT's D6 87, which has its own layout; DATA has no reply).
pub fn is_art_status(r: &[u8; 32]) -> bool {
    use protocol::art;
    r[0] == protocol::ART
        && r[1] & 0x80 != 0
        && [
            art::BEGIN,
            art::COMMIT,
            art::SELECT,
            art::STATUS,
            art::SAVE,
            art::ERASE,
        ]
        .contains(&(r[1] & 0x7f))
}

/// Whether the left's relay is in the middle of another job: a SELECT now would be refused
/// (PUSHING, -EBUSY in art_relay_select, with no error in the reply) or would cut another
/// client's blob short (RECEIVING).
pub fn left_busy(s: &protocol::ArtStatus) -> bool {
    use protocol::left_state;
    s.state == left_state::PUSHING || s.state == left_state::RECEIVING
}

/// How long the right's redraw count must hold before note_right_redraws takes it (#618): longer
/// than the 900 ms between a redraw's two shots (peripheral_status.c).
const REDRAW_SETTLE: Duration = Duration::from_millis(1200);

/// Capabilities come only from HELLO's capability bits, never from the version number.
/// Position presses also need the refusal bit: a press resolves through whatever layers are
/// active, so the firmware must be known to refuse bt/out/reset/bootloader/soft_off.
/// D7 0D ART_KEYS: the app takes over the art keys (#302); its events come back as D7 8D 01 <dir>.
const ART_KEYS: u8 = 0x0D;
/// HELLO capability bit 16: the keymap has &art_next/&art_prev and the art relay is built.
const ART_KEYS_CAP: u32 = 1 << 16;

pub fn unsupported(requirement: &str, hello: &Hello) -> Option<String> {
    let has = |bit| hello.capabilities & bit != 0;
    let reason = match requirement {
        "invoke_refusal" if has(caps::INVOKE_REFUSES) => return None,
        "invoke_refusal" => {
            "firmware doesn't advertise INVOKE refusal (HELLO bit 7); unsafe request was not sent"
        }
        // Firmware new enough to report its build (#133); the &bt refusal fix (#142) landed
        // with it, so the &bt cases also only send BT_SEL to the profile already in use.
        // The PC's lock state, checked when the case starts (Runner::locked).
        "unlocked" => return None,
        "build_info" if has(caps::BUILD_INFO) => return None,
        "build_info" => "firmware doesn't report its build (HELLO bit 12)",
        "screen_readback" if has(caps::SCREEN) => return None,
        "screen_readback" => "firmware has no screen readback (HELLO bit 6, #67)",
        "activity_relay" if has(caps::ACTIVITY_RELAY) => return None,
        "activity_relay" => "the left doesn't relay its activity to the right (HELLO bit 23, #603)",
        "rgb_shown" if has(caps::RGB_SHOWN) => return None,
        "rgb_shown" => "RGB_STATE doesn't say what the LEDs show (HELLO bit 24, #605)",
        "right_redraws" if has(caps::RIGHT_REDRAWS) => return None,
        "right_redraws" => "the left doesn't report the right's redraw count (HELLO bit 25, #618)",
        "art_keys" if hello.capabilities & ART_KEYS_CAP != 0 => return None,
        "art_keys" => "firmware has no art keys (HELLO bit 16, #302)",
        "position_press" if has(caps::POSITION) && has(caps::INVOKE_REFUSES) => return None,
        "position_press" => {
            "firmware has no position press with refusal (HELLO bits 5 and 7; test-hooks build)"
        }
        r if r.starts_with("host_link>=") => {
            if r[11..].parse::<u8>().is_ok_and(|v| hello.version >= v) {
                return None;
            }
            "host_link version requirement unmet"
        }
        _ => "unknown capability",
    };
    Some(reason.into())
}

pub struct Runner<'a, D> {
    pub device: &'a D,
    pub catalog: &'a Catalog,
    pub hello: Hello,
    pub cancelled: Arc<AtomicBool>,
    pub non_interactive: bool,
    pub out: &'a Path,
    /// Where goldens named in specs live (tests/hil, relative to the repository root).
    pub golden_dir: PathBuf,
    held_until: Option<Instant>,
    /// Positions this case pressed down and hasn't released yet; released in cleanup.
    held_positions: Vec<u8>,
    /// Files (relative to `out`) the current step saved for the report.
    attachments: Vec<String>,
    /// screens.ini as it was before this case changed it; put back in cleanup.
    screens_ini: Option<(PathBuf, Option<Vec<u8>>)>,
    /// File-name prefix for this case's attachments.
    case_name: String,
    /// The last capture_screen per half in this case: expect_screen_change compares with it.
    baselines: std::collections::HashMap<String, image::GrayImage>,
    /// Whether the PC is locked, for cases that require "unlocked" (tests swap it out).
    pub locked: fn() -> bool,
    /// The right's redraw count at the case's note_right_redraws (#618).
    redraw_note: Option<u32>,
    /// Where key presses are recorded as the harness's own input (`injections`, #618); None
    /// records nothing (offline tests). `foxglove-lab run` sets the shared file.
    pub injections: Option<PathBuf>,
    /// The running app's local API, while the app runs (#618): built-in art selects go through
    /// `art.select` then, because the app puts its own choice back after a direct SELECT (#622).
    /// `None`: the app isn't running, so SELECT goes to the keyboard directly.
    pub app: Option<&'a dyn crate::app_api::AppArt>,
}
impl<'a, D: Device> Runner<'a, D> {
    pub fn new(
        device: &'a D,
        catalog: &'a Catalog,
        hello: Hello,
        cancelled: Arc<AtomicBool>,
        non_interactive: bool,
        out: &'a Path,
    ) -> Self {
        Self {
            device,
            catalog,
            hello,
            cancelled,
            non_interactive,
            out,
            golden_dir: PathBuf::from("tests/hil"),
            held_until: None,
            held_positions: Vec::new(),
            attachments: Vec::new(),
            baselines: Default::default(),
            case_name: String::new(),
            screens_ini: None,
            locked: crate::platform::locked,
            redraw_note: None,
            injections: None,
            app: None,
        }
    }
    fn check(&self) -> Result<()> {
        ensure!(
            !self.cancelled.load(Ordering::Relaxed),
            "interrupted by Ctrl-C"
        );
        Ok(())
    }
    fn wait(&self, ms: u64, mut check: impl FnMut() -> Result<(bool, String)>) -> Result<String> {
        let end = Instant::now() + Duration::from_millis(ms);
        loop {
            self.check()?;
            let (matches, actual) = check()?;
            if matches {
                return Ok(actual);
            }
            ensure!(
                Instant::now() < end,
                "timeout after {ms} ms; last actual: {actual}"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    fn resolve(&self, i: &Invoke) -> Result<u16> {
        i.local_id.or_else(|| i.behavior.as_ref().and_then(|b| self.catalog.ids.get(b.trim_start_matches('&')).copied())).ok_or_else(|| anyhow::anyhow!("behavior {:?} absent from cached Studio metadata; provide local_id or --keymap",i.behavior))
    }
    fn invoke(&mut self, i: &Invoke, typing: bool) -> Result<String> {
        i.validate()?;
        let id = self.resolve(i)?;
        // Explicit ids are allowed only when metadata proves the safe behavior too.
        let known = |n: &str| self.catalog.ids.get(n) == Some(&id);
        let safe = [
            "mo",
            "tog",
            "to",
            "sl",
            "rgb_ug",
            "bl",
            "ext_power",
            "none",
            "trans",
        ]
        .iter()
        .any(|n| known(n));
        // A dangerous behavior may be sent only as a refusal test, and only to firmware that
        // advertises refusal (HELLO bit 7); the case is BLOCKED before this otherwise.
        let refusal_test =
            i.expect_status == "refused" && unsupported("invoke_refusal", &self.hello).is_none();
        let silent_key = known("kp") && crate::spec::non_typing_key(i.param1) && i.hold_ms <= 200;
        ensure!(
            if typing { known("kp") && i.hold_ms == 0 } else { safe || refusal_test || silent_key },
            "injection blocked: needs verified non-typing behavior (or kp F13-F24), or kp tap inside sink; refusal support unknown"
        );
        self.check()?;
        // Discard prior cases' reports, but retain reports arriving during this invoke.
        for _ in 0..1024 {
            if self.device.layer(Duration::ZERO)?.is_none() {
                break;
            }
        }
        // Acknowledgement follows press, so allow the entire hold after the reply too.
        let prior_release = self.held_until;
        let deadline = Instant::now() + Duration::from_millis(u64::from(i.hold_ms) + 1500);
        self.held_until = Some(prior_release.map_or(deadline, |prior| prior.max(deadline)));
        let _injecting = self.injecting(|| format!("invoke {id} param1={:#x}", i.param1));
        let status = settings::invoke(self.device, id, i.param1, i.param2, i.hold_ms)?;
        // The firmware schedules release even when the behavior press returns an error.
        if matches!(status, InvokeStatus::Ok | InvokeStatus::BehaviorFailed) {
            let deadline = Instant::now() + Duration::from_millis(u64::from(i.hold_ms) + 30);
            self.held_until = Some(prior_release.map_or(deadline, |prior| prior.max(deadline)));
        } else {
            self.held_until = prior_release;
        }
        let actual = match status {
            InvokeStatus::Ok => "ok",
            InvokeStatus::UnknownBehavior => "unknown_id",
            InvokeStatus::Busy => "busy",
            InvokeStatus::BehaviorFailed => "failed",
            InvokeStatus::NotBuiltIn => "not_built_in",
            InvokeStatus::BadLength => "bad_length",
            InvokeStatus::Refused => "refused",
            InvokeStatus::Other(_) => "unknown_status",
        };
        ensure!(
            actual == i.expect_status,
            "expected {}, actual {actual}",
            i.expect_status
        );
        Ok(format!(
            "local_id={id}, param1={}, param2={}, status={actual}",
            i.param1, i.param2
        ))
    }
    fn art_push(&mut self, a: &crate::spec::ArtPush) -> Result<String> {
        use crate::resilience::{bad_header, push, Cut};
        self.right_ready()?;
        let root = self.golden_dir.join("../..");
        let mut blob = std::fs::read(root.join(&a.blob))
            .map_err(|e| anyhow::anyhow!("blob {}: {e}", a.blob))?;
        ensure!(art::is_blob(&blob), "{} is not an art blob", a.blob);
        if a.corrupt == "header" {
            blob = bad_header(&blob);
        }
        // The left answers from its last relay read and asks again, so the second call is fresh.
        art::status(self.device)?;
        std::thread::sleep(Duration::from_millis(300));
        let before = art::status(self.device)?;
        let cut = match a.cut.as_str() {
            "data" => Cut::Data,
            "commit" => Cut::Commit,
            _ => Cut::None,
        };
        let sent = match push(self.device, &blob, a.corrupt == "crc", cut, a.drop_link) {
            Err(e) if e.to_string().contains("not built in") => {
                return Err(Blocked(e.to_string()).into())
            }
            other => other?,
        };
        let want = |s: &protocol::ArtStatus| {
            s.state == a.expect_left_state
                && (a.expect_errors.is_empty() || a.expect_errors.contains(&s.error))
                && a.expect_right_state.is_none_or(|r| s.right_state == r)
                && (!a.right_keeps_set || s.right_set == before.right_set)
        };
        let end = Instant::now() + Duration::from_millis(a.within_ms);
        loop {
            self.check()?;
            std::thread::sleep(Duration::from_millis(500));
            // Status can time out while the split link is being rebuilt; keep asking.
            if let Ok(s) = art::status(self.device) {
                if want(&s) {
                    return Ok(format!(
                        "{sent}; then {s} (before: right set {})",
                        before.right_set
                    ));
                }
                ensure!(
                    Instant::now() < end,
                    "{sent}; after {} ms: {s} (before: right set {})",
                    a.within_ms,
                    before.right_set
                );
            } else {
                ensure!(
                    Instant::now() < end,
                    "{sent}; no art STATUS reply for {} ms",
                    a.within_ms
                );
            }
        }
    }
    fn art_key(&mut self, k: &crate::spec::ArtKey) -> Result<String> {
        self.right_ready()?;
        let dir: i8 = if k.pos == 11 { 1 } else { -1 };
        let fresh = |runner: &Self| -> Result<protocol::ArtStatus> {
            art::status(runner.device)?;
            std::thread::sleep(Duration::from_millis(300));
            art::status(runner.device)
        };
        let subscribe = |runner: &Self, on: bool| -> Result<()> {
            let r = runner.device.request(
                &[protocol::SETTINGS, ART_KEYS, u8::from(on)],
                protocol::SETTINGS,
                ART_KEYS,
                Duration::from_secs(1),
            )?;
            ensure!(
                r[2] == 0 && r[3] == u8::from(on),
                "ART_KEYS {on} answered {:02x?}",
                &r[..4]
            );
            Ok(())
        };
        let before = fresh(self)?;
        let events = self
            .device
            .art_key_events()
            .ok_or_else(|| anyhow::anyhow!("this device can't deliver unsolicited reports"))?;
        if k.subscribed {
            subscribe(self, true)?;
        }
        let tap = crate::spec::Position {
            pos: k.pos,
            state: "tap".into(),
            hold_ms: 30,
            expect_status: "ok".into(),
        };
        let pressed = self.press(&tap);
        let outcome = (|| -> Result<String> {
            pressed?;
            if k.subscribed {
                let r = events
                    .rx
                    .recv_timeout(Duration::from_secs(2))
                    .map_err(|_| anyhow::anyhow!("no D7 8D 01 event within 2 s"))?;
                ensure!(
                    r[3] as i8 == dir,
                    "event direction {} (wanted {dir})",
                    r[3] as i8
                );
                std::thread::sleep(Duration::from_millis(1500));
                let after = fresh(self)?;
                ensure!(
                    after.right_set == before.right_set,
                    "the art stepped anyway: {} -> {}",
                    before.right_set,
                    after.right_set
                );
                return Ok(format!(
                    "D7 8D 01 {:02X} arrived; the art stayed on set {}",
                    r[3], after.right_set
                ));
            }
            ensure!(
                events.rx.try_recv().is_err(),
                "an art-key event arrived without the opt-in"
            );
            let end = Instant::now() + Duration::from_secs(6);
            loop {
                let after = fresh(self)?;
                if after.right_set != before.right_set {
                    return Ok(format!(
                        "the board stepped the art: set {} -> {} ({after})",
                        before.right_set, after.right_set
                    ));
                }
                ensure!(
                    Instant::now() < end,
                    "the art didn't step within 6 s: {after}"
                );
            }
        })();
        if k.subscribed && !k.keep_opt_in {
            subscribe(self, false)?;
        }
        outcome
    }
    fn panel_flood(&mut self, f: &crate::spec::PanelFlood) -> Result<String> {
        use crate::spec::Layer;
        let (w, h) = (self.hello.width, self.hello.height);
        let frames = [0u8, 1].map(|p| crate::resilience::test_frame(w, h, p));
        let start = Instant::now();
        let (end, mut reports, mut shown, mut presses) =
            (start + Duration::from_secs(f.seconds), 0, 0usize, 0);
        let mut next_press = Instant::now() + Duration::from_secs(2);
        while Instant::now() < end {
            self.check()?;
            reports += panel::show(self.device, &frames[shown % 2], None, w.div_ceil(8))?;
            shown += 1;
            // Paced: sends only queue, so an unpaced loop floods the PC's own queue, not the keyboard.
            let due = start + Duration::from_secs_f64(shown as f64 / f64::from(f.rate.max(1)));
            std::thread::sleep(due.saturating_duration_since(Instant::now()));
            if Instant::now() >= next_press {
                // NUMBER held and let go mid-flood, sent without waiting for POSITION's reply (a
                // flooded keyboard may drop replies); the layer reports must follow both ways.
                let raw = |state: u8| self.device.send(&[protocol::SETTINGS, POSITION, 43, state]);
                raw(1)?;
                let down = self.execute(&Step::ExpectLayer(Layer {
                    active_contains: vec![1],
                    top: None,
                    within_ms: 2000,
                }));
                raw(0)?;
                let up = self.execute(&Step::ExpectLayer(Layer {
                    active_contains: vec![],
                    top: Some(0),
                    within_ms: 2000,
                }));
                if let Err(e) = down.and(up) {
                    let _ = raw(0);
                    bail!("after {shown} frames, a NUMBER press wasn't followed: {e}");
                }
                presses += 1;
                next_press = Instant::now() + Duration::from_secs(2);
            }
        }
        let last = &frames[(shown + 1) % 2];
        let path = crate::host::expand_env(&f.save);
        std::fs::write(&path, crate::resilience::pbm(w, h, last))?;
        Ok(format!(
            "{shown} frames ({reports} reports) in {} s at {}/s, {presses} layer presses all followed; last frame saved to {path}",
            f.seconds, f.rate
        ))
    }
    fn art_select(&mut self, a: &crate::spec::Art) -> Result<String> {
        self.right_ready()?;
        ensure!(
            a.set >= 0,
            "pick a built-in set (0 or more) so the result can be checked"
        );
        // The left answers from its last relay read and asks again, so the second call is fresh.
        let fresh = |runner: &Self| -> Result<protocol::ArtStatus> {
            art::status(runner.device)?;
            std::thread::sleep(Duration::from_millis(300));
            art::status(runner.device)
        };
        let mut before = fresh(self)?;
        let mut waited = String::new();
        if left_busy(&before) {
            // Another client's relay job (the app's re-apply after a re-link, #618): a SELECT
            // now would be refused without an error, so wait for that job to end first.
            let started = Instant::now();
            self.left_idle(before, a.idle_within_ms)?;
            waited = format!(
                "the left was busy for {} ms first; ",
                started.elapsed().as_millis()
            );
            before = fresh(self)?;
        }
        ensure!(
            a.set < before.right_set_count as i8,
            "set {} out of range: the right has {} built-in sets ({before})",
            a.set,
            before.right_set_count
        );
        if before.right_set < 0 && !a.replace_pushed {
            return Err(Blocked(
                "the right shows pushed art, which a select would lose (RAM only); run with \
                 built-in art showing, or set replace_pushed = true"
                    .into(),
            )
            .into());
        }
        let count = before.right_set_count;
        let (path, actual) = self.select_built_in(a.set, count, a.within_ms)?;
        if a.restore && before.right_set >= 0 && before.right_set != a.set {
            self.select_built_in(before.right_set, count, a.within_ms)?;
            return Ok(format!(
                "{path}: {waited}{actual}; restored set {}",
                before.right_set
            ));
        }
        Ok(format!("{path}: {waited}{actual}"))
    }
    /// Selects built-in `set` and checks the right shows it; answers with the path taken and
    /// what was read back. While the app runs (`self.app`), through its `art.select`, which
    /// answers once its art job ended, then one fresh D6 STATUS pair and `art.status`;
    /// otherwise the D6 SELECT itself, then STATUS until the right shows the set.
    fn select_built_in(&self, set: i8, count: u8, ms: u64) -> Result<(String, String)> {
        let Some(app) = self.app else {
            art::select(self.device, set)?;
            let actual = self.wait(ms, || {
                std::thread::sleep(Duration::from_millis(250));
                let s = art::status(self.device)?;
                Ok((s.right_set == set, s.to_string()))
            })?;
            return Ok((
                "direct SELECT (D6 04; the app isn't running)".into(),
                actual,
            ));
        };
        let label = crate::app_api::built_in_label(count, set);
        let path = format!("through the app's API (art.select {label:?}; the app is running)");
        // The app's own wait for its job (20 s), plus the time asked for.
        let limit = Duration::from_millis(ms) + Duration::from_secs(20);
        app.select_built_in(&label, limit)
            .map_err(|e| anyhow::anyhow!("{path}: {e:#}"))?;
        art::status(self.device)?;
        std::thread::sleep(Duration::from_millis(300));
        let s = art::status(self.device)?;
        let shown = app
            .status(Duration::from_secs(5))
            .map_err(|e| anyhow::anyhow!("{path}: art.status: {e:#}"))?;
        let says = shown
            .built_in
            .clone()
            .unwrap_or_else(|| format!("no built-in ({})", shown.name.as_deref().unwrap_or("?")));
        ensure!(
            s.right_set == set
                && shown
                    .built_in
                    .as_deref()
                    .is_some_and(|b| b.eq_ignore_ascii_case(&label)),
            "{path}: the right doesn't show set {set}: D6 STATUS {s}; art.status {says}"
        );
        Ok((path, format!("{s}; art.status {says}")))
    }
    /// Waits until the left's relay leaves another job (state PUSHING or RECEIVING) for at most
    /// `ms`. The firmware sends no event when a job ends (host_link_art_status() fires on relay
    /// up/down only), so this waits on the reports themselves: every art relay report that
    /// arrives (the busy client's own STATUS reads while it waits for its job, relay events) is
    /// checked, and nothing is read on a timer. One STATUS read after subscribing gives the
    /// state now; one more at the deadline is the fallback (a job whose client stopped
    /// reading). Fails when the left is still busy then.
    fn left_idle(&self, last: protocol::ArtStatus, ms: u64) -> Result<protocol::ArtStatus> {
        let end = Instant::now() + Duration::from_millis(ms);
        // Subscribed before the read, so a report between the two isn't missed.
        let events = self.device.art_status_events();
        let now = art::status(self.device).unwrap_or(last);
        if !left_busy(&now) {
            return Ok(now);
        }
        if let Some(events) = &events {
            loop {
                self.check()?;
                let left = end.saturating_duration_since(Instant::now());
                match events.rx.recv_timeout(left) {
                    Ok(r) => {
                        let s = protocol::ArtStatus::parse(&r);
                        if !left_busy(&s) {
                            return Ok(s);
                        }
                    }
                    Err(_) => break, // the deadline (or the link stopped)
                }
            }
        } else {
            std::thread::sleep(end.saturating_duration_since(Instant::now()));
        }
        let s = art::status(self.device)?;
        ensure!(
            !left_busy(&s),
            "the left's relay stayed busy for {ms} ms (another client's job?): {s}"
        );
        Ok(s)
    }
    /// D7 05: presses or releases keymap position `p.pos` as if the key moved. Positions on the
    /// right half travel over the relay and come back through the split link.
    fn press(&mut self, p: &crate::spec::Position) -> Result<String> {
        if is_right(p.pos) && p.state != "up" {
            self.right_ready()?;
        }
        let send = |state: u8| -> Result<u8> {
            let r = self.device.request(
                &[protocol::SETTINGS, POSITION, p.pos, state],
                protocol::SETTINGS,
                POSITION,
                Duration::from_secs(4),
            )?;
            ensure!(
                r[3] == p.pos,
                "position reply for {}, wanted {}",
                r[3],
                p.pos
            );
            Ok(r[2])
        };
        let name = |status: u8| match status {
            0 => "ok",
            2 => "busy_or_no_relay",
            3 => "relay_timeout",
            4 => "not_built_in",
            5 => "bad_request",
            6 => "refused",
            _ => "unknown_status",
        };
        let _injecting = self.injecting(|| format!("position {} {}", p.pos, p.state));
        let status = match p.state.as_str() {
            "down" => {
                let s = send(1)?;
                if s == 0 && !self.held_positions.contains(&p.pos) {
                    self.held_positions.push(p.pos);
                }
                s
            }
            "up" => {
                self.held_positions.retain(|&q| q != p.pos);
                send(0)?
            }
            _ => {
                let s = send(1)?;
                if s == 0 {
                    std::thread::sleep(Duration::from_millis(u64::from(p.hold_ms)));
                    let up = send(0)?;
                    ensure!(up == 0, "release of {} answered {}", p.pos, name(up));
                }
                s
            }
        };
        ensure!(
            name(status) == p.expect_status,
            "position {} {}: expected {}, actual {}",
            p.pos,
            p.state,
            p.expect_status,
            name(status)
        );
        Ok(format!("position {} {}: {}", p.pos, p.state, name(status)))
    }
    /// Records the key press made while the guard lives as the harness's input (#618).
    fn injecting(&self, what: impl FnOnce() -> String) -> Option<crate::injections::Injection> {
        self.injections
            .as_ref()
            .map(|path| crate::injections::begin_at(path.clone(), what()))
    }
    /// Fails BLOCKED unless the left reports the right half's relay fully up (HELLO right link
    /// 2, relay discovery done). Steps that go to the right half over the relay check this
    /// first, so the harness never presses on a half-set-up split link (#122: poking it there
    /// once left the board not typing).
    fn right_ready(&self) -> Result<()> {
        let h = foxglove_link::panel::hello(self.device)?;
        if h.right_link == 2 && h.discovery == 4 {
            return Ok(());
        }
        Err(Blocked(format!(
            "right half relay not up (right link {}, discovery {}, tries {}); not touching the \
             right half (#122)",
            h.right_link, h.discovery, h.discovery_tries
        ))
        .into())
    }
    fn capture(&self, half: &str) -> Result<image::GrayImage> {
        if half == "right" {
            self.right_ready()?;
        }
        let bits = screen::snapshot(self.device, u8::from(half == "right")).map_err(|e| {
            match e.downcast_ref::<screen::SnapError>() {
                Some(screen::SnapError::NotBuiltIn) => {
                    anyhow::Error::from(Blocked(format!("{half} screen: {e}")))
                }
                _ => e.context(format!("{half} screen snapshot")),
            }
        })?;
        screen::portrait(&bits)
    }
    /// The right half's redraw count as the left last read it (D7 0E; each request starts a
    /// fresher read). BLOCKED when the right doesn't report it (right firmware before #618).
    fn right_redraws(&self) -> Result<u32> {
        let d = settings::battery_dim(self.device, None)?;
        d.right_redraws.ok_or_else(|| {
            Blocked("the right half reports no redraw count (no relay read yet, or right firmware before #618)".into()).into()
        })
    }
    /// Saves a portrait capture as a 4x PNG next to the report and attaches it to the step.
    fn save_png(&mut self, stem: &str, image: &image::GrayImage) -> Result<String> {
        // Prefixed with the case so captures from different cases in one run never overwrite.
        let name = format!("{}-{stem}.png", self.case_name);
        std::fs::write(self.out.join(&name), screen::png(image)?)?;
        self.attachments.push(name.clone());
        Ok(name)
    }
    /// Waits out the bounded firmware release even when cancelled; no unsupported release command.
    fn cleanup(&mut self) {
        self.baselines.clear();
        self.redraw_note = None;
        if let Some((path, original)) = self.screens_ini.take() {
            let _ = match original {
                Some(bytes) => std::fs::write(&path, bytes),
                None => std::fs::remove_file(&path),
            };
        }
        let held = std::mem::take(&mut self.held_positions);
        let _injecting = (!held.is_empty()).then(|| self.injecting(|| format!("release {held:?}")));
        for pos in held.into_iter().rev() {
            // Best effort; the firmware also releases owned downs after 10 s.
            let _ = self.device.request(
                &[protocol::SETTINGS, POSITION, pos, 0],
                protocol::SETTINGS,
                POSITION,
                Duration::from_secs(4),
            );
        }
        if let Some(end) = self.held_until.take() {
            std::thread::sleep(end.saturating_duration_since(Instant::now()));
        }
    }
    fn execute(&mut self, step: &Step) -> Result<String> {
        self.check()?;
        match step {
            Step::Invoke(i) => self.invoke(i, false),
            Step::ExpectLayer(l) => self.wait(l.within_ms, || {
                let Some(mask) = self.device.layer(Duration::from_millis(10))? else {
                    return Ok((false, "no layer report".into()));
                };
                let top = (mask != 0).then(|| 31 - mask.leading_zeros());
                Ok((
                    l.active_contains.iter().all(|n| mask & (1u32 << n) != 0)
                        && l.top.is_none_or(|n| top == Some(u32::from(n))),
                    format!("active/default mask={mask:#x}, top={top:?}"),
                ))
            }),
            Step::ExpectConn(c) => self.wait(c.within_ms, || {
                let s = settings::conn_state(self.device)?;
                let h = panel::hello(self.device)?;
                Ok((
                    c.usb.is_none_or(|v| v == s.usb_ready)
                        && c.ble_connected.is_none_or(|v| v == s.connected)
                        && c.right_link.is_none_or(|v| v == (h.right_link != 0))
                        && (!c.usb_or_ble || s.usb_ready || s.connected),
                    format!("{s:?}, right_link={}", h.right_link),
                ))
            }),
            Step::ExpectRgb(r) => self.wait(r.within_ms, || {
                let (s, shown) = settings::rgb_output(self.device)?;
                let look = |h: &protocol::RgbHalf| crate::spec::Look {
                    hue: h.hue,
                    sat: h.sat,
                    brightness: h.brightness,
                    effect: h.effect,
                };
                Ok((
                    s.right.is_some_and(|(right, age)| {
                        age <= r.right_age_max_s
                            && r.right_on.is_none_or(|on| on == right.on)
                            && (!r.right_same_look || look(&right) == look(&s.left))
                    }) && r.left_on.is_none_or(|on| on == s.left.on)
                        && r.left_look.is_none_or(|l| l == look(&s.left))
                        && (!r.shown || (shown.left.is_some() && shown.right.is_some())),
                    format!("{s:?}, shown {shown:?}"),
                ))
            }),
            Step::Prompt(p) => crate::platform::prompt(&p.ask, &self.cancelled),
            Step::TypeIntoSink(t) => {
                // Preflight everything before opening the sink or issuing a key.
                for s in &t.via {
                    match s {
                        Step::Invoke(i) => {
                            let id = self.resolve(i)?;
                            ensure!(
                                self.catalog.ids.get("kp") == Some(&id),
                                "sink accepts kp only"
                            );
                        }
                        Step::PressPosition(_) => {}
                        _ => bail!("sink accepts invoke and press_position taps only"),
                    }
                }
                let actual =
                    crate::platform::sink(&t.text, &t.via, &self.cancelled.clone(), |s| match s {
                        Step::Invoke(i) => self.invoke(i, true).map(|_| ()),
                        Step::PressPosition(p) => self.press(p).map(|_| ()),
                        _ => bail!("unexpected sink step"),
                    })?;
                ensure!(
                    actual == t.expect,
                    "expected {:?}, actual {:?}",
                    t.expect,
                    actual
                );
                Ok(format!("received {actual:?}"))
            }
            Step::ArtSelect(a) => self.art_select(a),
            Step::ScreensSetting(s) => {
                // Only ever a test config: the app under test must run with the same
                // FOXGLOVE_CONFIG_DIR. The user's real settings are never written.
                let Some(dir) = std::env::var_os("FOXGLOVE_CONFIG_DIR") else {
                    return Err(Blocked(
                        "screens_setting needs FOXGLOVE_CONFIG_DIR (a temp folder the app under \
                         test also uses); the user's real settings are never changed"
                            .into(),
                    )
                    .into());
                };
                // Foxglove's settings store (#196): foxglove.toml inside FOXGLOVE_CONFIG_DIR. The
                // screens service follows its [screens] table live.
                let path = PathBuf::from(dir).join("foxglove.toml");
                let current = std::fs::read(&path).ok();
                if self.screens_ini.is_none() {
                    self.screens_ini = Some((path.clone(), current.clone()));
                }
                let text = String::from_utf8_lossy(current.as_deref().unwrap_or_default());
                std::fs::write(
                    &path,
                    crate::host::set_store_screens(&text, &s.key, &s.value)?,
                )?;
                Ok(format!("foxglove.toml [screens] {}={}", s.key, s.value))
            }
            Step::PanelDump(d) => {
                let end = Instant::now() + Duration::from_millis(d.within_ms);
                loop {
                    self.check()?;
                    let path = crate::host::expand_env(&d.path);
                    let dump = std::fs::read(&path).map_err(|e| {
                        anyhow::anyhow!(
                            "no panel dump at {} ({e}): start Foxglove with FOXGLOVE_DUMP_PANEL",
                            d.path
                        )
                    })?;
                    let (w, h, bits) = screen::parse_pbm(&dump)?;
                    let image = self.capture("left")?;
                    let at = screen::find(&image, w, h, &bits);
                    if at.is_some() == d.shown {
                        self.save_png("left-panel", &image)?;
                        return Ok(match at {
                            Some((x, y)) => format!("{w}x{h} frame shown at portrait ({x}, {y})"),
                            None => format!("{w}x{h} frame not on the screen"),
                        });
                    }
                    if Instant::now() >= end {
                        self.save_png("left-panel", &image)?;
                        bail!(
                            "{w}x{h} frame {} the left screen after {} ms",
                            if d.shown { "not found on" } else { "still on" },
                            d.within_ms
                        );
                    }
                    std::thread::sleep(Duration::from_millis(300));
                }
            }
            Step::ExpectBuild(b) => {
                let half = u8::from(b.half == "right");
                let info = settings::build_info(self.device, half)?;
                let v = info.version;
                let (version, profile) = (
                    format!("{}.{}.{}", v.major, v.minor, v.patch),
                    if v.profile == 1 { "ga" } else { "dev" },
                );
                let actual = format!("{} {version} {profile} {}", b.half, info.sha);
                ensure!(
                    b.profile.as_deref().is_none_or(|p| p == profile)
                        && b.version.as_deref().is_none_or(|x| x == version)
                        && b.sha.as_deref().is_none_or(|s| info.sha.starts_with(s)),
                    "expected {:?}/{:?}/{:?}, running {actual}",
                    b.version,
                    b.profile,
                    b.sha
                );
                Ok(actual)
            }
            Step::ExpectHello(h) => {
                let raw = self.device.request(
                    &protocol::hello(),
                    protocol::PANEL,
                    protocol::panel::HELLO,
                    Duration::from_millis(500),
                )?;
                let hello = protocol::Hello::parse(&raw);
                let caps = hello.capabilities;
                let profile = hello
                    .firmware
                    .map(|f| if f.profile == 1 { "ga" } else { "dev" });
                // HELLO byte 31: how often the split guard (#147) dropped the right's link.
                let drops = raw[31];
                let mut wrong: Vec<String> = h
                    .caps_set
                    .iter()
                    .filter(|b| caps & (1 << **b) == 0)
                    .map(|b| format!("bit {b} clear"))
                    .chain(
                        h.caps_clear
                            .iter()
                            .filter(|b| caps & (1 << **b) != 0)
                            .map(|b| format!("bit {b} set")),
                    )
                    .collect();
                if let Some(p) = &h.profile {
                    if profile != Some(p.as_str()) {
                        wrong.push(format!("profile {profile:?}, expected {p}"));
                    }
                }
                if h.guard_drops_max.is_some_and(|max| drops > max) {
                    wrong.push(format!("split guard dropped the link {drops} times"));
                }
                ensure!(
                    wrong.is_empty(),
                    "HELLO caps {caps:#x}: {}",
                    wrong.join(", ")
                );
                Ok(format!(
                    "HELLO caps {caps:#x}, profile {profile:?}, split-guard drops {drops}"
                ))
            }
            Step::SplitGuardTest(g) => {
                let hello = |runner: &Self| -> Result<(u8, u8)> {
                    let raw = runner.device.request(
                        &protocol::hello(),
                        protocol::PANEL,
                        protocol::panel::HELLO,
                        Duration::from_millis(500),
                    )?;
                    Ok((raw[7], raw[31])) // right link, guard drops since boot
                };
                let (link, before) = hello(self)?;
                ensure!(
                    link == 2,
                    "right relay not up before the test (right link {link})"
                );
                let r = self.device.request(
                    &[protocol::SETTINGS, 0x0B],
                    protocol::SETTINGS,
                    0x0B,
                    Duration::from_secs(2),
                )?;
                match r[2] {
                    0 => {}
                    4 => {
                        return Err(Blocked(
                            "GUARD_TEST not built in (GA build, or no split guard)".into(),
                        )
                        .into())
                    }
                    s => bail!("GUARD_TEST answered status {s}"),
                }
                let started = Instant::now();
                let end = started + Duration::from_millis(g.within_ms);
                let (mut dropped_at, mut lowest) = (None, 2u8);
                let mut sent_down: Option<String> = None;
                loop {
                    self.check()?;
                    std::thread::sleep(Duration::from_millis(500));
                    // HELLO can time out while the split link is being rebuilt; keep waiting.
                    if let Ok((link, drops)) = hello(self) {
                        if link < 2 && dropped_at.is_none() {
                            dropped_at = Some(started.elapsed());
                        }
                        // The split link is down: only the left takes this (#624).
                        if let (0, Some(i), None) = (link, &g.while_down, &sent_down) {
                            sent_down = Some(self.invoke(i, false)?);
                        }
                        lowest = lowest.min(link);
                        if let (2, true, Some(dropped)) =
                            (link, drops == before.wrapping_add(1), dropped_at)
                        {
                            if g.while_down.is_some() && sent_down.is_none() {
                                return Err(Blocked(
                                    "the split link never read as down (right link 0): while_down wasn't sent".into(),
                                )
                                .into());
                            }
                            return Ok(format!(
                                "guard drops {before} -> {drops}; link dropped after {:.1} s (lowest {lowest}), relay back after {:.1} s",
                                dropped.as_secs_f32(),
                                started.elapsed().as_secs_f32()
                            ));
                        }
                        ensure!(
                            Instant::now() < end,
                            "after {} ms: right link {link}, guard drops {before} -> {drops}, link dropped: {}",
                            g.within_ms,
                            dropped_at.is_some()
                        );
                    } else {
                        ensure!(
                            Instant::now() < end,
                            "no HELLO reply for {} ms",
                            g.within_ms
                        );
                    }
                }
            }
            Step::ExpectNotBuiltIn(n) => {
                let (report, cmd): (&[u8], u8) = match n.command.as_str() {
                    "position" => (&[protocol::SETTINGS, POSITION, 1, 0], POSITION),
                    _ => (&[protocol::SETTINGS, 0x08, 0x00], 0x08),
                };
                let r =
                    self.device
                        .request(report, protocol::SETTINGS, cmd, Duration::from_secs(4))?;
                ensure!(
                    r[2] == 4,
                    "{} answered status {} (expected 4, not built in)",
                    n.command,
                    r[2]
                );
                Ok(format!("{}: status 4 (not built in)", n.command))
            }
            Step::UsbHalves(u) => {
                let halves = crate::host::classify(&crate::host::usb_entries()?);
                let text = halves
                    .iter()
                    .map(|h| format!("{}: {:?}, {}", h.half, h.verdict, h.message))
                    .collect::<Vec<_>>()
                    .join("; ");
                let bad = |v| v == Verdict::Fail || (u.strict && v == Verdict::Warn);
                ensure!(!halves.iter().any(|h| bad(h.verdict)), "{text}");
                Ok(text)
            }
            Step::ArtPush(a) => self.art_push(a),
            Step::PanelShow(p) => {
                let (w, h) = (self.hello.width, self.hello.height);
                let frame = crate::resilience::test_frame(w, h, p.pattern);
                panel::show(self.device, &frame, None, w.div_ceil(8))?;
                let path = crate::host::expand_env(&p.save);
                std::fs::write(&path, crate::resilience::pbm(w, h, &frame))?;
                Ok(format!(
                    "test frame {} ({w}x{h}) shown, saved to {path}",
                    p.pattern
                ))
            }
            Step::PanelFlood(f) => self.panel_flood(f),
            Step::ArtKey(k) => self.art_key(k),
            Step::PanelRelease(_) => {
                panel::release(self.device)?;
                Ok("PANEL RELEASE sent".into())
            }
            Step::IdleCpu(i) => {
                ensure!((1..=600).contains(&i.seconds), "seconds must be 1..=600");
                ensure!(i.settle_s <= 600, "settle_s must be 0..=600");
                let mut settled = String::new();
                if i.settle_s > 0 {
                    // One timed wait for what's left of the settle, from the app's start.
                    let age = crate::host::process_age_s(&i.process)?;
                    let left = (i.settle_s as f64 - age).max(0.0);
                    if left > 0.0 {
                        std::thread::sleep(Duration::from_secs_f64(left));
                    }
                    self.check()?;
                    settled = format!(
                        "; measured {:.0} s after its start (settle {} s, waited {left:.0} s)",
                        age + left,
                        i.settle_s
                    );
                }
                let start = crate::host::process_cpu_ms(&i.process)?;
                let end = Instant::now() + Duration::from_secs(i.seconds);
                while Instant::now() < end {
                    self.check()?;
                    std::thread::sleep(Duration::from_millis(100));
                }
                let used = crate::host::process_cpu_ms(&i.process)? - start;
                // Which left-screen layout the app drew while measured (#618): stats samples
                // once a second, info redraws once a minute, the keyboard's own screen is none.
                let layout = crate::host::left_layout_now();
                let text = format!(
                    "{} used {used:.0} ms CPU in {} s ({:.2} % of one core; limit {} ms){settled}; \
                     left screen: {layout}",
                    i.process,
                    i.seconds,
                    used / (i.seconds as f64 * 10.0),
                    i.max_cpu_ms
                );
                ensure!(used <= i.max_cpu_ms as f64, "{text}");
                Ok(text)
            }
            Step::PressPosition(p) => self.press(p),
            Step::CaptureScreen(c) => {
                let image = self.capture(&c.half)?;
                let name = self.save_png(&format!("{}-screen", c.half), &image)?;
                self.baselines.insert(c.half.clone(), image);
                Ok(format!("saved {name}"))
            }
            Step::Screen(sc) => {
                let image = self.capture(&sc.half)?;
                let stem = Path::new(&sc.golden)
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| sc.half.clone());
                let path = self.golden_dir.join(&sc.golden);
                let Ok(golden) = std::fs::read(&path) else {
                    let name = self.save_png(&format!("{stem}.candidate"), &image)?;
                    return Err(Blocked(format!(
                        "no golden at {}; review {name} in the report and copy it there",
                        path.display()
                    ))
                    .into());
                };
                let c = screen::compare(&image, &golden, &sc.mask)?;
                let actual = self.save_png(&format!("{stem}.actual"), &image)?;
                let diff = format!("{}-{stem}.diff.png", self.case_name);
                std::fs::write(self.out.join(&diff), &c.diff)?;
                self.attachments.push(diff);
                ensure!(
                    c.different == 0,
                    "{} pixels differ from {} (see {actual} and the diff)",
                    c.different,
                    sc.golden
                );
                Ok(format!("matches {}", sc.golden))
            }
            Step::NoteRightRedraws(_) => {
                self.right_ready()?;
                // The first answer is the left's last read; it asks the right again, so the
                // next one is fresh. A redraw's second shot lands 1 s after its first (#621), so
                // the count must hold for REDRAW_SETTLE before it's noted (at most 5 tries).
                let _ = settings::battery_dim(self.device, None)?;
                let mut n = None;
                for _ in 0..5 {
                    self.check()?;
                    std::thread::sleep(REDRAW_SETTLE);
                    let now = self.right_redraws()?;
                    if n == Some(now) {
                        self.redraw_note = Some(now);
                        return Ok(format!("right redraws {now}"));
                    }
                    n = Some(now);
                }
                bail!("the right's redraw count kept changing: last {n:?}")
            }
            Step::ExpectRightRedraws(c) => {
                let base = self
                    .redraw_note
                    .ok_or_else(|| anyhow::anyhow!("no note_right_redraws earlier in the case"))?;
                let started = Instant::now();
                let end = started + Duration::from_millis(c.within_ms);
                loop {
                    self.check()?;
                    std::thread::sleep(Duration::from_millis(300));
                    let n = self.right_redraws()?;
                    if n >= base.saturating_add(c.at_least) {
                        return Ok(format!(
                            "right redraws {base} -> {n} after {} ms",
                            started.elapsed().as_millis()
                        ));
                    }
                    ensure!(
                        Instant::now() < end,
                        "right redraws {base} -> {n} within {} ms; expected at least +{}",
                        c.within_ms,
                        c.at_least
                    );
                }
            }
            Step::ExpectScreenStill(c) => {
                let first = match self.baselines.remove(&c.half) {
                    Some(image) => image,
                    None => self.capture(&c.half)?,
                };
                let started = Instant::now();
                let end = started + Duration::from_millis(c.ms);
                let mut captures = 0;
                let mut last = first.clone();
                while Instant::now() < end {
                    self.check()?;
                    std::thread::sleep(Duration::from_millis(250));
                    last = self.capture(&c.half)?;
                    captures += 1;
                    let n = screen::changed(&first, &last, c.region.as_ref());
                    if n > 0 {
                        self.save_png(&format!("{}-still-before", c.half), &first)?;
                        self.save_png(&format!("{}-still-after", c.half), &last)?;
                        bail!(
                            "{} screen changed ({n} pixels) after {} ms; expected it still for {} ms",
                            c.half,
                            started.elapsed().as_millis(),
                            c.ms
                        );
                    }
                }
                self.baselines.insert(c.half.clone(), last);
                Ok(format!(
                    "still for {} ms ({captures} captures)",
                    started.elapsed().as_millis()
                ))
            }
            Step::ExpectScreenChange(c) => {
                let first = match self.baselines.remove(&c.half) {
                    Some(image) => image,
                    None => self.capture(&c.half)?,
                };
                let started = Instant::now();
                let end = started + Duration::from_millis(c.within_ms);
                loop {
                    self.check()?;
                    let now = self.capture(&c.half)?;
                    let n = screen::changed(&first, &now, c.region.as_ref());
                    if n > 0 {
                        self.save_png(&format!("{}-before", c.half), &first)?;
                        self.save_png(&format!("{}-after", c.half), &now)?;
                        return Ok(format!(
                            "{n} pixels changed after {} ms",
                            started.elapsed().as_millis()
                        ));
                    }
                    ensure!(
                        Instant::now() < end,
                        "{} screen didn't change within {} ms",
                        c.half,
                        c.within_ms
                    );
                    std::thread::sleep(Duration::from_millis(250));
                }
            }
        }
    }
    pub fn case(&mut self, c: &Case, selected: bool) -> CaseResult {
        let started = Instant::now();
        self.case_name = c
            .name
            .chars()
            .map(|ch| {
                if ch.is_ascii_alphanumeric() || ch == '-' {
                    ch
                } else {
                    '_'
                }
            })
            .collect();
        let mut blocked = c
            .requires
            .iter()
            .filter_map(|r| unsupported(r, &self.hello))
            .chain(c.steps.iter().filter_map(|s| match s {
                Step::Screen(_)
                | Step::CaptureScreen(_)
                | Step::ExpectScreenChange(_)
                | Step::ExpectScreenStill(_) => unsupported("screen_readback", &self.hello),
                Step::NoteRightRedraws(_) | Step::ExpectRightRedraws(_) => {
                    unsupported("right_redraws", &self.hello)
                }
                Step::PressPosition(p) => {
                    unsupported("position_press", &self.hello).or_else(|| {
                        (p.expect_status == "refused")
                            .then(|| unsupported("invoke_refusal", &self.hello))
                            .flatten()
                    })
                }
                Step::TypeIntoSink(t)
                    if t.via.iter().any(|v| matches!(v, Step::PressPosition(_))) =>
                {
                    unsupported("position_press", &self.hello)
                }
                Step::Prompt(p) if self.non_interactive => {
                    Some(format!("manual prompt: {}", p.ask))
                }
                Step::ExpectConn(_) if self.hello.capabilities & protocol::CAP_CONN_STATE == 0 => {
                    Some("CONN_STATE capability missing".into())
                }
                Step::ExpectRgb(_)
                    if self.hello.capabilities & protocol::capabilities::SETTINGS_RGB == 0 =>
                {
                    Some("RGB_STATE capability missing".into())
                }
                Step::Invoke(i) if i.expect_status == "refused" => {
                    unsupported("invoke_refusal", &self.hello)
                }
                _ => None,
            }))
            .collect::<Vec<_>>();
        // Checked as the case starts, not in unsupported(): it's the PC, not the firmware.
        if c.requires.iter().any(|r| r == "unlocked") && (self.locked)() {
            blocked.push(
                "the PC is locked: Foxglove hands the left panel back while away (#410)".into(),
            );
        }
        if c.manual && self.non_interactive {
            blocked.push("manual case requires interactive confirmation".into());
        }
        let mut result = CaseResult {
            name: c.name.clone(),
            description: c.description.clone(),
            tags: c.tags.clone(),
            manual: c.manual || c.steps.iter().any(|s| matches!(s, Step::Prompt(_))),
            status: Status::Pass,
            elapsed_ms: 0,
            message: String::new(),
            steps: Vec::new(),
        };
        if !selected {
            result.status = Status::Skipped;
            result.message = "filtered by --only".into();
        } else if !blocked.is_empty() {
            result.status = Status::Blocked;
            result.message = blocked.join("; ");
        }
        let mut stop = result.status != Status::Pass;
        for s in &c.steps {
            let start = Instant::now();
            let (status, actual) = if stop {
                (
                    if result.status == Status::Blocked {
                        Status::Blocked
                    } else {
                        Status::Skipped
                    },
                    result.message.clone(),
                )
            } else {
                match self.execute(s) {
                    Ok(actual) => {
                        if matches!(s, Step::Prompt(_))
                            && !["y", "yes"].contains(&actual.trim().to_lowercase().as_str())
                        {
                            stop = true;
                            result.status = Status::Fail;
                            result.message = "manual check was not confirmed".into();
                            (Status::Fail, actual)
                        } else {
                            (Status::Pass, actual)
                        }
                    }
                    Err(e) => {
                        stop = true;
                        result.status = if e.downcast_ref::<Blocked>().is_some() {
                            Status::Blocked
                        } else {
                            Status::Fail
                        };
                        result.message = format!("{e:#}");
                        (result.status, result.message.clone())
                    }
                }
            };
            result.steps.push(StepResult {
                status,
                elapsed_ms: start.elapsed().as_millis(),
                expected: serde_json::to_value(s).unwrap(),
                actual,
                attachments: std::mem::take(&mut self.attachments),
            });
        }
        self.cleanup();
        if result.status == Status::Pass && result.manual {
            result.message = "manual, confirmed by user".into();
        }
        result.elapsed_ms = started.elapsed().as_millis();
        result
    }
}
impl<D> Drop for Runner<'_, D> {
    fn drop(&mut self) {
        if let Some(end) = self.held_until.take() {
            std::thread::sleep(end.saturating_duration_since(Instant::now()));
        }
    }
}
