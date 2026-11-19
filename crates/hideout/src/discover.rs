// SPDX-License-Identifier: MIT
//! Product slice 5: data-driven discovery. The Modules page's **Found** card and **Add an app…**.
//!
//! Where apps come from, in this order:
//! 1. **Self-registration.** An app's installer writes `HKCU\Software\Hideout\Modules\<id>`; the
//!    key's default value (or a value named `Path`) is the folder that holds the app's
//!    `module.toml`, or the file itself. The manifest names the app, its version and its program;
//!    nothing here knows any app.
//! 2. **The fallback list** (`products.json`, read at run time by `find_installed`), for apps whose
//!    installers do not register themselves yet.
//! 3. **Browse**: the user picks an app's folder.
//!
//! Adding registers the app through the existing `--adopt` code ([`crate::adopt::run`]). It
//! writes no grant and starts nothing: the app then waits for the normal first-run Allow dialog,
//! which the page opens. Reads only; the machine (registry, files) and the folder picker are the
//! [`Machine`] and [`Picker`] traits, so every test runs on fakes and never touches the real
//! registry.
#![cfg_attr(not(windows), allow(dead_code))]

use std::path::{Path, PathBuf};

use hideout_contract::module::{Context, Kind, Module};

use crate::find_installed::{self, Found, Machine};

/// The registry key apps register under (below `HKCU`).
pub const MODULES_KEY: &str = r"Software\Hideout\Modules";

/// Every string the user sees from this feature.
pub mod words {
    pub const FOUND_TITLE: &str = "Found on this PC";
    pub const FOUND_BODY: &str =
        "Hideout can run these. Before one starts, you choose what it may do.";
    pub const ADD: &str = "Add";
    pub const ADD_AN_APP: &str = "Add an app\u{2026}";
    pub const ADD_TITLE: &str = "Add an app";
    pub const ADD_BODY: &str =
        "Choose the app's folder. Hideout looks there for the file that describes the app.";
    pub const BROWSE: &str = "Browse\u{2026}";
    pub const CANCEL: &str = "Cancel";
    pub const NO_FILE: &str = "That folder has no app file Hideout can use.";
    pub const NOT_READ: &str = "That app's file could not be read.";
    pub const NOT_ADDABLE: &str = "That app can't be added this way.";
    pub const NOT_THIS_PC: &str = "That folder is not on this PC.";
    pub fn already(name: &str) -> String {
        format!("{name} is already in Hideout.")
    }
    pub fn not_added(name: &str, why: &str) -> String {
        format!("{name} could not be added: {why}")
    }
    /// The line for one found app.
    pub fn found_line(name: &str, version: &str) -> String {
        format!("{name} {version}")
    }
    pub fn add_hover(name: &str) -> String {
        format!("Add {name} to Hideout")
    }
}

/// Where a found app's registration comes from.
#[derive(Clone, Debug)]
pub enum Origin {
    /// A folder with the app's own `module.toml` (self-registered or browsed to).
    Folder(PathBuf),
    /// A fallback-list product, with a manifest made from its template.
    Product(Found),
}

/// An app Hideout could add.
#[derive(Clone, Debug)]
pub struct Candidate {
    pub id: String,
    pub name: String,
    pub version: String,
    pub origin: Origin,
}

/// The folder picker (a trait so tests choose without a window).
pub trait Picker {
    /// The folder the user chose, or `None` when cancelled.
    fn pick_folder(&self) -> Option<PathBuf>;
}

/// An absolute, local path: never relative, never UNC.
fn is_local(p: &Path) -> bool {
    let s = p.to_string_lossy();
    p.is_absolute() && !s.starts_with(r"\\") && !s.starts_with("//")
}

fn is_manifest_file(p: &Path) -> bool {
    p.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.eq_ignore_ascii_case("module.toml"))
}

