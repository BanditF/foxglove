//! The hub's hand-over request, from the standalone app's side (first-run FR5).
//!
//! When the user allows an app that is also open on its own, Hideout may ask that standalone copy
//! over its pipe to quit cleanly, then start the app inside Hideout (`--module`). The copy obeys
//! only a caller the kernel names as a verified `hideout.exe` (`owner::verify_pid`: same user,
//! Medium integrity or above, no AppContainer, a trusted install folder). The request carries
//! nothing the decision reads. In doubt (no peer id, an unverified image, a process that went)
//! the copy stays open and answers "denied"; it never quits for a guess. Nothing here touches
//! the Startup entry.

use foxglove_plugin::owner::{self, Expect, PeerPaths, Probe};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

/// Where Hideout's installer records its exe: `HKCU\Software\Hideout\Installer`, value `Exe`
/// (Hideout's own installer puts it in its own folder, not this app's).
const HUB_KEY: &str = r"Software\Hideout\Installer";
const HUB_VALUE: &str = "Exe";

/// The hub's installed exe from its installer record, so a hideout.exe in another folder is
/// trusted only when it is exactly the file the installer recorded. A stray hideout.exe with no
/// record (or at another path) is not. `read(key, value)` reads a current-user registry string.
pub struct HubRecord<F: Fn(&str, &str) -> Option<String>>(pub F);

impl<F: Fn(&str, &str) -> Option<String>> PeerPaths for HubRecord<F> {
    fn installed_exe(&self, exe: &str) -> Option<PathBuf> {
        if !exe.eq_ignore_ascii_case("hideout.exe") {
            return None;
        }
        let v = (self.0)(HUB_KEY, HUB_VALUE)?;
        let v = v.trim().trim_matches('"').to_string();
        (!v.is_empty()).then(|| PathBuf::from(v))
    }
    fn run0(&self, _exe: &str) -> Vec<PathBuf> {
        Vec::new()
    }
}

/// The current user's registry string, read as the OS gives it.
#[cfg(windows)]
fn read_hkcu(key: &str, value: &str) -> Option<String> {
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::ERROR_SUCCESS;
    use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_SZ};
    let wide = |t: &str| -> Vec<u16> { t.encode_utf16().chain(std::iter::once(0)).collect() };
    let (k, v) = (wide(key), wide(value));
    let mut buf = [0u16; 1024];
    let mut bytes = (buf.len() * 2) as u32;
    // SAFETY: reads one string into `buf`, whose size in bytes is `bytes`.
    let got = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            PCWSTR(k.as_ptr()),
            PCWSTR(v.as_ptr()),
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr().cast()),
            Some(&mut bytes),
        )
    };
    if got != ERROR_SUCCESS {
        return None;
    }
    let len = (bytes as usize / 2).min(buf.len());
    Some(
        String::from_utf16_lossy(&buf[..len])
            .trim_end_matches('\0')
            .to_string(),
    )
}

#[cfg(not(windows))]
fn read_hkcu(_key: &str, _value: &str) -> Option<String> {
    None
}

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
        owner::verify_pid(pid, Expect::HIDEOUT, &HubRecord(read_hkcu))
    })
}

static QUITTING: AtomicBool = AtomicBool::new(false);

/// How long the answer gets to leave before the app starts handing back (the exit follows it).
const REPLY_GRACE: std::time::Duration = std::time::Duration::from_millis(250);

/// The hub's request at the pipe: the answer goes out, then the app hands back and exits as the
/// tray's Quit does. Anything but a verified hub is refused with nothing changed.
pub fn handle() -> Result<serde_json::Value, foxglove_plugin_api::rpc::Error> {
    use foxglove_plugin_api::rpc::{code, Error};
    match verdict_now() {
        // A second verified request while the first is quitting changes nothing.
        Verdict::Quit if QUITTING.swap(true, Ordering::SeqCst) => Ok(serde_json::json!({})),
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

    #[test]
    fn the_hub_is_trusted_where_its_installer_recorded_it() {
        let seen = std::cell::RefCell::new(Vec::new());
        let at = r"C:\Users\u\AppData\Local\Programs\Hideout\hideout.exe";
        let rec = HubRecord(|k: &str, v: &str| {
            seen.borrow_mut().push(format!("{k}|{v}"));
            Some(format!("\"{at}\""))
        });
        assert_eq!(rec.installed_exe("hideout.exe"), Some(PathBuf::from(at)));
        assert_eq!(
            seen.borrow().as_slice(),
            [r"Software\Hideout\Installer|Exe"]
        );
        // Only the hub's record answers: no record, an empty one, or another program: nothing.
        assert_eq!(rec.installed_exe("evil.exe"), None);
        assert_eq!(
            HubRecord(|_: &str, _: &str| None).installed_exe("hideout.exe"),
            None
        );
        assert_eq!(
            HubRecord(|_: &str, _: &str| Some("  ".into())).installed_exe("hideout.exe"),
            None
        );
        assert!(rec.run0("hideout.exe").is_empty());
    }

    #[cfg(windows)]
    #[test]
    fn this_test_binary_is_not_the_hub() {
        // The test process is not hideout.exe: a request from it is refused, nothing exits.
        let v = decide(Some(std::process::id()), |pid| {
            owner::verify_pid(pid, Expect::HIDEOUT, &HubRecord(read_hkcu))
        });
        assert!(matches!(v, Verdict::Stay(why) if why.contains("not hideout.exe")));
    }
}
