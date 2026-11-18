//! H5.5: the sign-in double start (design-h5-rebrand.md, H5.5; contract §8.6 one-owner rule).
//!
//! With module mode on, `Foxglove.lnk` and `Hideout.lnk` both fire at sign-in, so a
//! `foxglove.exe --tray` start can find a hub that already hosts Foxglove. This file is only the
//! decision table, as pure functions of facts the caller gathers: nothing here opens a pipe,
//! starts a process, exits or writes a file, and nothing calls it yet (UNWIRED: wiring is the
//! module-mode go, with the lab sign-in test). No credential, token or secret is involved:
//! "sign-in" is the Windows sign-in.
//!
//! Safety rule of the table: no keyboard app at sign-in is the failure to avoid, so every doubt
//! (hub not verified, hosting not confirmed, own module mode) resolves to [`Start::Normally`],
//! which is today's behaviour.

#![allow(dead_code)] // unwired until the module-mode go

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

/// What the hub says about Foxglove's row. `Unknown` until the hub can be asked (open question
/// in review/h55-open-questions.md); the table treats it like `OnItsOwn`.
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

    #[test]
    fn unwired_nothing_outside_this_file_calls_it() {
        // Guard: enabling this is the module-mode go. Fails when a caller appears, to be
        // replaced by the wiring's own tests.
        fn walk(d: &std::path::Path, hits: &mut Vec<String>) {
            for e in std::fs::read_dir(d).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    walk(&p, hits);
                } else if p.extension().is_some_and(|x| x == "rs")
                    && p.file_name().unwrap() != "signin_handoff.rs"
                {
                    let t = std::fs::read_to_string(&p).unwrap_or_default();
                    if t.contains(concat!("signin_handoff", "::")) {
                        hits.push(p.display().to_string());
                    }
                }
            }
        }
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut hits = Vec::new();
        walk(&root, &mut hits);
        assert!(hits.is_empty(), "{hits:?}");
    }
}
