// SPDX-License-Identifier: MIT
//! Runs the trust and config vectors of tests/hideout-vectors/ against the registry and
//! matches index.json (#933). Every other vector belongs to `hideout-contract`'s own run.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use foxglove_plugin_trust::sha256_hex;
use hideout_contract::module::{Context, Module, Visibility};
use hideout_registry::hub_config::HubConfig;
use hideout_registry::trust::{SeenAnchor, ROOT_ANCHOR};
use hideout_registry::{Failure, Registry};
use serde_json::Value;

fn vectors() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/hideout-vectors")
}

fn read(rel: &str) -> Vec<u8> {
    std::fs::read(vectors().join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

fn text(rel: &str) -> String {
    String::from_utf8(read(rel)).unwrap()
}

/// The base64 line of a minisign `.pub` file.
fn key(rel: &str) -> String {
    text(rel)
        .lines()
        .find(|l| !l.starts_with("untrusted comment:") && !l.trim().is_empty())
        .unwrap()
        .trim()
        .to_string()
}

static HOMES: AtomicUsize = AtomicUsize::new(0);

/// A fresh, empty registry home, removed on drop.
struct Home(PathBuf);

impl Home {
    fn new() -> Home {
        let n = HOMES.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir()
            .join("hideout-registry-vectors")
            .join(format!("{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Home(dir)
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

const NOW: &str = "2026-11-09T12:00:00Z";

fn now(ctx: &Value) -> String {
    ctx["now"].as_str().unwrap_or(NOW).to_string()
}

fn id_of(f: &Failure) -> String {
    f.id().map_or_else(|| format!("({f})"), String::from)
}

/// Seeds `seen` and the user anchor, opens the registry and accepts the context's root.
fn registry(home: &Home, ctx: &Value) -> Result<Registry, Failure> {
    let mut reg = Registry::open(&home.0, ctx["developer_mode"].as_bool().unwrap_or(false))?;
    if let Some(seen) = ctx["seen"].as_object() {
        for (anchor, s) in seen {
            let s: SeenAnchor = serde_json::from_value(s.clone()).unwrap();
            reg.inventory.seen.insert(anchor.clone(), s);
        }
    }
    let anchor = &ctx["anchor"];
    if anchor["kind"] == "user" {
        let scope: Vec<String> = serde_json::from_value(anchor["scope"].clone()).unwrap();
        let toml = format!(
            "[[trust.anchors]]\nid = {:?}\nkind = \"user\"\nkey = {:?}\nscope = {:?}\n",
            anchor["id"].as_str().unwrap(),
            key(anchor["key"].as_str().unwrap()),
            scope
        );
        reg.hub = HubConfig::parse(&toml).unwrap();
    }
    if let Some(root) = ctx["root"].as_str() {
        let pinned = ctx["root_pinned_key"]
            .as_str()
            .unwrap_or("trust/keys/root.pub");
        reg.accept_root(
            &read(root),
            &text(&format!("{root}.minisig")),
            &key(pinned),
            &now(ctx),
        )?;
    }
    Ok(reg)
}

fn accept(reg: &mut Registry, ctx: &Value, catalog: &str) -> Result<(), Failure> {
    let source = &ctx["source"];
    let visibility = match source["visibility"].as_str() {
        Some("private") => Visibility::Private,
        _ => Visibility::Public,
    };
    reg.accept_catalog(
        &read(catalog),
        &text(&format!("{catalog}.minisig")),
        ctx["anchor"]["id"].as_str().unwrap_or(ROOT_ANCHOR),
        source["id"].as_str().unwrap(),
        visibility,
        &now(ctx),
    )
}

fn root_vector(home: &Home, key: &str, exp: &Value) -> Result<(), String> {
    let mut ctx = exp["context"].clone();
    ctx["root"] = key.into();
    let reg = registry(home, &ctx).map_err(|f| id_of(&f))?;
    let root = reg.trust.root.as_ref().unwrap();
    let families: Vec<&str> = root.families.keys().map(String::as_str).collect();
    let mut want: Vec<&str> = exp["families"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f.as_str().unwrap())
        .collect();
    want.sort();
    if families != want {
        return Err(format!("families {families:?}, want {want:?}"));
    }
    if let Some(after) = exp.get("seen_after") {
        let seen = &reg.inventory.seen[ROOT_ANCHOR];
        if after["root_version"] != seen.root_version || after["epoch"] != seen.epoch {
            return Err(format!("seen {seen:?}, want {after}"));
        }
    }
    Ok(())
}

fn catalog_vector(home: &Home, key: &str, exp: &Value) -> Result<(), String> {
    let ctx = &exp["context"];
    let mut reg = registry(home, ctx).map_err(|f| id_of(&f))?;
    accept(&mut reg, ctx, key).map_err(|f| id_of(&f))?;
    let accepted = reg.trust.catalogs.last().unwrap();
    let manifest = ctx["manifest"].as_str().map(|m| {
        let dir = vectors().join(m);
        hideout_contract::module::read_dir(
            dir.parent().unwrap(),
            &Context::default(),
            reg.legacy_api,
        )
        .unwrap()
        .1
        .unwrap()
    });
    let mut got = Vec::new();
    for entry in &accepted.catalog.modules {
        if let Some(m) = manifest.as_ref().filter(|m: &&Module| m.id == entry.id) {
            entry.check_manifest(m).map_err(|e| e.id().to_string())?;
        }
        entry
            .check_source(accepted.visibility)
            .map_err(|e| e.id().to_string())?;
        got.push(format!("{}@{}", entry.id, entry.version));
    }
    let want: Vec<String> = serde_json::from_value(exp["accepts"].clone()).unwrap();
    if got != want {
        return Err(format!("accepts {got:?}, want {want:?}"));
    }
    if let Some(after) = exp.get("seen_after") {
        let seen = &reg.inventory.seen[after["anchor"].as_str().unwrap()];
        let source = after["source"].as_str().unwrap();
        if seen.catalogs.get(source).copied() != after["sequence"].as_u64() {
            return Err(format!("seen {seen:?}, want {after}"));
        }
    }
    Ok(())
}

fn install_vector(home: &Home, exp: &Value) -> Result<(), String> {
    let ctx = &exp["context"];
    let fail = |f: Failure| id_of(&f);
    let mut reg = registry(home, ctx).map_err(fail)?;
    let catalogs: Vec<String> = match ctx["catalog"].as_str() {
        Some(c) => vec![c.to_string()],
        None => serde_json::from_value(ctx["accepted_catalogs"].clone()).unwrap_or_default(),
    };
    for c in &catalogs {
        accept(&mut reg, ctx, c).map_err(fail)?;
    }
    if let Some(spec) = ctx["install"].as_str() {
        let (id, version) = spec.split_once('@').unwrap();
        return reg
            .trust
            .installable(id, version)
            .map(|_| ())
            .map_err(|e| e.id().to_string());
    }
    if let Some(archive) = ctx["archive"].as_str() {
        return reg
            .trust
            .archive(&sha256_hex(&read(archive)))
            .map(|_| ())
            .map_err(|e| e.id().to_string());
    }
    // Copies the folder so a vector's changed or extra files never touch the vectors.
    let folder = ctx["folder"].as_str().unwrap();
    let src = home.0.join("offered");
    let from = vectors().join(folder);
    std::fs::create_dir_all(&src).unwrap();
    for f in std::fs::read_dir(&from).unwrap() {
        let f = f.unwrap();
        std::fs::copy(f.path(), src.join(f.file_name())).unwrap();
    }
    for table in ["replace_files", "extra_files"] {
        if let Some(files) = ctx[table].as_object() {
            for (name, body) in files {
                std::fs::write(src.join(name), body.as_str().unwrap()).unwrap();
            }
        }
    }
    let installed = reg.install(&src, &now(ctx)).map_err(fail)?;
    let id = reg.inventory.modules.keys().next().unwrap().clone();
    if !installed.granted.is_empty() {
        return Err(format!("{id}: granted at install: {:?}", installed.granted));
    }
    reg.set_enabled(&id, true).map_err(fail)?;
    reg.pre_launch(&id).map(|_| ()).map_err(fail)
}

fn config_vector(home: &Home, key: &str, exp: &Value) -> Result<(), String> {
    let ctx = &exp["context"];
    std::fs::write(home.0.join("hideout.toml"), read(key)).unwrap();
    if let Some(scopes) = ctx["root_scopes"].as_object() {
        let scopes: BTreeMap<String, Vec<String>> = scopes
            .iter()
            .map(|(f, v)| {
                (
                    f.clone(),
                    serde_json::from_value(v["scope"].clone()).unwrap(),
                )
            })
            .collect();
        let reg = Registry::open(&home.0, false).map_err(|f| id_of(&f))?;
        return reg.hub.validate(&scopes).map_err(|e| e.id().to_string());
    }
    // accept_root checks hideout.toml's sources and anchors against the root's scopes.
    registry(home, ctx).map(|_| ()).map_err(|f| id_of(&f))
}

fn outcome(exp: &Value, got: Result<(), String>) -> Result<(), String> {
    match (exp["result"].as_str(), got) {
        (Some("valid"), Ok(())) => Ok(()),
        (Some("valid"), Err(e)) => Err(format!("want valid, got {e}")),
        (_, Ok(())) => Err(format!("want {}, got valid", exp["id"])),
        (_, Err(e)) if exp["id"] == e.as_str() => Ok(()),
        (_, Err(e)) => Err(format!("want {}, got {e}", exp["id"])),
    }
}

#[test]
fn trust_and_config_vectors_match_the_index() {
    let index: Value = serde_json::from_slice(&read("index.json")).unwrap();
    let mut passed: BTreeMap<&str, usize> = BTreeMap::new();
    let mut failed = Vec::new();
    for (key, exp) in index["vectors"].as_object().unwrap() {
        let home = Home::new();
        let group = match key.split('/').take(2).collect::<Vec<_>>()[..] {
            ["trust", _] => "trust",
            // `[windows]` pop-out placement (spec v1.6) is the hub's, not the registry's:
            // hideout-contract's vector test reads those.
            ["config", f] if f.starts_with("windows.") => continue,
            ["config", _] => "config",
            _ => continue,
        };
        let got = match key.split('/').nth(1).unwrap() {
            "root" => root_vector(&home, key, exp),
            "catalogs" => catalog_vector(&home, key, exp),
            "install" => install_vector(&home, exp),
            _ if group == "config" => config_vector(&home, key, exp),
            other => Err(format!("no reader for trust/{other}")),
        };
        match outcome(exp, got) {
            Ok(()) => *passed.entry(group).or_default() += 1,
            Err(e) => failed.push(format!("{key}: {e}")),
        }
    }
    println!("passed {passed:?}");
    assert!(
        failed.is_empty(),
        "{} failed:\n{}",
        failed.len(),
        failed.join("\n")
    );
    assert_eq!(passed.get("trust"), Some(&42), "every trust vector runs");
    assert_eq!(passed.get("config"), Some(&8), "every config vector runs");
}
