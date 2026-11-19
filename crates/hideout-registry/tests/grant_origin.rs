// SPDX-License-Identifier: MIT
//! Grants follow the origin of an install, not its id (trust model F1/F2, slice S0): a changed
//! source, anchor, kind or sideload folder voids what was carried. Window-free, mock-only.

use std::path::{Path, PathBuf};

use hideout_contract::module::{Kind, Visibility};
use hideout_registry::inventory::{Bound, FollowFolder, Inventory, ADOPTED, SIDELOAD};
use hideout_registry::keyboard::{GrantState, KeyboardGrant};
use hideout_registry::trust::ROOT_ANCHOR;
use hideout_registry::Registry;

const NOW: &str = "2026-11-18T12:00:00Z";
const ASKED: [&str; 3] = ["hub.notify", "key.events", "ui.window"];
const NOTES: &str = "example.private.notes";

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir()
        .join("hideout-origin-tests")
        .join(format!("{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn sideload_folder(name: &str, version: &str) -> PathBuf {
    let dir = std::env::temp_dir()
        .join("hideout-origin-tests")
        .join(format!("{}-{name}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("notes.exe"), b"exe").unwrap();
    std::fs::write(
        dir.join("module.toml"),
        format!(
            "id = \"{NOTES}\"\nname = \"Notes\"\nversion = \"{version}\"\n\
             kind = \"service\"\nfamily = \"third-party\"\nhub = \"1.0\"\nvisibility = \"private\"\n\
             run = [\"notes.exe\"]\npermissions = [\"hub.notify\", \"ui.window\", \"key.events\"]\n"
        ),
    )
    .unwrap();
    dir
}

fn grant_all(reg: &mut Registry, home: &Path, id: &str) {
    let m = reg.inventory.modules.get_mut(id).unwrap();
    m.granted = ASKED.iter().map(|s| s.to_string()).collect();
    m.pending_permissions.clear();
    m.last_choice.clear();
    reg.inventory.save(&home.join("inventory.json")).unwrap();
}

fn assert_voided(reg: &Registry, id: &str) {
    let m = &reg.inventory.modules[id];
    assert!(m.granted.is_empty(), "granted: {:?}", m.granted);
    assert_eq!(m.last_choice, ASKED, "the old choice is the dialog default");
    assert_eq!(m.pending_permissions, ASKED);
    assert!(m.bound.is_none());
}

fn assert_carried(reg: &Registry, id: &str) {
    let m = &reg.inventory.modules[id];
    assert_eq!(m.granted, ASKED);
    assert!(m.pending_permissions.is_empty());
}

#[test]
fn same_sideload_folder_update_still_carries() {
    let home = temp("same-home");
    let a = sideload_folder("same-a", "0.1.0");
    let mut reg = Registry::open(&home, true).unwrap();
    reg.install(&a, NOW).unwrap();
    assert!(reg.inventory.modules[NOTES].sideload_folder.is_some());
    grant_all(&mut reg, &home, NOTES);
    // The same folder, a new version.
    let a2 = sideload_folder("same-a", "0.2.0");
    reg.install(&a2, NOW).unwrap();
    assert_carried(&reg, NOTES);
}

#[test]
fn sideload_folder_a_to_b_voids() {
    let home = temp("ab-home");
    let a = sideload_folder("ab-a", "0.1.0");
    let b = sideload_folder("ab-b", "0.2.0");
    let mut reg = Registry::open(&home, true).unwrap();
    reg.install(&a, NOW).unwrap();
    grant_all(&mut reg, &home, NOTES);
    reg.install(&b, NOW).unwrap();
    assert_voided(&reg, NOTES);
    let mut reg = Registry::open(&home, true).unwrap();
    assert_voided(&reg, NOTES);
    // Allowed again by the user, then an update from B carries.
    grant_all(&mut reg, &home, NOTES);
    reg.install(&sideload_folder("ab-b", "0.3.0"), NOW).unwrap();
    assert_carried(&reg, NOTES);
}

#[test]
fn user_anchor_to_sideload_voids() {
    let home = temp("anchor-home");
    let a = sideload_folder("anchor-a", "0.1.0");
    let mut reg = Registry::open(&home, true).unwrap();
    reg.install(&a, NOW).unwrap();
    grant_all(&mut reg, &home, NOTES);
    {
        // What a user-anchored install of the same id left behind.
        let m = reg.inventory.modules.get_mut(NOTES).unwrap();
        m.source = "my-catalog".into();
        m.anchor = Some("user-key-1".into());
        m.sideload_folder = None;
    }
    reg.inventory.save(&home.join("inventory.json")).unwrap();
    reg.install(&sideload_folder("anchor-a", "0.2.0"), NOW)
        .unwrap();
    assert_eq!(reg.inventory.modules[NOTES].source, SIDELOAD);
    assert_voided(&reg, NOTES);
}

fn signed(home: &Path, vectors: &Path) -> Registry {
    let rd = |rel: &str| std::fs::read(vectors.join(rel)).unwrap();
    let txt = |rel: &str| String::from_utf8(rd(rel)).unwrap();
    let key = txt("keys/root.pub")
        .lines()
        .nth(1)
        .unwrap()
        .trim()
        .to_string();
    let mut reg = Registry::open(home, false).unwrap();
    let root = "root/root.valid.json";
    reg.accept_root(&rd(root), &txt(&format!("{root}.minisig")), &key, NOW)
        .unwrap();
    let cat = "catalogs/foxglove.with-revocations.catalog.json";
    reg.accept_catalog(
        &rd(cat),
        &txt(&format!("{cat}.minisig")),
        ROOT_ANCHOR,
        "test-public",
        Visibility::Public,
        NOW,
    )
    .unwrap();
    reg
}

#[test]
fn adopted_to_catalog_voids_and_same_origin_carries() {
    let home = temp("cat-home");
    let vectors = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/hideout-vectors/trust");
    let folder = vectors.join("folders/foxglove.example-art-0.4.0");
    let id = "foxglove.example-art";
    let mut reg = signed(&home, &vectors);
    reg.install(&folder, NOW).unwrap();

    // Same origin (catalog): nothing is voided. This catalog module asks for nothing, so the
    // install's own ask() drops `granted`; a sentinel in last_choice shows the void did not run
    // (a void would replace it with the old grants).
    {
        let m = reg.inventory.modules.get_mut(id).unwrap();
        m.granted = vec!["hub.notify".into()];
        m.last_choice = vec!["sentinel".into()];
        m.version = "0.0.1".into();
    }
    reg.install(&folder, NOW).unwrap();
    assert_eq!(reg.inventory.modules[id].last_choice, ["sentinel"]);

    // The old entry was adopted: the carried grant is voided into the dialog default.
    {
        let m = reg.inventory.modules.get_mut(id).unwrap();
        m.granted = vec!["hub.notify".into()];
        m.last_choice.clear();
        m.source = ADOPTED.into();
        m.anchor = None;
        m.kind = Kind::Adopted;
        m.version = "0.0.1".into();
    }
    reg.install(&folder, NOW).unwrap();
    let m = &reg.inventory.modules[id];
    assert_eq!(m.source, "test-public");
    assert!(m.granted.is_empty());
    assert_eq!(m.last_choice, ["hub.notify"]);
    assert!(m.keyboard.is_none());
}

#[test]
fn adopted_kind_change_voids_the_keyboard_grant_too() {
    let install = temp("kb-install");
    std::fs::write(install.join("fg.exe"), b"exe").unwrap();
    let exe = install
        .join("fg.exe")
        .display()
        .to_string()
        .replace('\\', "/");
    let folder = temp("kb-manifest");
    let write = |version: &str| {
        std::fs::write(
            folder.join("module.toml"),
            format!(
                "id = \"foxglove\"\nname = \"Foxglove\"\nversion = \"{version}\"\nkind = \"adopted\"\n\
                 family = \"foxglove\"\nhub = \"1.0\"\nrun = ['{exe}']\napi = {{ foxglove = \"1.0\" }}\n\
                 permissions = [\"hub.notify\", \"ui.window\", \"key.events\"]\n"
            ),
        )
        .unwrap();
    };
    let home = temp("kb-home");
    write("1.0.0");
    let mut reg = Registry::open(&home, true).unwrap();
    reg.register_adopted(&folder, NOW).unwrap();
    grant_all(&mut reg, &home, "foxglove");
    reg.inventory.modules.get_mut("foxglove").unwrap().keyboard = Some(KeyboardGrant {
        state: GrantState::On,
        path: exe.clone(),
        sha256: "a".repeat(64),
        at: NOW.into(),
        reason: None,
    });
    reg.inventory.save(&home.join("inventory.json")).unwrap();

    // Same origin: a re-register keeps the grants and the keyboard record.
    write("1.1.0");
    reg.register_adopted(&folder, NOW).unwrap();
    assert_carried(&reg, "foxglove");
    assert_eq!(
        reg.inventory.modules["foxglove"]
            .keyboard
            .as_ref()
            .unwrap()
            .state,
        GrantState::On
    );

    // The recorded entry was another kind: everything is voided.
    reg.inventory.modules.get_mut("foxglove").unwrap().kind = Kind::Service;
    write("1.2.0");
    reg.register_adopted(&folder, NOW).unwrap();
    assert_voided(&reg, "foxglove");
    let k = reg.inventory.modules["foxglove"].keyboard.as_ref().unwrap();
    assert_eq!(k.state, GrantState::Void);
    assert!(k.sha256.is_empty());
}

#[test]
fn an_old_inventory_without_the_folder_loads_and_fails_closed_once() {
    let home = temp("old-home");
    let a = sideload_folder("old-a", "0.1.0");
    let mut reg = Registry::open(&home, true).unwrap();
    reg.install(&a, NOW).unwrap();
    grant_all(&mut reg, &home, NOTES);
    // Rewrite the file as an older build would have: no `sideload_folder` key.
    let path = home.join("inventory.json");
    let mut json: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    json["modules"][NOTES]
        .as_object_mut()
        .unwrap()
        .remove("sideload_folder")
        .unwrap();
    std::fs::write(&path, serde_json::to_string_pretty(&json).unwrap()).unwrap();
    let inv = Inventory::load(&path).unwrap();
    assert!(inv.modules[NOTES].sideload_folder.is_none());
    assert_eq!(inv.format, "hideout-inventory-v1");

    let mut reg = Registry::open(&home, true).unwrap();
    reg.install(&sideload_folder("old-a", "0.2.0"), NOW)
        .unwrap();
    assert_voided(&reg, NOTES);
    assert!(reg.inventory.modules[NOTES].sideload_folder.is_some());
}

fn bind(reg: &mut Registry, home: &Path, id: &str) {
    let m = reg.inventory.modules.get_mut(id).unwrap();
    m.bound = Some(Bound {
        manifest_sha256: "m".repeat(8),
        exe_path: "C:/x/notes.exe".into(),
        exe_sha256: "a".repeat(64),
        list_hash: "l".into(),
        at: NOW.into(),
        by: "dialog".into(),
        run_args: Vec::new(),
        void: false,
        attested: Vec::new(),
        follow: Some(FollowFolder {
            folder: "C:/x".into(),
            file_name: "notes.exe".into(),
            at: NOW.into(),
            by: "dialog".into(),
        }),
    });
    reg.inventory.save(&home.join("inventory.json")).unwrap();
}

#[test]
fn the_binding_and_keep_ok_are_cleared_on_void_and_kept_on_same_origin() {
    let home = temp("bind-home");
    let mut reg = Registry::open(&home, true).unwrap();
    reg.install(&sideload_folder("bind-a", "0.1.0"), NOW)
        .unwrap();
    grant_all(&mut reg, &home, NOTES);
    bind(&mut reg, &home, NOTES);
    reg.install(&sideload_folder("bind-a", "0.2.0"), NOW)
        .unwrap();
    assert!(reg.inventory.modules[NOTES]
        .bound
        .as_ref()
        .unwrap()
        .follow
        .is_some());
    reg.install(&sideload_folder("bind-b", "0.3.0"), NOW)
        .unwrap();
    assert_voided(&reg, NOTES);
    assert!(reg.inventory.modules[NOTES].bound.is_none());
}

#[test]
fn the_same_folder_spelled_differently_still_carries() {
    let home = temp("spell-home");
    let a = sideload_folder("spell-a", "0.1.0");
    let mut reg = Registry::open(&home, true).unwrap();
    reg.install(&a, NOW).unwrap();
    grant_all(&mut reg, &home, NOTES);

    // Another case.
    let upper = PathBuf::from(a.display().to_string().to_uppercase());
    sideload_folder("spell-a", "0.2.0");
    reg.install(&upper, NOW).unwrap();
    assert_carried(&reg, NOTES);

    // A trailing slash.
    sideload_folder("spell-a", "0.3.0");
    let slash = PathBuf::from(format!("{}\\", a.display()));
    reg.install(&slash, NOW).unwrap();
    assert_carried(&reg, NOTES);

    // A junction to the same folder; a junction to another folder voids.
    let link = temp("spell-link-parent").join("link");
    let ok = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(&link)
        .arg(&a)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if ok {
        sideload_folder("spell-a", "0.4.0");
        reg.install(&link, NOW).unwrap();
        assert_carried(&reg, NOTES);
        let b = sideload_folder("spell-b", "0.5.0");
        let link_b = temp("spell-link-b-parent").join("link");
        let ok_b = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&link_b)
            .arg(&b)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if ok_b {
            reg.install(&link_b, NOW).unwrap();
            assert_voided(&reg, NOTES);
        }
    }
}

#[test]
fn the_recorded_folder_has_no_verbatim_prefix() {
    let home = temp("prefix-home");
    let mut reg = Registry::open(&home, true).unwrap();
    reg.install(&sideload_folder("prefix-a", "0.1.0"), NOW)
        .unwrap();
    let f = reg.inventory.modules[NOTES]
        .sideload_folder
        .clone()
        .unwrap();
    assert!(!f.starts_with(r"\\?\"), "{f}");
}
