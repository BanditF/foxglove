//! H5.5: the sign-in double start (design-h5-rebrand.md, H5.5; contract §8.6 one-owner rule).
//!
//! With module mode on, `Foxglove.lnk` and `Hideout.lnk` both fire at sign-in, so a
//! `foxglove.exe --tray` start can find a hub that already hosts Foxglove. This file is only the
//! decision table, as pure functions of facts the caller gathers, and [`at_startup`], the one
//! caller (app main, just before the single-instance claim). No credential, token or secret is
//! involved: "sign-in" is the Windows sign-in.
//!
//! Opt-in is module mode itself, no separate switch: the hub fact `module.foxglove.state` reads
//! "running" only when the hub supervises Foxglove as its module, and only that fact makes the
//! app defer. With module mode off the fact is absent, so the answer is "not hosted" and the
//! start is today's. The fact is read only when a hub verifiably holds the `foxglove-<user>`
//! name, so with no hub nothing is opened or asked.
//!
//! Safety rule of the table: no keyboard app at sign-in is the failure to avoid, so every doubt
//! (hub not verified, hosting not confirmed, own module mode) resolves to [`Start::Normally`],
//! which is today's behaviour.

#![allow(dead_code)] // `hub_side` is the hub supervisor's half, not wired in the app

use super::one_owner::Coexistence;

/// How this process was started.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Launch {
    /// `--tray`: the Startup shortcut, the app's own relaunches, On-call's restarts.
    Tray,
    /// No arguments: by hand, the Start menu.
    Window,
    /// `--module`: the hub started this process.
    Module,
}

/// What the hub says about Foxglove's row (the fact `module.foxglove.state`, read over the
/// verified hub pipe by `hub_fact`). `Unknown` when it could not be asked; the table treats it
/// like `OnItsOwn`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hosting {
    Unknown,
    /// The row says "Runs on its own", or the hub is not hosting a Foxglove instance.
    OnItsOwn,
    /// The hub runs, or is starting, `foxglove.exe --module` for this user.
    Hosted,
}

/// What start-up does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Start {
    /// Carry on as today.
    Normally,
    /// A verified hub hosts Foxglove: ask it to show Foxglove's page (only when `show`), then
    /// exit 0 without claiming the keyboard or the pipe.
    DeferToHub { show: bool },
}

/// The start-up decision. `enabled` is the opt-in (off by default): false is always `Normally`.
pub fn decide(enabled: bool, launch: Launch, hub: &Coexistence, hosting: Hosting) -> Start {
    if !enabled {
        return Start::Normally;
    }
    match (launch, hub, hosting) {
        // The hub's own child never defers to the hub.
        (Launch::Module, _, _) => Start::Normally,
        // Only a verified hub that confirms it hosts Foxglove is deferred to.
        (_, Coexistence::HubHasTheName { .. }, Hosting::Hosted) => Start::DeferToHub {
            show: launch == Launch::Window,
        },
        _ => Start::Normally,
    }
}

/// The reverse order (Foxglove started first): what the hub's supervisor does with its row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HubSide {
    /// Start `foxglove.exe --module` as usual.
    StartModule,
    /// Leave the row "Runs on its own" and start nothing (a standalone Foxglove holds the
    /// keyboard); the hub looks again only when that process ends (an event, no polling).
    LeaveStandalone,
}

/// `standalone_running` is a verified standalone `foxglove.exe` on `foxglove-<user>`.
pub fn hub_side(standalone_running: bool) -> HubSide {
    if standalone_running {
        HubSide::LeaveStandalone
    } else {
        HubSide::StartModule
    }
}

/// What a module does when its hub went away (Q8: the hub's exe is gone for good).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Orphan {
    /// Keep running as a module (the hub is there, or only restarting).
    Stay,
    /// Restart once as standalone `--tray`.
    RestartStandalone,
    /// Already restarted once for this run, or not a module: do nothing more.
    Nothing,
}

