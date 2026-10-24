//! Bounded execution. Unknown firmware features fail closed before any injection.
use crate::{
    behaviors::Catalog,
    report::{CaseResult, Status, StepResult},
    spec::{Case, Invoke, Step},
};
use anyhow::{bail, ensure, Result};
use digitalis::{
    link::{Link, Subscription},
    panel,
    protocol::{self, Hello, InvokeStatus},
    settings, Transport,
};
use std::{
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

pub trait Device: Transport {
    fn layer(&self, timeout: Duration) -> Result<Option<u32>>;
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

/// Current HELLO has no trustworthy refusal, position, or screen capability.
/// Do not infer these from CONN_STATE support or from the unchanged v1 number.
pub fn unsupported(requirement: &str, hello: &Hello) -> Option<String> {
    let reason = match requirement {
        "invoke_refusal" => {
            "INVOKE refusal cannot be proven by current HELLO; unsafe request was not sent"
        }
        "screen_readback" => "needs screen readback #67",
        "position_press" => "needs position press #78",
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
    held_until: Option<Instant>,
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
            held_until: None,
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
        ensure!(if typing { known("kp") && i.hold_ms == 0 } else { safe }, "injection blocked: needs verified non-typing behavior, or kp tap inside sink; refusal support unknown");
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
    /// Waits out the bounded firmware release even when cancelled; no unsupported release command.
    fn cleanup(&mut self) {
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
                let s = settings::rgb_state(self.device)?;
                Ok((
                    s.right.is_some_and(|(_, age)| age <= r.right_age_max_s),
                    format!("{s:?}"),
                ))
            }),
            Step::Prompt(p) => crate::platform::prompt(&p.ask, &self.cancelled),
            Step::TypeIntoSink(t) => {
                // Preflight everything before opening the sink or issuing a key.
                for s in &t.via {
                    let Step::Invoke(i) = s else {
                        bail!("non-invoke in sink")
                    };
                    let id = self.resolve(i)?;
                    ensure!(
                        self.catalog.ids.get("kp") == Some(&id),
                        "sink accepts kp only"
                    );
                }
                let actual =
                    crate::platform::sink(&t.text, &t.via, &self.cancelled.clone(), |i| {
                        self.invoke(i, true).map(|_| ())
                    })?;
                ensure!(
                    actual == t.expect,
                    "expected {:?}, actual {:?}",
                    t.expect,
                    actual
                );
                Ok(format!("received {actual:?}"))
            }
            Step::Screen(_) => bail!("needs screen readback #67"),
            Step::PressPosition(_) => bail!("needs position press #78"),
        }
    }
    pub fn case(&mut self, c: &Case, selected: bool) -> CaseResult {
        let started = Instant::now();
        let mut blocked = c
            .requires
            .iter()
            .filter_map(|r| unsupported(r, &self.hello))
            .chain(c.steps.iter().filter_map(|s| match s {
                Step::Screen(_) => Some("needs screen readback #67".into()),
                Step::PressPosition(_) => Some("needs position press #78".into()),
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
                        result.status = Status::Fail;
                        result.message = format!("{e:#}");
                        (Status::Fail, result.message.clone())
                    }
                }
            };
            result.steps.push(StepResult {
                status,
                elapsed_ms: start.elapsed().as_millis(),
                expected: serde_json::to_value(s).unwrap(),
                actual,
                attachments: Vec::new(),
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
