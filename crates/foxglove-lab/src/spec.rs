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
    CaptureScreen(Capture),
    ExpectScreenChange(ScreenChange),
    ExpectScreenStill(ScreenStill),
    PanelDump(PanelDump),
    ScreensSetting(ScreensSetting),
    ArtSelect(Art),
    UsbHalves(Usb),
    ExpectBuild(BuildExpect),
    ExpectHello(HelloExpect),
    ExpectNotBuiltIn(NotBuiltIn),
    SplitGuardTest(GuardTest),
    IdleCpu(Idle),
    ArtPush(ArtPush),
    PanelShow(PanelShow),
    PanelFlood(PanelFlood),
    PanelRelease(PanelRelease),
    ArtKey(ArtKey),
    NoteRightRedraws(NoteRedraws),
    ExpectRightRedraws(RightRedraws),
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
    /// RGB_STATE's on/off for the left and the right, when set (#603). The right's comes from
    /// the left's last read of it, so it's only as fresh as `right_age_max_s` allows.
    pub left_on: Option<bool>,
    pub right_on: Option<bool>,
    /// Both halves' on/off must come from what their LEDs show (HELLO bit 24, #605): a half the
    /// keyboard can't say that for fails the step.
    #[serde(default)]
    pub shown: bool,
    /// The left's stored look must be this (#624).
    pub left_look: Option<Look>,
    /// The right's stored hue, saturation, brightness and effect must equal the left's (#624).
    #[serde(default)]
    pub right_same_look: bool,
    #[serde(default = "timeout")]
    pub within_ms: u64,
}
/// A half's stored underglow look, as RGB_STATE reports it.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Look {
    pub hue: u16,
    pub sat: u8,
    pub brightness: u8,
    pub effect: u8,
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
/// Compares the frame the app last sent (its FOXGLOVE_DUMP_PANEL file) with the left screen
/// readback: `shown = true` expects the frame somewhere on the screen pixel for pixel,
/// `shown = false` expects it gone (the firmware drawing that area again).
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PanelDump {
    pub path: String,
    #[serde(default = "yes")]
    pub shown: bool,
    #[serde(default = "art_timeout")]
    pub within_ms: u64,
}
/// Sets one key of the [screens] table in the app's foxglove.toml (inside FOXGLOVE_CONFIG_DIR),
/// which the screens service follows live. The original file is put back at the end of the case.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ScreensSetting {
    pub key: String,
    pub value: String,
}
/// A half's running build (D7 09 BUILD_INFO); unset fields aren't checked.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BuildExpect {
    pub half: String,
    pub profile: Option<String>,
    pub version: Option<String>,
    pub sha: Option<String>,
}
/// HELLO capability bits that must be set or clear.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HelloExpect {
    #[serde(default)]
    pub caps_set: Vec<u8>,
    #[serde(default)]
    pub caps_clear: Vec<u8>,
    /// The left's build profile from HELLO byte 30 ("dev" or "ga").
    pub profile: Option<String>,
    /// HELLO byte 31: split-guard link drops since boot (#147) must not exceed this.
    pub guard_drops_max: Option<u8>,
}
/// A test-hook command must answer status 4 (not built in), as in GA builds. Only harmless
/// requests are sent: POSITION releases Q (pos 1, state up), BOOTLOADER is ARM only (no
/// TRIGGER, so nothing reboots even on a dev build).
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct NotBuiltIn {
    pub command: String,
}
/// D7 0B GUARD_TEST (#184, dev builds): the #122 split guard drops the current split link once.
/// Passes when HELLO byte 31 (guard drops since boot) goes up by exactly one and the right's
/// relay is back (right link 2) within `within_ms`.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GuardTest {
    #[serde(default = "guard_timeout")]
    pub within_ms: u64,
    /// Sent once while HELLO shows the split link down (right link 0), so only the left takes it
    /// (#624: the right misses a change). BLOCKED when the link never reads as down.
    pub while_down: Option<Invoke>,
}
fn guard_timeout() -> u64 {
    60_000
}
/// Fails on any half whose USB picture means trouble (see host::classify).
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Usb {
    /// Also fail when a half is in the bootloader or the left is off USB.
    #[serde(default)]
    pub strict: bool,
}
/// A D6 art push as a misbehaving PC would send it (docs/resilience.md, #25), then the art STATUS
/// it should leave: `cut` "data" (BEGIN and a few DATA, then silence) or "commit" (silence after
/// COMMIT); `corrupt` "crc" (BEGIN announces a wrong CRC) or "header" (a width byte the right
/// rejects); `drop_link` drops the split link mid-relay (D7 0B, dev builds).
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ArtPush {
    /// A blob file, relative to the repository root.
    pub blob: String,
    #[serde(default)]
    pub cut: String,
    #[serde(default)]
    pub corrupt: String,
    #[serde(default)]
    pub drop_link: bool,
    pub expect_left_state: u8,
    /// Accepted left errors; empty = any.
    #[serde(default)]
    pub expect_errors: Vec<u8>,
    pub expect_right_state: Option<u8>,
    /// The right must still show the set it showed before this push.
    #[serde(default)]
    pub right_keeps_set: bool,
    #[serde(default = "push_timeout")]
    pub within_ms: u64,
}
fn push_timeout() -> u64 {
    30_000
}
/// Shows test frame `pattern` on the left panel (ROWS + PRESENT, as the app does) and saves it as a
/// PBM at `save` for panel_dump.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PanelShow {
    pub pattern: u8,
    pub save: String,
}
/// Panel frames at `rate` a second (default 100: the app's 2 Hz stream ×50) for `seconds`, with a
/// layer key pressed and checked every 2 s (no dropped, doubled or stuck key). Saves the last
/// frame at `save`.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PanelFlood {
    pub seconds: u64,
    pub save: String,
    #[serde(default = "flood_rate")]
    pub rate: u32,
}
fn flood_rate() -> u32 {
    100
}
/// One art key press with SYSTEM held (#302): `pos` 11 (P, &art_next) or 10 (O, &art_prev).
/// Without `subscribed` the board steps the right-screen art itself; with it, D7 0D 01 goes first
/// and a D7 8D 01 <dir> event must arrive while the art stays put (the opt-in is ended after).
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ArtKey {
    pub pos: u8,
    #[serde(default)]
    pub subscribed: bool,
    /// Leave the opt-in on afterwards (to check that a USB unplug ends it).
    #[serde(default)]
    pub keep_opt_in: bool,
}
/// PANEL RELEASE: the firmware draws its own panel again.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PanelRelease {}
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
    /// "down", "up", or "tap" (down, hold_ms, up).
    pub state: String,
    #[serde(default = "tap_ms")]
    pub hold_ms: u16,
    #[serde(default = "ok")]
    pub expect_status: String,
}
fn tap_ms() -> u16 {
    40
}
/// Saves a capture of one half's screen (portrait, as a person sees it) to the report.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Capture {
    pub half: String,
}
/// Keeps capturing until the screen differs inside `region` (portrait 68x160 coordinates;
/// whole screen when absent) from the case's last capture_screen of that half, or from a
/// capture taken at the start of this step when there is none.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ScreenChange {
    pub half: String,
    pub region: Option<Rect>,
    pub within_ms: u64,
}

