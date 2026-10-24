//! Versioned, strict TOML test specifications.
use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};
use std::{collections::HashSet, path::Path};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Suite {
    pub schema: u32,
    #[serde(default)]
    pub include: Vec<String>,
    #[serde(default)]
    pub cases: Vec<Case>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Case {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub manual: bool,
    #[serde(default)]
    pub requires: Vec<String>,
    pub steps: Vec<Step>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Step {
    Invoke(Invoke),
    ExpectLayer(Layer),
    ExpectConn(Conn),
    ExpectRgb(Rgb),
    TypeIntoSink(Typing),
    Prompt(Prompt),
    Screen(Screen),
    PressPosition(Position),
    ArtSelect(Art),
    UsbHalves(Usb),
    IdleCpu(Idle),
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Invoke {
    pub behavior: Option<String>,
    pub local_id: Option<u16>,
    #[serde(default)]
    pub param1: u32,
    #[serde(default)]
    pub param2: u32,
    #[serde(default)]
    pub hold_ms: u16,
    #[serde(default = "ok")]
    pub expect_status: String,
}
fn ok() -> String {
    "ok".into()
}
fn timeout() -> u64 {
    500
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Layer {
    #[serde(default)]
    pub active_contains: Vec<u8>,
    pub top: Option<u8>,
    #[serde(default = "timeout")]
    pub within_ms: u64,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Conn {
    pub usb: Option<bool>,
    pub ble_connected: Option<bool>,
    pub right_link: Option<bool>,
    #[serde(default)]
    pub usb_or_ble: bool,
    #[serde(default = "timeout")]
    pub within_ms: u64,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Rgb {
    pub right_age_max_s: u8,
    #[serde(default = "timeout")]
    pub within_ms: u64,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Typing {
    pub text: String,
    pub via: Vec<Step>,
    pub expect: String,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Prompt {
    pub ask: String,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Screen {
    pub half: String,
    pub golden: String,
    #[serde(default)]
    pub mask: Vec<Rect>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}
/// Shows built-in art set `set` on the right half through the relay, checks the right reports
/// it, then puts the previous built-in set back. Pushed art lives in RAM and can't be restored,
/// so the step refuses to replace it unless `replace_pushed` is set.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Art {
    pub set: i8,
    #[serde(default = "yes")]
    pub restore: bool,
    #[serde(default)]
    pub replace_pushed: bool,
    #[serde(default = "art_timeout")]
    pub within_ms: u64,
}
fn yes() -> bool {
    true
}
fn art_timeout() -> u64 {
    10_000
}
/// Fails on any half whose USB picture means trouble (see host::classify).
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Usb {
    /// Also fail when a half is in the bootloader or the left is off USB.
    #[serde(default)]
    pub strict: bool,
}
/// Measures a process's CPU time over `seconds` while nothing is pressed.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Idle {
    #[serde(default = "foxglove")]
    pub process: String,
    pub seconds: u64,
    pub max_cpu_ms: u64,
}
fn foxglove() -> String {
    "foxglove".into()
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Position {
    pub pos: u8,
    pub state: String,
}

impl Invoke {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.behavior.is_some() != self.local_id.is_some(),
            "invoke needs exactly one of behavior/local_id"
        );
        ensure!(
            self.behavior.as_ref().is_none_or(|s| !s.is_empty()),
            "empty behavior"
        );
        ensure!(self.hold_ms <= 10000, "hold_ms exceeds 10 seconds");
        ensure!(
            [
                "ok",
                "unknown_id",
                "busy",
                "failed",
                "not_built_in",
                "bad_length",
                "refused"
            ]
            .contains(&self.expect_status.as_str()),
            "unknown expect_status"
        );
        Ok(())
    }
}
impl Step {
    pub fn validate(&self) -> Result<()> {
        let wait = |ms| -> Result<()> {
            ensure!((1..=30000).contains(&ms), "within_ms must be 1..30000");
            Ok(())
        };
        match self {
            Self::Invoke(i) => i.validate()?,
            Self::ExpectLayer(l) => {
                wait(l.within_ms)?;
                ensure!(
                    l.top.is_some() || !l.active_contains.is_empty(),
                    "empty layer expectation"
                );
                ensure!(
                    l.top.is_none_or(|n| n < 32) && l.active_contains.iter().all(|n| *n < 32),
                    "layer out of range"
                );
            }
            Self::ExpectConn(c) => {
                wait(c.within_ms)?;
                ensure!(
                    c.usb.is_some()
                        || c.ble_connected.is_some()
                        || c.right_link.is_some()
                        || c.usb_or_ble,
                    "empty connection expectation"
                );
            }
            Self::ExpectRgb(r) => {
                wait(r.within_ms)?;
                ensure!(r.right_age_max_s < 255, "invalid RGB age");
            }
            Self::ArtSelect(a) => {
                wait(a.within_ms)?;
                ensure!(a.set >= 0, "art_select needs a built-in set (0 or more)");
            }
            Self::UsbHalves(_) => {}
            Self::IdleCpu(i) => {
                ensure!(
                    (1..=600).contains(&i.seconds),
                    "idle_cpu seconds must be 1..=600"
                );
                ensure!(!i.process.is_empty(), "idle_cpu needs a process name");
            }
            Self::TypeIntoSink(t) => {
                ensure!(!t.via.is_empty(), "typing needs via invokes");
                for s in &t.via {
                    let Self::Invoke(i) = s else {
                        anyhow::bail!("via accepts only invoke steps")
                    };
                    i.validate()?;
                    ensure!(
                        i.hold_ms == 0 && i.expect_status == "ok",
                        "sink accepts successful taps only"
                    );
                }
            }
            Self::Prompt(p) => ensure!(!p.ask.trim().is_empty(), "empty prompt"),
            Self::Screen(s) => {
                ensure!(["left", "right"].contains(&s.half.as_str()), "invalid half");
                ensure!(
                    !s.golden.is_empty()
                        && Path::new(&s.golden).extension().is_some_and(|x| x == "png"),
                    "golden must be a PNG path"
                );
                for r in &s.mask {
                    ensure!(
                        r.width > 0
                            && r.height > 0
                            && r.x.checked_add(r.width).is_some_and(|x| x <= 160)
                            && r.y.checked_add(r.height).is_some_and(|y| y <= 68),
                        "mask outside 160x68"
                    );
                }
            }
            Self::PressPosition(p) => ensure!(
                p.pos < 48 && ["up", "down"].contains(&p.state.as_str()),
                "invalid position/state"
            ),
        }
        Ok(())
    }
}

/// Loads includes relative to their suite, rejecting cycles and duplicate names.
pub fn load(path: &Path) -> Result<Suite> {
    fn read(path: &Path, stack: &mut HashSet<std::path::PathBuf>) -> Result<Suite> {
        let path = path.canonicalize()?;
        ensure!(stack.insert(path.clone()), "suite include cycle");
        let mut s: Suite = toml::from_str(&std::fs::read_to_string(&path)?)?;
        ensure!(s.schema == 1, "unsupported schema {}", s.schema);
        let mut cases = Vec::new();
        for include in &s.include {
            cases.extend(read(&path.parent().unwrap().join(include), stack)?.cases);
        }
        cases.append(&mut s.cases);
        s.cases = cases;
        stack.remove(&path);
        Ok(s)
    }
    let s = read(path, &mut HashSet::new())?;
    let mut names = HashSet::new();
    ensure!(!s.cases.is_empty(), "empty suite");
    for c in &s.cases {
        ensure!(
            !c.name.trim().is_empty() && names.insert(&c.name),
            "empty/duplicate case name"
        );
        ensure!(!c.steps.is_empty(), "empty case {}", c.name);
        ensure!(
            !c.manual || c.steps.iter().any(|s| matches!(s, Step::Prompt(_))),
            "manual case {} needs a prompt",
            c.name
        );
        for r in &c.requires {
            ensure!(
                ["position_press", "screen_readback", "invoke_refusal"].contains(&r.as_str())
                    || r.strip_prefix("host_link>=")
                        .is_some_and(|s| s.parse::<u8>().is_ok()),
                "unknown requirement {r}"
            );
        }
        for step in &c.steps {
            step.validate()?;
        }
    }
    Ok(s)
}