pub fn orphaned(is_module: bool, hub_exe_present: bool, already_restarted: bool) -> Orphan {
    match (is_module, hub_exe_present, already_restarted) {
        (false, _, _) | (_, _, true) => Orphan::Nothing,
        (true, true, false) => Orphan::Stay,
        (true, false, false) => Orphan::RestartStandalone,
    }
}

/// The decision for a start, reading the hub fact only when it can matter: a hub verifiably on
/// the name and a start that is not the hub's own child. `hosting` is the (bounded) read.
pub fn resolve(launch: Launch, hub: &Coexistence, hosting: impl FnOnce() -> Hosting) -> Start {
    let ask = launch != Launch::Module && matches!(hub, Coexistence::HubHasTheName { .. });
    decide(
        true,
        launch,
        hub,
        if ask { hosting() } else { Hosting::Unknown },
    )
}

/// [`resolve`] over a hub connection, and the `hub.show` the decision calls for.
pub fn handoff<C: super::hub_fact::Connect>(
    launch: Launch,
    hub: &Coexistence,
    conn: std::sync::Arc<C>,
    limit: std::time::Duration,
) -> Start {
    let start = resolve(launch, hub, || {
        super::hub_fact::hosting(conn.clone(), limit)
    });
    if start == (Start::DeferToHub { show: true }) {
        super::hub_fact::show(conn, limit);
    }
    start
}

/// The longest a `--tray` start waits for Hideout's answer (sign-in hand-over, slice 3).
pub const WAIT_LIMIT: std::time::Duration = std::time::Duration::from_secs(30);

/// What a start does before the check, from the `[signin]` setting (a pure table).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wait {
    /// Today's start, no wait: not a `--tray` start, or Hideout was not said to start Foxglove.
    No,
    /// Hideout starts Foxglove and its exe is there: wait for its answer, at most [`WAIT_LIMIT`].
    ForHideout,
    /// The setting is on but the recorded `hideout.exe` is gone: forget it, no wait.
    ForgetAndStart,
}

/// The wait decision: only a `--tray` start, only with the flag, only with the recorded exe.
pub fn wait_plan(launch: Launch, starts_me: bool, exe_present: bool) -> Wait {
    match (launch, starts_me, exe_present) {
        (Launch::Tray, true, true) => Wait::ForHideout,
        (Launch::Tray, true, false) => Wait::ForgetAndStart,
        _ => Wait::No,
    }
}

/// The hand-over event (a kernel event on Windows; a fake in tests).
pub trait Answer {
    /// True when Hideout answered within `limit`.
    fn wait(&self, limit: std::time::Duration) -> bool;
}

/// How the wait ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Waited {
    Answered(std::time::Duration),
    TimedOut,
}

/// Waits on `event` at most [`WAIT_LIMIT`]; `now` is the clock (injected for tests).
pub fn wait_for(event: &dyn Answer, now: impl Fn() -> std::time::Instant) -> Waited {
    let t0 = now();
    if event.wait(WAIT_LIMIT) {
        Waited::Answered(now().saturating_duration_since(t0))
    } else {
        Waited::TimedOut
    }
}

/// `4.2 s`: the "waited" figure of the log lines.
pub fn secs(d: std::time::Duration) -> String {
    format!("{:.1} s", d.as_secs_f64())
}

pub const LOG_WAITING: &str = "sign-in: Hideout starts Foxglove; waiting up to 30 s for it";

pub fn gone_line(exe: &std::path::Path) -> String {
    format!(
        "sign-in: the recorded Hideout ({}) is gone; starting on its own and forgetting it",
        exe.display()
    )
}

#[cfg(windows)]
struct Event(hideout_acl::signin_event::SignInEvent);

#[cfg(windows)]
impl Answer for Event {
    fn wait(&self, limit: std::time::Duration) -> bool {
        self.0.wait(limit)
    }
}

/// The launch kind from this process's flags.
pub fn launch_of(module: bool, tray: bool) -> Launch {
    if module {
        Launch::Module
    } else if tray {
        Launch::Tray
    } else {
        Launch::Window
    }
}

