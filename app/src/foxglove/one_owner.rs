//! The one-owner rule from the app's side (contract §8.6, #1143 P4).
//!
//! `foxglove.exe` and `hideout.exe` have different single-instance names
//! (`Local\BanditF.Foxglove.{scope}.instance` here, `Local\BanditF.Hideout.Hub{suffix}` in the
//! hub), so the mutexes cannot see each other and both may run. The pipe `foxglove-<user>` is
//! the shared resource: the app takes it with FILE_FLAG_FIRST_PIPE_INSTANCE, the hub serves it as
//! an alias only when it is free. Before the app claims it, [`check`] looks at who is on the
//! name (the server's process, never a reply) and logs the coexistence. It changes no behaviour:
//! standalone, the app keeps its pipe; beside a hub that got there first, the claim fails as
//! before and the log says why. An unverified server is only logged and ignored.
//!
//! Not done here (follow-up on #1143): "the starting app asks the other to show its window,
//! then exits", and one HID/LampArray writer when both run.

use foxglove_plugin::owner::{self, Expect, Probe};

/// What the app's start-up learns from the pipe names.
#[derive(Debug, PartialEq, Eq)]
pub enum Coexistence {
    /// Nobody on the name, or the name is not a hub: nothing to say.
    Alone,
    /// A verified `hideout.exe` serves the name: both programs are running.
    HubHasTheName { pid: u32 },
    /// A server that did not verify as `hideout.exe` is on the name (another program, a
    /// Test-lab app, or a process failing the identity checks): logged, ignored.
    Unverified { why: String },
}

/// The result of probing `pipe` for the hub.
pub fn classify(p: &Probe) -> Coexistence {
    match p {
        Probe::Free => Coexistence::Alone,
        Probe::Verified { pid, .. } => Coexistence::HubHasTheName { pid: *pid },
        Probe::Foreign { pid, why } => Coexistence::Unverified {
            // No pid in the log; `why` carries a file name, never a path.
            why: {
                let _ = pid;
                why.clone()
            },
        },
    }
}

/// The log line for a result, `None` when there is nothing to say.
pub fn line(c: &Coexistence, pipe: &str) -> Option<String> {
    match c {
        Coexistence::Alone => None,
        Coexistence::HubHasTheName { .. } => Some(format!(
            "Foxglove: hideout.exe already serves {pipe}; both programs are running, \
             so the local API is not available here until the hub lets go of the name"
        )),
        Coexistence::Unverified { why } => Some(format!(
            "Foxglove: {pipe} is held by a server that did not verify as hideout.exe ({why}); ignored"
        )),
    }
}

#[cfg(windows)]
/// Probes the real name before the API pipe is claimed, and logs.
pub fn check() {
    let pipe = foxglove_plugin_host::pipe::name();
    log_for(&pipe, Expect::HIDEOUT);
}

/// [`check`] for any pipe name and expected hub image (tests).
pub fn log_for(pipe: &str, hub: Expect) -> Coexistence {
    let c = classify(&owner::probe(pipe, hub, owner::TIMEOUT));
    if let Some(text) = line(&c, pipe) {
        super::app_log::early(&text);
    }
    c
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use foxglove_plugin::channel::ServerPipe;

    fn name(tag: &str) -> String {
        format!(
            r"\\.\pipe\foxglove-test-oneowner-{tag}-{}",
            std::process::id()
        )
    }

    fn own() -> Expect {
        let n = std::env::current_exe()
            .unwrap()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        Expect {
            exe: Box::leak(n.into_boxed_str()),
        }
    }

    fn serve(name: &str) {
        let first = ServerPipe::create(name, true).unwrap();
        std::thread::spawn(move || {
            let _ = first.accept();
        });
    }

    #[test]
    fn running_alone_says_nothing_and_changes_nothing() {
        let t = std::time::Instant::now();
        assert_eq!(log_for(&name("alone"), own()), Coexistence::Alone);
        assert!(t.elapsed() < owner::TIMEOUT);
        assert!(line(&Coexistence::Alone, "p").is_none());
    }

    #[test]
    fn a_verified_hub_on_the_name_is_detected() {
        let n = name("hub");
        serve(&n);
        // In this test the "hub" is this process, under its own image name.
        assert_eq!(
            log_for(&n, own()),
            Coexistence::HubHasTheName {
                pid: std::process::id()
            }
        );
    }

    #[test]
    fn a_fake_hub_is_ignored_not_trusted() {
        let n = name("fake");
        serve(&n);
        match log_for(&n, Expect::HIDEOUT) {
            Coexistence::Unverified { why } => assert!(why.contains("not hideout.exe"), "{why}"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn the_two_single_instance_names_never_collide_so_coexistence_is_logged_not_blocked() {
        // The hub's mutex (hideout-shell platform.rs: `Local\BanditF.Hideout.Hub{suffix}`) and
        // the app's differ for every scope, which is why the pipe probe exists.
        let (app, _) = super::super::single_instance::names_for_tests(None);
        assert!(app.starts_with("Local\\BanditF.Foxglove."));
        assert_ne!(app, "Local\\BanditF.Hideout.Hub");
        let (scoped, _) = super::super::single_instance::names_for_tests(Some(
            std::ffi::OsStr::new(r"C:\test-scope"),
        ));
        assert_ne!(scoped, app);
    }
}
