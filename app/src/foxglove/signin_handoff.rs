//! H5.5: the sign-in double start (design-h5-rebrand.md, H5.5; contract §8.6 one-owner rule).
//!
//! With module mode on, `Foxglove.lnk` and `Hideout.lnk` both fire at sign-in, so a
//! `foxglove.exe --tray` start can find a hub that already hosts Foxglove. This file is only the
//! decision table, as pure functions of facts the caller gathers, and [`at_startup`], the one
//! caller (app main, right after the single-instance claim). No credential, token or secret is
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

/// App start-up, after the single-instance claim: true when a hub already hosts Foxglove and
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
        let pipe = foxglove_plugin_host::pipe::name();
        let hub = super::one_owner::log_for(&pipe, foxglove_plugin::owner::Expect::HIDEOUT);
        let start = handoff(
            launch,
            &hub,
            std::sync::Arc::new(super::hub_fact::HubPipe),
            super::hub_fact::LIMIT,
        );
        if let Start::DeferToHub { show } = start {
            super::app_log::early(&format!(
                "Foxglove: the hub already runs Foxglove as its module; exiting{}",
                if show {
                    " after asking it to show Foxglove"
                } else {
                    ""
                }
            ));
            return true;
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
