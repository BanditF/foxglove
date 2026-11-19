// SPDX-License-Identifier: MIT
//! Dev folders (trust model, dev mode): an app inside a folder the user marked keeps its grants
//! over a rebuild; nothing else does. Window-free.

use std::path::PathBuf;

use hideout_registry::dev_folder::{path_within, DEV_ATTESTED_BY};
use hideout_registry::inventory::IdentityClass;
use hideout_registry::keyboard::{canonical_path, same_path, VoidReason};
use hideout_registry::Registry;

const NOW: &str = "2026-11-19T12:00:00Z";
const LATER: &str = "2026-11-19T20:40:00Z";
const ID: &str = "foxglove";

fn temp(name: &str) -> PathBuf {
    // Not %TEMP%: a dev folder must not be writable by other accounts, and a PC's temp folder
    // often is (sandbox users, services).
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("hideout-devfolder-tests")
        .join(format!("{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn manifest(exe: &std::path::Path) -> String {
    format!(
        "id = \"{ID}\"\nname = \"Foxglove\"\nversion = \"1.0.0\"\nkind = \"adopted\"\n\
         family = \"foxglove\"\nhub = \"1.0\"\nrun = [\"{}\"]\napi = {{ foxglove = \"1.0\" }}\n\
         permissions = [\"hub.notify\", \"ui.window\"]\n",
        exe.display().to_string().replace('\\', "/")
    )
}

struct Fx {
    home: PathBuf,
    /// The folder the build output lands in.
    out: PathBuf,
    exe: PathBuf,
    reg: Registry,
}

/// An adopted app at `<out>\foxglove.exe`, with `out` marked as a dev folder.
fn fx(name: &str) -> Fx {
    let out = temp(&format!("{name}-out"));
    let exe = out.join("foxglove.exe");
    std::fs::write(&exe, b"build 1").unwrap();
    let folder = temp(&format!("{name}-manifest"));
    std::fs::write(folder.join("module.toml"), manifest(&exe)).unwrap();
    let home = temp(&format!("{name}-home"));
    let mut reg = Registry::open(&home, true).unwrap();
    reg.register_adopted(&folder, NOW).unwrap();
    reg.add_dev_folder(&out.display().to_string()).unwrap();
    Fx {
        home,
        out,
        exe,
        reg,
    }
}

fn allow(f: &mut Fx, by: &str, chosen: &[&str]) {
    let list = f.reg.permission_list(ID).unwrap();
    let chosen: Vec<String> = chosen.iter().map(|s| s.to_string()).collect();
    let declined: Vec<String> = list
        .items
        .iter()
        .map(|i| i.raw.clone())
        .filter(|r| !chosen.contains(r))
        .collect();
    f.reg
        .approve_listed(ID, list.code(), &chosen, &declined, NOW, by, false)
        .unwrap();
    f.reg.set_enabled(ID, true).unwrap();
}

fn rebuild(f: &Fx, bytes: &[u8]) {
    std::fs::write(&f.exe, bytes).unwrap();
}

fn granted(f: &Fx) -> Vec<String> {
    f.reg.inventory.modules[ID].granted.clone()
}

#[test]
fn a_rebuild_in_a_dev_folder_keeps_the_grants_with_no_new_allow() {
    let mut f = fx("rebuild");
    allow(&mut f, "dialog", &["hub.notify", "ui.window"]);
    let m = &f.reg.inventory.modules[ID];
    let id = m.identity.as_ref().expect("enrolled by the dialog");
    assert!(
        matches!(&id.class, IdentityClass::DevFolder { folder: Some(d) }
        if same_path(d, &canonical_path(&f.out).unwrap()))
    );
    assert_eq!(id.established_by.as_deref(), Some("dialog"));
    assert!(id.sensitive_confirmed.is_empty(), "nothing is inherited");
    assert!(f.reg.dev_folder_of(ID).is_some());
    let before = granted(&f);
    let old = m.bound.as_ref().unwrap().exe_sha256.clone();
    for n in 2..5 {
        rebuild(&f, format!("build {n}").as_bytes());
        assert_eq!(f.reg.binding_void(ID), None, "build {n}");
        f.reg.pre_launch(ID).unwrap();
        assert_eq!(f.reg.void_if_changed(ID, LATER).unwrap(), None);
    }
    let b = f.reg.inventory.modules[ID].bound.clone().unwrap();
    assert!(!b.void);
    assert_ne!(b.exe_sha256, old);
    assert_eq!(b.attested.len(), 3);
    assert!(b.attested.iter().all(|a| a.by == DEV_ATTESTED_BY));
    assert_eq!(granted(&f), before);
    // It is on disk, not only in memory.
    let again = Registry::open(&f.home, true).unwrap();
    assert_eq!(again.binding_void(ID), None);
    assert!(again.inventory.modules[ID].identity.is_some());
}

#[test]
fn a_rebuild_never_adds_a_grant() {
    let mut f = fx("noadd");
    allow(&mut f, "dialog", &["hub.notify"]);
    rebuild(&f, b"build 2");
    f.reg.pre_launch(ID).unwrap();
    assert_eq!(granted(&f), ["hub.notify"]);
    let m = &f.reg.inventory.modules[ID];
    assert!(m.identity.as_ref().unwrap().sensitive_confirmed.is_empty());
}

#[test]
fn with_dev_mode_off_a_rebuild_voids_as_ever() {
    let mut f = fx("off");
    allow(&mut f, "dialog", &["hub.notify", "ui.window"]);
    // The same home read by a shipped build: the list is there, and counts for nothing.
    let mut shipped = Registry::open(&f.home, false).unwrap();
    assert!(!shipped.dev_folders().is_empty());
    assert!(shipped.dev_folder_of(ID).is_none());
    rebuild(&f, b"build 2");
    assert_eq!(shipped.binding_void(ID), Some(VoidReason::Changed));
    assert_eq!(
        shipped.void_if_changed(ID, LATER).unwrap(),
        Some(VoidReason::Changed)
    );
    assert!(shipped.inventory.modules[ID].granted.is_empty());
    assert!(shipped
        .add_dev_folder(&f.out.display().to_string())
        .is_err());
}

#[test]
fn only_the_window_enrols_an_app() {
    let mut f = fx("cli");
    allow(&mut f, "cli", &["hub.notify", "ui.window"]);
    assert!(f.reg.inventory.modules[ID].identity.is_none());
    rebuild(&f, b"build 2");
    assert_eq!(f.reg.binding_void(ID), Some(VoidReason::Changed));
}

#[test]
fn an_app_outside_every_dev_folder_is_not_enrolled() {
    let mut f = fx("outside");
    let out = f.out.display().to_string();
    f.reg.remove_dev_folder(&out).unwrap();
    allow(&mut f, "dialog", &["hub.notify", "ui.window"]);
    assert!(f.reg.inventory.modules[ID].identity.is_none());
    rebuild(&f, b"build 2");
    assert_eq!(f.reg.binding_void(ID), Some(VoidReason::Changed));
}

#[test]
fn removing_the_folder_ends_the_carry_and_a_void_drops_the_identity() {
    let mut f = fx("removed");
    allow(&mut f, "dialog", &["hub.notify", "ui.window"]);
    let out = f.out.display().to_string();
    f.reg.remove_dev_folder(&out).unwrap();
    assert!(f.reg.dev_folders().is_empty());
    rebuild(&f, b"build 2");
    assert_eq!(f.reg.binding_void(ID), Some(VoidReason::Changed));
    assert_eq!(
        f.reg.void_if_changed(ID, LATER).unwrap(),
        Some(VoidReason::Changed)
    );
    let m = &f.reg.inventory.modules[ID];
    assert!(m.granted.is_empty());
    assert!(m.identity.is_none(), "the dev-folder identity went with it");
    assert!(f.reg.dev_folder_of(ID).is_none());
}

#[test]
fn an_app_that_moves_outside_the_folder_is_a_new_identity() {
    let mut f = fx("moved");
    allow(&mut f, "dialog", &["hub.notify", "ui.window"]);
    let elsewhere = temp("moved-elsewhere");
    let exe2 = elsewhere.join("foxglove.exe");
    std::fs::write(&exe2, b"build 1").unwrap();
    // The manifest now runs the copy outside the folder (same bytes, so only the place moved).
    let dir = f.reg.module_dir(ID, "1.0.0");
    std::fs::write(dir.join("module.toml"), manifest(&exe2)).unwrap();
    let mut reg = Registry::open(&f.home, true).unwrap();
    assert_eq!(reg.binding_void(ID), Some(VoidReason::Moved));
    assert_eq!(
        reg.void_if_changed(ID, LATER).unwrap(),
        Some(VoidReason::Moved)
    );
    let m = &reg.inventory.modules[ID];
    assert!(m.granted.is_empty());
    assert!(m.identity.is_none());
}

#[test]
fn a_dev_folder_must_be_a_local_folder_that_is_not_a_drive() {
    let mut f = fx("guards");
    assert!(f.reg.add_dev_folder(r"C:\").is_err());
    assert!(f.reg.add_dev_folder(r"\\server\share\dev").is_err());
    assert!(f.reg.add_dev_folder(&f.exe.display().to_string()).is_err());
    assert!(f
        .reg
        .add_dev_folder(&f.out.join("nope").display().to_string())
        .is_err());
    // Added once, stored canonical, listed again after a reopen.
    let out = f.out.display().to_string();
    assert_eq!(f.reg.dev_folders().len(), 1);
    f.reg.add_dev_folder(&out).unwrap();
    assert_eq!(f.reg.dev_folders().len(), 1);
    let again = Registry::open(&f.home, true).unwrap();
    assert!(same_path(
        &again.dev_folders()[0],
        &canonical_path(&f.out).unwrap()
    ));
}

#[test]
fn a_path_is_inside_a_folder_by_whole_components() {
    assert!(path_within(r"C:\Dev", r"C:\Dev\a\x.exe"));
    assert!(path_within(r"c:/dev/", r"C:\DEV\x.exe"));
    assert!(path_within(r"\\?\C:\Dev", r"C:\Dev\x.exe"));
    assert!(!path_within(r"C:\Dev", r"C:\Dev2\x.exe"));
    assert!(!path_within(r"C:\Dev", r"C:\Dev"));
    assert!(!path_within(r"C:\Dev", r"C:\Other\Dev\x.exe"));
    assert!(!path_within("", r"C:\x.exe"));
}

// ---- review round: folder guards (M1), keyboard record (M2), lows ----

/// Gives Everyone (S-1-1-0) `rights` on `dir` through icacls (`"(OI)(CI)M"` or `"(W)"`).
fn grant_everyone(dir: &std::path::Path, rights: &str) {
    let st = std::process::Command::new("icacls")
        .arg(dir)
        .arg("/grant")
        .arg(format!("*S-1-1-0:{rights}"))
        .output()
        .unwrap();
    assert!(st.status.success(), "icacls failed");
}

fn hand_edit(f: &Fx, folders: &[&str]) {
    let list = folders
        .iter()
        .map(|p| format!("'{p}'"))
        .collect::<Vec<_>>()
        .join(", ");
    std::fs::write(
        f.home.join("hideout.toml"),
        format!("[dev]\nfolders = [{list}]\n"),
    )
    .unwrap();
}

#[test]
fn a_hand_edited_drive_entry_counts_for_nothing() {
    let f = fx("handroot");
    let exe = f.exe.display().to_string();
    assert!(f.reg.dev_folder_for(&exe).is_some());
    let drive = exe[..2].to_string();
    for entry in [format!("{drive}\\"), drive.clone(), format!("{drive}/")] {
        hand_edit(&f, &[&entry]);
        assert!(
            f.reg.dev_folder_for(&exe).is_none(),
            "entry {entry:?} must not make a drive a dev folder"
        );
    }
}

#[test]
fn a_hand_edited_system_or_profile_entry_counts_for_nothing() {
    let f = fx("handsys");
    let exe = f.exe.display().to_string();
    let profile = std::env::var("USERPROFILE").unwrap();
    let windir = std::env::var("SystemRoot").unwrap();
    // A folder that holds the whole profile (and so the test folder) is refused too.
    for entry in [profile, windir, r"C:\Users\Public".into()] {
        hand_edit(&f, &[&entry]);
        assert!(f.reg.dev_folder_for(&exe).is_none(), "{entry:?}");
    }
}

#[test]
fn a_shared_or_system_folder_is_refused_when_added() {
    let mut f = fx("shared");
    for dir in [
        std::env::var("USERPROFILE").unwrap(),
        std::env::var("TEMP").unwrap(),
        std::env::var("SystemRoot").unwrap(),
        std::env::var("ProgramFiles").unwrap(),
        std::env::var("ProgramData").unwrap(),
        r"C:\Users\Public".to_string(),
    ] {
        if std::path::Path::new(&dir).is_dir() {
            assert!(f.reg.add_dev_folder(&dir).is_err(), "{dir}");
        }
    }
}

#[test]
fn a_folder_other_accounts_can_write_is_refused() {
    let mut f = fx("everyone");
    let open = temp("everyone-open");
    grant_everyone(&open, "(OI)(CI)M");
    assert!(f.reg.add_dev_folder(&open.display().to_string()).is_err());
    // A folder above is enough: the child is private, its parent lets anyone swap it out.
    let parent = temp("everyone-parent");
    grant_everyone(&parent, "(W)");
    let child = parent.join("build");
    std::fs::create_dir_all(&child).unwrap();
    assert!(f.reg.add_dev_folder(&child.display().to_string()).is_err());
}

#[test]
fn a_listed_folder_that_becomes_writable_stops_counting() {
    let mut f = fx("becomes");
    allow(&mut f, "dialog", &["hub.notify", "ui.window"]);
    assert!(f.reg.dev_folder_of(ID).is_some());
    grant_everyone(&f.out, "(OI)(CI)M");
    assert!(f.reg.dev_folder_of(ID).is_none());
    rebuild(&f, b"build 2");
    assert_eq!(f.reg.binding_void(ID), Some(VoidReason::Changed));
}

#[test]
fn a_dev_folder_rebuild_does_not_move_the_keyboard_record() {
    let mut f = fx("keyboard");
    allow(&mut f, "dialog", &["hub.notify", "ui.window"]);
    let (path, old) = f.reg.current_program(ID).unwrap();
    f.reg.allow_keyboard(ID, &path, &old, NOW).unwrap();
    rebuild(&f, b"build 2");
    let (_, new) = f.reg.current_program(ID).unwrap();
    // The grants carry, the keyboard grant does not: it stays at the file it was allowed for, so
    // the check on turn-on voids it and the user is asked again.
    assert_eq!(f.reg.void_if_changed(ID, LATER).unwrap(), None);
    assert_eq!(granted(&f).len(), 2);
    let k = f.reg.inventory.modules[ID].keyboard.clone().unwrap();
    assert!(k.allows(&path, &old));
    assert!(!k.allows(&path, &new));
    // On disk too, which is what a fresh router loads.
    let again = Registry::open(&f.home, true).unwrap();
    let k = again.inventory.modules[ID].keyboard.clone().unwrap();
    assert!(!k.allows(&path, &new));
    assert!(again.attested_keyboard_moves().is_empty());
}

#[test]
fn a_deny_in_the_window_does_not_enrol() {
    let mut f = fx("deny");
    let list = f.reg.permission_list(ID).unwrap();
    let declined: Vec<String> = list.items.iter().map(|i| i.raw.clone()).collect();
    f.reg
        .deny_listed(ID, list.code(), &declined, NOW, "dialog")
        .unwrap();
    assert!(f.reg.inventory.modules[ID].identity.is_none());
}

#[test]
fn removing_a_folder_by_another_spelling_works_and_says_so() {
    let mut f = fx("alias");
    let alias = format!(r"{}.", f.out.display());
    assert!(f.reg.remove_dev_folder(&alias).unwrap());
    assert!(f.reg.dev_folders().is_empty());
    assert!(!f.reg.remove_dev_folder(&alias).unwrap());
}
