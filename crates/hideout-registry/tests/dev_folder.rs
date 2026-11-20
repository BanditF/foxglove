// SPDX-License-Identifier: MIT
//! Dev folders (trust model, dev mode): an app inside a folder the user marked keeps its grants
//! over a rebuild; nothing else does. Window-free.

use std::os::windows::process::CommandExt as _;
use std::path::PathBuf;

use hideout_registry::dev_folder::{clear_tree_cache, path_within, DEV_ATTESTED_BY};
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

/// DF1: Remove takes effect for carries at once. A copy of the registry that was opened before
/// the Remove (the supervisor's, with the old enrolment still in its memory) reads the folder list
/// from disk at every carry, so a build that lands right after the Remove is not carried: the
/// carry itself is refused (the binding is not moved), the grant voids at that build, and the
/// copy that was running is left alone.
#[test]
fn a_carry_attempted_right_after_remove_is_refused_even_on_a_registry_opened_before() {
    let mut f = fx("carry-after-remove");
    allow(&mut f, "dialog", &["hub.notify", "ui.window"]);
    let out = f.out.display().to_string();
    // The other holder of the registry (a hub thread that read it earlier) still has the carry.
    let mut stale = Registry::open(&f.home, true).unwrap();
    assert!(stale.dev_folder_of(ID).is_some());
    let bound_before = stale.inventory.modules[ID].bound.clone().unwrap();
    assert!(f.reg.remove_dev_folder(&out).unwrap());
    // Now, with no restart and no reload: the stale copy no longer counts the app as enrolled.
    assert!(stale.dev_folder_of(ID).is_none());
    rebuild(&f, b"build 2");
    // The carry is refused: nothing is moved, nothing is attested.
    assert!(!stale.keep_followed(ID, LATER).unwrap());
    let b = stale.inventory.modules[ID].bound.clone().unwrap();
    assert_eq!(
        b.exe_sha256, bound_before.exe_sha256,
        "the binding did not move"
    );
    assert!(b.attested.iter().all(|a| a.by != DEV_ATTESTED_BY));
    // The check at a start voids the grant at this build, as for any app.
    assert_eq!(
        stale.void_if_changed(ID, LATER).unwrap(),
        Some(VoidReason::Changed)
    );
    assert!(stale.inventory.modules[ID].granted.is_empty());
    // On disk, too, and the running copy's file was never touched by the Remove.
    let disk = Registry::open(&f.home, true).unwrap();
    assert!(disk.inventory.modules[ID].identity.is_none());
    assert!(f.exe.exists());
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

#[test]
fn a_case_variant_is_inside_and_a_shared_prefix_is_not() {
    assert!(path_within(r"C:\Dev\Out", r"c:\dev\OUT\x.exe"));
    assert!(!path_within(r"C:\Dev\Out", r"C:\Dev\Outside\x.exe"));
}

// ---- review round: folder guards (M1), keyboard record (M2), lows ----

/// Gives Everyone (S-1-1-0) `rights` on `dir` through icacls (`"(OI)(CI)M"` or `"(W)"`).
fn grant_everyone(dir: &std::path::Path, rights: &str) {
    let st = std::process::Command::new("icacls")
        .creation_flags(0x0800_0000)
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

/// An enrolled app with the keyboard allowed for build 1, then `n` more builds carried.
fn with_keyboard(name: &str, n: usize) -> Fx {
    let mut f = fx(name);
    allow(&mut f, "dialog", &["hub.notify", "ui.window"]);
    let (path, old) = f.reg.current_program(ID).unwrap();
    f.reg.allow_keyboard(ID, &path, &old, NOW).unwrap();
    for i in 2..2 + n {
        rebuild(&f, format!("build {i}").as_bytes());
        assert_eq!(f.reg.void_if_changed(ID, LATER).unwrap(), None);
    }
    f
}

#[test]
fn the_keyboard_record_follows_a_chain_of_three_dev_rebuilds() {
    let f = with_keyboard("kb-chain", 3);
    let (path, new) = f.reg.current_program(ID).unwrap();
    let k = f.reg.inventory.modules[ID].keyboard.clone().unwrap();
    assert!(k.allows(&path, &new), "the record moved on disk");
    let moves = f.reg.attested_keyboard_moves();
    assert_eq!(moves.len(), 1);
    assert_eq!(moves[0].ancestors.len(), 3);
    assert!(moves[0].dev_folder.is_some());
    assert!(moves[0].to.allows(&path, &new));
    // A fresh registry on the same home sees the same chain.
    let again = Registry::open(&f.home, true).unwrap();
    assert_eq!(again.attested_keyboard_moves(), moves);
}

#[test]
fn a_path_change_inside_the_folder_keeps_the_chain_and_one_outside_ends_it() {
    let mut f = with_keyboard("kb-path", 1);
    // The manifest now names another file inside the folder.
    let other = f.out.join("foxglove2.exe");
    std::fs::write(&other, b"build 3").unwrap();
    let dir = f.reg.module_dir(ID, "1.0.0");
    std::fs::write(dir.join("module.toml"), manifest(&other)).unwrap();
    f.reg = Registry::open(&f.home, true).unwrap();
    assert_eq!(f.reg.void_if_changed(ID, LATER).unwrap(), None);
    let (path, new) = f.reg.current_program(ID).unwrap();
    assert!(same_path(&path, &canonical_path(&other).unwrap()));
    let moves = f.reg.attested_keyboard_moves();
    assert_eq!(moves.len(), 1);
    assert_eq!(moves[0].ancestors.len(), 2);
    assert!(moves[0].to.allows(&path, &new));

    // A program outside the folder is no dev step: the grants and the keyboard void.
    let mut g = fx("kb-path-out");
    allow(&mut g, "dialog", &["hub.notify", "ui.window"]);
    let (p0, h0) = g.reg.current_program(ID).unwrap();
    g.reg.allow_keyboard(ID, &p0, &h0, NOW).unwrap();
    let outside = temp("kb-path-elsewhere").join("foxglove.exe");
    std::fs::write(&outside, b"build 2").unwrap();
    let dir = g.reg.module_dir(ID, "1.0.0");
    std::fs::write(dir.join("module.toml"), manifest(&outside)).unwrap();
    g.reg = Registry::open(&g.home, true).unwrap();
    assert!(g.reg.void_if_changed(ID, LATER).unwrap().is_some());
    assert!(g.reg.attested_keyboard_moves().is_empty());
}

#[test]
fn the_chain_ends_when_the_folder_is_removed() {
    let mut f = with_keyboard("kb-removed", 3);
    assert_eq!(f.reg.attested_keyboard_moves().len(), 1);
    f.reg
        .remove_dev_folder(&f.out.display().to_string())
        .unwrap();
    assert!(f.reg.attested_keyboard_moves().is_empty());
}

#[test]
fn the_chain_is_nothing_with_dev_mode_off() {
    let f = with_keyboard("kb-off", 3);
    let shipped = Registry::open(&f.home, false).unwrap();
    assert!(shipped.attested_keyboard_moves().is_empty());
}

#[test]
fn the_chain_ends_when_the_exe_or_folder_is_opened_to_others() {
    let f = with_keyboard("kb-acl-exe", 2);
    grant_everyone(&f.exe, "M");
    assert!(f.reg.attested_keyboard_moves().is_empty());
    let g = with_keyboard("kb-acl-dir", 2);
    grant_everyone(&g.out, "(OI)(CI)M");
    assert!(g.reg.attested_keyboard_moves().is_empty());
}

#[test]
fn a_cli_allow_ends_the_chain() {
    let mut f = with_keyboard("kb-cli", 2);
    assert_eq!(f.reg.attested_keyboard_moves().len(), 1);
    // The Allow of the command line, over whatever the list shows now.
    let list = f.reg.permission_list(ID).unwrap();
    let chosen: Vec<String> = list.items.iter().map(|i| i.raw.clone()).collect();
    f.reg
        .approve_listed(ID, list.code(), &chosen, &[], LATER, "cli", false)
        .unwrap();
    assert!(f.reg.inventory.modules[ID].identity.is_none());
    assert!(f.reg.attested_keyboard_moves().is_empty());
}

#[test]
fn a_dev_step_not_recorded_by_this_enrolment_gives_no_move() {
    // The record on disk names a hash the history does not reach (an edit): no move.
    let f = with_keyboard("kb-edit", 2);
    let mut reg = Registry::open(&f.home, true).unwrap();
    let k = reg
        .inventory
        .modules
        .get_mut(ID)
        .unwrap()
        .keyboard
        .as_mut()
        .unwrap();
    k.sha256 = "ab".repeat(32);
    assert!(reg.attested_keyboard_moves().is_empty());
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

// ---- re-review: below the folder (M3), conditional ACEs (L8), drop zones (L7) ----

/// A dev-folder app at `<out>\suboxglove.exe`, enrolled, then `edit` runs on the layout.
fn nested(name: &str) -> (Fx, PathBuf) {
    let mut f = fx(name);
    let sub = f.out.join("sub");
    std::fs::create_dir_all(&sub).unwrap();
    let exe = sub.join("foxglove.exe");
    std::fs::write(&exe, b"build 1").unwrap();
    let dir = f.reg.module_dir(ID, "1.0.0");
    std::fs::write(dir.join("module.toml"), manifest(&exe)).unwrap();
    let reg = Registry::open(&f.home, true).unwrap();
    f.reg = reg;
    f.exe = exe;
    (f, sub)
}

#[test]
fn a_subfolder_other_accounts_can_write_is_not_carried() {
    let (mut f, sub) = nested("sub-open");
    allow(&mut f, "dialog", &["hub.notify", "ui.window"]);
    assert!(
        f.reg.dev_folder_of(ID).is_some(),
        "private subfolder is fine"
    );
    grant_everyone(&sub, "(OI)(CI)M");
    assert!(f.reg.dev_folder_of(ID).is_none());
    rebuild(&f, b"build 2");
    assert_eq!(f.reg.binding_void(ID), Some(VoidReason::Changed));
}

#[test]
fn an_exe_other_accounts_can_write_is_not_carried() {
    let (mut f, _) = nested("exe-open");
    allow(&mut f, "dialog", &["hub.notify", "ui.window"]);
    assert!(f.reg.dev_folder_of(ID).is_some());
    grant_everyone(&f.exe, "M");
    assert!(f.reg.dev_folder_of(ID).is_none());
    rebuild(&f, b"build 2");
    assert_eq!(f.reg.binding_void(ID), Some(VoidReason::Changed));
}

#[test]
fn a_conditional_allow_ace_counts_as_write() {
    let mut f = fx("conditional");
    let dir = temp("conditional-dir");
    // A callback ACE (SDDL XA) gives Everyone full control when a condition holds.
    let script = format!(
        "$p='{}'; $a=Get-Acl -LiteralPath $p;          $a.SetSecurityDescriptorSddlForm($a.Sddl + '(XA;OICI;FA;;;WD;(Member_of{{SID(BU)}}))');          Set-Acl -LiteralPath $p -AclObject $a",
        dir.display()
    );
    let out = std::process::Command::new("powershell")
        .creation_flags(0x0800_0000)
        .args(["-NoProfile", "-Command", &script])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(f.reg.add_dev_folder(&dir.display().to_string()).is_err());
}

#[test]
fn the_children_of_temp_and_appdata_are_refused() {
    let mut f = fx("dropzones");
    for var in ["TEMP", "LOCALAPPDATA", "APPDATA"] {
        let base = PathBuf::from(std::env::var(var).unwrap());
        let child = base.join(format!("hideout-devfolder-refused-{}", std::process::id()));
        std::fs::create_dir_all(&child).unwrap();
        let r = f.reg.add_dev_folder(&child.display().to_string());
        let _ = std::fs::remove_dir_all(&child);
        assert!(r.is_err(), "{var}");
    }
}

// ---- review of R1: a refused dev step must not leave the file `on` (M1), refusals (L2) ----

use hideout_registry::keyboard::KeyboardState;

fn granted_for(reg: &Registry) -> bool {
    matches!(reg.keyboard_state(ID), KeyboardState::Granted { .. })
}

/// What the hub does before it reads the file: void a dev move whose chain broke.
fn hub_pass(home: &std::path::Path) -> (Registry, Vec<String>) {
    let mut reg = Registry::open(home, true).unwrap();
    let voided = reg.void_unchained_dev_keyboards(LATER);
    (reg, voided)
}

#[test]
fn a_chain_that_still_holds_is_left_on() {
    let f = with_keyboard("m1-ok", 2);
    let (reg, voided) = hub_pass(&f.home);
    assert!(voided.is_empty());
    assert!(granted_for(&reg));
}

#[test]
fn a_refused_dev_move_voids_the_record_on_disk_and_stays_void_after_a_reload() {
    // Folder removed, dev mode off and an exe opened to others all make the chain refuse.
    let mut f = with_keyboard("m1-folder", 2);
    f.reg
        .remove_dev_folder(&f.out.display().to_string())
        .unwrap();
    let (reg, voided) = hub_pass(&f.home);
    assert_eq!(voided, [ID]);
    assert!(!granted_for(&reg));
    let (_, new) = reg.current_program(ID).unwrap();
    assert!(!reg.inventory.modules[ID]
        .keyboard
        .clone()
        .unwrap()
        .allows(&reg.current_program(ID).unwrap().0, &new));
    // A fresh read of the file (what a restarted router loads) still says void, and re-adding
    // the folder does not revive it.
    let mut again = Registry::open(&f.home, true).unwrap();
    assert!(!granted_for(&again));
    again.add_dev_folder(&f.out.display().to_string()).unwrap();
    let again = Registry::open(&f.home, true).unwrap();
    assert!(!granted_for(&again));
    assert!(again.attested_keyboard_moves().is_empty());

    let g = with_keyboard("m1-off", 2);
    let mut shipped = Registry::open(&g.home, false).unwrap();
    assert_eq!(shipped.void_unchained_dev_keyboards(LATER), [ID]);
    assert!(!granted_for(&Registry::open(&g.home, true).unwrap()));

    let h = with_keyboard("m1-acl", 2);
    grant_everyone(&h.exe, "M");
    let (reg, voided) = hub_pass(&h.home);
    assert_eq!(voided, [ID]);
    assert!(!granted_for(&reg));
    assert!(!granted_for(&Registry::open(&h.home, true).unwrap()));
}

#[test]
fn a_fresh_allow_after_the_rebuild_is_not_voided() {
    let mut f = with_keyboard("m1-fresh", 2);
    let (path, new) = f.reg.current_program(ID).unwrap();
    f.reg
        .allow_keyboard(ID, &path, &new, "2026-11-19T23:00:00Z")
        .unwrap();
    f.reg
        .remove_dev_folder(&f.out.display().to_string())
        .unwrap();
    let (reg, voided) = hub_pass(&f.home);
    assert!(voided.is_empty());
    assert!(granted_for(&reg));
}

/// Edits `inventory.json` of the fixture by hand, then reads it again.
fn edited(f: &Fx, edit: impl FnOnce(&mut serde_json::Value)) -> Registry {
    let p = f.home.join("inventory.json");
    let mut v: serde_json::Value = serde_json::from_slice(&std::fs::read(&p).unwrap()).unwrap();
    edit(&mut v["modules"][ID]);
    std::fs::write(&p, serde_json::to_vec(&v).unwrap()).unwrap();
    Registry::open(&f.home, true).unwrap()
}

#[test]
fn hand_edited_histories_give_no_move() {
    // Each case starts from a chain that does give a move, so the edit is what refuses.
    let case = |name: &str, edit: &dyn Fn(&mut serde_json::Value)| {
        let f = with_keyboard(name, 3);
        assert_eq!(f.reg.attested_keyboard_moves().len(), 1, "{name} control");
        let reg = edited(&f, |m| edit(m));
        assert!(reg.attested_keyboard_moves().is_empty(), "{name}");
    };
    // The module is not enrolled (no identity), or was enrolled by the command line.
    case("no-identity", &|m| m["identity"] = serde_json::Value::Null);
    case("cli-enrolled", &|m| {
        m["identity"]["established_by"] = "cli".into()
    });
    // The newest step starts outside the folder.
    case("from-outside", &|m| {
        let a = m["bound"]["attested"].as_array_mut().unwrap();
        a.last_mut().unwrap()["from_path"] = r"C:\Elsewhere\foxglove.exe".into();
    });
    // A splice: the newest step does not end where the bound file is.
    case("spliced", &|m| {
        let a = m["bound"]["attested"].as_array_mut().unwrap();
        let sub = a.last().unwrap()["to_path"]
            .as_str()
            .unwrap()
            .replace("foxglove.exe", "other/foxglove.exe");
        a.last_mut().unwrap()["to_path"] = sub.into();
    });
    // Steps older than the binding are not its own.
    case("older-than-binding", &|m| {
        m["bound"]["at"] = "2099-01-01T00:00:00Z".into()
    });
}

#[test]
fn removing_a_folder_and_adding_it_back_needs_an_allow_again() {
    let mut f = fx("readd");
    allow(&mut f, "dialog", &["hub.notify", "ui.window"]);
    let out = f.out.display().to_string();
    assert!(f.reg.dev_folder_of(ID).is_some());
    f.reg.remove_dev_folder(&out).unwrap();
    assert!(f.reg.inventory.modules[ID].identity.is_none());
    f.reg.add_dev_folder(&out).unwrap();
    // Re-read from disk: the identity is gone there too, not only in memory.
    let again = Registry::open(&f.home, true).unwrap();
    assert!(again.inventory.modules[ID].identity.is_none());
    assert!(again.dev_folder_of(ID).is_none());
    rebuild(&f, b"build 2");
    assert_eq!(again.binding_void(ID), Some(VoidReason::Changed));
    assert!(again.attested_keyboard_moves().is_empty());
}

#[test]
fn turning_dev_mode_off_and_on_needs_an_allow_again() {
    let mut f = fx("offon");
    allow(&mut f, "dialog", &["hub.notify", "ui.window"]);
    assert!(f.reg.inventory.modules[ID].identity.is_some());
    // A launch with dev mode off ends the enrolment, on disk.
    let shipped = Registry::open(&f.home, false).unwrap();
    assert!(shipped.inventory.modules[ID].identity.is_none());
    // Dev mode on again: the folder is still listed, but nothing is enrolled.
    let back = Registry::open(&f.home, true).unwrap();
    assert!(!back.dev_folders().is_empty());
    assert!(back.inventory.modules[ID].identity.is_none());
    assert!(back.dev_folder_of(ID).is_none());
    rebuild(&f, b"build 2");
    assert_eq!(back.binding_void(ID), Some(VoidReason::Changed));
    // A new Allow in the window enrols it again.
    f.reg = back;
    f.reg.void_if_changed(ID, LATER).unwrap();
    allow(&mut f, "dialog", &["hub.notify", "ui.window"]);
    assert!(f.reg.inventory.modules[ID].identity.is_some());
}

#[test]
fn a_hand_edited_removal_ends_the_enrolment_and_re_listing_needs_an_allow() {
    let mut f = fx("handremove");
    allow(&mut f, "dialog", &["hub.notify", "ui.window"]);
    let out = canonical_path(&f.out).unwrap();
    hand_edit(&f, &[]);
    let gone = Registry::open(&f.home, true).unwrap();
    assert!(gone.inventory.modules[ID].identity.is_none());
    hand_edit(&f, &[&out]);
    let back = Registry::open(&f.home, true).unwrap();
    assert!(back.dev_folder_of(ID).is_none());
    rebuild(&f, b"build 2");
    assert_eq!(back.binding_void(ID), Some(VoidReason::Changed));
}

/// `mklink /J link target` (a junction needs no admin rights). cmd reads `/` as a switch, so the
/// paths go in with backslashes only.
fn junction(link: &std::path::Path, target: &std::path::Path) {
    let plain = |p: &std::path::Path| p.display().to_string().replace('/', "\\");
    let st = std::process::Command::new("cmd")
        .creation_flags(0x0800_0000)
        .args(["/C", "mklink", "/J"])
        .arg(plain(link))
        .arg(plain(target))
        .output()
        .unwrap();
    assert!(
        st.status.success(),
        "mklink failed: {} {}",
        String::from_utf8_lossy(&st.stdout),
        String::from_utf8_lossy(&st.stderr)
    );
}

#[test]
fn a_junction_out_of_the_folder_or_an_alias_into_it_never_matches_by_spelling() {
    let f = fx("junction");
    let canon_exe = canonical_path(&f.exe).unwrap();
    assert!(f.reg.dev_folder_for(&canon_exe).is_some());
    // A junction inside the folder that leads outside: the program behind it canonicalizes to
    // the outside place, which is in no dev folder.
    let outside = temp("junction-outside");
    std::fs::write(outside.join("foxglove.exe"), b"evil").unwrap();
    let link = f.out.join("escape");
    junction(&link, &outside);
    let behind = canonical_path(&link.join("foxglove.exe")).unwrap();
    assert!(
        !path_within(&canonical_path(&f.out).unwrap(), &behind),
        "{behind}"
    );
    assert!(f.reg.dev_folder_for(&behind).is_none());
    // The uncanonical spelling through the junction is not inside the folder either as text:
    // only canonical paths are ever asked, and a raw alias never matches.
    let spelled = link.join("foxglove.exe").display().to_string();
    assert!(f.reg.dev_folder_for(&spelled).is_none());
    // The escaping junction would now keep the whole folder from counting; drop it first.
    assert!(f.reg.dev_folder_for(&canon_exe).is_none());
    std::fs::remove_dir(&link).unwrap();
    // A junction to the folder from elsewhere: its raw spelling is another path and never
    // matches; its canonical form is the folder's own.
    let alias = temp("junction-alias").join("in");
    junction(&alias, &f.out);
    let raw = alias.join("foxglove.exe").display().to_string();
    assert!(f.reg.dev_folder_for(&raw).is_none());
    // Same file, so only the spelling differs: the canonical form is accepted, the raw one is not.
    let via = canonical_path(&alias.join("foxglove.exe")).unwrap();
    assert!(same_path(&via, &canon_exe));
    assert!(f.reg.dev_folder_for(&via).is_some());
}

#[test]
fn an_8_3_short_name_or_other_case_is_the_same_folder_not_another_one() {
    let f = fx("short-name-folder-with-a-long-name");
    let canon_exe = canonical_path(&f.exe).unwrap();
    // Case variants compare equal.
    assert!(f.reg.dev_folder_for(&canon_exe.to_uppercase()).is_some());
    // The short spelling (when the volume makes one) canonicalizes back to the long one; a raw
    // short spelling never matches by text.
    let out = std::process::Command::new("cmd")
        .creation_flags(0x0800_0000)
        .args(["/C", "for", "%I", "in"])
        .arg(format!("({})", f.exe.display()))
        .args(["do", "@echo", "%~sI"])
        .output()
        .unwrap();
    let short = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if !short.is_empty() && short.to_lowercase() != f.exe.display().to_string().to_lowercase() {
        assert!(f.reg.dev_folder_for(&short).is_none(), "{short}");
        assert!(f
            .reg
            .dev_folder_for(&canonical_path(std::path::Path::new(&short)).unwrap())
            .is_some());
    }
}

#[test]
fn a_dotdot_path_inside_the_folder_text_never_matches() {
    let f = fx("dotdot");
    let outside = temp("dotdot-outside");
    std::fs::write(outside.join("foxglove.exe"), b"evil").unwrap();
    let name = outside.file_name().unwrap().to_string_lossy().into_owned();
    // `<folder>\..\<sibling>oxglove.exe` starts with the folder's text but is the sibling's file.
    let canon_out = canonical_path(&f.out).unwrap();
    let sneaky = format!(r"{canon_out}\..\{name}\foxglove.exe");
    assert!(
        path_within(&canon_out, &sneaky),
        "the text looks inside: {sneaky}"
    );
    assert!(std::path::Path::new(&sneaky).exists(), "{sneaky}");
    assert!(f.reg.dev_folder_for(&sneaky).is_none());
    // The real file, spelled canonically, is accepted.
    assert!(f
        .reg
        .dev_folder_for(&canonical_path(&f.exe).unwrap())
        .is_some());
}

// ---- R5: Settings > Developer ---------------------------------------------------------------

mod settings {
    use super::*;
    use hideout_registry::dev_folder::{DevFolderRefusal, FolderProblem, RowProblem};

    #[test]
    fn try_add_says_why_in_a_value_and_writes_nothing_when_refused() {
        let home = temp("try-home");
        let mut reg = Registry::open(&home, true).unwrap();
        let gone = temp("try-gone").join("nope");
        assert!(matches!(
            reg.try_add_dev_folder(&gone.display().to_string()),
            Err(DevFolderRefusal::NotAFolder)
        ));
        assert!(matches!(
            reg.try_add_dev_folder(r"C:\"),
            Err(DevFolderRefusal::Problem(FolderProblem::Drive))
        ));
        assert!(reg.dev_folders().is_empty());
        let ok = temp("try-ok");
        let a = reg.try_add_dev_folder(&ok.display().to_string()).unwrap();
        assert!(!a.was_listed);
        assert!(
            reg.try_add_dev_folder(&ok.display().to_string())
                .unwrap()
                .was_listed
        );
        assert_eq!(reg.dev_folders(), vec![a.path]);
        // Mode off: refused before anything else, nothing written.
        let mut off = Registry::open(&temp("try-off"), false).unwrap();
        assert!(matches!(
            off.try_add_dev_folder(&ok.display().to_string()),
            Err(DevFolderRefusal::ModeOff)
        ));
        assert!(off.dev_folders().is_empty());
    }

    #[test]
    fn the_old_string_api_keeps_its_words() {
        let mut reg = Registry::open(&temp("old-home"), true).unwrap();
        let e = format!("{:?}", reg.add_dev_folder(r"C:\").unwrap_err());
        assert!(
            e.contains("a drive is too much to trust as a dev folder"),
            "{e}"
        );
        assert_eq!(
            hideout_registry::dev_folder::folder_problem(r"C:\"),
            Some("a drive is too much to trust as a dev folder")
        );
    }

    #[test]
    fn rows_name_the_apps_and_the_folders_that_stopped_qualifying() {
        let f = fx("rows");
        let rows = f.reg.dev_folder_rows();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].problem, None);
        // Enrolment happens when the app is allowed from the folder.
        let mut f = f;
        allow(&mut f, "dialog", &["hub.notify", "ui.window"]);
        let rows = f.reg.dev_folder_rows();
        assert_eq!(rows[0].apps, vec!["Foxglove".to_string()]);
        std::fs::remove_dir_all(&f.out).unwrap();
        assert_eq!(
            f.reg.dev_folder_rows()[0].problem,
            Some(RowProblem::Missing)
        );
    }

    // ---- DF3 and DF4: the place checks, with fake probes ----------------------------------------

    use hideout_registry::dev_folder::DevProbes;
    use hideout_registry::drive::{BusType, DriveProbe, NotLocalKind};
    use hideout_registry::sync_root::{SyncAnswer, SyncProbe};
    use std::sync::{Arc, Mutex};

    /// One answer for every drive.
    #[derive(Clone, Copy)]
    struct Drives {
        fixed: bool,
        bus: Option<BusType>,
        remote: bool,
    }
    impl DriveProbe for Drives {
        fn is_fixed(&self, _: &str) -> bool {
            self.fixed
        }
        fn bus_type(&self, _: &str) -> Option<BusType> {
            self.bus
        }
        fn is_remote(&self, _: &str) -> bool {
            self.remote
        }
    }
    const INTERNAL: Drives = Drives {
        fixed: true,
        bus: Some(BusType::Nvme),
        remote: false,
    };

    type Answer = Box<dyn Fn(&str) -> SyncAnswer + Send>;

    /// Answers by path, changeable while a registry holds it.
    #[derive(Clone)]
    struct Fake(Arc<Mutex<Answer>>);
    impl Fake {
        fn all(a: SyncAnswer) -> Fake {
            Fake(Arc::new(Mutex::new(Box::new(move |_| a))))
        }
        fn set(&self, f: impl Fn(&str) -> SyncAnswer + Send + 'static) {
            *self.0.lock().unwrap() = Box::new(f);
        }
    }
    impl SyncProbe for Fake {
        fn check(&self, path: &str) -> SyncAnswer {
            (self.0.lock().unwrap())(path)
        }
    }

    fn probes(d: Drives, s: &Fake) -> DevProbes {
        DevProbes {
            drives: Arc::new(d),
            sync: Arc::new(s.clone()),
        }
    }

    fn fake_reg(name: &str, d: Drives, s: &Fake) -> Registry {
        Registry::open(&temp(name), true)
            .unwrap()
            .with_dev_probes(probes(d, s))
    }

    fn only(path: &str) -> impl Fn(&str) -> SyncAnswer + Send + 'static {
        let path = path.to_string();
        move |p| {
            if same_path(p, &path) {
                SyncAnswer::Synced
            } else {
                SyncAnswer::Local
            }
        }
    }

    #[test]
    fn a_drive_that_is_not_the_pcs_own_is_refused_by_kind_and_nothing_is_written() {
        let dir = temp("kind-dir").display().to_string();
        let local = Fake::all(SyncAnswer::Local);
        let d = |fixed, bus, remote| Drives { fixed, bus, remote };
        let cases = [
            (d(true, Some(BusType::Usb), false), NotLocalKind::PlugIn),
            (d(false, Some(BusType::Sd), false), NotLocalKind::PlugIn),
            (
                d(true, Some(BusType::FileBackedVirtual), false),
                NotLocalKind::VirtualOrUnknown,
            ),
            (d(true, None, false), NotLocalKind::VirtualOrUnknown),
            (d(false, None, true), NotLocalKind::Network),
        ];
        for (i, (dr, want)) in cases.into_iter().enumerate() {
            let mut reg = fake_reg(&format!("kind-home{i}"), dr, &local);
            match reg.try_add_dev_folder(&dir) {
                Err(DevFolderRefusal::NotLocal(k)) => assert_eq!(k, want, "case {i}"),
                other => panic!("case {i}: {other:?}"),
            }
            assert!(reg.dev_folders().is_empty(), "case {i}");
        }
        // The same folder on an internal fixed drive is added.
        let mut reg = fake_reg("kind-ok", INTERNAL, &local);
        assert!(reg.try_add_dev_folder(&dir).is_ok());
    }

    #[test]
    fn a_listed_folder_that_moves_to_a_plug_in_drive_shows_its_kind_and_is_not_carried() {
        let mut f = fx("kind-row");
        allow(&mut f, "dialog", &["hub.notify", "ui.window"]);
        let local = Fake::all(SyncAnswer::Local);
        f.reg.set_dev_probes(probes(INTERNAL, &local));
        assert!(f.reg.dev_folder_of(ID).is_some());
        f.reg.set_dev_probes(probes(
            Drives {
                fixed: true,
                bus: Some(BusType::Usb),
                remote: false,
            },
            &local,
        ));
        assert_eq!(
            f.reg.dev_folder_rows()[0].problem,
            Some(RowProblem::NotLocal(NotLocalKind::PlugIn))
        );
        assert!(f.reg.dev_folder_of(ID).is_none());
    }

    #[test]
    fn a_cloud_synced_folder_is_refused_on_add_and_nothing_is_written() {
        let dir = temp("cloud-dir").display().to_string();
        let s = Fake::all(SyncAnswer::Synced);
        let mut reg = fake_reg("cloud-home", INTERNAL, &s);
        assert!(matches!(
            reg.try_add_dev_folder(&dir),
            Err(DevFolderRefusal::Cloud)
        ));
        assert!(reg.dev_folders().is_empty());
        // The old string API refuses too.
        assert!(reg.add_dev_folder(&dir).is_err());
        assert!(reg.dev_folders().is_empty());
        // A probe that can't answer fails closed, with its own refusal.
        s.set(|_| SyncAnswer::Unknown);
        assert!(matches!(
            reg.try_add_dev_folder(&dir),
            Err(DevFolderRefusal::CloudUnknown)
        ));
        assert!(reg.dev_folders().is_empty());
        // Not synced: added.
        s.set(|_| SyncAnswer::Local);
        assert!(reg.try_add_dev_folder(&dir).is_ok());
    }

    /// Every carry asks again: the folder, or the program alone, turning out synced (or not
    /// readable) stops the carry at once and the row says why; asking Local again restores it.
    #[test]
    fn a_cloud_synced_folder_or_program_stops_the_carry_on_every_check() {
        let mut f = fx("cloud-carry");
        allow(&mut f, "dialog", &["hub.notify", "ui.window"]);
        let s = Fake::all(SyncAnswer::Local);
        f.reg.set_dev_probes(probes(INTERNAL, &s));
        let exe = canonical_path(&f.exe).unwrap();
        let folder = f.reg.dev_folders()[0].clone();
        assert_eq!(f.reg.dev_folder_for(&exe).as_deref(), Some(folder.as_str()));
        assert!(f.reg.dev_folder_of(ID).is_some());
        // The folder syncs.
        s.set(only(&folder));
        assert_eq!(f.reg.dev_folder_for(&exe), None);
        assert!(f.reg.dev_folder_of(ID).is_none());
        assert_eq!(f.reg.dev_folder_rows()[0].problem, Some(RowProblem::Cloud));
        // Only the program is a placeholder.
        s.set(only(&exe));
        assert_eq!(f.reg.dev_folder_for(&exe), None);
        assert!(f.reg.dev_folder_of(ID).is_none());
        // Can't tell: refused, and the row says so in its own words.
        s.set(|_| SyncAnswer::Unknown);
        assert_eq!(f.reg.dev_folder_for(&exe), None);
        assert_eq!(
            f.reg.dev_folder_rows()[0].problem,
            Some(RowProblem::CloudUnknown)
        );
        // Back to plain local: carried again (the enrolment was never dropped).
        s.set(|_| SyncAnswer::Local);
        assert!(f.reg.dev_folder_of(ID).is_some());
    }

    #[test]
    fn a_rebuild_in_a_cloud_synced_folder_voids_instead_of_carrying() {
        let mut f = fx("cloud-void");
        allow(&mut f, "dialog", &["hub.notify", "ui.window"]);
        let s = Fake::all(SyncAnswer::Synced);
        f.reg.set_dev_probes(probes(INTERNAL, &s));
        rebuild(&f, b"build 2");
        assert!(
            f.reg.binding_void(ID).is_some(),
            "a synced folder carries nothing"
        );
        s.set(|_| SyncAnswer::Local);
        assert_eq!(f.reg.binding_void(ID), None);
    }

    #[test]
    fn a_folder_inside_a_listed_one_is_refused_and_a_parent_is_allowed() {
        let local = Fake::all(SyncAnswer::Local);
        let mut reg = fake_reg("nest-home", INTERNAL, &local);
        let parent = temp("nest-parent");
        let child = parent.join("child");
        std::fs::create_dir_all(&child).unwrap();
        let a = reg
            .try_add_dev_folder(&parent.display().to_string())
            .unwrap();
        match reg.try_add_dev_folder(&child.display().to_string()) {
            Err(DevFolderRefusal::InsideListed(p)) => assert_eq!(p, a.path),
            other => panic!("{other:?}"),
        }
        assert_eq!(reg.dev_folders(), vec![a.path.clone()]);
        // Already listed still says so (not "inside").
        assert!(
            reg.try_add_dev_folder(&parent.display().to_string())
                .unwrap()
                .was_listed
        );
        // The other order: child first, then its parent. Both rows stay; the child says where.
        let mut reg = fake_reg("nest-home2", INTERNAL, &local);
        let c = reg
            .try_add_dev_folder(&child.display().to_string())
            .unwrap();
        let p = reg
            .try_add_dev_folder(&parent.display().to_string())
            .unwrap();
        let rows = reg.dev_folder_rows();
        assert_eq!(rows.len(), 2);
        let crow = rows.iter().find(|r| r.path == c.path).unwrap();
        let prow = rows.iter().find(|r| r.path == p.path).unwrap();
        assert_eq!(crow.inside.as_deref(), Some(p.path.as_str()));
        assert_eq!(prow.inside, None);
    }

    #[test]
    fn a_listed_parent_that_is_ignored_does_not_block_a_child() {
        let s = Fake::all(SyncAnswer::Local);
        let mut reg = fake_reg("nest-ignored", INTERNAL, &s);
        let parent = temp("nest-ign-parent");
        let child = parent.join("child");
        std::fs::create_dir_all(&child).unwrap();
        let pp = reg
            .try_add_dev_folder(&parent.display().to_string())
            .unwrap()
            .path;
        // The parent (only) is synced now; the child on its own is not.
        s.set(only(&pp));
        assert!(reg.try_add_dev_folder(&child.display().to_string()).is_ok());
    }

    /// A folder whose name is not valid Unicode canonicalizes to a lossy string that names
    /// nothing: refused with its own reason, and never stored.
    #[test]
    fn a_folder_name_that_is_not_unicode_is_refused() {
        use std::os::windows::ffi::OsStringExt;
        let base = temp("badname");
        let bad = base.join(std::ffi::OsString::from_wide(&[0x62, 0xD800, 0x64]));
        std::fs::create_dir_all(&bad).unwrap();
        let link = base.join("link");
        use std::os::windows::ffi::OsStrExt;
        let back = |p: &std::path::Path| {
            let w: Vec<u16> = p
                .as_os_str()
                .encode_wide()
                .map(|c| if c == 0x2F { 0x5C } else { c })
                .collect();
            std::ffi::OsString::from_wide(&w)
        };
        // Raw OsStr arguments: `junction` would turn the name lossy and point at nothing.
        let st = std::process::Command::new("cmd")
            .creation_flags(0x0800_0000)
            .args(["/C", "mklink", "/J"])
            .arg(back(&link))
            .arg(back(&bad))
            .output()
            .unwrap();
        assert!(st.status.success(), "mklink failed: {st:?}");
        let s = Fake::all(SyncAnswer::Local);
        let mut reg = fake_reg("badname-home", INTERNAL, &s);
        assert!(matches!(
            reg.try_add_dev_folder(&link.display().to_string()),
            Err(DevFolderRefusal::NameUnreadable)
        ));
        assert!(reg.dev_folders().is_empty());
    }

    #[test]
    fn the_real_probes_let_an_ordinary_folder_through() {
        let mut reg = Registry::open(&temp("real-home"), true).unwrap();
        let r = reg.try_add_dev_folder(&temp("real-dir").display().to_string());
        assert!(r.is_ok(), "{r:?}");
    }

    #[test]
    fn the_restart_switch_defaults_on_and_is_saved_in_hideout_toml() {
        let home = temp("sw-home");
        let mut reg = Registry::open(&home, true).unwrap();
        assert!(reg.restart_after_rebuild());
        reg.set_restart_after_rebuild(false).unwrap();
        assert!(!reg.restart_after_rebuild());
        let text = std::fs::read_to_string(home.join("hideout.toml")).unwrap();
        assert!(text.contains("restart_after_rebuild = false"), "{text}");
        assert!(!Registry::open(&home, true).unwrap().restart_after_rebuild());
        reg.set_restart_after_rebuild(true).unwrap();
        assert!(reg.restart_after_rebuild());
        // The switch is not a grant: it never lists a folder.
        assert!(reg.dev_folders().is_empty());
    }
}

#[test]
fn a_file_deep_in_the_tree_that_other_accounts_can_write_is_not_carried() {
    let mut f = fx("tree-file-open");
    allow(&mut f, "dialog", &["hub.notify", "ui.window"]);
    assert!(f.reg.dev_folder_of(ID).is_some());
    let deep = f.out.join("a").join("b");
    std::fs::create_dir_all(&deep).unwrap();
    let helper = deep.join("helper.dll");
    std::fs::write(&helper, b"dll").unwrap();
    assert!(f.reg.dev_folder_of(ID).is_some(), "private tree is fine");
    grant_everyone(&helper, "M");
    clear_tree_cache();
    assert!(f.reg.dev_folder_of(ID).is_none());
    rebuild(&f, b"build 2");
    assert_eq!(f.reg.binding_void(ID), Some(VoidReason::Changed));
}

#[test]
fn a_tree_past_the_depth_or_entry_limit_is_not_carried() {
    let mut f = fx("tree-limits");
    allow(&mut f, "dialog", &["hub.notify", "ui.window"]);
    let nest = |n: usize| {
        let mut p = f.out.join("d");
        for _ in 1..n {
            p = p.join("d");
        }
        p
    };
    std::fs::create_dir_all(nest(8)).unwrap(); // exactly 8 levels: the limit
    clear_tree_cache();
    assert!(f.reg.dev_folder_of(ID).is_some(), "8 levels is allowed");
    std::fs::create_dir_all(nest(9)).unwrap();
    clear_tree_cache();
    assert!(f.reg.dev_folder_of(ID).is_none(), "9 levels is too deep");
    std::fs::remove_dir_all(f.out.join("d")).unwrap();
    clear_tree_cache();
    assert!(f.reg.dev_folder_of(ID).is_some());
    let many = f.out.join("many");
    std::fs::create_dir_all(&many).unwrap();
    for i in 0..20_000 {
        std::fs::write(many.join(format!("{i}.o")), b"").unwrap();
    }
    clear_tree_cache();
    assert!(f.reg.dev_folder_of(ID).is_none(), "too many entries");
}

#[test]
fn a_junction_in_the_tree_is_refused_outside_and_skipped_inside() {
    let mut f = fx("tree-junction");
    allow(&mut f, "dialog", &["hub.notify", "ui.window"]);
    // Inside the folder: not followed, fine.
    let target = f.out.join("real");
    std::fs::create_dir_all(&target).unwrap();
    junction(&f.out.join("alias"), &target);
    clear_tree_cache();
    assert!(f.reg.dev_folder_of(ID).is_some());
    // Outside: refused, even though the target is private.
    let outside = temp("tree-junction-outside");
    junction(&f.out.join("escape"), &outside);
    clear_tree_cache();
    assert!(f.reg.dev_folder_of(ID).is_none());
}

#[test]
fn a_writable_link_inside_the_tree_is_refused_even_when_it_leads_inside() {
    let mut f = fx("tree-link-acl");
    allow(&mut f, "dialog", &["hub.notify", "ui.window"]);
    let target = f.out.join("real");
    std::fs::create_dir_all(&target).unwrap();
    let link = f.out.join("alias");
    junction(&link, &target);
    clear_tree_cache();
    assert!(f.reg.dev_folder_of(ID).is_some(), "private link is fine");
    // /L: the link's own ACL, not the target's.
    let st = std::process::Command::new("icacls")
        .creation_flags(0x0800_0000)
        .arg(&link)
        .args(["/grant", "*S-1-1-0:(W)", "/L"])
        .output()
        .unwrap();
    assert!(st.status.success(), "icacls failed");
    clear_tree_cache();
    assert!(f.reg.dev_folder_of(ID).is_none());
}

#[test]
fn a_cleared_tree_answer_is_asked_again() {
    let mut f = fx("tree-cache");
    allow(&mut f, "dialog", &["hub.notify", "ui.window"]);
    let file = f.out.join("helper.dll");
    std::fs::write(&file, b"x").unwrap();
    clear_tree_cache();
    assert!(f.reg.dev_folder_of(ID).is_some());
    grant_everyone(&file, "M");
    // The remembered answer ages out in seconds (timing, so not asserted); clearing it asks again.
    clear_tree_cache();
    assert!(f.reg.dev_folder_of(ID).is_none());
}

#[test]
fn a_case_variant_of_a_path_behind_an_escaping_junction_never_matches() {
    let f = fx("junction-case");
    let outside = temp("junction-case-outside");
    std::fs::write(outside.join("foxglove.exe"), b"evil").unwrap();
    junction(&f.out.join("escape"), &outside);
    let behind = canonical_path(&f.out.join("escape").join("foxglove.exe")).unwrap();
    assert!(f.reg.dev_folder_for(&behind.to_uppercase()).is_none());
    let spelled = f
        .out
        .join("ESCAPE")
        .join("FOXGLOVE.EXE")
        .display()
        .to_string();
    assert!(f.reg.dev_folder_for(&spelled).is_none());
}
