// SPDX-License-Identifier: MIT
//! `seams_hold`: the guard for the sandbox-parking seams (review/sandbox-parking.md, 11/17).
//! Sandboxing is parked; this keeps the cheap seams from eroding so un-parking stays a switch
//! plus a few known fixes. Window-free and mock-only: it only reads source files under the
//! workspace root (no process, no pipe, no GUI), so it is deterministic.
//!
//! Checked here:
//! - S1: the hub hands modules its name by env (`HIDEOUT_PIPE`, `HIDEOUT_TOKEN`,
//!   `HIDEOUT_INSTANCE`, `HIDEOUT_HUB_PID`) in `driver.rs`; the SDKs read them.
//! - S2: no module source (`app/src`, `crates/plugin-*/src`, `examples/*/src`, `sdk/python`)
//!   names `inventory.json`, `hideout.toml` or `HIDEOUT_HOME` (one documented docstring
//!   allowed), and no module manifest links `hideout-registry`/`hideout-supervisor` outside
//!   `[dev-dependencies]`.
//! - S5: `CreateFileMapping` appears only in `hideout-shell/src/platform.rs` (read-only fonts).
//! - S6: `LEGACY_FOXGLOVE_ID`/`LEGACY_MODULE` lines in `crates/*/src` may not exceed 25.
//! - S7: `HIDEOUT_HUB_PID` is read by the Rust and Python SDKs.
//! - Hand-off path (S1/S6): `foxglove_update::handoff::` is used from exactly three non-test
//!   places (the app updater writes, the supervisor's `driver.rs` and `attest.rs` read) and the
//!   file name literal lives only in `foxglove-update`.
//!
//! NOT checked here (not cheap or deterministic as a source scan):
//! - S3 (grants keyed by id + token, narrow-only reload): needs a running router; covered by
//!   `cargo test -p hideout-router --test module_grants`.
//! - S4 (sandbox plan from granted permissions, never module names): behavioural; covered by
//!   `cargo test -p hideout-supervisor --lib contain`.
//! - S8 (one spawn path, Job Object, a container that cannot be made fails the launch): needs
//!   real processes and Windows; covered by `cargo test -p hideout-supervisor --test appcontainer`.
//! - S7's actual pipe-server pid comparison, and S6's "paths come from the hub" half: need a
//!   running module; only the env-name wiring is scanned here.

use std::fs;
use std::path::{Path, PathBuf};

/// The most `LEGACY_*` lines the hub sources may hold (S6). Lower it as they go; never raise it.
const LEGACY_MAX: usize = 25;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

/// Files under `dir` with one of `exts`, skipping build output and caches. Missing dir: empty.
fn files(dir: &Path, exts: &[&str], out: &mut Vec<PathBuf>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        let name = e.file_name().to_string_lossy().into_owned();
        if p.is_dir() {
            if !matches!(
                name.as_str(),
                "target" | "__pycache__" | ".git" | "node_modules"
            ) {
                files(&p, exts, out);
            }
        } else if p
            .extension()
            .and_then(|x| x.to_str())
            .is_some_and(|x| exts.contains(&x))
        {
            out.push(p);
        }
    }
}

/// `<root>/<parent>/<child>/<sub>` for every child of `parent` whose name starts with `prefix`.
fn children(parent: &str, prefix: &str, sub: &str) -> Vec<PathBuf> {
    let Ok(rd) = fs::read_dir(root().join(parent)) else {
        return vec![];
    };
    let mut v: Vec<PathBuf> = rd
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with(prefix))
        .map(|e| e.path().join(sub))
        .collect();
    v.sort();
    v
}

/// Module source roots (S2): everything that runs as, or ships for, a module.
fn module_src_roots() -> Vec<PathBuf> {
    let mut v = vec![root().join("app/src"), root().join("sdk/python")];
    v.extend(children("crates", "plugin-", "src"));
    v.extend(children("examples", "", "src"));
    v
}

fn module_manifests() -> Vec<PathBuf> {
    let mut v = vec![root().join("app/Cargo.toml")];
    v.extend(children("crates", "plugin-", "Cargo.toml"));
    v.extend(children("examples", "", "Cargo.toml"));
    v.retain(|p| p.exists());
    v
}

