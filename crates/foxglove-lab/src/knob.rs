//! The knob end to end (#719): the PC's hold of the keymap's `&knob` / `&knob_press` (D7 14 KNOB,
//! HELLO bit 31; firmware host_link.c `knob_command`, knob.c), as cases with a clear pass, fail
//! or blocked, for the test after a batch flash. `foxglove-lab health` runs them after the
//! self-report; `foxglove-lab knob` runs them alone.
//!
//! The cases, in order (each reads the D7 94 reply: status, held, target, the keymap's parts):
//! - Read: the hold as found (D7 14 FF); held with no app running is a hold a dead app left.
//! - Hold: held, on master (D7 14 01 00).
//! - Target app: held, on an app (D7 14 01 01).
//! - Health while held: the left's self-report (D7 13, block v2 bytes 96-103) says held, on an
//!   app, with the parts the KNOB reply named.
//! - Back to master: held, on master again.
//! - Bad request refused: hold byte 2 (D7 14 02 00) gets status 5; the hold stays as it was.
//! - Release: not held (D7 14 00 00), and a fresh read says the same: the board has the knob back.
//! - Health row: the block says not held and no hold dropped during the case, and the health
//!   view's "Knob" row says so.
//!
//! Not lab-driven: turning the knob, a click, a double click and a hold need a hand on it. They
//! are one HUMAN line (a `Not tested` warning in `health`), never faked. The app's side (the
//! sessions, the popup) is the app's own tests.
//!
//! Every request waits at most the link's 500 ms. The time on an app is the health read; the
//! release runs at the end whatever happened (a drop guard), so the knob is the volume keys and
//! mute again. A lab killed mid-run leaves the hold until the next USB disconnect or endpoint
//! change, or until the user's Foxglove holds it again on its next HELLO.

use crate::health::{row_line, Outcome, BLOCKED, FAIL, PASS};
use foxglove_link::health::{self as kh, knob_flags, HealthRow, Level, Report};
use foxglove_link::protocol::{
    self, capabilities,
    knob::{self, State, Target},
};
use foxglove_link::Transport;
use std::time::Duration;

/// The rows' group.
pub const GROUP: &str = "knob";

/// Each request's wait: the link's own.
const WAIT: Duration = Duration::from_millis(500);
/// Replies re-read when a knob event (a hand on the knob) came instead.
const EVENT_RETRIES: usize = 3;

/// What only a person can check, as the HUMAN line says it.
pub const HUMAN: &str = "turns, click, double click and hold need a hand on the knob: on master \
the turns are Windows' volume; held on an app a click goes to the next app, a hold of 0.5 s \
back to master (a sitting row, not lab-driven)";

/// What the cases found: their rows, what couldn't be tested and why, and what needs a person.
#[derive(Clone, Debug, Default)]
pub struct Cases {
    pub rows: Vec<HealthRow>,
    pub blocked: Vec<String>,
    pub human: Vec<String>,
}

impl Cases {
    /// One reason for the whole set (no keyboard, the app holds it).
    pub fn blocked_by(why: impl Into<String>) -> Cases {
        Cases {
            blocked: vec![why.into()],
            ..Cases::default()
        }
    }

    /// Exit code: 1 when a case failed, else 2 when something couldn't be tested, else 0. The
    /// human line is neither: the lab can't do it, by design.
    pub fn code(&self) -> u8 {
        if kh::worst(&self.rows) == Level::Fail {
            FAIL
        } else if !self.blocked.is_empty() {
            BLOCKED
        } else {
            PASS
        }
    }

