// SPDX-License-Identifier: MIT
//! The first-run click-count harness (review/ux-plan-first-run.md section 4, slice FR0).
//!
//! It drives the real state layers with no window and counts the user's actions from the first
//! installer screen to "the app is allowed and told to start": the installer's WiX dialog
//! sequence (read as data from `wix/hideout.wxs`), `discover`, the Found card's Add,
//! `ModulesPage::after_add`, the permission dialog (`PermUi`, `PermDialog`) and `after_allow`
//! against a fake supervisor that only records what it was asked. Each element of the path is a
//! user click, and each click is one line in [`Clicks`]. Everything runs in a temp hub folder
//! with a fake `Machine`; nothing reads or writes the real registry, starts a program or opens
//! a window.
//!
//! The bound is a ratchet. Today it is the measured number; each slice of the plan lowers the
//! constants below in its own PR (the test fails on both a regression and a forgotten
//! lowering). Plan targets: installer 2 (or 3 with an accept box), app 4, total 6.
//!
//! Not covered yet (later slices): the module's own "running" fact and a keyboard fact (FR4),
//! the hand-over line for a standalone copy (FR5), the copy test (FR2).

use super::*;
use crate::discover::{DiscoverUi, Picker, MODULES_KEY};
use crate::find_installed::Machine;
use crate::permission_actions::PermUi;
use hideout_registry::Registry;
use hideout_router::server::Hub;
use hideout_router::{Router, Settings};
use hideout_shell::permission_dialog::PermAnswer;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc;

/// Today's measured bound (main 614f3c78). Lower these, never raise them.
const BOUND_INSTALLER: usize = 3;
const BOUND_LAUNCH: usize = 1;
const BOUND_APP: usize = 10;
const BOUND_TOTAL: usize = 14;

const NOW: &str = "2026-11-19T08:00:00Z";
const HUB_WXS: &str = include_str!("../../wix/hideout.wxs");
const FOXGLOVE_MANIFEST: &str = include_str!("../../../../app/src/foxglove/hub_link/module.toml");

/// The sensitive asks of the generic fixture, and how many everyday ones it adds (the plan's
/// "4 sensitive and 17 everyday").
const SENSITIVE: [&str; 4] = [
    "device.hid:keyboard",
    "device.hid:lighting",
    "device.hid:vendor",
    "net",
];
const EVERYDAY: usize = 17;

/// The user's actions, in order, each with the screen it is on.
#[derive(Default)]
struct Clicks(Vec<String>);

impl Clicks {
    fn click(&mut self, what: impl Into<String>) {
        self.0.push(what.into());
    }
    fn len(&self) -> usize {
        self.0.len()
    }
}

/// What one run measured.
struct Run {
    clicks: Clicks,
    installer: usize,
    launch: usize,
    app: usize,
    allowed: usize,
}

// ---------------------------------------------------------------- the installer, as data

/// Removes `<!-- ... -->` so comments never count as installer content.
fn strip_xml_comments(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find("<!--") {
        out.push_str(&rest[..i]);
        match rest[i..].find("-->") {
            Some(j) => rest = &rest[i + j + 3..],
            None => return out,
        }
    }
    out.push_str(rest);
    out
}

/// The clicks the installer asks for, from its dialog set. A dialog set this harness has no
/// sequence for fails loudly: whoever changes the installer adds its screens here.
fn installer_clicks(wxs: &str) -> Vec<String> {
    let wxs = strip_xml_comments(wxs);
    if wxs.contains(r#"<UIRef Id="WixUI_Minimal""#) {
        // WixUI_Minimal: one welcome+license page (accept box, Install), then the finish page.
        vec![
            "installer: tick I accept the terms in the License Agreement".into(),
            "installer: Install".into(),
            "installer: Finish".into(),
        ]
    } else {
        panic!("first-run harness: the installer's dialog set has no click sequence here yet");
    }
}

/// The click that opens Hideout after the installer: none when the finish page offers (and
/// ticks) "Open Hideout now", else the user finds it in the Start menu.
fn launch_clicks(wxs: &str) -> Vec<String> {
    let wxs = strip_xml_comments(wxs);
    if wxs.contains("WIXUI_EXITDIALOGOPTIONALCHECKBOX") {
        Vec::new()
    } else {
        vec!["start menu: open Hideout".into()]
    }
}

/// The installer never starts anything at sign-in: no Run value, no Startup folder entry.
fn assert_installer_has_no_autostart(wxs: &str) {
    let wxs = strip_xml_comments(wxs);
    for banned in [
        r"CurrentVersion\Run",
        "StartupFolder",
        "Hideout.lnk",
        "--tray",
    ] {
        assert!(
            !wxs.contains(banned),
            "the installer must not write {banned}"
        );
    }
}

