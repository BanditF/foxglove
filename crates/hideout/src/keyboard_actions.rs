// SPDX-License-Identifier: MIT
//! What the keyboard grant's dialog and row do (#1155-B): Allow pins and records the program
//! file, Not now remembers the decline, Turn off revokes at once. Each change is written by the
//! registry (`inventory.json` for the grant, `hideout.toml` for the decline) and pushed to the
//! router in the same call, so a turn-off closes the legacy module's areas at once. Nothing here
//! grants anything by itself: only `allow` does, and it is called only from the user's Allow.

// Not drawn into the shell yet (the Modules page and the dialog host call these in the next
// step); tested here on their own.
#![allow(dead_code)]

use std::path::Path;

use hideout_registry::keyboard::KeyboardState;
use hideout_registry::Registry;
use hideout_router::server::Hub;
use hideout_shell::keyboard_grant::Situation;

/// Maps the registry's view to the shell's. `started_without_act`: the module started with no
/// click of the user's (autostart, an upgrade), which shows the header line.
pub fn situation(s: KeyboardState, started_without_act: bool) -> Situation {
    match s {
        KeyboardState::NotApplicable => Situation::Standalone,
        KeyboardState::NotAsked => Situation::NotAsked {
            started_without_act,
        },
        KeyboardState::Declined => Situation::Declined,
        KeyboardState::Granted { at } => Situation::Granted { at },
        KeyboardState::Revoked { .. } => Situation::Revoked,
        KeyboardState::Changed { at, reason } => Situation::Changed {
            at,
            moved: reason == hideout_registry::keyboard::VoidReason::Moved,
        },
    }
}

fn push(reg: &Registry, hub: &Hub) {
    let grants = hideout_supervisor::driver::keyboard_grants(reg)
        .into_iter()
        .collect();
    hub.act(|r| r.set_keyboard_grants(grants));
}

/// The path the dialog shows: the canonical full path of the program Allow would bind to.
pub fn dialog_path(home: &Path, id: &str) -> Result<String, String> {
    let reg = Registry::open_from_env(home).map_err(|e| format!("{e:?}"))?;
    let exe = reg.keyboard_exe(id).map_err(|e| format!("{e:?}"))?;
    hideout_registry::keyboard::canonical_path(Path::new(&exe)).map_err(|e| e.to_string())
}

/// Allow: the program file is pinned (write-locked) and hashed through that one handle, and the
/// grant records that path and hash. Refused when the program isn't where the module is
/// registered.
pub fn allow(home: &Path, hub: &Hub, id: &str) -> Result<(), String> {
    let mut reg = Registry::open_from_env(home).map_err(|e| format!("{e:?}"))?;
    let exe = reg.keyboard_exe(id).map_err(|e| format!("{e:?}"))?;
    let pin = hideout_supervisor::pin::Pin::open(Path::new(&exe)).map_err(|e| e.to_string())?;
    reg.allow_keyboard(id, &pin.path, &pin.sha256, &hideout_supervisor::now_utc())
        .map_err(|e| format!("{e:?}"))?;
    push(&reg, hub);
    Ok(())
}

/// Not now, Esc or the close box: remembered in `hideout.toml`, never asked by itself again.
pub fn decline(home: &Path, id: &str) -> Result<(), String> {
    let mut reg = Registry::open_from_env(home).map_err(|e| format!("{e:?}"))?;
    reg.decline_keyboard(id).map_err(|e| format!("{e:?}"))
}

/// Turn off: acts at once. Foxglove keeps running; its areas are dropped from the router now.
pub fn turn_off(home: &Path, hub: &Hub, id: &str) -> Result<(), String> {
    let mut reg = Registry::open_from_env(home).map_err(|e| format!("{e:?}"))?;
    reg.revoke_keyboard(id, &hideout_supervisor::now_utc())
        .map_err(|e| format!("{e:?}"))?;
    push(&reg, hub);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use hideout_router::{Router, Settings};
    use std::sync::mpsc;

    const NOW: &str = "2026-11-12T10:00:00Z";

    fn setup(tag: &str) -> (std::path::PathBuf, Hub) {
        let base = std::env::temp_dir().join(format!("hideout-kact-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let install = base.join("install");
        std::fs::create_dir_all(&install).unwrap();
        let exe = install.join("foxglove.exe");
        std::fs::write(&exe, b"fake").unwrap();
        let folder = base.join("manifest");
        std::fs::create_dir_all(&folder).unwrap();
        let run = exe.display().to_string().replace('\\', "/");
        std::fs::write(
            folder.join("module.toml"),
            format!(
                "id = \"foxglove\"\nname = \"Foxglove\"\nversion = \"0.1.0\"\nkind = \"adopted\"\n\
                 family = \"foxglove\"\nhub = \"1.0\"\nrun = ['{run}']\napi = {{ foxglove = \"1.0\" }}\n"
            ),
        )
        .unwrap();
        let home = base.join("home");
        let mut reg = Registry::open(&home, false).unwrap();
        reg.register_adopted(&folder, NOW).unwrap();
        reg.set_enabled("foxglove", true).unwrap();
        let router = Router::new(Settings::default(), Box::new(|_, asked| asked.to_vec()));
        let (events, _rx) = mpsc::channel();
        (home, Hub::new(router, events))
    }

    fn state(home: &Path) -> KeyboardState {
        Registry::open_from_env(home)
            .unwrap()
            .keyboard_state("foxglove")
    }

    #[test]
    fn allow_records_the_pinned_file_and_turn_off_revokes() {
        let (home, hub) = setup("allow");
        assert_eq!(state(&home), KeyboardState::NotAsked);
        allow(&home, &hub, "foxglove").unwrap();
        assert!(matches!(state(&home), KeyboardState::Granted { .. }));
        let path = dialog_path(&home, "foxglove").unwrap();
        let g = Registry::open_from_env(&home).unwrap().inventory.modules["foxglove"]
            .keyboard
            .clone()
            .unwrap();
        assert!(g.allows(
            &path,
            &hideout_supervisor::sha256_file(Path::new(&path)).unwrap()
        ));
        turn_off(&home, &hub, "foxglove").unwrap();
        assert!(matches!(state(&home), KeyboardState::Revoked { .. }));
        // Turn on again is a new Allow; Turn off never reset anything else.
        allow(&home, &hub, "foxglove").unwrap();
        assert!(matches!(state(&home), KeyboardState::Granted { .. }));
        let _ = std::fs::remove_dir_all(home.parent().unwrap());
    }

    #[test]
    fn not_now_is_remembered_and_grants_nothing() {
        let (home, _hub) = setup("decline");
        decline(&home, "foxglove").unwrap();
        assert_eq!(state(&home), KeyboardState::Declined);
        let reg = Registry::open_from_env(&home).unwrap();
        assert!(!reg.keyboard_needs_ok("foxglove"));
        assert!(reg.inventory.modules["foxglove"].keyboard.is_none());
        let _ = std::fs::remove_dir_all(home.parent().unwrap());
    }

    #[test]
    fn the_registry_states_map_to_the_shells_words() {
        use hideout_registry::keyboard::VoidReason;
        assert_eq!(
            situation(KeyboardState::NotApplicable, true),
            Situation::Standalone
        );
        assert_eq!(
            situation(KeyboardState::NotAsked, true),
            Situation::NotAsked {
                started_without_act: true
            }
        );
        assert_eq!(
            situation(
                KeyboardState::Changed {
                    at: NOW.into(),
                    reason: VoidReason::Moved
                },
                false
            ),
            Situation::Changed {
                at: NOW.into(),
                moved: true
            }
        );
    }
}