fn rel(p: &Path) -> String {
    p.strip_prefix(root())
        .unwrap_or(p)
        .to_string_lossy()
        .replace('\\', "/")
}

fn read(p: &Path) -> String {
    fs::read_to_string(p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()))
}

/// Lines of `text` that contain any of `needles`, with their 1-based numbers.
fn hits<'a>(text: &'a str, needles: &[&str]) -> Vec<(usize, &'a str)> {
    text.lines()
        .enumerate()
        .filter(|(_, l)| needles.iter().any(|n| l.contains(n)))
        .map(|(i, l)| (i + 1, l))
        .collect()
}

/// Names of hub-side crates declared outside `[dev-dependencies]` in a manifest's text.
fn forbidden_deps(manifest: &str) -> Vec<String> {
    const HUB: [&str; 2] = ["hideout-registry", "hideout-supervisor"];
    let mut bad = vec![];
    let mut live = false; // inside a non-dev dependency table
    for raw in manifest.lines() {
        let l = raw.trim();
        if let Some(h) = l.strip_prefix('[').and_then(|h| h.strip_suffix(']')) {
            let h = h.trim();
            let is_dev = h.contains("dev-dependencies");
            // `[dependencies.hideout-registry]` style: the table itself is the dependency.
            if let Some((_, name)) = h.split_once("dependencies.") {
                if !is_dev && HUB.contains(&name.trim()) {
                    bad.push(name.trim().to_string());
                }
            }
            live = !is_dev && h.ends_with("dependencies");
            continue;
        }
        if live {
            let key = l.split(['=', ' ', '.']).next().unwrap_or("");
            if HUB.contains(&key) {
                bad.push(key.to_string());
            }
        }
    }
    bad
}

#[test]
fn s1_hub_hands_modules_its_name_by_env() {
    let driver = read(&root().join("crates/hideout-supervisor/src/driver.rs"));
    for var in [
        "HIDEOUT_PIPE",
        "HIDEOUT_TOKEN",
        "HIDEOUT_INSTANCE",
        "HIDEOUT_HUB_PID",
    ] {
        assert!(
            driver.contains(var),
            "S1: driver.rs no longer sets {var} for modules"
        );
    }
    let rust_sdk = read(&root().join("crates/plugin-sdk/src/hideout.rs"));
    assert!(
        rust_sdk.contains("HIDEOUT_PIPE"),
        "S1: the Rust SDK no longer reads HIDEOUT_PIPE"
    );
    let mut py = vec![];
    files(&root().join("sdk/python"), &["py"], &mut py);
    assert!(
        py.iter().any(|p| read(p).contains("HIDEOUT_PIPE")),
        "S1: the Python SDK no longer reads HIDEOUT_PIPE"
    );
}

#[test]
fn s2_no_module_touches_hub_files() {
    // (file, line fragment) pairs that only *mention* a name in documentation.
    const DOC_ONLY: [(&str, &str); 1] =
        [("sdk/python/hideout_module/models.py", "`[windows]` table")];
    let mut found = vec![];
    for dir in module_src_roots() {
        let mut all = vec![];
        files(&dir, &["rs", "py"], &mut all);
        for f in all {
            let text = read(&f);
            for (n, line) in hits(&text, &["inventory.json", "hideout.toml", "HIDEOUT_HOME"]) {
                let allowed = DOC_ONLY
                    .iter()
                    .any(|(p, frag)| rel(&f) == *p && line.contains(frag));
                if !allowed {
                    found.push(format!("{}:{n}: {}", rel(&f), line.trim()));
                }
            }
        }
    }
    assert!(
        found.is_empty(),
        "S2: module code names hub files:\n{}",
        found.join("\n")
    );
}

#[test]
fn s2_no_module_links_hub_crates_outside_dev_deps() {
    let manifests = module_manifests();
    assert!(
        manifests.len() >= 5,
        "S2: found only {} module manifests; paths moved?",
        manifests.len()
    );
    let mut bad = vec![];
    for m in manifests {
        for dep in forbidden_deps(&read(&m)) {
            bad.push(format!("{} links {dep}", rel(&m)));
        }
    }
    assert!(bad.is_empty(), "S2: {}", bad.join("; "));
}