/// The app described by the manifest in `at`: the folder, the manifest file, or a folder with
/// the manifest one level down in `hideout\`. Plain-words error when it is not an addable app.
pub fn from_folder(m: &dyn Machine, at: &Path) -> Result<Candidate, String> {
    if !is_local(at) {
        return Err(words::NOT_THIS_PC.into());
    }
    let folder = if is_manifest_file(at) {
        at.parent().map(Path::to_path_buf).unwrap_or_default()
    } else {
        at.to_path_buf()
    };
    let (folder, text) = [folder.clone(), folder.join("hideout")]
        .into_iter()
        .find_map(|f| m.read_text(&f.join("module.toml")).map(|t| (f, t)))
        .ok_or_else(|| words::NO_FILE.to_string())?;
    let module =
        Module::parse(&text, &Context::default()).map_err(|_| words::NOT_READ.to_string())?;
    if module.kind != Kind::Adopted {
        return Err(words::NOT_ADDABLE.into());
    }
    Ok(Candidate {
        id: module.id,
        name: module.name,
        version: module.version,
        origin: Origin::Folder(folder),
    })
}

/// The apps found on this PC that are not in Hideout yet (`installed` = the ids it already has):
/// self-registered ones first, then fallback-list products they did not already cover.
pub fn discover(m: &dyn Machine, installed: &[String]) -> Vec<Candidate> {
    let mut out: Vec<Candidate> = Vec::new();
    for sub in m.registry_subkeys(MODULES_KEY) {
        let key = format!(r"{MODULES_KEY}\{sub}");
        let Some(path) = m
            .registry_string(&key, "")
            .or_else(|| m.registry_string(&key, "Path"))
        else {
            continue;
        };
        match from_folder(m, Path::new(&path)) {
            // The key names the app; a manifest for another id is not that key's app.
            Ok(c) if c.id == sub => out.push(c),
            Ok(c) => log::warn!(
                "discover: key {sub} points at an app file for {}; skipped",
                c.id
            ),
            Err(why) => log::warn!("discover: key {sub}: {why}"),
        }
    }
    for p in find_installed::products() {
        if out.iter().any(|c| c.id == p.id) {
            continue;
        }
        if let Some(f) = find_installed::find(m, p) {
            out.push(Candidate {
                id: p.id.clone(),
                name: p.name.clone(),
                version: f.version.clone(),
                origin: Origin::Product(f),
            });
        }
    }
    out.retain(|c| !installed.contains(&c.id));
    out.sort_by_key(|c| c.name.to_lowercase());
    out
}

/// Registers `c` in the hub folder `home` (the `--adopt` code; no grant, nothing started).
/// Returns the app's id.
pub fn add(
    m: &dyn Machine,
    home: &Path,
    staging: &Path,
    c: &Candidate,
    now: &str,
) -> Result<String, String> {
    let done = match &c.origin {
        Origin::Folder(f) => crate::adopt::run(home, f, now),
        Origin::Product(found) => find_installed::adopt(m, home, staging, found, now),
    };
    done.map(|_| c.id.clone())
        .map_err(|why| words::not_added(&c.name, &why))
}

/// What the Found card and Add an app… show, and the steps behind their buttons.
#[derive(Default)]
pub struct AddApp {
    pub found: Vec<Candidate>,
    /// The Add an app… box is open.
    pub adding: bool,
    /// The last thing to tell the user (a refusal, "already in Hideout").
    pub note: Option<String>,
    scanned_for: Option<Vec<String>>,
}

impl AddApp {
    /// Looks again when what Hideout already holds changed since the last look (or `force`).
    pub fn scan(&mut self, m: &dyn Machine, installed: &[String], force: bool) {
        if !force && self.scanned_for.as_deref() == Some(installed) {
            return;
        }
        self.found = discover(m, installed);
        self.scanned_for = Some(installed.to_vec());
    }

    /// **Add an app…**: opens the box (and looks again, in case an app was installed since).
    pub fn open_box(&mut self, m: &dyn Machine, installed: &[String]) {
        self.adding = true;
        self.note = None;
        self.scan(m, installed, true);
    }

