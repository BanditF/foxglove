//! The hub's hand-over request, from the standalone app's side (first-run FR5).
//!
//! When the user allows an app that is also open on its own, Hideout may ask that standalone copy
//! over its pipe to quit cleanly, then start the app inside Hideout (`--module`). The copy obeys
//! only a caller the kernel names as a verified `hideout.exe` (`owner::verify_pid`: same user,
//! Medium integrity or above, no AppContainer, a trusted install folder). The request carries
//! nothing the decision reads. In doubt (no peer id, an unverified image, a process that went)
//! the copy stays open and answers "denied"; it never quits for a guess. Nothing here touches
//! the Startup entry.

use foxglove_plugin::owner::{self, Expect, NoRecords, Probe};

/// What the app does with a hand-over request.
#[derive(Debug, PartialEq, Eq)]
pub enum Verdict {
    /// The caller is the hub: hand back and exit.
    Quit,
    /// Stay open; the reason is for the log (a file name, never a path).
    Stay(String),
}

/// The decision, from the result of verifying the caller's process as `hideout.exe`.
pub fn decide(peer: Option<u32>, verified: impl FnOnce(u32) -> Probe) -> Verdict {
    let Some(pid) = peer else {
        return Verdict::Stay("the caller's process is unknown".into());
    };
    match verified(pid) {
        Probe::Verified { .. } => Verdict::Quit,
        Probe::Free => Verdict::Stay("the caller is gone".into()),
        Probe::Foreign { why, .. } => Verdict::Stay(format!("the caller is not the hub ({why})")),
    }
}

/// [`decide`] for the connection this thread serves, against the real process.
pub fn verdict_now() -> Verdict {
    decide(foxglove_plugin_host::peer_pid(), |pid| {
        owner::verify_pid(pid, Expect::HIDEOUT, &NoRecords)
    })
}

/// How long the answer gets to leave before the app starts handing back (the exit follows it).
const REPLY_GRACE: std::time::Duration = std::time::Duration::from_millis(250);

/// The hub's request at the pipe: the answer goes out, then the app hands back and exits as the
/// tray's Quit does. Anything but a verified hub is refused with nothing changed.
pub fn handle() -> Result<serde_json::Value, foxglove_plugin_api::rpc::Error> {
    use foxglove_plugin_api::rpc::{code, Error};
    match verdict_now() {
        Verdict::Quit => {
            super::app_log::line("Foxglove: Hideout asked this copy to quit; handing back");
            let _ = std::thread::Builder::new()
                .name("hand-over-quit".into())
                .spawn(|| {
                    std::thread::sleep(REPLY_GRACE);
                    super::quit::hand_back("Hideout took over");
                    std::process::exit(0)
                });
            Ok(serde_json::json!({}))
        }
        Verdict::Stay(why) => {
            super::app_log::line(&format!(
                "Foxglove: a hand-over request was refused ({why}); staying open"
            ));
            Err(Error {
                code: code::DENIED,
                message: "only Hideout may ask this".into(),
                data: None,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok() -> Probe {
        Probe::Verified {
            pid: 7,
            image: "hideout.exe".into(),
        }
    }

    #[test]
    fn only_a_verified_hub_is_obeyed() {
        assert_eq!(decide(Some(7), |_| ok()), Verdict::Quit);
    }

    #[test]
    fn every_doubt_stays_open() {
        assert!(matches!(decide(None, |_| ok()), Verdict::Stay(_)));
        assert!(matches!(decide(Some(7), |_| Probe::Free), Verdict::Stay(_)));
        let foreign = |_| Probe::Foreign {
            pid: Some(7),
            why: "runs evil.exe, not hideout.exe".into(),
        };
        match decide(Some(7), foreign) {
            Verdict::Stay(why) => assert!(why.contains("evil.exe"), "{why}"),
            v => panic!("{v:?}"),
        }
    }

    #[test]
    fn the_caller_is_checked_by_its_process_id_alone() {
        let mut asked = None;
        decide(Some(4242), |pid| {
            asked = Some(pid);
            Probe::Free
        });
        assert_eq!(asked, Some(4242));
    }

    #[cfg(windows)]
    #[test]
    fn this_test_binary_is_not_the_hub() {
        // The test process is not hideout.exe: a request from it is refused, nothing exits.
        let v = decide(Some(std::process::id()), |pid| {
            owner::verify_pid(pid, Expect::HIDEOUT, &NoRecords)
        });
        assert!(matches!(v, Verdict::Stay(why) if why.contains("not hideout.exe")));
    }
}