#[test]
fn the_manifest_scan_tells_dev_deps_from_real_ones() {
    let ok =
        "[dependencies]\nserde = \"1\"\n[dev-dependencies]\nhideout-registry = { path = \"x\" }\n\
              [target.'cfg(windows)'.dev-dependencies]\nhideout-supervisor = { path = \"y\" }\n";
    assert!(forbidden_deps(ok).is_empty());
    let bad = "[dependencies]\nhideout-registry = { path = \"x\" }\n\
               [target.'cfg(windows)'.dependencies]\nhideout-supervisor.path = \"y\"\n\
               [build-dependencies.hideout-registry]\npath = \"z\"\n";
    assert_eq!(forbidden_deps(bad).len(), 3);
}

#[test]
fn s5_no_shared_memory_beyond_the_font_map() {
    let mut found = vec![];
    let mut all = vec![];
    for d in ["app/src", "crates", "sdk"] {
        files(&root().join(d), &["rs", "py"], &mut all);
    }
    for f in all {
        // This file names the API in its own text.
        if f.file_name().is_some_and(|n| n == "seams_hold.rs") {
            continue;
        }
        if !hits(&read(&f), &["CreateFileMapping"]).is_empty() {
            found.push(rel(&f));
        }
    }
    assert_eq!(
        found,
        vec!["crates/hideout-shell/src/platform.rs".to_string()],
        "S5: CreateFileMapping must appear only in the read-only font map"
    );
}

#[test]
fn s6_legacy_id_special_cases_only_shrink() {
    let mut n = 0;
    let mut by_file = vec![];
    for src in children("crates", "", "src") {
        let mut all = vec![];
        files(&src, &["rs"], &mut all);
        for f in all {
            let c = hits(&read(&f), &["LEGACY_FOXGLOVE_ID", "LEGACY_MODULE"]).len();
            if c > 0 {
                by_file.push(format!("{} x{c}", rel(&f)));
            }
            n += c;
        }
    }
    assert!(
        n <= LEGACY_MAX,
        "S6: {n} legacy-id lines in crates/*/src, ceiling {LEGACY_MAX} (must shrink, not grow):\n{}",
        by_file.join("\n")
    );
}

#[test]
fn s7_sdks_check_the_pipe_server_is_the_hub() {
    assert!(
        read(&root().join("crates/plugin-sdk/src/hideout.rs")).contains("HIDEOUT_HUB_PID"),
        "S7: the Rust SDK no longer reads HIDEOUT_HUB_PID"
    );
    let mut py = vec![];
    files(&root().join("sdk/python"), &["py"], &mut py);
    assert!(
        py.iter().any(|p| read(p).contains("HIDEOUT_HUB_PID")),
        "S7: the Python SDK no longer reads HIDEOUT_HUB_PID"
    );
}

#[test]
fn the_update_hand_off_keeps_one_path_and_three_users() {
    let mut users = vec![];
    let mut literal = vec![];
    let mut all = vec![];
    for d in ["app/src", "crates"] {
        files(&root().join(d), &["rs"], &mut all);
    }
    for f in all {
        let r = rel(&f);
        if r.contains("/tests/") {
            continue;
        }
        // The app's sign-in module (H5.5) is not the update hand-off.
        let text = read(&f).replace("signin_handoff::", "");
        if !hits(&text, &["handoff::"]).is_empty() {
            users.push(r.clone());
        }
        if !hits(&text, &["attest-handoff"]).is_empty() {
            literal.push(r);
        }
    }
    users.sort();
    assert_eq!(
        users,
        vec![
            "app/src/foxglove/update.rs",
            "crates/hideout-supervisor/src/attest.rs",
            "crates/hideout-supervisor/src/driver.rs",
        ],
        "hand-off users changed: a new reader or writer of the side channel needs a review"
    );
    assert_eq!(
        literal,
        vec!["crates/foxglove-update/src/lib.rs"],
        "the hand-off file name must be spelled in one place"
    );
}
