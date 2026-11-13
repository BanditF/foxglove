// SPDX-License-Identifier: MIT
//! The permission grant bound to the list that was shown (#1178): TOCTOU, narrowing, void on a
//! program change, and the 13 `foxglove` lines.

use std::path::{Path, PathBuf};

use hideout_registry::permission::{describe, Group, ItemState};
use hideout_registry::{Failure, Registry};

const NOW: &str = "2026-11-13T12:00:00Z";
const ID: &str = "vulpes";

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir()
        .join("hideout-permission-tests")
        .join(format!("{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn manifest_for(id: &str, exe: &Path, perms: &[&str]) -> String {
    let exe = exe.display().to_string().replace('\\', "/");
    let perms: Vec<String> = perms.iter().map(|p| format!("\"{p}\"")).collect();
    format!(
        "id = \"{id}\"\nname = \"Example\"\nversion = \"1.0.0\"\nkind = \"adopted\"\n\
         family = \"arete\"\nhub = \"1.0\"\nrun = [\"{exe}\"]\napi = {{ arete = \"1.0\" }}\n\
         permissions = [{}]\n",
        perms.join(", ")
    )
}

fn manifest(exe: &Path, perms: &[&str]) -> String {
    manifest_for(ID, exe, perms)
}

struct Fx {
    home: PathBuf,
    exe: PathBuf,
    folder: PathBuf,
    reg: Registry,
}

fn fx(name: &str, perms: &[&str]) -> Fx {
    let install = temp(&format!("{name}-install"));
    let exe = install.join("hub.exe");
    std::fs::write(&exe, b"program v1").unwrap();
    let folder = temp(&format!("{name}-manifest"));
    std::fs::write(folder.join("module.toml"), manifest(&exe, perms)).unwrap();
    let home = temp(&format!("{name}-home"));
    let mut reg = Registry::open(&home, true).unwrap();
    reg.register_adopted(&folder, NOW).unwrap();
    Fx {
        home,
        exe,
        folder,
        reg,
    }
}

fn strs(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

fn id_of<T: std::fmt::Debug>(r: Result<T, Failure>) -> String {
    match r {
        Ok(v) => format!("ok {v:?}"),
        Err(f) => f.id().map_or_else(|| f.to_string(), String::from),
    }
}

#[test]
fn allow_grants_only_the_listed_chosen_items_and_binds_the_program() {
    let mut f = fx("allow", &["hub.notify", "ui.window", "provide:a.read"]);
    let list = f.reg.permission_list(ID).unwrap();
    assert_eq!(list.items.len(), 3);
    assert_eq!(list.code().len(), 16);
    // Nothing is granted by asking.
    assert!(f.reg.inventory.modules[ID].granted.is_empty());
    let out = f
        .reg
        .approve_listed(
            ID,
            list.code(),
            &strs(&["hub.notify", "provide:a.read"]),
            &strs(&["ui.window"]),
            NOW,
            "dialog",
        )
        .unwrap();
    assert_eq!(out.granted, ["hub.notify", "provide:a.read"]);
    let m = &f.reg.inventory.modules[ID];
    assert_eq!(m.granted, ["hub.notify", "provide:a.read"]);
    assert!(m.pending_permissions.is_empty());
    assert_eq!(m.declined_permissions, ["ui.window"]);
    let b = m.bound.as_ref().unwrap();
    assert_eq!(b.by, "dialog");
    assert_eq!(b.list_hash, list.list_hash);
    assert_eq!(b.exe_sha256.len(), 64);
    // It survives a reopen.
    let again = Registry::open(&f.home, true).unwrap();
    assert_eq!(again.inventory.modules[ID], f.reg.inventory.modules[ID]);
}

#[test]
fn a_list_that_changed_after_it_was_shown_is_refused_and_writes_nothing() {
    let mut f = fx("toctou", &["hub.notify"]);
    let shown = f.reg.permission_list(ID).unwrap();
    // Another process re-adopts with a bigger manifest after the dialog opened.
    std::fs::write(
        f.folder.join("module.toml"),
        manifest(&f.exe, &["hub.notify", "ui.window"]),
    )
    .unwrap();
    let mut other = Registry::open(&f.home, true).unwrap();
    other.register_adopted(&f.folder, NOW).unwrap();
    let before = std::fs::read(f.home.join("inventory.json")).unwrap();
    let r = f
        .reg
        .approve_listed(ID, shown.code(), &strs(&["hub.notify"]), &[], NOW, "dialog");
    assert_eq!(id_of(r), "permission.list_changed");
    assert_eq!(
        std::fs::read(f.home.join("inventory.json")).unwrap(),
        before
    );
    assert!(f.reg.inventory.modules[ID].granted.is_empty());
    // Also the program: swapped after the dialog opened.
    let shown = f.reg.permission_list(ID).unwrap();
    std::fs::write(&f.exe, b"program swapped").unwrap();
    let r = f
        .reg
        .approve_listed(ID, shown.code(), &strs(&["hub.notify"]), &[], NOW, "cli");
    assert_eq!(id_of(r), "permission.list_changed");
    // A made-up code, a short one and an empty one are refused too.
    let now = f.reg.permission_list(ID).unwrap();
    for bad in ["", "abc", "0000000000000000"] {
        let r = f
            .reg
            .approve_listed(ID, bad, &strs(&["hub.notify"]), &[], NOW, "cli");
        assert_eq!(id_of(r), "permission.list_changed", "{bad:?}");
    }
    // The full 64 digits, in upper case, are accepted.
    let ok = f.reg.approve_listed(
        ID,
        &now.list_hash.to_uppercase(),
        &strs(&["hub.notify"]),
        &[],
        NOW,
        "cli",
    );
    assert!(ok.is_ok(), "{ok:?}");
}

#[test]
fn chosen_items_must_be_in_the_shown_list_and_disjoint_from_declined() {
    let mut f = fx("subset", &["hub.notify"]);
    let list = f.reg.permission_list(ID).unwrap();
    let r = f.reg.approve_listed(
        ID,
        list.code(),
        &strs(&["hub.notify", "ui.window"]),
        &[],
        NOW,
        "cli",
    );
    assert!(matches!(r, Err(Failure::Io(_))), "{r:?}");
    let r = f.reg.approve_listed(
        ID,
        list.code(),
        &strs(&["hub.notify"]),
        &strs(&["hub.notify"]),
        NOW,
        "cli",
    );
    assert!(matches!(r, Err(Failure::Io(_))), "{r:?}");
    assert!(f.reg.inventory.modules[ID].granted.is_empty());
}

#[test]
fn items_in_neither_set_stay_pending_and_never_items_are_refused() {
    let mut f = fx("partial", &["hub.notify", "ui.window", "modules.manage"]);
    let list = f.reg.permission_list(ID).unwrap();
    let never = list
        .items
        .iter()
        .find(|i| i.raw == "modules.manage")
        .unwrap();
    assert_eq!(never.state, ItemState::Never);
    assert_eq!(never.group, Group::CantBeAllowed);
    let out = f
        .reg
        .approve_listed(
            ID,
            list.code(),
            &strs(&["modules.manage", "hub.notify"]),
            &[],
            NOW,
            "cli",
        )
        .unwrap();
    assert_eq!(out.granted, ["hub.notify"]);
    assert_eq!(out.refused.len(), 1);
    assert_eq!(out.refused[0].1.id(), "permission.never");
    let m = &f.reg.inventory.modules[ID];
    assert_eq!(m.pending_permissions, ["ui.window"]);
    assert_eq!(m.declined_permissions, ["modules.manage"]);
}

#[test]
fn after_a_grant_the_registry_only_narrows() {
    let mut f = fx("narrow", &["hub.notify", "ui.window"]);
    let list = f.reg.permission_list(ID).unwrap();
    f.reg
        .approve_listed(
            ID,
            list.code(),
            &strs(&["hub.notify", "ui.window"]),
            &[],
            NOW,
            "dialog",
        )
        .unwrap();
    let left = f.reg.revoke_items(ID, Some(&strs(&["ui.window"]))).unwrap();
    assert_eq!(left, ["hub.notify"]);
    // An item that is not allowed can't be "turned off", and revoke never grants.
    assert!(f.reg.revoke_items(ID, Some(&strs(&["ui.window"]))).is_err());
    assert!(f.reg.revoke_items(ID, Some(&strs(&["x.y"]))).is_err());
    assert_eq!(f.reg.inventory.modules[ID].granted, ["hub.notify"]);
    // The off item is in the list again; only an Allow with the list's code grants it.
    let list = f.reg.permission_list(ID).unwrap();
    assert_eq!(list.items.len(), 1);
    assert_eq!(f.reg.revoke_items(ID, None).unwrap(), Vec::<String>::new());
    assert!(f.reg.inventory.modules[ID].granted.is_empty());
}

#[test]
fn a_manifest_that_asks_for_less_drops_what_it_no_longer_asks() {
    let mut f = fx("drop", &["hub.notify", "ui.window"]);
    let list = f.reg.permission_list(ID).unwrap();
    f.reg
        .approve_listed(
            ID,
            list.code(),
            &strs(&["hub.notify", "ui.window"]),
            &[],
            NOW,
            "dialog",
        )
        .unwrap();
    std::fs::write(
        f.folder.join("module.toml"),
        manifest(&f.exe, &["hub.notify"]),
    )
    .unwrap();
    f.reg.register_adopted(&f.folder, NOW).unwrap();
    assert_eq!(f.reg.inventory.modules[ID].granted, ["hub.notify"]);
}

#[test]
fn a_new_program_file_voids_the_grant_and_the_old_file_does_not_revive_it() {
    let mut f = fx("void", &["hub.notify", "ui.window"]);
    let list = f.reg.permission_list(ID).unwrap();
    f.reg
        .approve_listed(
            ID,
            list.code(),
            &strs(&["hub.notify", "ui.window"]),
            &[],
            NOW,
            "dialog",
        )
        .unwrap();
    f.reg.set_enabled(ID, true).unwrap();
    assert!(f.reg.pre_launch(ID).is_ok());
    assert!(f.reg.void_if_changed(ID, NOW).unwrap().is_none());

    std::fs::write(&f.exe, b"program v2").unwrap();
    let err = f.reg.pre_launch(ID).unwrap_err();
    assert_eq!(err.id(), Some("trust.mismatch"));
    let why = f.reg.void_if_changed(ID, NOW).unwrap();
    assert!(why.is_some());
    let m = &f.reg.inventory.modules[ID];
    assert!(m.granted.is_empty());
    assert_eq!(m.pending_permissions, ["hub.notify", "ui.window"]);
    assert_eq!(m.last_choice, ["hub.notify", "ui.window"]);
    assert!(m.bound.as_ref().unwrap().void);

    // Putting the old file back does not revive it.
    std::fs::write(&f.exe, b"program v1").unwrap();
    assert!(f.reg.pre_launch(ID).is_err());
    assert!(f.reg.inventory.modules[ID].granted.is_empty());

    // One new Allow (Review, then Allow) binds the program that is there now.
    let list = f.reg.permission_list(ID).unwrap();
    f.reg
        .approve_listed(
            ID,
            list.code(),
            &strs(&["hub.notify", "ui.window"]),
            &[],
            NOW,
            "dialog",
        )
        .unwrap();
    assert!(f.reg.pre_launch(ID).is_ok());
}

#[test]
fn a_provide_that_overlaps_another_adopted_grant_is_listed_as_overlap() {
    let mut f = fx("overlap", &["provide:gpu.*"]);
    // A second adopted module that already holds the area.
    let install = temp("overlap-other-install");
    let exe = install.join("o.exe");
    std::fs::write(&exe, b"o").unwrap();
    let folder = temp("overlap-other-manifest");
    std::fs::write(
        folder.join("module.toml"),
        manifest_for("other", &exe, &["provide:gpu.*"]),
    )
    .unwrap();
    f.reg.register_adopted(&folder, NOW).unwrap();
    let l = f.reg.permission_list("other").unwrap();
    f.reg
        .approve_listed(
            "other",
            l.code(),
            &strs(&["provide:gpu.*"]),
            &[],
            NOW,
            "dialog",
        )
        .unwrap();
    let mine = f.reg.permission_list(ID).unwrap();
    assert_eq!(mine.items[0].state, ItemState::Overlap);
    assert_eq!(mine.items[0].group, Group::CantBeAllowed);
}

#[test]
fn every_foxglove_permission_has_a_line_and_a_group() {
    let text = include_str!("../../../app/src/foxglove/hub_link/module.toml");
    let doc: toml::Value =
        toml::from_str(&text.replace("@VERSION@", "0.1.0").replace("@EXE@", "x")).unwrap();
    let perms: Vec<&str> = doc["permissions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(perms.len(), 13);
    let (mut share, mut changes, mut answers) = (0, 0, 0);
    for p in perms {
        let (g, line, sensitive) = describe(p);
        assert!(!sensitive, "{p}");
        assert!(
            !line.is_empty() && !line.starts_with("Unknown") && line != p,
            "{p}: {line}"
        );
        match g {
            Group::ShareStatus => share += 1,
            Group::MakeChanges => changes += 1,
            Group::AnswerQuestions => answers += 1,
            other => panic!("{p}: {other:?}"),
        }
    }
    assert_eq!((share, answers, changes), (2, 7, 4));
}

fn allow_all(f: &mut Fx) {
    let list = f.reg.permission_list(ID).unwrap();
    let all: Vec<String> = list.items.iter().map(|i| i.raw.clone()).collect();
    f.reg
        .approve_listed(ID, list.code(), &all, &[], NOW, "dialog")
        .unwrap();
    f.reg.set_enabled(ID, true).unwrap();
}

#[test]
fn an_attested_update_keeps_the_grants_and_is_recorded() {
    let mut f = fx("attest", &["hub.notify", "ui.window"]);
    allow_all(&mut f);
    let old = f.reg.inventory.modules[ID]
        .bound
        .clone()
        .unwrap()
        .exe_sha256;
    // The updater put the next version in the same install folder and vouches for it.
    std::fs::write(&f.exe, b"program v2").unwrap();
    f.reg.attest_update(ID, "1.1.0", "updater", NOW).unwrap();
    assert!(f.reg.pre_launch(ID).is_ok());
    assert!(f.reg.void_if_changed(ID, NOW).unwrap().is_none());
    let m = &f.reg.inventory.modules[ID];
    assert_eq!(m.granted, ["hub.notify", "ui.window"]);
    let b = m.bound.as_ref().unwrap();
    assert_eq!(b.attested.len(), 1);
    assert_eq!(b.attested[0].from_sha256, old);
    assert_eq!(b.attested[0].to_sha256, b.exe_sha256);
    assert_eq!(
        (b.attested[0].version.as_str(), b.attested[0].by.as_str()),
        ("1.1.0", "updater")
    );
    // The history survives a reopen, and a second update adds a second entry.
    std::fs::write(&f.exe, b"program v3").unwrap();
    let mut again = Registry::open(&f.home, true).unwrap();
    again.attest_update(ID, "1.2.0", "updater", NOW).unwrap();
    assert_eq!(
        again.inventory.modules[ID]
            .bound
            .as_ref()
            .unwrap()
            .attested
            .len(),
        2
    );
    assert!(again.pre_launch(ID).is_ok());
}

#[test]
fn an_unattested_swap_voids_even_after_an_earlier_attested_update() {
    let mut f = fx("unattested", &["hub.notify"]);
    allow_all(&mut f);
    std::fs::write(&f.exe, b"program v2").unwrap();
    f.reg.attest_update(ID, "1.1.0", "updater", NOW).unwrap();
    std::fs::write(&f.exe, b"somebody else's program").unwrap();
    assert_eq!(
        f.reg.pre_launch(ID).unwrap_err().id(),
        Some("trust.mismatch")
    );
    assert!(f.reg.void_if_changed(ID, NOW).unwrap().is_some());
    assert!(f.reg.inventory.modules[ID].granted.is_empty());
    // A void grant is not revived by attesting afterwards: it needs one new Allow.
    assert!(f.reg.attest_update(ID, "1.2.0", "updater", NOW).is_err());
    assert!(f.reg.inventory.modules[ID].granted.is_empty());
}

#[test]
fn a_program_in_another_folder_voids_and_cannot_be_attested() {
    let mut f = fx("folder", &["hub.notify"]);
    allow_all(&mut f);
    let other = temp("folder-other");
    let exe2 = other.join("hub.exe");
    std::fs::write(&exe2, b"program v1").unwrap();
    // The manifest now runs the same bytes from another folder.
    std::fs::write(
        f.folder.join("module.toml"),
        manifest(&exe2, &["hub.notify"]),
    )
    .unwrap();
    let mut other_reg = Registry::open(&f.home, true).unwrap();
    other_reg.register_adopted(&f.folder, NOW).unwrap();
    let mut reg = Registry::open(&f.home, true).unwrap();
    assert!(reg.attest_update(ID, "1.1.0", "updater", NOW).is_err());
    assert_eq!(reg.pre_launch(ID).unwrap_err().id(), Some("trust.mismatch"));
    assert!(reg.void_if_changed(ID, NOW).unwrap().is_some());
}

#[test]
fn a_module_has_no_way_to_grant_or_attest() {
    // The grant and attest methods are hub-side Rust calls. No router, contract or module SDK
    // source names them, so no module request can reach them (#1178).
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut hits = Vec::new();
    for krate in ["hideout-router", "hideout-contract", "plugin-sdk"] {
        scan(&root.join(krate).join("src"), &mut hits);
    }
    assert!(hits.is_empty(), "{hits:?}");
}

fn scan(dir: &Path, hits: &mut Vec<String>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            scan(&p, hits);
        } else if p.extension().is_some_and(|x| x == "rs") {
            let t = std::fs::read_to_string(&p).unwrap_or_default();
            for name in [
                "approve_listed",
                "attest_update",
                "revoke_items",
                "deny_listed",
            ] {
                if t.contains(name) {
                    hits.push(format!("{}: {name}", p.display()));
                }
            }
        }
    }
}

#[test]
fn an_exe_path_written_with_forward_slashes_and_other_case_is_the_same_file() {
    let mut f = fx("slashes", &["hub.notify"]);
    allow_all(&mut f);
    // The manifest spells run[0] in upper case with forward slashes; it is the same file.
    let spelled = f.exe.display().to_string().to_uppercase();
    let text = manifest(&f.exe, &["hub.notify"]).replace(
        &f.exe.display().to_string().replace('\\', "/"),
        &spelled.replace('\\', "/"),
    );
    std::fs::write(f.folder.join("module.toml"), text).unwrap();
    let mut other = Registry::open(&f.home, true).unwrap();
    other.register_adopted(&f.folder, NOW).unwrap();
    let reg = Registry::open(&f.home, true).unwrap();
    assert!(reg.binding_void(ID).is_none());
    assert!(reg.pre_launch(ID).is_ok());
}
