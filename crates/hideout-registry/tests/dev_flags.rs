// SPDX-License-Identifier: MIT
//! The two developer switches stay apart: `HIDEOUT_DEV_UNSIGNED` turns on unsigned sideload and
//! nothing about dev folders; `HIDEOUT_DEV_MODE` turns on the dev-folder path and nothing about
//! sideload. Anything unclear is off. Window-free.

use std::path::{Path, PathBuf};

use hideout_registry::keyboard::VoidReason;
use hideout_registry::{dev_folders_from_env, dev_unsigned_from_env, DevFlags, Failure, Registry};

const NOW: &str = "2026-11-19T12:00:00Z";
const LATER: &str = "2026-11-19T20:40:00Z";
const ID: &str = "foxglove";
const SIDELOAD_ID: &str = "example.private.notes";

const UNSIGNED_ONLY: DevFlags = DevFlags {
    unsigned: true,
    folders: false,
};
const FOLDERS_ONLY: DevFlags = DevFlags {
    unsigned: false,
    folders: true,
};

fn temp(name: &str) -> PathBuf {
    // Not %TEMP%: a dev folder must not be writable by other accounts.
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("hideout-devflags-tests")
        .join(format!("{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn sideload_folder() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/hideout-vectors/trust/folders/example.private.notes-0.1.0")
}

fn id_of(f: &Failure) -> String {
    f.id().map_or_else(|| format!("({f})"), String::from)
}

/// An adopted app at `<out>\foxglove.exe`, `out` a dev folder and the app enrolled by a window
/// Allow, all made with both switches on. Returns (home, exe).
fn enrolled(name: &str) -> (PathBuf, PathBuf) {
    let out = temp(&format!("{name}-out"));
    let exe = out.join("foxglove.exe");
    std::fs::write(&exe, b"build 1").unwrap();
    let folder = temp(&format!("{name}-manifest"));
    std::fs::write(
        folder.join("module.toml"),
        format!(
            "id = \"{ID}\"\nname = \"Foxglove\"\nversion = \"1.0.0\"\nkind = \"adopted\"\n\
             family = \"foxglove\"\nhub = \"1.0\"\nrun = [\"{}\"]\n\
             api = {{ foxglove = \"1.0\" }}\npermissions = [\"hub.notify\"]\n",
            exe.display().to_string().replace('\\', "/")
        ),
    )
    .unwrap();
    let home = temp(&format!("{name}-home"));
    let mut reg = Registry::open_with(&home, DevFlags::both(true)).unwrap();
    reg.register_adopted(&folder, NOW).unwrap();
    reg.add_dev_folder(&out.display().to_string()).unwrap();
    let list = reg.permission_list(ID).unwrap();
    let chosen = vec!["hub.notify".to_string()];
    reg.approve_listed(ID, list.code(), &chosen, &[], NOW, "dialog", false)
        .unwrap();
    reg.set_enabled(ID, true).unwrap();
    assert!(reg.dev_folder_of(ID).is_some(), "the fixture is enrolled");
    (home, exe)
}

#[test]
fn unsigned_alone_allows_sideload_and_no_dev_folder() {
    let home = temp("unsigned-home");
    let mut reg = Registry::open_with(&home, UNSIGNED_ONLY).unwrap();
    assert!(!reg.dev_folders_on());
    // Sideload works.
    let m = reg.install(&sideload_folder(), NOW).unwrap();
    assert_eq!(m.source, "sideload");
    reg.set_enabled(SIDELOAD_ID, true).unwrap();
    assert!(reg.pre_launch(SIDELOAD_ID).is_ok());
    // The dev-folder path does not.
    let out = temp("unsigned-out");
    let err = reg.add_dev_folder(&out.display().to_string()).unwrap_err();
    assert!(err.to_string().contains("developer mode is off"), "{err}");
    assert!(reg.dev_folders().is_empty());
}

#[test]
fn dev_folders_alone_allow_dev_folders_and_no_sideload() {
    let home = temp("folders-home");
    let mut reg = Registry::open_with(&home, FOLDERS_ONLY).unwrap();
    assert!(reg.dev_folders_on());
    // The dev-folder path works.
    let out = temp("folders-out");
    reg.add_dev_folder(&out.display().to_string()).unwrap();
    assert_eq!(reg.dev_folders().len(), 1);
    // Sideload does not: not at install, not at launch of one installed earlier.
    assert_eq!(
        id_of(&reg.install(&sideload_folder(), NOW).unwrap_err()),
        "install.dev-mode-required"
    );
    let mut earlier = Registry::open_with(&home, UNSIGNED_ONLY).unwrap();
    earlier.install(&sideload_folder(), NOW).unwrap();
    earlier.set_enabled(SIDELOAD_ID, true).unwrap();
    let later = Registry::open_with(&home, FOLDERS_ONLY).unwrap();
    assert_eq!(
        id_of(&later.pre_launch(SIDELOAD_ID).unwrap_err()),
        "install.dev-mode-required"
    );
}

#[test]
fn neither_switch_allows_either_path() {
    let home = temp("neither-home");
    let mut reg = Registry::open_with(&home, DevFlags::OFF).unwrap();
    assert!(!reg.dev_folders_on());
    assert_eq!(
        id_of(&reg.install(&sideload_folder(), NOW).unwrap_err()),
        "install.dev-mode-required"
    );
    let out = temp("neither-out");
    assert!(reg.add_dev_folder(&out.display().to_string()).is_err());
}

#[test]
fn unsigned_alone_does_not_carry_an_enrolled_dev_folder_app() {
    let (home, exe) = enrolled("carry-unsigned");
    // The same home read with only the sideload switch: the enrolment ends, on disk.
    let mut reg = Registry::open_with(&home, UNSIGNED_ONLY).unwrap();
    assert!(reg.inventory.modules[ID].identity.is_none());
    assert!(reg.dev_folder_of(ID).is_none());
    std::fs::write(&exe, b"build 2").unwrap();
    assert_eq!(reg.binding_void(ID), Some(VoidReason::Changed));
    assert!(reg.attested_keyboard_moves().is_empty());
    reg.void_if_changed(ID, LATER).unwrap();
}

#[test]
fn dev_folders_alone_carry_an_enrolled_app_over_a_rebuild() {
    let (home, exe) = enrolled("carry-folders");
    let reg = Registry::open_with(&home, FOLDERS_ONLY).unwrap();
    assert!(reg.dev_folder_of(ID).is_some());
    std::fs::write(&exe, b"build 2").unwrap();
    assert_eq!(reg.binding_void(ID), None);
}

/// One test owns the process environment (the tests above never read it).
#[test]
fn the_environment_flags_are_independent_and_exact() {
    const VARS: [&str; 3] = [
        "HIDEOUT_DEV_UNSIGNED",
        "HIDEOUT_DEV_MODE",
        "FOXGLOVE_DEV_UNSIGNED_PLUGINS",
    ];
    let set = |pairs: &[(&str, &str)]| {
        for v in VARS {
            std::env::remove_var(v);
        }
        for (k, val) in pairs {
            std::env::set_var(k, val);
        }
        (dev_unsigned_from_env(), dev_folders_from_env())
    };
    assert_eq!(set(&[]), (false, false));
    assert_eq!(set(&[("HIDEOUT_DEV_UNSIGNED", "1")]), (true, false));
    assert_eq!(set(&[("HIDEOUT_DEV_MODE", "1")]), (false, true));
    assert_eq!(
        set(&[("HIDEOUT_DEV_UNSIGNED", "1"), ("HIDEOUT_DEV_MODE", "1")]),
        (true, true)
    );
    // The legacy Foxglove variable is sideload only.
    assert_eq!(
        set(&[("FOXGLOVE_DEV_UNSIGNED_PLUGINS", "1")]),
        (true, false)
    );
    // Exactly "1": anything else is off.
    for bad in ["0", "", "true", "yes", " 1", "1 "] {
        assert_eq!(
            set(&[("HIDEOUT_DEV_UNSIGNED", bad)]),
            (false, false),
            "{bad:?}"
        );
        assert_eq!(set(&[("HIDEOUT_DEV_MODE", bad)]), (false, false), "{bad:?}");
    }
    // A registry opened from the environment follows the same split.
    let home = temp("env-home");
    set(&[("HIDEOUT_DEV_UNSIGNED", "1")]);
    let reg = Registry::open_from_env(&home).unwrap();
    assert!((reg.dev_unsigned, reg.dev_folders) == (true, false));
    set(&[("HIDEOUT_DEV_MODE", "1")]);
    let reg = Registry::open_from_env(&home).unwrap();
    assert!((reg.dev_unsigned, reg.dev_folders) == (false, true));
    set(&[]);
}