// ---------------------------------------------------------------- the fake PC

#[derive(Default)]
struct Pc {
    reg: BTreeMap<(String, String), String>,
    subkeys: Vec<String>,
    files: BTreeMap<PathBuf, Option<String>>,
}

impl Machine for Pc {
    fn registry_string(&self, key: &str, value: &str) -> Option<String> {
        self.reg.get(&(key.into(), value.into())).cloned()
    }
    fn env(&self, _name: &str) -> Option<String> {
        None
    }
    fn is_file(&self, p: &Path) -> bool {
        self.files.contains_key(p)
    }
    fn exe_version(&self, p: &Path) -> Option<String> {
        self.files.contains_key(p).then(|| "1.0.0".to_string())
    }
    fn read_text(&self, p: &Path) -> Option<String> {
        self.files.get(p).cloned().flatten()
    }
    fn registry_subkeys(&self, key: &str) -> Vec<String> {
        if key == MODULES_KEY {
            self.subkeys.clone()
        } else {
            Vec::new()
        }
    }
    fn canonical(&self, p: &Path) -> Option<PathBuf> {
        Some(p.to_path_buf())
    }
    fn is_local_fixed(&self, _p: &Path) -> bool {
        true
    }
}

struct NoPicker;
impl Picker for NoPicker {
    fn pick_folder(&self) -> Option<PathBuf> {
        None
    }
}

/// What the page asked of the supervisor. Shared so the test reads it while the page owns it.
#[derive(Clone, Default)]
struct Asked(std::rc::Rc<std::cell::RefCell<Vec<String>>>);

impl Actions for Asked {
    fn start_normally(&self) {}
    fn start_again(&self, id: &str) {
        self.0.borrow_mut().push(format!("start-again {id}"));
    }
    fn start_with_defaults(&self, _id: &str) {}
    fn go_back(&self, _id: &str) {}
    fn turn_off(&self, _id: &str) {}
    fn turn_on(&self, id: &str) -> Result<(), String> {
        self.0.borrow_mut().push(format!("turn-on {id}"));
        Ok(())
    }
    fn check_for_updates(&self, _id: &str) {}
    fn force_update(&self, _id: &str) {}
}

// ---------------------------------------------------------------- the fixture apps

/// A generic keyboard app: 4 sensitive asks and 17 everyday ones, no product name anywhere.
fn gizmo_manifest(exe: &Path) -> String {
    let run = exe.display().to_string().replace('\\', "/");
    let mut perms: Vec<String> = SENSITIVE.iter().map(|s| s.to_string()).collect();
    perms.push("facts.write:keyboard.*".into());
    perms.push("facts.read:module.*".into());
    perms.push("files".into());
    let mut provides = vec!["\"facts:keyboard.*\"".to_string()];
    for i in 0..EVERYDAY - 3 {
        perms.push(format!("provide:gizmo.m{i:02}"));
        provides.push(format!("\"method:gizmo.m{i:02}\""));
    }
    let perms: Vec<String> = perms.iter().map(|p| format!("\"{p}\"")).collect();
    format!(
        "id = \"gizmo-keys\"\nname = \"Gizmo Keys\"\nversion = \"1.0.0\"\nkind = \"adopted\"\n\
         family = \"arete\"\nhub = \"1.0\"\nrun = ['{run}', '--module']\n\
         api = {{ arete = \"1.0\" }}\npermissions = [{}]\n",
        perms.join(", ")
    )
}

/// Foxglove's shipped manifest with this PC's program path filled in.
fn foxglove_manifest(exe: &Path) -> String {
    FOXGLOVE_MANIFEST
        .replace("@VERSION@", "0.3.0")
        .replace("@EXE@", &exe.display().to_string().replace('\\', "/"))
}

struct Fx {
    base: PathBuf,
    id: String,
    page: ModulesPage,
    asked: Asked,
}