    pub fn cancel(&mut self) {
        self.adding = false;
        self.note = None;
    }

    /// **Browse…**: the user picks a folder; an addable app there is registered. `Some(id)` when
    /// it was added (the page then opens its Allow dialog).
    pub fn browse(
        &mut self,
        m: &dyn Machine,
        picker: &dyn Picker,
        home: &Path,
        staging: &Path,
        installed: &[String],
        now: &str,
    ) -> Option<String> {
        let folder = picker.pick_folder()?;
        match from_folder(m, &folder) {
            Ok(c) if installed.contains(&c.id) => {
                self.note = Some(words::already(&c.name));
                None
            }
            Ok(c) => self.register(m, home, staging, &c, now),
            Err(why) => {
                self.note = Some(why);
                None
            }
        }
    }

    /// **Add** on a found app (by its id).
    pub fn add_found(
        &mut self,
        m: &dyn Machine,
        id: &str,
        home: &Path,
        staging: &Path,
        now: &str,
    ) -> Option<String> {
        let c = self.found.iter().find(|c| c.id == id)?.clone();
        self.register(m, home, staging, &c, now)
    }

    fn register(
        &mut self,
        m: &dyn Machine,
        home: &Path,
        staging: &Path,
        c: &Candidate,
        now: &str,
    ) -> Option<String> {
        match add(m, home, staging, c, now) {
            Ok(id) => {
                self.adding = false;
                self.note = None;
                self.found.retain(|f| f.id != id);
                Some(id)
            }
            Err(why) => {
                self.note = Some(why);
                None
            }
        }
    }
}

#[cfg(windows)]
pub use ui::DiscoverUi;

#[cfg(windows)]
mod ui {
    use super::*;
    use hideout_shell::egui::{self, RichText};
    use hideout_shell::theme::Tokens;

    /// The Windows folder picker, modal to the hub window.
    pub struct ShellPicker;

    impl Picker for ShellPicker {
        fn pick_folder(&self) -> Option<PathBuf> {
            hideout_shell::pick_folder()
        }
    }

    /// The page's discovery part: the machine, the picker, the hub folder and the card state.
    pub struct DiscoverUi {
        pub machine: Box<dyn Machine>,
        pub picker: Box<dyn Picker>,
        pub home: PathBuf,
        pub state: AddApp,
    }

    enum Click {
        Add(String),
        OpenBox,
        Browse,
        Cancel,
    }

    fn c(x: hideout_shell::theme::Rgb) -> egui::Color32 {
        egui::Color32::from_rgb(x.0, x.1, x.2)
    }

    impl DiscoverUi {
        pub fn new(home: PathBuf) -> DiscoverUi {
            DiscoverUi {
                machine: Box::new(find_installed::RealMachine),
                picker: Box::new(ShellPicker),
                home,
                state: AddApp::default(),
            }
        }

        fn staging(&self) -> PathBuf {
            std::env::temp_dir().join("hideout-adopt")
        }

        /// Carries out a click; `Some(id)` when an app was added.
        fn act(&mut self, click: Click, installed: &[String]) -> Option<String> {
            let now = hideout_registry::catalogs::now_utc();
            let staging = self.staging();
            match click {
                Click::Add(id) => {
                    self.state
                        .add_found(self.machine.as_ref(), &id, &self.home, &staging, &now)
                }
                Click::OpenBox => {
                    self.state.open_box(self.machine.as_ref(), installed);
                    None
                }
                Click::Browse => self.state.browse(
                    self.machine.as_ref(),
                    self.picker.as_ref(),
                    &self.home,
                    &staging,
                    installed,
                    &now,
                ),
                Click::Cancel => {
                    self.state.cancel();
                    None
                }
            }
        }