/// App start-up, before the single-instance claim: true when a hub already hosts Foxglove and
/// this process should exit 0 (it has asked the hub to show Foxglove's page when it was a start
/// by hand). Skipped (false) under `FOXGLOVE_CONFIG_DIR` or a test sandbox, so tests never touch
/// the real hub, and off Windows.
pub fn at_startup() -> bool {
    #[cfg(windows)]
    {
        if std::env::var_os("FOXGLOVE_CONFIG_DIR").is_some()
            || super::store::test_sandbox().is_some()
        {
            return false;
        }
        let launch = launch_of(
            super::hub_link::module_mode(),
            super::settings_open::tray_start(),
        );
        if launch == Launch::Module {
            return false;
        }
        // Hideout starts Foxglove at sign-in (remembered from its fact): wait for its answer first.
        let settings = super::store::section("core", "signin")
            .try_load::<super::signin_flag::SigninSettings>()
            .unwrap_or_default();
        let exe_present = settings.hideout_exe.as_deref().is_some_and(|p| {
            super::signin_flag::usable_exe(p, |p| p.try_exists().unwrap_or(false))
        });
        let waited = match wait_plan(launch, settings.hideout_starts_me, exe_present) {
            Wait::No => None,
            Wait::ForgetAndStart => {
                let exe = settings.hideout_exe.clone().unwrap_or_default();
                super::app_log::early(&gone_line(&exe));
                super::signin_flag::forget();
                None
            }
            Wait::ForHideout => match hideout_acl::signin_event::SignInEvent::open() {
                Ok(ev) => {
                    super::app_log::early(LOG_WAITING);
                    Some(wait_for(&Event(ev), std::time::Instant::now))
                }
                Err(e) => {
                    super::app_log::early(&format!(
                        "sign-in: the hand-over event could not be opened ({e}); starting on its own"
                    ));
                    None
                }
            },
        };
        if waited == Some(Waited::TimedOut) {
            // No answer in 30 s: start on its own, without asking a hub that is not answering.
            super::signin_counts::note(super::signin_counts::Kind::Timeout);
            super::app_log::early(super::signin_counts::LOG_TIMEOUT);
            return false;
        }
        let waited = match waited {
            Some(Waited::Answered(d)) => Some(d),
            _ => None,
        };
        let pipe = foxglove_plugin_host::pipe::name();
        let hub = super::one_owner::log_for(&pipe, foxglove_plugin::owner::Expect::HIDEOUT);
        let start = handoff(
            launch,
            &hub,
            std::sync::Arc::new(super::hub_fact::HubPipe),
            super::hub_fact::LIMIT,
        );
        if let Start::DeferToHub { show } = start {
            if launch == Launch::Tray {
                super::signin_counts::note(super::signin_counts::Kind::Deferred);
                super::app_log::early(&super::signin_counts::deferred_line(waited));
            } else {
                super::app_log::early(&format!(
                    "Foxglove: Hideout already runs Foxglove as its app; exiting{}",
                    if show {
                        " after asking it to show Foxglove"
                    } else {
                        ""
                    }
                ));
            }
            return true;
        }
        // Hideout starts Foxglove at sign-in, yet this copy starts on its own: the backup ran.
        if launch == Launch::Tray && settings.hideout_starts_me && exe_present {
            super::signin_counts::note(super::signin_counts::Kind::OnItsOwn);
            super::app_log::early(&super::signin_counts::on_its_own_line(
                "not running as an app",
                waited,
            ));
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    const HUB: Coexistence = Coexistence::HubHasTheName { pid: 7 };

    fn fake() -> Coexistence {
        Coexistence::Unverified { why: "x".into() }
    }

    #[test]
    fn off_by_default_changes_nothing_whatever_the_facts() {
        for l in [Launch::Tray, Launch::Window, Launch::Module] {
            assert_eq!(decide(false, l, &HUB, Hosting::Hosted), Start::Normally);
        }
    }

    #[test]
    fn a_tray_start_beside_a_hosting_hub_defers_quietly() {
        assert_eq!(
            decide(true, Launch::Tray, &HUB, Hosting::Hosted),
            Start::DeferToHub { show: false }
        );
    }

    #[test]
    fn a_window_start_beside_a_hosting_hub_asks_it_to_show() {
        assert_eq!(
            decide(true, Launch::Window, &HUB, Hosting::Hosted),
            Start::DeferToHub { show: true }
        );
    }

    #[test]
    fn every_doubt_starts_normally_so_there_is_always_a_keyboard_app() {
        for hosting in [Hosting::Unknown, Hosting::OnItsOwn] {
            assert_eq!(decide(true, Launch::Tray, &HUB, hosting), Start::Normally);
        }
        for hub in [Coexistence::Alone, fake()] {
            assert_eq!(
                decide(true, Launch::Tray, &hub, Hosting::Hosted),
                Start::Normally
            );
        }
    }

    #[test]
    fn the_hubs_own_child_never_defers() {
        assert_eq!(
            decide(true, Launch::Module, &HUB, Hosting::Hosted),
            Start::Normally
        );
    }

    #[test]
    fn a_standalone_foxglove_first_keeps_the_row_on_its_own() {
        assert_eq!(hub_side(true), HubSide::LeaveStandalone);
        assert_eq!(hub_side(false), HubSide::StartModule);
    }

    #[test]
    fn an_orphaned_module_restarts_standalone_once() {
        assert_eq!(orphaned(true, false, false), Orphan::RestartStandalone);
        assert_eq!(orphaned(true, false, true), Orphan::Nothing);
        assert_eq!(orphaned(true, true, false), Orphan::Stay);
        assert_eq!(orphaned(false, false, false), Orphan::Nothing);
    }

    struct Fake {
        answers: bool,
        asked: std::cell::Cell<Option<std::time::Duration>>,
    }
    impl Answer for Fake {
        fn wait(&self, limit: std::time::Duration) -> bool {
            self.asked.set(Some(limit));
            self.answers
        }
    }

    #[test]
    fn only_a_tray_start_with_the_flag_and_the_exe_waits() {
        for l in [Launch::Tray, Launch::Window, Launch::Module] {
            for starts in [false, true] {
                for exe in [false, true] {
                    let want = match (l, starts, exe) {
                        (Launch::Tray, true, true) => Wait::ForHideout,
                        (Launch::Tray, true, false) => Wait::ForgetAndStart,
                        _ => Wait::No,
                    };
                    assert_eq!(wait_plan(l, starts, exe), want, "{l:?} {starts} {exe}");
                }
            }
        }
    }

    #[test]
    fn the_wait_is_bounded_at_30_seconds() {
        assert_eq!(WAIT_LIMIT, std::time::Duration::from_secs(30));
        let ev = Fake {
            answers: false,
            asked: Default::default(),
        };
        assert_eq!(wait_for(&ev, std::time::Instant::now), Waited::TimedOut);
        assert_eq!(ev.asked.get(), Some(WAIT_LIMIT));
        let ev = Fake {
            answers: true,
            asked: Default::default(),
        };
        assert!(matches!(
            wait_for(&ev, std::time::Instant::now),
            Waited::Answered(_)
        ));
    }

    #[test]
    fn the_wait_lines_are_exact() {
        assert_eq!(
            LOG_WAITING,
            "sign-in: Hideout starts Foxglove; waiting up to 30 s for it"
        );
        assert_eq!(
            gone_line(std::path::Path::new(r"C:\H\hideout.exe")),
            r"sign-in: the recorded Hideout (C:\H\hideout.exe) is gone; starting on its own and forgetting it"
        );
        assert_eq!(secs(std::time::Duration::from_millis(4200)), "4.2 s");
    }

    #[test]
    fn the_wait_sits_before_the_single_instance_claim_and_before_the_check() {
        let src = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/foxglove/signin_handoff.rs"),
        )
        .unwrap();
        let body = &src[src.find("pub fn at_startup").unwrap()..src.find("#[cfg(test)]").unwrap()];
        let sandbox = body.find("test_sandbox()").expect("sandbox guard");
        let wait = body.find("wait_for(").expect("wait");
        let check = body.find("let start = handoff(").expect("check");
        assert!(
            sandbox < wait && wait < check,
            "guard, then wait, then the check"
        );
    }

    use super::super::hub_fact::{Connect, Ends, Refused};
    use std::sync::{Arc, Mutex};

    /// A hub that answers in `reply` and records what it was sent and how often it was opened.
    struct Hub {
        reply: String,
        wrote: Arc<Mutex<Vec<u8>>>,
        opened: Arc<Mutex<u32>>,
    }
    struct Sink(Arc<Mutex<Vec<u8>>>);
    impl std::io::Write for Sink {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    impl Connect for Hub {
        fn open(&self) -> Result<Ends, Refused> {
            *self.opened.lock().unwrap() += 1;
            Ok((
                Box::new(std::io::Cursor::new(self.reply.clone().into_bytes())),
                Box::new(Sink(self.wrote.clone())),
            ))
        }
    }
    type Mock = (Arc<Hub>, Arc<Mutex<Vec<u8>>>, Arc<Mutex<u32>>);
    fn hub(state: &str) -> Mock {
        let wrote = Arc::new(Mutex::new(Vec::new()));
        let opened = Arc::new(Mutex::new(0));
        let init = r#"{"jsonrpc":"2.0","id":0,"result":{}}"#;
        let reply = format!(
            "{init}\n{{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{{\"value\":\"{state}\"}}}}\n"
        );
        let h = Hub {
            reply,
            wrote: wrote.clone(),
            opened: opened.clone(),
        };
        (Arc::new(h), wrote, opened)
    }
    const L: std::time::Duration = std::time::Duration::from_secs(2);

    #[test]
    fn the_hand_off_check_runs_before_the_single_instance_claim() {
        let main = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/main.rs"),
        )
        .unwrap();
        let ask = main.find("signin_handoff::at_startup()").expect("wired");
        let claim = main.find("single_instance::claim(").expect("claim");
        assert!(ask < claim, "the deferral must be decided before the claim");
    }

    #[test]
    fn launch_kind_follows_the_flags() {
        assert_eq!(launch_of(true, true), Launch::Module);
        assert_eq!(launch_of(false, true), Launch::Tray);
        assert_eq!(launch_of(false, false), Launch::Window);
    }

    #[test]
    fn no_hub_on_the_name_asks_nothing_and_starts_normally() {
        for co in [Coexistence::Alone, fake()] {
            let (h, wrote, opened) = hub("running");
            assert_eq!(handoff(Launch::Tray, &co, h, L), Start::Normally);
            assert_eq!(*opened.lock().unwrap(), 0);
            assert!(wrote.lock().unwrap().is_empty());
        }
    }

    #[test]
    fn module_mode_off_is_todays_start_even_with_a_hub_running() {
        // The hub runs but does not host Foxglove: the fact is not "running".
        let (h, _, _) = hub("restarting");
        assert_eq!(handoff(Launch::Tray, &HUB, h, L), Start::Normally);
    }

    #[test]
    fn the_hubs_own_child_never_asks() {
        let (h, _, opened) = hub("running");
        assert_eq!(handoff(Launch::Module, &HUB, h, L), Start::Normally);
        assert_eq!(*opened.lock().unwrap(), 0);
    }

    #[test]
    fn a_hosted_tray_start_defers_without_asking_to_show() {
        let (h, wrote, _) = hub("running");
        assert_eq!(
            handoff(Launch::Tray, &HUB, h, L),
            Start::DeferToHub { show: false }
        );
        let text = String::from_utf8(wrote.lock().unwrap().clone()).unwrap();
        assert!(!text.contains("hub.show"), "{text}");
    }

    #[test]
    fn a_hosted_start_by_hand_defers_and_asks_the_hub_to_show() {
        let (h, wrote, _) = hub("running");
        assert_eq!(
            handoff(Launch::Window, &HUB, h, L),
            Start::DeferToHub { show: true }
        );
        let text = String::from_utf8(wrote.lock().unwrap().clone()).unwrap();
        assert!(text.contains(r#""method":"hub.show""#), "{text}");
    }
}