/// Captures one half's screen for `ms` and fails on any change inside `region` (portrait 68x160
/// coordinates; whole screen when absent): an animation that should be paused (#603). Compares
/// with the case's last capture_screen of that half, or a capture taken at the start, and leaves
/// its last capture as the baseline for a following expect_screen_change.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ScreenStill {
    pub half: String,
    pub region: Option<Rect>,
    pub ms: u64,
}

/// Notes the right half's whole-screen rewrites since its power-on (D7 8E bytes 16-20, HELLO bit
/// 24, #618) for a following expect_right_redraws. Readback is LVGL's model, so it can't show
/// panel noise after a power cut; this count shows the panel was rewritten.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct NoteRedraws {}

/// The right's redraw count grew by at least `at_least` since the case's note_right_redraws,
/// within `within_ms` (the D7 0E read is asked every 300 ms; each answer is the left's last read
/// of the right, so allow a second for it).
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RightRedraws {
    #[serde(default = "one")]
    pub at_least: u32,
    #[serde(default = "redraw_timeout")]
    pub within_ms: u64,
}
fn one() -> u32 {
    1
}
fn redraw_timeout() -> u64 {
    5000
}

/// F13–F24 as `&kp` params (HID keyboard page 7, usages 0x68–0x73, no modifiers). They type
/// nothing, so a short tap is safe outside the typing sink: it's how a test presses a key bound
/// to a plugin action (#212).
pub fn non_typing_key(param1: u32) -> bool {
    (0x0007_0068..=0x0007_0073).contains(&param1)
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
                // Up to 2 minutes: long enough to wait out the keyboard's 30 s idle (#603).
                ensure!(
                    (1..=120_000).contains(&r.within_ms),
                    "expect_rgb within_ms must be 1..=120000"
                );
                ensure!(r.right_age_max_s < 255, "invalid RGB age");
                if let Some(l) = r.left_look {
                    ensure!(
                        l.hue < 360 && l.sat <= 100 && l.brightness <= 100 && l.effect <= 3,
                        "left_look: hue 0-359, sat and brightness 0-100, effect 0-3"
                    );
                }
            }
            Self::ArtSelect(a) => {
                wait(a.within_ms)?;
                ensure!(a.set >= 0, "art_select needs a built-in set (0 or more)");
            }
            Self::UsbHalves(_) => {}
            Self::ExpectBuild(b) => {
                ensure!(["left", "right"].contains(&b.half.as_str()), "invalid half");
                ensure!(
                    b.profile.as_deref().is_none_or(|p| p == "dev" || p == "ga"),
                    "profile must be dev or ga"
                );
            }
            Self::ExpectHello(h) => ensure!(
                h.caps_set.iter().chain(&h.caps_clear).all(|b| *b < 32)
                    && !(h.caps_set.is_empty()
                        && h.caps_clear.is_empty()
                        && h.profile.is_none()
                        && h.guard_drops_max.is_none()),
                "expect_hello needs capability bits 0..31"
            ),
            Self::SplitGuardTest(g) => {
                ensure!(
                    (5_000..=120_000).contains(&g.within_ms),
                    "split_guard_test within_ms must be 5000..=120000"
                );
                if let Some(i) = &g.while_down {
                    i.validate()?;
                }
            }
            Self::ExpectNotBuiltIn(n) => ensure!(
                ["position", "bootloader"].contains(&n.command.as_str()),
                "expect_not_built_in supports position and bootloader"
            ),
            Self::PanelDump(d) => {
                wait(d.within_ms)?;
                ensure!(!d.path.is_empty(), "panel_dump needs the dump path");
            }
            Self::ScreensSetting(s) => ensure!(
                ["drive_left", "fps", "layout"].contains(&s.key.as_str()),
                "screens_setting supports drive_left, fps and layout"
            ),
            Self::ArtPush(a) => {
                ensure!(
                    ["", "data", "commit"].contains(&a.cut.as_str()),
                    "art_push cut is data or commit"
                );
                ensure!(
                    ["", "crc", "header"].contains(&a.corrupt.as_str()),
                    "art_push corrupt is crc or header"
                );
                ensure!(a.expect_left_state <= 4, "left states are 0..=4");
                ensure!(
                    (1_000..=180_000).contains(&a.within_ms),
                    "art_push within_ms must be 1000..=180000"
                );
            }
            Self::PanelFlood(f) => ensure!(
                (1..=300).contains(&f.seconds),
                "panel_flood seconds must be 1..=300"
            ),
            Self::PanelShow(_) | Self::PanelRelease(_) => {}
            Self::ArtKey(k) => ensure!(
                [10, 11].contains(&k.pos),
                "art_key pos is 11 (P, next) or 10 (O, previous)"
            ),
            Self::IdleCpu(i) => {
                ensure!(
                    (1..=600).contains(&i.seconds),
                    "idle_cpu seconds must be 1..=600"
                );
                ensure!(!i.process.is_empty(), "idle_cpu needs a process name");
            }
            Self::TypeIntoSink(t) => {
                ensure!(!t.via.is_empty(), "typing needs via steps");
                for s in &t.via {
                    match s {
                        Self::Invoke(i) => {
                            i.validate()?;
                            ensure!(
                                i.hold_ms == 0 && i.expect_status == "ok",
                                "sink accepts successful taps only"
                            );
                        }
                        Self::PressPosition(p) => {
                            s.validate()?;
                            ensure!(
                                p.state == "tap" && p.expect_status == "ok",
                                "sink accepts successful position taps only"
                            );
                        }
                        _ => anyhow::bail!("via accepts only invoke and press_position taps"),
                    }
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
                            && r.x.checked_add(r.width).is_some_and(|x| x <= 68)
                            && r.y.checked_add(r.height).is_some_and(|y| y <= 160),
                        "mask outside the portrait 68x160 screen"
                    );
                }
            }
            Self::PressPosition(p) => {
                ensure!(
                    p.pos < 48 && ["up", "down", "tap"].contains(&p.state.as_str()),
                    "invalid position/state"
                );
                ensure!(
                    p.hold_ms <= 2000,
                    "position tap hold_ms must be at most 2000"
                );
                ensure!(
                    ["ok", "refused"].contains(&p.expect_status.as_str()),
                    "press_position expect_status must be ok or refused"
                );
            }
            Self::CaptureScreen(c) => {
                ensure!(["left", "right"].contains(&c.half.as_str()), "invalid half")
            }
            Self::ExpectScreenChange(c) => {
                ensure!(["left", "right"].contains(&c.half.as_str()), "invalid half");
                ensure!(
                    (1..=120_000).contains(&c.within_ms),
                    "screen change within_ms must be 1..=120000"
                );
                if let Some(r) = &c.region {
                    ensure!(
                        r.width > 0 && r.height > 0 && r.x + r.width <= 68 && r.y + r.height <= 160,
                        "region outside the portrait 68x160 screen"
                    );
                }
            }
            Self::NoteRightRedraws(_) => {}
            Self::ExpectRightRedraws(r) => {
                ensure!(
                    (1..=60_000).contains(&r.within_ms),
                    "expect_right_redraws within_ms must be 1..=60000"
                );
                ensure!(
                    r.at_least >= 1,
                    "expect_right_redraws at_least must be 1 or more"
                );
            }
            Self::ExpectScreenStill(c) => {
                ensure!(["left", "right"].contains(&c.half.as_str()), "invalid half");
                ensure!(
                    (1..=120_000).contains(&c.ms),
                    "screen still ms must be 1..=120000"
                );
                if let Some(r) = &c.region {
                    ensure!(
                        r.width > 0 && r.height > 0 && r.x + r.width <= 68 && r.y + r.height <= 160,
                        "region outside the portrait 68x160 screen"
                    );
                }
            }
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
                [
                    "position_press",
                    "art_keys",
                    "screen_readback",
                    "invoke_refusal",
                    "build_info",
                    "unlocked",
                    "activity_relay",
                    "rgb_shown",
                    "right_redraws"
                ]
                .contains(&r.as_str())
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