        /// Draws the Found card and the Add an app… line above the rows. `Some(id)` when an app
        /// was added this frame: the page then opens its Allow dialog.
        pub fn ui(
            &mut self,
            ui: &mut egui::Ui,
            t: &Tokens,
            installed: &[String],
        ) -> Option<String> {
            self.state.scan(self.machine.as_ref(), installed, false);
            let mut click = None;
            if !self.state.found.is_empty() {
                frame(ui, t, |ui| {
                    let head =
                        ui.label(RichText::new(words::FOUND_TITLE).strong().color(c(t.text)));
                    ui.ctx().accesskit_node_builder(head.id, |n| {
                        n.set_live(egui::accesskit::Live::Polite);
                    });
                    ui.label(RichText::new(words::FOUND_BODY).color(c(t.text_muted)));
                    ui.add_space(6.0);
                    for f in &self.state.found {
                        ui.push_id(("hub.found", &f.id), |ui| {
                            ui.horizontal(|ui| {
                                ui.label(
                                    RichText::new(words::found_line(&f.name, &f.version))
                                        .color(c(t.text)),
                                );
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        let b = ui
                                            .add(
                                                egui::Button::new(
                                                    RichText::new(words::ADD).color(c(t.on_accent)),
                                                )
                                                .fill(c(t.accent)),
                                            )
                                            .on_hover_text(words::add_hover(&f.name));
                                        if b.clicked() {
                                            click = Some(Click::Add(f.id.clone()));
                                        }
                                    },
                                );
                            });
                        });
                    }
                });
                ui.add_space(8.0);
            }
            if self.state.adding {
                frame(ui, t, |ui| {
                    ui.label(RichText::new(words::ADD_TITLE).strong().color(c(t.text)));
                    ui.label(RichText::new(words::ADD_BODY).color(c(t.text_muted)));
                    if let Some(note) = &self.state.note {
                        let n = ui.label(RichText::new(note).color(c(t.danger)));
                        ui.ctx().accesskit_node_builder(n.id, |x| {
                            x.set_live(egui::accesskit::Live::Polite);
                        });
                    }
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        if ui
                            .add(
                                egui::Button::new(
                                    RichText::new(words::BROWSE).color(c(t.on_accent)),
                                )
                                .fill(c(t.accent)),
                            )
                            .clicked()
                        {
                            click = Some(Click::Browse);
                        }
                        if ui.button(words::CANCEL).clicked() {
                            click = Some(Click::Cancel);
                        }
                    });
                });
                ui.add_space(8.0);
            } else if ui.button(words::ADD_AN_APP).clicked() {
                click = Some(Click::OpenBox);
            }
            ui.add_space(8.0);
            click.and_then(|k| self.act(k, installed))
        }
    }

    fn frame(ui: &mut egui::Ui, t: &Tokens, body: impl FnOnce(&mut egui::Ui)) {
        egui::Frame::new()
            .fill(c(t.surface))
            .stroke(egui::Stroke::new(1.0_f32, c(t.border)))
            .corner_radius(egui::CornerRadius::same(6))
            .inner_margin(egui::Margin::symmetric(14, 10))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                body(ui);
            });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[derive(Default)]
    struct Fake {
        reg: BTreeMap<(String, String), String>,
        subkeys: Vec<String>,
        env: BTreeMap<String, String>,
        /// path -> (file version, text)
        files: BTreeMap<PathBuf, (String, Option<String>)>,
    }

    impl Machine for Fake {
        fn registry_string(&self, key: &str, value: &str) -> Option<String> {
            self.reg.get(&(key.into(), value.into())).cloned()
        }
        fn env(&self, n: &str) -> Option<String> {
            self.env.get(n).cloned()
        }
        fn is_file(&self, p: &Path) -> bool {
            self.files.contains_key(p)
        }
        fn exe_version(&self, p: &Path) -> Option<String> {
            self.files.get(p).map(|f| f.0.clone())
        }
        fn read_text(&self, p: &Path) -> Option<String> {
            self.files.get(p).and_then(|f| f.1.clone())
        }
        fn registry_subkeys(&self, key: &str) -> Vec<String> {
            if key == MODULES_KEY {
                self.subkeys.clone()
            } else {
                Vec::new()
            }
        }
    }

    struct Pick(Option<PathBuf>);
    impl Picker for Pick {
        fn pick_folder(&self) -> Option<PathBuf> {
            self.0.clone()
        }
    }

    fn manifest(id: &str, name: &str, version: &str, kind: &str, exe: &str) -> String {
        format!(
            "id = \"{id}\"\nname = \"{name}\"\nversion = \"{version}\"\nkind = \"{kind}\"\n\
             family = \"vulpes\"\nhub = \"1.1\"\nrun = ['{exe}', 'hub']\n\
             api = {{ vulpes = \"1.0\" }}\n[activation]\non = [\"open\", \"call\"]\n"
        )
    }

    fn dir(name: &str) -> PathBuf {
        PathBuf::from(format!(r"C:\Apps\{name}"))
    }

    /// Registers `id` under the Modules key, pointing at a folder with its manifest.
    fn register(f: &mut Fake, id: &str, name: &str, version: &str, as_file: bool) {
        let folder = dir(id);
        let toml = folder.join("module.toml");
        f.files.insert(
            toml.clone(),
            (
                String::new(),
                Some(manifest(id, name, version, "adopted", "C:/Apps/x.exe")),
            ),
        );
        f.subkeys.push(id.to_string());
        let at = if as_file { toml } else { folder };
        f.reg.insert(
            (format!(r"{MODULES_KEY}\{id}"), String::new()),
            at.display().to_string(),
        );
    }

    #[test]
    fn self_registered_apps_are_found_by_id_from_a_folder_or_a_manifest_path() {
        let mut f = Fake::default();
        register(&mut f, "gizmo", "Gizmo", "2.0.1", false);
        register(&mut f, "widget", "Widget", "0.3.0", true);
        let found = discover(&f, &[]);
        let names: Vec<_> = found
            .iter()
            .map(|c| (c.id.as_str(), c.name.as_str()))
            .collect();
        assert_eq!(names, [("gizmo", "Gizmo"), ("widget", "Widget")]);
        assert_eq!(found[1].version, "0.3.0");
        assert!(matches!(&found[0].origin, Origin::Folder(p) if *p == dir("gizmo")));
    }

    #[test]
    fn the_value_can_be_named_path_and_a_key_for_another_id_is_not_that_app() {
        let mut f = Fake::default();
        register(&mut f, "gizmo", "Gizmo", "1.0.0", false);
        let k = format!(r"{MODULES_KEY}\gizmo");
        let v = f.reg.remove(&(k.clone(), String::new())).unwrap();
        f.reg.insert((k, "Path".into()), v);
        // A key "other" whose manifest says "gizmo".
        f.subkeys.push("other".into());
        f.reg.insert(
            (format!(r"{MODULES_KEY}\other"), String::new()),
            dir("gizmo").display().to_string(),
        );
        let found = discover(&f, &[]);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id, "gizmo");
    }

    #[test]
    fn a_bad_registration_is_skipped_and_the_rest_still_show() {
        let mut f = Fake::default();
        register(&mut f, "good", "Good", "1.0.0", false);
        // Points nowhere, points at a share, points at a non-adopted manifest, unreadable toml.
        for (id, at) in [("gone", r"C:\Nowhere"), ("share", r"\\srv\share\app")] {
            f.subkeys.push(id.into());
            f.reg
                .insert((format!(r"{MODULES_KEY}\{id}"), String::new()), at.into());
        }
        f.subkeys.push("svc".into());
        f.reg.insert(
            (format!(r"{MODULES_KEY}\svc"), String::new()),
            dir("svc").display().to_string(),
        );
        f.files.insert(
            dir("svc").join("module.toml"),
            (
                String::new(),
                Some(manifest("svc", "Svc", "1.0.0", "service", "C:/x.exe")),
            ),
        );
        f.subkeys.push("junk".into());
        f.reg.insert(
            (format!(r"{MODULES_KEY}\junk"), String::new()),
            dir("junk").display().to_string(),
        );
        f.files.insert(
            dir("junk").join("module.toml"),
            (String::new(), Some("not toml {".into())),
        );
        let found = discover(&f, &[]);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id, "good");
    }

    #[test]
    fn the_fallback_list_still_finds_the_two_known_apps_and_a_registration_wins() {
        let mut f = Fake::default();
        // Foxglove from its known folder; Vulpes self-registers (and has an installer record).
        f.env
            .insert("LOCALAPPDATA".into(), r"C:\Users\t\AppData\Local".into());
        f.files.insert(
            PathBuf::from(r"C:\Users\t\AppData\Local\Programs\Foxglove\foxglove.exe"),
            ("0.3.0".into(), None),
        );
        register(&mut f, "vulpes", "Vulpes", "0.1.0", false);
        f.reg.insert(
            (r"Software\Vulpes\Installer".into(), "App".into()),
            r"C:\Apps\vulpes".into(),
        );
        f.files.insert(
            PathBuf::from(r"C:\Apps\vulpes\venv\Scripts\arete.exe"),
            ("0.1.0".into(), None),
        );
        let found = discover(&f, &[]);
        let ids: Vec<_> = found.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, ["foxglove", "vulpes"]);
        let vulpes = found.iter().find(|c| c.id == "vulpes").unwrap();
        assert!(
            matches!(vulpes.origin, Origin::Folder(_)),
            "the registration wins"
        );
        let fox = found.iter().find(|c| c.id == "foxglove").unwrap();
        assert!(matches!(fox.origin, Origin::Product(_)));
    }

    #[test]
    fn apps_hideout_already_has_are_not_offered() {
        let mut f = Fake::default();
        register(&mut f, "gizmo", "Gizmo", "1.0.0", false);
        register(&mut f, "widget", "Widget", "1.0.0", false);
        let found = discover(&f, &["gizmo".to_string()]);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id, "widget");
    }

    fn temp(name: &str) -> PathBuf {
        let d =
            std::env::temp_dir().join(format!("hideout-discover-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// A real folder with a real exe and manifest (the add path writes to a temp hub folder),
    /// with a fake `Machine` that reads that manifest.
    fn app_on_disk(name: &str, id: &str) -> (PathBuf, Fake) {
        let install = temp(name);
        let exe = install.join("tool.exe");
        std::fs::write(&exe, b"x").unwrap();
        let text = manifest(
            id,
            "Gizmo",
            "1.4.0",
            "adopted",
            &exe.display().to_string().replace('\\', "/"),
        );
        std::fs::write(install.join("module.toml"), &text).unwrap();
        let mut f = Fake::default();
        f.files
            .insert(install.join("module.toml"), (String::new(), Some(text)));
        (install, f)
    }

    const NOW: &str = "2026-11-19T08:00:00Z";

    #[test]
    fn browse_adds_the_app_in_the_folder_and_writes_no_grant() {
        let (install, f) = app_on_disk("browse", "gizmo");
        let home = temp("browse-home");
        let mut s = AddApp::default();
        s.open_box(&f, &[]);
        assert!(s.adding);
        let id = s.browse(&f, &Pick(Some(install)), &home, &temp("stg"), &[], NOW);
        assert_eq!(id.as_deref(), Some("gizmo"));
        assert!(!s.adding && s.note.is_none());
        let reg = hideout_registry::Registry::open_from_env(&home).unwrap();
        assert!(reg.inventory.modules.contains_key("gizmo"));
        // Registered, never allowed: the first-run Allow dialog is the user's.
        assert!(!reg.hub.enabled("gizmo"));
    }

    #[test]
    fn browse_says_why_in_plain_words_and_cancel_adds_nothing() {
        let (install, f) = app_on_disk("browse2", "gizmo");
        let home = temp("browse2-home");
        let stg = temp("stg2");
        let mut s = AddApp::default();
        // Cancelled picker: nothing, no note.
        assert!(s.browse(&f, &Pick(None), &home, &stg, &[], NOW).is_none());
        assert!(s.note.is_none());
        // An empty folder.
        let _ = s.browse(
            &f,
            &Pick(Some(PathBuf::from(r"C:\Empty"))),
            &home,
            &stg,
            &[],
            NOW,
        );
        assert_eq!(s.note.as_deref(), Some(words::NO_FILE));
        // A share.
        let _ = s.browse(
            &f,
            &Pick(Some(PathBuf::from(r"\\srv\x"))),
            &home,
            &stg,
            &[],
            NOW,
        );
        assert_eq!(s.note.as_deref(), Some(words::NOT_THIS_PC));
        // Already added.
        let _ = s.browse(
            &f,
            &Pick(Some(install)),
            &home,
            &stg,
            &["gizmo".into()],
            NOW,
        );
        assert_eq!(s.note.as_deref(), Some("Gizmo is already in Hideout."));
        let reg = hideout_registry::Registry::open_from_env(&home).unwrap();
        assert!(reg.inventory.modules.is_empty());
    }

    #[test]
    fn adding_a_found_app_registers_it_and_leaves_the_card_and_a_refusal_keeps_it() {
        let (install, mut f) = app_on_disk("found", "gizmo");
        f.subkeys.push("gizmo".into());
        f.reg.insert(
            (format!(r"{MODULES_KEY}\gizmo"), String::new()),
            install.display().to_string(),
        );
        let home = temp("found-home");
        let stg = temp("stg3");
        let mut s = AddApp::default();
        s.scan(&f, &[], false);
        assert_eq!(s.found.len(), 1);
        assert_eq!(
            s.add_found(&f, "gizmo", &home, &stg, NOW).as_deref(),
            Some("gizmo")
        );
        assert!(s.found.is_empty());
        // The program is gone: refused with a sentence, the card stays.
        let (gone, mut g) = app_on_disk("found2", "gizmo");
        g.subkeys.push("gizmo".into());
        g.reg.insert(
            (format!(r"{MODULES_KEY}\gizmo"), String::new()),
            gone.display().to_string(),
        );
        std::fs::remove_file(gone.join("tool.exe")).unwrap();
        let mut s2 = AddApp::default();
        s2.scan(&g, &[], false);
        assert!(s2
            .add_found(&g, "gizmo", &temp("found2-home"), &stg, NOW)
            .is_none());
        assert!(s2
            .note
            .as_deref()
            .unwrap()
            .starts_with("Gizmo could not be added: "));
        assert_eq!(s2.found.len(), 1);
    }

    #[test]
    fn scan_only_looks_again_when_the_installed_list_changes() {
        let mut f = Fake::default();
        let mut s = AddApp::default();
        s.scan(&f, &[], false);
        assert!(s.found.is_empty());
        register(&mut f, "gizmo", "Gizmo", "1.0.0", false);
        s.scan(&f, &[], false);
        assert!(s.found.is_empty(), "same list: no new registry read");
        s.scan(&f, &["other".into()], false);
        assert_eq!(s.found.len(), 1);
    }

    /// No UI string names a product: the words module is generic.
    #[test]
    fn the_words_name_no_product() {
        let all = [
            words::FOUND_TITLE,
            words::FOUND_BODY,
            words::ADD,
            words::ADD_AN_APP,
            words::ADD_TITLE,
            words::ADD_BODY,
            words::BROWSE,
            words::CANCEL,
            words::NO_FILE,
            words::NOT_READ,
            words::NOT_ADDABLE,
            words::NOT_THIS_PC,
        ];
        for w in all {
            let l = w.to_lowercase();
            for p in find_installed::products() {
                assert!(!l.contains(&p.name.to_lowercase()), "{w}");
            }
            assert!(!l.contains("arete") && !l.contains("module.toml"), "{w}");
        }
    }
}