    /// One line per row as `health` prints them, one per untested set and per human check, then
    /// the result.
    pub fn lines(&self) -> Vec<String> {
        let mut out: Vec<String> = self.rows.iter().map(row_line).collect();
        out.extend(
            self.blocked
                .iter()
                .map(|why| format!("BLOCK {GROUP:<10} {why}")),
        );
        out.extend(
            self.human
                .iter()
                .map(|what| format!("HUMAN {GROUP:<10} {what}")),
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

    /// As JSON, in `health --json`'s shape, with `blocked` and `human` lists.
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
        v["human"] = self.human.clone().into();
        v
    }

    /// Into `health`'s result: the rows as they are; each untested set and each human check a
    /// `Not tested` warning, so a build without the knob still passes. Failures fail it.
    pub fn fold_into(self, out: &mut Outcome) {
        out.rows.extend(self.rows);
        out.rows.extend(
            self.blocked
                .into_iter()
                .map(|why| HealthRow::new(GROUP, "Not tested", Level::Warn, why)),
        );
        out.rows.extend(self.human.into_iter().map(|what| {
            HealthRow::new(GROUP, "Not tested", Level::Warn, format!("by hand: {what}"))
        }));
    }
}

/// Runs the cases on `t`; `caps` are HELLO's.
pub fn run(t: &impl Transport, caps: u32) -> Cases {
    let mut cases = Cases::default();
    if caps & capabilities::KNOB == 0 {
        cases.blocked.push(
            "KNOB not tested: no HELLO bit 31 (no &knob or &knob_press in this build)".into(),
        );
        return cases;
    }
    let health = caps & capabilities::HEALTH != 0;
    if !health {
        cases
            .blocked
            .push("the knob's health row not tested: no HELLO bit 28 (no self-report)".into());
    }
    cases.human.push(HUMAN.into());
    let _release = Release { t };
    let mut r = Run { t, cases, health };
    r.cases_in_order();
    r.cases
}

/// The release, whatever the cases did: the knob is the volume keys and mute again.
struct Release<'a, T: Transport> {
    t: &'a T,
}

impl<T: Transport> Drop for Release<'_, T> {
    fn drop(&mut self) {
        let _ = self.t.request(
            &knob::release(),
            protocol::SETTINGS,
            protocol::settings::KNOB,
            WAIT,
        );
    }
}

struct Run<'a, T: Transport> {
    t: &'a T,
    cases: Cases,
    health: bool,
}

fn status_text(s: u8) -> &'static str {
    match s {
        0 => "ok",
        4 => "not built in",
        5 => "bad request",
        _ => "unknown",
    }
}

fn target_text(t: Target) -> &'static str {
    match t {
        Target::Master => "master",
        Target::App => "an app",
    }
}

fn parts_text(knobs: u8) -> &'static str {
    match knobs & (knob::HAS_TURN | knob::HAS_PRESS) {
        3 => "the turn and the click",
        1 => "the turn only",
        2 => "the click only",
        _ => "no knob parts",
    }
}

fn hold_text(s: &State) -> String {
    if s.held {
        format!("held, on {}", target_text(s.target))
    } else {
        "not held (the volume keys and mute)".into()
    }
}

/// The block's knob byte as a [`State`]'s hold.
fn block_hold(flags: u8) -> (bool, Target) {
    let target = if flags & knob_flags::ON_APP != 0 {
        Target::App
    } else {
        Target::Master
    };
    (flags & knob_flags::HELD != 0, target)
}