impl Fx {
    fn new(tag: &str, id: &str, manifest: impl Fn(&Path) -> String) -> Fx {
        let base =
            std::env::temp_dir().join(format!("hideout-firstrun-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let install = base.join("install");
        std::fs::create_dir_all(&install).unwrap();
        let exe = install.join("app.exe");
        std::fs::write(&exe, b"program").unwrap();
        let text = manifest(&exe);
        std::fs::write(install.join("module.toml"), &text).unwrap();
        // The app registered itself under Hideout's Modules key, as an installed app does.
        let mut pc = Pc::default();
        pc.files.insert(exe, None);
        pc.files.insert(install.join("module.toml"), Some(text));
        pc.subkeys.push(id.to_string());
        pc.reg.insert(
            (format!(r"{MODULES_KEY}\{id}"), String::new()),
            install.display().to_string(),
        );
        // The hub folder: empty, as on a first start.
        let home = base.join("home");
        Registry::open(&home, false).unwrap();
        let router = Router::new(Settings::default(), Box::new(|_, asked| asked.to_vec()));
        let (events, _rx) = mpsc::channel();
        let perm = PermUi::new(&home, Hub::new(router, events));
        let asked = Asked::default();
        let page = ModulesPage {
            installed: Vec::new(),
            facts: None,
            actions: Some(Box::new(asked.clone())),
            safe_by_closes: false,
            finder: None,
            fresh: None,
            grant: None,
            perm: Some(perm),
            discover: Some(DiscoverUi {
                machine: Box::new(pc),
                picker: Box::new(NoPicker),
                home,
                state: Default::default(),
            }),
            restarts: Default::default(),
            ask: Default::default(),
        };
        Fx {
            base,
            id: id.into(),
            page,
            asked,
        }
    }

    fn home(&self) -> PathBuf {
        self.page.discover.as_ref().unwrap().home.clone()
    }

    fn view(&self) -> hideout_registry::permission::View {
        Registry::open_from_env(&self.home())
            .unwrap()
            .permission_view(&self.id)
            .unwrap()
    }

    /// The Found card is up, with exactly this app on it (a look, not a click).
    fn look(&mut self) {
        let d = self.page.discover.as_mut().unwrap();
        let ids: Vec<String> = self.page.installed.iter().map(|m| m.id.clone()).collect();
        d.state.scan(&*d.machine, &ids, false);
        let shown: Vec<_> = d.state.shown().iter().map(|c| c.id.clone()).collect();
        let why =
            d.state.found.is_empty().then(|| {
                crate::discover::from_folder(&*d.machine, &self.base.join("install")).err()
            });
        assert_eq!(
            shown,
            vec![self.id.clone()],
            "the Found card offers the app ({why:?})"
        );
    }

    /// **Add** on the Found card. Registers the app and opens the Allow dialog; grants nothing.
    fn add(&mut self, clicks: &mut Clicks) {
        clicks.click("hideout: Add on the Found card");
        let staging = self.base.join("staging");
        let id = {
            let d = self.page.discover.as_mut().unwrap();
            let home = d.home.clone();
            d.state
                .add_found(&*d.machine, &self.id, &home, &staging, NOW)
                .expect("Add registers the app")
        };
        self.page.after_add(&id);
        // Registered, listed, dialog open, and still nothing allowed, on or started.
        assert!(self.page.installed.iter().any(|m| m.id == id && !m.enabled));
        assert!(self.view().granted.is_empty(), "Add grants nothing");
        assert!(self.page.perm.as_ref().unwrap().dialog.is_some());
        assert!(self.asked.0.borrow().is_empty(), "Add starts nothing");
    }

    /// Ticks every sensitive item and its I understand (2 clicks each), as today's dialog asks.
    fn tick_sensitive(&mut self, clicks: &mut Clicks) {
        let dlg = self.page.perm.as_mut().unwrap().dialog.as_mut().unwrap();
        let raws: Vec<String> = dlg
            .items
            .iter()
            .filter(|i| i.allowable && i.sensitive)
            .map(|i| i.raw.clone())
            .collect();
        for raw in raws {
            // Ticked, not yet understood: the item does not count.
            clicks.click(format!("dialog: tick {raw}"));
            dlg.set_checked(&raw, true);
            assert!(
                !dlg.allowed().chosen.contains(&raw),
                "{raw} counts before its I understand"
            );
            clicks.click(format!("dialog: I understand ({raw})"));
            dlg.items
                .iter_mut()
                .find(|i| i.raw == raw)
                .unwrap()
                .confirmed = true;
        }
    }

    /// **Allow**: records the shown list, then the page turns the app on. Returns how many
    /// items were recorded.
    fn allow(&mut self, clicks: &mut Clicks) -> usize {
        clicks.click("dialog: Allow");
        let perm = self.page.perm.as_mut().unwrap();
        let a = perm.dialog.as_ref().unwrap().allowed();
        let n = a.chosen.len();
        let done = perm.answer(PermAnswer::Allow(a)).expect("Allow records");
        assert_eq!(done.then, crate::permission_actions::Then::TurnOn);
        self.page.after_allow(done);
        n
    }

    /// No shortcut (so no Startup entry) appeared anywhere under the temp folder.
    fn assert_no_autostart_files(&self) {
        fn walk(p: &Path, out: &mut Vec<PathBuf>) {
            if let Ok(rd) = std::fs::read_dir(p) {
                for e in rd.flatten() {
                    let path = e.path();
                    if path.is_dir() {
                        walk(&path, out);
                    } else {
                        out.push(path);
                    }
                }
            }
        }
        let mut files = Vec::new();
        walk(&self.base, &mut files);
        assert!(
            !files
                .iter()
                .any(|f| f.extension().is_some_and(|x| x.eq_ignore_ascii_case("lnk"))),
            "a shortcut appeared: {files:?}"
        );
    }
}

/// The whole path, today's way: installer, open Hideout, Add, tick and confirm each sensitive
/// item, Allow.
fn run_first_run(fx: &mut Fx) -> Run {
    let mut c = Clicks::default();
    for s in installer_clicks(HUB_WXS) {
        c.click(s);
    }
    let installer = c.len();
    for s in launch_clicks(HUB_WXS) {
        c.click(s);
    }
    let launch = c.len() - installer;
    fx.look();
    let before_app = c.len();
    fx.add(&mut c);
    fx.tick_sensitive(&mut c);
    let allowed = fx.allow(&mut c);
    let app = c.len() - before_app;
    assert_eq!(
        fx.asked.0.borrow().as_slice(),
        [format!("turn-on {}", fx.id)],
        "Allow turns the app on, and that is the only thing asked of the supervisor"
    );
    assert!(fx.page.installed.iter().any(|m| m.id == fx.id && m.enabled));
    fx.assert_no_autostart_files();
    println!(
        "first run: {} allowed after {} clicks (installer {installer}, launch {launch}, in-app {app}): {:#?}",
        fx.id,
        c.len(),
        c.0
    );
    Run {
        clicks: c,
        installer,
        launch,
        app,
        allowed,
    }
}

// ---------------------------------------------------------------- the tests

#[test]
fn the_installer_sequence_is_read_as_data_and_starts_nothing_at_sign_in() {
    assert_eq!(installer_clicks(HUB_WXS).len(), BOUND_INSTALLER);
    assert_eq!(launch_clicks(HUB_WXS).len(), BOUND_LAUNCH);
    assert_installer_has_no_autostart(HUB_WXS);
    // A comment that names a Run value is not an installer entry.
    assert_eq!(strip_xml_comments("a<!-- Hideout.lnk -->b"), "ab");
}

/// The ratchet. Today: 3 installer + 1 Start menu + 1 Add + 4 x (tick + I understand) + 1
/// Allow = 14. A slice that lowers the path lowers the constants at the top of this file.
#[test]
fn first_run_with_a_generic_keyboard_app_costs_exactly_the_bound() {
    let mut fx = Fx::new("gizmo", "gizmo-keys", gizmo_manifest);
    let r = run_first_run(&mut fx);
    assert_eq!(r.allowed, SENSITIVE.len() + EVERYDAY, "all 21 recorded");
    assert!(fx.view().pending.is_empty());
    assert_eq!(r.installer + r.launch + r.app, r.clicks.len());
    assert_eq!(
        (r.installer, r.launch, r.app, r.clicks.len()),
        (BOUND_INSTALLER, BOUND_LAUNCH, BOUND_APP, BOUND_TOTAL),
        "the first-run click count changed: lower the BOUND_* constants if it dropped, \
         fix the regression if it grew"
    );
}

/// The same path for Foxglove's real manifest: the keyboard-sharing section is not on the
/// first-run dialog (FR2), so it costs nothing and shows no concept. The harness code names no product; only the file
/// it reads does.
#[test]
fn first_run_with_the_shipped_foxglove_manifest_costs_the_same() {
    let mut fx = Fx::new("foxglove", "foxglove", foxglove_manifest);
    let r = run_first_run(&mut fx);
    assert_eq!(r.allowed, 21);
    assert_eq!(r.app, BOUND_APP);
    assert_eq!(r.clicks.len(), BOUND_TOTAL);
    assert!(fx.view().pending.is_empty());
}

#[test]
fn not_now_grants_and_starts_nothing() {
    let mut fx = Fx::new("notnow", "gizmo-keys", gizmo_manifest);
    let mut c = Clicks::default();
    fx.look();
    fx.add(&mut c);
    assert!(fx
        .page
        .perm
        .as_mut()
        .unwrap()
        .answer(PermAnswer::NotNow)
        .is_none());
    assert!(fx.view().granted.is_empty());
    assert!(fx.asked.0.borrow().is_empty());
    assert!(!fx.page.installed.iter().any(|m| m.enabled));
    assert!(fx.page.perm.as_ref().unwrap().needs_ok(&fx.id));
    fx.assert_no_autostart_files();
}

/// Allow with the sensitive items ticked but not understood, or never ticked, records only the
/// everyday ones: no sensitive item is granted without its own I understand.
#[test]
fn no_sensitive_item_is_granted_without_its_i_understand() {
    for ticked in [false, true] {
        let mut fx = Fx::new(&format!("sens{ticked}"), "gizmo-keys", gizmo_manifest);
        let mut c = Clicks::default();
        fx.look();
        fx.add(&mut c);
        if ticked {
            let dlg = fx.page.perm.as_mut().unwrap().dialog.as_mut().unwrap();
            for s in SENSITIVE {
                dlg.set_checked(s, true);
            }
        }
        let n = fx.allow(&mut c);
        assert_eq!(n, EVERYDAY);
        let v = fx.view();
        for s in SENSITIVE {
            assert!(!v.granted.iter().any(|g| g.raw == s), "{s} was granted");
        }
        assert_eq!(v.granted.len(), EVERYDAY);
    }
}

/// FR2: what the first-run dialog says and offers, for the generic fixture and for the shipped
/// manifest. No product name is behind it, the keyboard-sharing section stays out, and the
/// dialog still grants nothing on its own.
#[test]
fn the_first_run_dialog_shows_what_the_app_can_do_and_what_needs_ok() {
    use hideout_shell::permission_dialog::{needs_ok_heading, ALLOW_AND_START};
    for (tag, id, manifest, can) in [
        (
            "fr2-gizmo",
            "gizmo-keys",
            gizmo_manifest as fn(&Path) -> String,
            "Gizmo Keys can: use your keyboard and PC lights, talk to other devices from the \
             same maker, go online, and read and save its own files.",
        ),
        (
            "fr2-foxglove",
            "foxglove",
            foxglove_manifest as fn(&Path) -> String,
            "Foxglove can: use your keyboard and PC lights",
        ),
    ] {
        let mut fx = Fx::new(tag, id, manifest);
        let mut c = Clicks::default();
        fx.look();
        fx.add(&mut c);
        let d = fx.page.perm.as_ref().unwrap().dialog.as_ref().unwrap();
        assert!(
            d.keyboard.is_none(),
            "{id}: no keyboard-sharing section on first run"
        );
        assert!(d.view.starts, "{id}: Allow starts the app");
        assert_eq!(d.sensitive_count(), SENSITIVE.len(), "{id}");
        assert_eq!(d.everyday(), EVERYDAY, "{id}");
        assert!(
            !d.everyday_shown(),
            "{id}: the everyday items start collapsed"
        );
        assert_eq!(
            needs_ok_heading(d.sensitive_count()),
            "These need your OK (4)"
        );
        let line = d.capability_line().expect("the capability line");
        assert!(line.starts_with(can), "{id}: {line}");
        assert!(
            d.source_line().unwrap().starts_with("Not signed \u{b7} "),
            "{id}"
        );
        assert_eq!(
            d.allow_label(),
            "Allow 17",
            "{id}: sensitive items unticked"
        );
        let _ = ALLOW_AND_START;
        // Collapsed means still ticked: Allow alone records the 17 and no sensitive item.
        assert_eq!(d.count(), EVERYDAY, "{id}");
        assert!(
            fx.view().granted.is_empty(),
            "{id}: showing the dialog grants nothing"
        );
        let n = fx.allow(&mut c);
        assert_eq!(n, EVERYDAY, "{id}");
    }
}

/// Allow with the keyboard section gone leaves the keyboard unasked, so the app's Permissions
/// offer it later.
#[test]
fn leaving_the_keyboard_section_out_of_first_run_records_nothing_for_it() {
    let mut fx = Fx::new("fr2-kbd", "foxglove", foxglove_manifest);
    let mut c = Clicks::default();
    fx.look();
    fx.add(&mut c);
    fx.tick_sensitive(&mut c);
    fx.allow(&mut c);
    let reg = Registry::open_from_env(&fx.home()).unwrap();
    assert!(matches!(
        reg.keyboard_state("foxglove"),
        hideout_registry::keyboard::KeyboardState::NotAsked
    ));
}