impl<T: Transport> Run<'_, T> {
    fn row(&mut self, item: &str, level: Level, detail: impl Into<String>) {
        self.cases
            .rows
            .push(HealthRow::new(GROUP, item, level, detail));
    }

    /// One KNOB request and its reply. A knob event in its place (someone touched the knob;
    /// events share D7 94) is skipped and the hold read again, a bounded number of times.
    fn ask(&self, report: &[u8]) -> Result<State, String> {
        let get = |r: &[u8]| {
            self.t
                .request(r, protocol::SETTINGS, protocol::settings::KNOB, WAIT)
                .map_err(|e| format!("no reply to KNOB: {e:#}"))
        };
        let mut reply = get(report)?;
        for _ in 0..EVENT_RETRIES {
            if let Some(s) = knob::state(&reply) {
                return Ok(s);
            }
            if knob::event(&reply).is_none() {
                return Err(format!("not a KNOB reply: {:02x?}", &reply[..7]));
            }
            reply = get(&knob::read())?;
        }
        knob::state(&reply).ok_or_else(|| {
            "only knob events came back: is someone turning or clicking the knob?".into()
        })
    }

    /// A hold request's reply as a row: status 0, then the hold `want` (held, target). Whether
    /// it holds as asked.
    fn hold_row(&mut self, item: &str, report: &[u8], want: (bool, Target)) -> bool {
        let s = match self.ask(report) {
            Err(e) => {
                self.row(item, Level::Fail, e);
                return false;
            }
            Ok(s) => s,
        };
        if s.status != 0 {
            self.row(
                item,
                Level::Fail,
                format!("KNOB status {} ({})", s.status, status_text(s.status)),
            );
            return false;
        }
        let want_s = State {
            held: want.0,
            target: want.1,
            ..s
        };
        if (s.held, s.target) != want || (!s.held && s.target != Target::Master) {
            self.row(
                item,
                Level::Fail,
                format!(
                    "KNOB ok but {} (want {})",
                    hold_text(&s),
                    hold_text(&want_s)
                ),
            );
            return false;
        }
        self.row(item, Level::Ok, hold_text(&s));
        true
    }

    fn cases_in_order(&mut self) {
        // As found. HELLO bit 31 is set only with a knob part in the keymap, so status 4 is the
        // firmware contradicting itself.
        let start = match self.ask(&knob::read()) {
            Err(e) => return self.row("Read", Level::Fail, e),
            Ok(s) if s.status != 0 => {
                return self.row(
                    "Read",
                    Level::Fail,
                    format!(
                        "KNOB read status {} ({}) though HELLO says KNOB",
                        s.status,
                        status_text(s.status)
                    ),
                )
            }
            Ok(s) if s.knobs & (knob::HAS_TURN | knob::HAS_PRESS) == 0 => {
                return self.row(
                    "Read",
                    Level::Fail,
                    "KNOB says the keymap has no knob parts though HELLO says KNOB",
                )
            }
            Ok(s) => s,
        };
        let parts = parts_text(start.knobs);
        if start.held {
            self.row(
                "Read",
                Level::Warn,
                format!(
                    "{} with no app running: a hold an app left behind; the keymap has {parts}",
                    hold_text(&start)
                ),
            );
        } else {
            self.row(
                "Read",
                Level::Ok,
                format!("{}; the keymap has {parts}", hold_text(&start)),
            );
        }
        // The block before the hold, for its drop count.
        let before = self.block("Health while held");

        if !self.hold_row("Hold", &knob::hold(Target::Master), (true, Target::Master)) {
            return self.release(start.knobs, before);
        }
        if self.hold_row("Target app", &knob::hold(Target::App), (true, Target::App)) {
            self.health_held(start.knobs);
        }
        if self.hold_row(
            "Back to master",
            &knob::hold(Target::Master),
            (true, Target::Master),
        ) {
            self.bad_request();
        }
        self.release(start.knobs, before);
    }

    /// The left's block, `None` without the self-report or when it can't be read (a FAIL row on
    /// `item`).
    fn block(&mut self, item: &str) -> Option<kh::Block> {
        if !self.health {
            return None;
        }
        match kh::read(self.t, 0) {
            Ok(b) => Some(b),
            Err(e) => {
                self.row(
                    item,
                    Level::Fail,
                    format!("the left's self-report couldn't be read: {e}"),
                );
                None
            }
        }
    }

    /// The block's knob part, or a FAIL row on `item`.
    fn knob_part(&mut self, item: &str, b: &kh::Block) -> Option<kh::Knob> {
        if b.knob.is_none() {
            self.row(
                item,
                Level::Fail,
                format!(
                    "the left's self-report (version {}) has no knob part (bytes 96-103, version 2)",
                    b.version
                ),
            );
        }
        b.knob
    }

    /// The parts the block names against the KNOB reply's; whether they agree (a FAIL row if not).
    fn parts_agree(&mut self, item: &str, k: &kh::Knob, knobs: u8) -> bool {
        let block = k.flags & (knob_flags::TURN | knob_flags::PRESS);
        let reply = knobs & (knob::HAS_TURN | knob::HAS_PRESS);
        if block != reply {
            self.row(
                item,
                Level::Fail,
                format!(
                    "the self-report says {}, the KNOB reply {}",
                    parts_text(block),
                    parts_text(reply)
                ),
            );
        }
        block == reply
    }

    /// While held on an app: the block says so.
    fn health_held(&mut self, knobs: u8) {
        const ITEM: &str = "Health while held";
        let Some(b) = self.block(ITEM) else {
            return;
        };
        let Some(k) = self.knob_part(ITEM, &b) else {
            return;
        };
        if !self.parts_agree(ITEM, &k, knobs) {
            return;
        }
        if block_hold(k.flags) != (true, Target::App) {
            return self.row(
                ITEM,
                Level::Fail,
                format!(
                    "the self-report says {} while KNOB says held, on an app",
                    kh::knob_words(&k)
                ),
            );
        }
        self.row(ITEM, Level::Ok, kh::knob_words(&k));
    }

    /// A hold byte the board doesn't know: status 5, and the hold (on master) as it was.
    fn bad_request(&mut self) {
        const ITEM: &str = "Bad request refused";
        match self.ask(&[protocol::SETTINGS, protocol::settings::KNOB, 2, 0]) {
            Err(e) => self.row(ITEM, Level::Fail, e),
            Ok(s) if s.status != 5 => self.row(
                ITEM,
                Level::Fail,
                format!(
                    "hold byte 2 got status {} ({}), want 5",
                    s.status,
                    status_text(s.status)
                ),
            ),
            Ok(s) if !s.held || s.target != Target::Master => self.row(
                ITEM,
                Level::Fail,
                format!("refused, but the hold changed: {}", hold_text(&s)),
            ),
            Ok(_) => self.row(
                ITEM,
                Level::Ok,
                "hold byte 2 refused with status 5; still held, on master",
            ),
        }
    }

    /// RELEASE, then a fresh read: the board has the knob back. Then the health row.
    fn release(&mut self, knobs: u8, before: Option<kh::Block>) {
        const ITEM: &str = "Release";
        if !self.hold_row(ITEM, &knob::release(), (false, Target::Master)) {
            return;
        }
        match self.ask(&knob::read()) {
            Err(e) => return self.row("Hand-back read", Level::Fail, e),
            Ok(s) if s.held => {
                return self.row(
                    "Hand-back read",
                    Level::Fail,
                    format!("RELEASE said not held, a fresh read says {}", hold_text(&s)),
                )
            }
            Ok(_) => {}
        }
        // The release row says what the reply said; the fresh read confirms it.
        if let Some(last) = self.cases.rows.last_mut() {
            last.detail =
                "not held after RELEASE and on a fresh read: the volume keys and mute".into();
        }
        self.health_row(knobs, before);
    }

    /// After the release: the block says not held, no hold dropped during the case, and the
    /// health view's Knob row says the same.
    fn health_row(&mut self, knobs: u8, before: Option<kh::Block>) {
        const ITEM: &str = "Health row";
        let Some(b) = self.block(ITEM) else {
            return;
        };
        let Some(k) = self.knob_part(ITEM, &b) else {
            return;
        };
        if !self.parts_agree(ITEM, &k, knobs) {
            return;
        }
        if k.flags & knob_flags::HELD != 0 {
            return self.row(
                ITEM,
                Level::Fail,
                format!("the self-report says {} after RELEASE", kh::knob_words(&k)),
            );
        }
        // A PC's RELEASE isn't a drop: only a USB disconnect or an endpoint change is.
        let was = before.and_then(|b| b.knob);
        if let Some(w) = was.filter(|w| k.drops != w.drops) {
            return self.row(
                ITEM,
                Level::Fail,
                format!(
                    "the board dropped {} hold(s) during the case (a USB disconnect or an endpoint change)",
                    k.drops.wrapping_sub(w.drops)
                ),
            );
        }
        let report = Report {
            left: Ok(b),
            right: Err(kh::ReadError::Other("not read".into())),
        };
        let rows = kh::rows(&report, &kh::Thresholds::default());
        let Some(view) = rows.iter().find(|r| r.subsystem == "knob") else {
            return self.row(
                ITEM,
                Level::Fail,
                "the health view shows no Knob row for this block",
            );
        };
        let touched = was.map_or(0, |w| k.events.wrapping_sub(w.events));
        let detail = if touched > 0 {
            format!(
                "{}: {} (someone touched the knob during the case)",
                view.item, view.detail
            )
        } else {
            format!("{}: {}", view.item, view.detail)
        };
        self.row(ITEM, view.level, detail);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use foxglove_link::protocol::REPORT;
    use std::cell::RefCell;

    /// The keyboard's side of D7 14 and D7 13, as host_link.c's `knob_command` and health.c
    /// behave, with knobs for the broken ones.
    struct Kb {
        held: bool,
        on_app: bool,
        parts: u8,
        events: u16,
        drops: u16,
        /// Every KNOB request answers this status.
        status: Option<u8>,
        /// A RELEASE that answers ok and changes nothing.
        release_keeps: bool,
        /// The reply says released but the board still holds (a read says otherwise).
        release_lies: bool,
        /// The self-report: block version 1 (no knob part).
        old_block: bool,
        /// The self-report's knob byte ignores the hold.
        block_stale: bool,
        /// Knob events to answer before the next reply (a hand on the knob).
        events_first: usize,
        /// A click while held on the app: its event comes in place of that hold's reply.
        click_on_app: bool,
        /// The board drops the hold on the next request (an endpoint change mid-case).
        drop_next: bool,
    }

    impl Default for Kb {
        fn default() -> Self {
            Kb {
                held: false,
                on_app: false,
                parts: knob::HAS_TURN | knob::HAS_PRESS,
                events: 3,
                drops: 1,
                status: None,
                release_keeps: false,
                release_lies: false,
                old_block: false,
                block_stale: false,
                events_first: 0,
                click_on_app: false,
                drop_next: false,
            }
        }
    }

    #[derive(Default)]
    struct Fake {
        kb: RefCell<Kb>,
        sent: RefCell<Vec<Vec<u8>>>,
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
                ..Fake::default()
            }
        }

        fn block(kb: &Kb) -> kh::Block {
            let mut flags = kb.parts;
            if kb.held && !kb.block_stale {
                flags |= knob_flags::HELD;
                if kb.on_app {
                    flags |= knob_flags::ON_APP;
                }
            }
            kh::Block {
                version: if kb.old_block { 1 } else { 2 },
                half: 0,
                sha: "abc1234".into(),
                knob: (!kb.old_block).then_some(kh::Knob {
                    flags,
                    last: 0,
                    events: kb.events,
                    keys: 0,
                    drops: kb.drops,
                }),
                ..Default::default()
            }
        }
    }

    impl Transport for Fake {
        fn send(&self, _: &[u8]) -> anyhow::Result<()> {
            panic!("the knob cases wait for every reply");
        }

        fn request(
            &self,
            r: &[u8],
            proto: u8,
            cmd: u8,
            _: Duration,
        ) -> anyhow::Result<[u8; REPORT]> {
            self.sent.borrow_mut().push(r.to_vec());
            assert_eq!(proto, 0xd7);
            let mut kb = self.kb.borrow_mut();
            if kb.drop_next {
                kb.drop_next = false;
                if kb.held {
                    kb.held = false;
                    kb.on_app = false;
                    kb.drops += 1;
                }
            }
            match cmd {
                0x14 => {
                    if kb.events_first > 0 {
                        kb.events_first -= 1;
                        kb.events += 1;
                        return Ok(reply(&[0xd7, 0x94, 1, 2, 0, u8::from(kb.on_app), 9]));
                    }
                    let mut status = kb.status.unwrap_or(0);
                    let mut lie = false;
                    if status == 0 && r.len() >= 3 && r[2] != 0xff {
                        let (hold, target) = (r[2], r.get(3).copied().unwrap_or(0));
                        if hold > 1 || target > 1 {
                            status = 5;
                        } else if hold == 0 && (kb.release_keeps || kb.release_lies) {
                            lie = kb.release_lies;
                        } else {
                            kb.held = hold == 1;
                            kb.on_app = kb.held && target == 1;
                        }
                    }
                    if kb.click_on_app && kb.on_app && r[2] == 1 {
                        kb.click_on_app = false;
                        kb.events += 1;
                        return Ok(reply(&[0xd7, 0x94, 1, 2, 0, 1, 9]));
                    }
                    let (held, app) = if lie {
                        (false, false)
                    } else {
                        (kb.held, kb.on_app)
                    };
                    Ok(reply(&[
                        0xd7,
                        0x94,
                        0,
                        status,
                        u8::from(held),
                        u8::from(app),
                        kb.parts,
                    ]))
                }
                0x13 => {
                    let replies = Fake::block(&kb).replies(1);
                    Ok(replies[usize::from(r[3])])
                }
                other => panic!("unexpected request D7 {other:02x}"),
            }
        }
    }

    const ALL: u32 = capabilities::KNOB | capabilities::HEALTH;

    fn has(out: &Cases, line: &str) -> bool {
        out.lines().iter().any(|l| l == line)
    }

    fn released(f: &Fake) -> bool {
        let kb = f.kb.borrow();
        !kb.held && !kb.on_app
    }

    #[test]
    fn a_healthy_keyboard_passes_and_gets_its_knob_back() {
        let f = Fake::default();
        let out = run(&f, ALL);
        let lines = out.lines();
        assert_eq!(out.code(), PASS, "{lines:#?}");
        assert!(out.rows.iter().all(|r| r.level == Level::Ok), "{lines:#?}");
        let want = [
            "PASS knob       Read: not held (the volume keys and mute); the keymap has the turn and the click",
            "PASS knob       Hold: held, on master",
            "PASS knob       Target app: held, on an app",
            "PASS knob       Health while held: held by Foxglove, on an app; 3 events, 0 keys, 1 hold dropped",
            "PASS knob       Back to master: held, on master",
            "PASS knob       Bad request refused: hold byte 2 refused with status 5; still held, on master",
            "PASS knob       Release: not held after RELEASE and on a fresh read: the volume keys and mute",
            "PASS knob       Health row: Knob: not held: the volume keys and mute; 3 events, 0 keys, 1 hold dropped",
        ];
        assert_eq!(&lines[..want.len()], &want, "{lines:#?}");
        assert!(
            lines[want.len()].starts_with("HUMAN knob       turns, click, double click and hold")
        );
        assert_eq!(lines.last().unwrap(), "PASS: nothing failed, 0 warning(s)");
        assert!(released(&f));
        let j = out.json();
        assert_eq!(j["result"], "pass");
        assert_eq!(j["human"].as_array().unwrap().len(), 1);
        // The hold went on in the issue's order: master, the app, master, then the release.
        let holds: Vec<Vec<u8>> = f
            .sent
            .borrow()
            .iter()
            .filter(|r| r[1] == 0x14 && r[2] != 0xff)
            .cloned()
            .collect();
        assert_eq!(
            holds,
            [
                knob::hold(Target::Master),
                knob::hold(Target::App),
                knob::hold(Target::Master),
                vec![0xd7, 0x14, 2, 0],
                knob::release(),
                knob::release(), // the drop guard's
            ]
        );
    }

    #[test]
    fn a_release_that_keeps_the_knob_fails() {
        let f = Fake::with(Kb {
            release_keeps: true,
            ..Kb::default()
        });
        let out = run(&f, ALL);
        assert_eq!(out.code(), FAIL);
        assert!(
            has(
                &out,
                "FAIL knob       Release: KNOB ok but held, on master (want not held (the volume keys and mute))"
            ),
            "{:#?}",
            out.lines()
        );

        // A reply that says released while the board still holds: the fresh read catches it.
        let f = Fake::with(Kb {
            release_lies: true,
            ..Kb::default()
        });
        let out = run(&f, ALL);
        assert_eq!(out.code(), FAIL);
        assert!(has(
            &out,
            "FAIL knob       Hand-back read: RELEASE said not held, a fresh read says held, on master"
        ));
    }

    #[test]
    fn the_health_row_must_follow_the_hold() {
        let f = Fake::with(Kb {
            block_stale: true,
            ..Kb::default()
        });
        let out = run(&f, ALL);
        assert_eq!(out.code(), FAIL);
        assert!(has(
            &out,
            "FAIL knob       Health while held: the self-report says not held: the volume keys and mute; 3 events, 0 keys, 1 hold dropped while KNOB says held, on an app"
        ), "{:#?}", out.lines());
        assert!(released(&f));

        let f = Fake::with(Kb {
            old_block: true,
            ..Kb::default()
        });
        let out = run(&f, ALL);
        assert_eq!(out.code(), FAIL);
        assert!(has(
            &out,
            "FAIL knob       Health row: the left's self-report (version 1) has no knob part (bytes 96-103, version 2)"
        ));

        // A keymap with the turn only.
        let f = Fake::with(Kb {
            parts: knob::HAS_TURN,
            ..Kb::default()
        });
        let out = run(&f, ALL);
        assert_eq!(out.code(), PASS, "{:#?}", out.lines());
        assert!(has(
            &out,
            "PASS knob       Read: not held (the volume keys and mute); the keymap has the turn only"
        ));
    }

    #[test]
    fn a_hold_dropped_mid_case_fails() {
        // The board drops the hold (an endpoint change) just before the back-to-master request.
        let f = Fake::with(Kb::default());
        let out = {
            struct Dropper<'a>(&'a Fake, RefCell<usize>);
            impl Transport for Dropper<'_> {
                fn send(&self, r: &[u8]) -> anyhow::Result<()> {
                    self.0.send(r)
                }
                fn request(
                    &self,
                    r: &[u8],
                    p: u8,
                    c: u8,
                    t: Duration,
                ) -> anyhow::Result<[u8; REPORT]> {
                    if c == 0x14 && r == knob::hold(Target::Master).as_slice() {
                        *self.1.borrow_mut() += 1;
                        if *self.1.borrow() == 2 {
                            self.0.kb.borrow_mut().drop_next = true;
                        }
                    }
                    self.0.request(r, p, c, t)
                }
            }
            run(&Dropper(&f, RefCell::new(0)), ALL)
        };
        // The board re-holds on the request itself, so the hold looks fine; the drop shows in the
        // block's count.
        assert_eq!(out.code(), FAIL, "{:#?}", out.lines());
        assert!(has(
            &out,
            "FAIL knob       Health row: the board dropped 1 hold(s) during the case (a USB disconnect or an endpoint change)"
        ));
    }

    #[test]
    fn a_knob_event_in_place_of_a_reply_is_skipped() {
        let f = Fake::with(Kb {
            click_on_app: true,
            ..Kb::default()
        });
        let out = run(&f, ALL);
        assert_eq!(out.code(), PASS, "{:#?}", out.lines());
        assert!(has(&out, "PASS knob       Target app: held, on an app"));
        assert!(out
            .lines()
            .iter()
            .any(|l| l.ends_with("(someone touched the knob during the case)")));

        // Only events: a failure, not a hang.
        let f = Fake::with(Kb {
            events_first: 100,
            ..Kb::default()
        });
        let out = run(&f, ALL);
        assert_eq!(out.code(), FAIL);
        assert!(has(
            &out,
            "FAIL knob       Read: only knob events came back: is someone turning or clicking the knob?"
        ));
    }

    #[test]
    fn a_leftover_hold_warns_and_is_handed_back() {
        let f = Fake::with(Kb {
            held: true,
            on_app: true,
            ..Kb::default()
        });
        let out = run(&f, ALL);
        assert_eq!(out.code(), PASS, "{:#?}", out.lines());
        assert!(has(
            &out,
            "WARN knob       Read: held, on an app with no app running: a hold an app left behind; the keymap has the turn and the click"
        ));
        assert!(released(&f));
    }

    #[test]
    fn a_bad_status_fails_and_still_releases() {
        let f = Fake::with(Kb {
            status: Some(4),
            ..Kb::default()
        });
        let out = run(&f, ALL);
        assert_eq!(out.code(), FAIL);
        assert!(has(
            &out,
            "FAIL knob       Read: KNOB read status 4 (not built in) though HELLO says KNOB"
        ));
        assert_eq!(
            f.sent.borrow().last().unwrap(),
            &knob::release(),
            "the drop guard releases"
        );
    }

    #[test]
    fn a_missing_capability_blocks() {
        let f = Fake::default();
        let out = run(&f, 0);
        assert_eq!(out.code(), BLOCKED);
        assert!(out.rows.is_empty() && out.human.is_empty());
        assert!(f.sent.borrow().is_empty(), "nothing sent without bit 31");
        assert_eq!(
            out.lines().last().unwrap(),
            "BLOCKED: KNOB not tested: no HELLO bit 31 (no &knob or &knob_press in this build)"
        );

        // The knob without the self-report: the hold cases run and pass, the health row blocks.
        let f = Fake::default();
        let out = run(&f, capabilities::KNOB);
        assert_eq!(out.code(), BLOCKED, "{:#?}", out.lines());
        assert!(out.rows.iter().all(|r| r.level == Level::Ok));
        assert!(!out.lines().iter().any(|l| l.contains("Health")));
        assert!(f.sent.borrow().iter().all(|r| r[1] == 0x14));
        assert!(released(&f));

        // Something that failed outranks something untested.
        let f = Fake::with(Kb {
            release_keeps: true,
            ..Kb::default()
        });
        assert_eq!(run(&f, capabilities::KNOB).code(), FAIL);
    }

    #[test]
    fn health_takes_the_rows_and_warns_for_the_human_checks() {
        let f = Fake::default();
        let mut health = Outcome {
            rows: Vec::new(),
            blocked: None,
        };
        run(&f, ALL).fold_into(&mut health);
        assert_eq!(health.code(), PASS, "{:#?}", health.lines());
        assert!(health.lines().iter().any(|l| l.starts_with(
            "WARN knob       Not tested: by hand: turns, click, double click and hold"
        )));

        let mut health = Outcome {
            rows: Vec::new(),
            blocked: None,
        };
        Cases::blocked_by("the app holds the keyboard").fold_into(&mut health);
        assert_eq!(health.code(), PASS);
        assert!(health
            .lines()
            .iter()
            .any(|l| l == "WARN knob       Not tested: the app holds the keyboard"));

        let f = Fake::with(Kb {
            release_keeps: true,
            ..Kb::default()
        });
        let mut health = Outcome {
            rows: Vec::new(),
            blocked: None,
        };
        run(&f, ALL).fold_into(&mut health);
        assert_eq!(health.code(), FAIL);
    }
}
