// SPDX-License-Identifier: MIT
//! The whole command is what the user allows (#1452): the list code covers the arguments, and
//! an app whose program runs whatever it is handed (a script runner, a program in the Windows
//! folder) gets an OK for one run only, with nothing bound or remembered by any caller.
//! Window-free; the program files are text placeholders and are never started.

use std::path::{Path, PathBuf};

use hideout_registry::inventory::RunState;
use hideout_registry::keyboard::{canonical_path, VoidReason};
use hideout_registry::one_run::OneRun;
use hideout_registry::permission::LIST_FORMAT;
use hideout_registry::{Failure, Registry};

const NOW: &str = "2026-11-25T12:00:00Z";
const LATER: &str = "2026-11-25T20:40:00Z";
const ID: &str = "foxglove";

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir()
        .join("hideout-one-run-tests")
        .join(format!("{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn manifest(run: &[String]) -> String {
    let run: Vec<String> = run
        .iter()
        .map(|a| format!("\"{}\"", a.replace('\\', "/")))
        .collect();
    format!(
        "id = \"{ID}\"\nname = \"Foxglove\"\nversion = \"1.0.0\"\nkind = \"adopted\"\n\
         family = \"foxglove\"\nhub = \"1.0\"\nrun = [{}]\napi = {{ foxglove = \"1.0\" }}\n\
         permissions = [\"hub.notify\", \"ui.window\"]\n",
        run.join(", ")
    )
}

struct Fx {
    home: PathBuf,
    install: PathBuf,
    exe: PathBuf,
    folder: PathBuf,
    reg: Registry,
}

impl Fx {
    fn run(&self, args: &[&str]) -> Vec<String> {
        std::iter::once(self.exe.display().to_string())
            .chain(args.iter().map(|a| a.to_string()))
            .collect()
    }

    /// Points the manifest at other arguments and re-adopts it, as `--adopt` does.
    fn readopt(&mut self, args: &[&str]) {
        std::fs::write(self.folder.join("module.toml"), manifest(&self.run(args))).unwrap();
        let mut other = Registry::open(&self.home, true).unwrap();
        other.register_adopted(&self.folder, NOW).unwrap();
        self.reg = Registry::open(&self.home, true).unwrap();
    }

    fn module(&self) -> &hideout_registry::inventory::Installed {
        &self.reg.inventory.modules[ID]
    }

    fn state(&self) -> Option<RunState> {
        self.module().one_run.as_ref().map(|r| r.state)
    }

    /// The program as the launch holds it: canonical path and the hash of its bytes.
    fn pinned(&self) -> (String, String) {
        let list = self.reg.permission_list(ID).unwrap();
        (list.exe_path, list.exe_sha256)
    }

    fn on_disk(&self) -> serde_json::Value {
        let text = std::fs::read_to_string(self.home.join("inventory.json")).unwrap();
        serde_json::from_str::<serde_json::Value>(&text).unwrap()["modules"][ID].clone()
    }
}

/// An adopted app whose program file is `file` (a text placeholder), run with `args`.
fn fx(name: &str, file: &str, args: &[&str]) -> Fx {
    let install = temp(&format!("{name}-install"));
    let exe = install.join(file);
    std::fs::write(&exe, b"build 1").unwrap();
    let folder = temp(&format!("{name}-manifest"));
    let home = temp(&format!("{name}-home"));
    let mut f = Fx {
        reg: Registry::open(&home, true).unwrap(),
        home,
        install,
        exe,
        folder,
    };
    std::fs::write(f.folder.join("module.toml"), manifest(&f.run(args))).unwrap();
    f.reg.register_adopted(&f.folder, NOW).unwrap();
    f
}

fn allow(
    f: &mut Fx,
    by: &str,
    follow: bool,
) -> Result<hideout_registry::permission::Outcome, Failure> {
    let list = f.reg.permission_list(ID)?;
    let all: Vec<String> = list.items.iter().map(|i| i.raw.clone()).collect();
    let o = f
        .reg
        .approve_listed(ID, list.code(), &all, &[], NOW, by, follow)?;
    f.reg.set_enabled(ID, true)?;
    Ok(o)
}

fn windows_of(dir: &Path) -> String {
    canonical_path(dir).unwrap()
}

#[test]
fn a_script_runner_app_is_allowed_for_one_run_and_asks_again_at_the_next_start() {
    for by in ["cli", "dialog"] {
        let mut f = fx(&format!("runner-{by}"), "python.exe", &["-m", "my_app"]);
        assert_eq!(f.reg.one_run(ID), Some(OneRun::Scripts));
        let o = allow(&mut f, by, false).unwrap();
        assert_eq!(o.one_run, Some(OneRun::Scripts), "{by}");
        assert_eq!(o.granted.len(), 2);
        // Nothing is bound, in memory or in the file.
        assert!(f.module().bound.is_none(), "{by}");
        assert!(f.on_disk().get("bound").is_none(), "{by}");
        assert_eq!(f.state(), Some(RunState::Allowed));
        assert_eq!(f.reg.binding_void(ID), None);
        let (path, sha) = f.pinned();
        // A launch whose start was not counted does not run on the OK.
        assert_eq!(
            f.reg.binding_void_pinned(ID, &path, &sha),
            Some(VoidReason::Changed)
        );

        let args = vec!["-m".to_string(), "my_app".to_string()];
        assert!(!f.reg.start_counted(ID), "{by}");
        assert_eq!(
            f.reg.binding_void_launch(ID, &path, &sha, &args, false),
            Some(VoidReason::Changed)
        );

        // The first start uses the OK: it is the counted one, for the registry that counted it.
        f.reg.keep_followed(ID, LATER).unwrap();
        assert_eq!(f.state(), Some(RunState::Started));
        assert!(f.reg.start_counted(ID), "{by}");
        assert_eq!(f.reg.binding_void(ID), None);
        assert_eq!(
            f.reg.binding_void_launch(ID, &path, &sha, &args, true),
            None
        );
        // The record alone (`started` in the file) passes no launch: not without the count,
        // not through the form that knows of no start, not for a registry that only read it.
        assert_eq!(
            f.reg.binding_void_launch(ID, &path, &sha, &args, false),
            Some(VoidReason::Changed)
        );
        assert_eq!(
            f.reg.binding_void_pinned(ID, &path, &sha),
            Some(VoidReason::Changed)
        );
        let read = Registry::open(&f.home, true).unwrap();
        assert!(!read.start_counted(ID), "{by}");
        assert_eq!(
            read.binding_void_launch(ID, &path, &sha, &args, read.start_counted(ID)),
            Some(VoidReason::Changed)
        );
        // The launch is held to the arguments it starts the process with.
        let other = vec!["-m".to_string(), "other_app".to_string()];
        assert_eq!(
            f.reg.binding_void_launch(ID, &path, &sha, &other, true),
            Some(VoidReason::Changed)
        );
        assert_eq!(
            f.reg.binding_void_launch(ID, &path, &sha, &[], true),
            Some(VoidReason::Changed)
        );
        assert!(f.reg.pre_launch(ID).is_ok(), "{by}");
        assert_eq!(f.module().granted.len(), 2);

        // The next start (a restart, a crash restart, the hub starting again) asks again: a
        // registry read from the file, as the launch opens one.
        let mut next = Registry::open(&f.home, true).unwrap();
        next.keep_followed(ID, LATER).unwrap();
        f.reg = Registry::open(&f.home, true).unwrap();
        assert_eq!(f.state(), Some(RunState::Ended));
        assert!(f.module().granted.is_empty(), "{by}");
        assert_eq!(f.module().pending_permissions.len(), 2);
        assert_eq!(f.module().last_choice.len(), 2);
        assert!(f.module().bound.is_none());
        assert!(f.on_disk().get("bound").is_none());
        assert_eq!(f.reg.binding_void(ID), Some(VoidReason::Changed));
        assert_eq!(
            f.reg.binding_void_pinned(ID, &path, &sha),
            Some(VoidReason::Changed)
        );
        assert!(f.reg.pre_launch(ID).is_err(), "{by}");
        let v = f.reg.permission_view(ID).unwrap();
        assert_eq!(v.one_run, Some(OneRun::Scripts));
        assert!(v.asks_again, "an ordinary ask, not 'the app changed'");
        // Further starts change nothing and allow nothing.
        f.reg.keep_followed(ID, LATER).unwrap();
        assert_eq!(f.state(), Some(RunState::Ended));
        assert_eq!(f.reg.void_if_changed(ID, LATER).unwrap(), None);

        // The next Allow is again for one run.
        let o = allow(&mut f, by, false).unwrap();
        assert_eq!(o.one_run, Some(OneRun::Scripts));
        assert_eq!(f.state(), Some(RunState::Allowed));
        assert!(f.module().bound.is_none());
        assert!(!f.reg.permission_view(ID).unwrap().asks_again);
    }
}

#[test]
fn keeping_the_ok_is_refused_for_a_one_run_app_and_nothing_is_written() {
    let mut f = fx("keep", "node.exe", &["server.js"]);
    let before = f.on_disk();
    let err = allow(&mut f, "dialog", true).unwrap_err();
    assert!(err.to_string().contains("one run only"), "{err}");
    assert_eq!(f.on_disk(), before);
    assert!(f.module().bound.is_none());
    assert!(f.module().one_run.is_none());
    assert!(f.module().granted.is_empty());
}

#[test]
fn a_program_in_the_windows_folder_gets_a_one_run_ok() {
    let mut f = fx("windows", "foxglove.exe", &["thing.dll,Entry"]);
    assert_eq!(f.reg.one_run(ID), None);
    let windows = windows_of(&f.install);
    f.reg.set_windows_folder(Some(&windows));
    assert_eq!(f.reg.one_run(ID), Some(OneRun::Windows));
    let o = allow(&mut f, "cli", false).unwrap();
    assert_eq!(o.one_run, Some(OneRun::Windows));
    assert!(f.module().bound.is_none());
    assert_eq!(f.state(), Some(RunState::Allowed));
    assert!(allow(&mut f, "dialog", true).is_err());
    // A folder whose name only starts the same is another folder.
    f.reg.set_windows_folder(Some(&format!("{windows}-old")));
    assert_eq!(f.reg.one_run(ID), None);
    let parent = windows_of(f.install.parent().unwrap());
    f.reg.set_windows_folder(Some(&parent));
    assert_eq!(f.reg.one_run(ID), Some(OneRun::Windows));
}

#[test]
fn a_remembered_ok_from_before_the_rule_is_not_an_ok_for_a_one_run_app() {
    let mut f = fx("before", "foxglove.exe", &[]);
    allow(&mut f, "dialog", true).unwrap();
    assert!(f.module().bound.as_ref().unwrap().follow.is_some());
    assert_eq!(f.reg.binding_void(ID), None);
    // The same record, once the program counts as part of Windows.
    let windows = windows_of(&f.install);
    f.reg.set_windows_folder(Some(&windows));
    assert_eq!(f.reg.binding_void(ID), Some(VoidReason::Changed));
    let (path, sha) = f.pinned();
    assert_eq!(
        f.reg.binding_void_pinned(ID, &path, &sha),
        Some(VoidReason::Changed)
    );
    assert!(f.reg.permission_view(ID).unwrap().asks_again);
    // The start drops the binding and asks.
    f.reg.keep_followed(ID, LATER).unwrap();
    assert!(f.module().bound.is_none());
    assert!(f.on_disk().get("bound").is_none());
    assert_eq!(f.state(), Some(RunState::Ended));
    assert!(f.module().granted.is_empty());
    assert_eq!(f.module().last_choice.len(), 2);
    assert!(f.reg.permission_view(ID).unwrap().asks_again);

    // The turn-on check does the same without a start.
    let mut g = fx("before-on", "foxglove.exe", &[]);
    allow(&mut g, "cli", false).unwrap();
    let windows = windows_of(&g.install);
    g.reg.set_windows_folder(Some(&windows));
    assert_eq!(
        g.reg.void_if_changed(ID, LATER).unwrap(),
        Some(VoidReason::Changed)
    );
    assert!(g.module().bound.is_none());
    assert_eq!(g.state(), Some(RunState::Ended));
    assert!(g.module().granted.is_empty());
}

/// A start whose count can't be written down is not the counted one, so the launch asks again
/// instead of running on an OK that stays unused.
#[test]
fn a_start_that_cannot_be_counted_is_not_the_one_the_ok_is_for() {
    let mut f = fx("uncounted", "python.exe", &["app.py"]);
    allow(&mut f, "dialog", false).unwrap();
    let (path, sha) = f.pinned();
    let args = vec!["app.py".to_string()];
    // The inventory can't be written: a folder is where its new copy goes.
    let blocker = f.home.join("inventory.json.tmp");
    std::fs::create_dir_all(&blocker).unwrap();
    for _ in 0..3 {
        assert!(f.reg.keep_followed(ID, LATER).is_err());
        assert!(!f.reg.start_counted(ID));
        assert_eq!(
            f.reg
                .binding_void_launch(ID, &path, &sha, &args, f.reg.start_counted(ID)),
            Some(VoidReason::Changed)
        );
    }
    // Once it can be written, the OK is still there for exactly one start.
    std::fs::remove_dir_all(&blocker).unwrap();
    let mut next = Registry::open(&f.home, true).unwrap();
    assert_eq!(
        next.inventory.modules[ID].one_run.as_ref().map(|r| r.state),
        Some(RunState::Allowed)
    );
    next.keep_followed(ID, LATER).unwrap();
    assert!(next.start_counted(ID));
    assert_eq!(next.binding_void_launch(ID, &path, &sha, &args, true), None);
    next.keep_followed(ID, LATER).unwrap();
    assert!(!next.start_counted(ID), "the second start ends the OK");
    assert_eq!(
        next.binding_void_launch(ID, &path, &sha, &args, next.start_counted(ID)),
        Some(VoidReason::Changed)
    );
}

/// A Deny allows nothing, so it is not an OK for a run and writes none.
#[test]
fn a_deny_writes_no_ok_for_a_one_run_app() {
    let mut f = fx("deny", "python.exe", &["app.py"]);
    let list = f.reg.permission_list(ID).unwrap();
    let all: Vec<String> = list.items.iter().map(|i| i.raw.clone()).collect();
    for by in ["cli", "dialog"] {
        f.reg.deny_listed(ID, list.code(), &all, NOW, by).unwrap();
        assert!(f.module().one_run.is_none(), "{by}");
        assert!(f.module().bound.is_none(), "{by}");
        assert!(f.on_disk().get("one_run").is_none(), "{by}");
        assert!(f.module().granted.is_empty(), "{by}");
    }

    // A Deny of what is asked later leaves a live OK as it was, and brings back none that is
    // over.
    let mut g = fx("deny-live", "python.exe", &["app.py"]);
    allow(&mut g, "dialog", false).unwrap();
    let code = g.reg.permission_list(ID).unwrap().code().to_string();
    g.reg.deny_listed(ID, &code, &[], LATER, "dialog").unwrap();
    assert_eq!(g.state(), Some(RunState::Allowed));
    assert_eq!(g.module().one_run.as_ref().unwrap().at, NOW);
    g.reg.keep_followed(ID, LATER).unwrap();
    g.reg.keep_followed(ID, LATER).unwrap();
    assert_eq!(g.state(), Some(RunState::Ended));
    let list = g.reg.permission_list(ID).unwrap();
    let all: Vec<String> = list.items.iter().map(|i| i.raw.clone()).collect();
    g.reg
        .deny_listed(ID, list.code(), &all, LATER, "dialog")
        .unwrap();
    assert!(g.module().one_run.is_none());
    assert!(g.module().granted.is_empty());
}

#[test]
fn other_arguments_or_other_bytes_end_a_one_run_ok() {
    let mut f = fx("args", "python.exe", &["app.py"]);
    allow(&mut f, "cli", false).unwrap();
    f.readopt(&["other.py"]);
    assert_eq!(f.state(), Some(RunState::Allowed), "re-adopting keeps it");
    assert_eq!(f.reg.binding_void(ID), Some(VoidReason::Changed));
    assert_eq!(
        f.reg.void_if_changed(ID, LATER).unwrap(),
        Some(VoidReason::Changed)
    );
    assert_eq!(f.state(), Some(RunState::Void));
    assert!(f.module().granted.is_empty());
    assert!(f.module().bound.is_none());
    // The old arguments do not bring it back.
    f.readopt(&["app.py"]);
    assert_eq!(f.reg.binding_void(ID), Some(VoidReason::Changed));
    assert!(!f.reg.permission_view(ID).unwrap().asks_again);

    let mut g = fx("bytes", "python.exe", &["app.py"]);
    allow(&mut g, "cli", false).unwrap();
    g.reg.keep_followed(ID, LATER).unwrap();
    std::fs::write(&g.exe, b"build 2").unwrap();
    assert_eq!(g.reg.binding_void(ID), Some(VoidReason::Changed));
}

#[test]
fn the_keyboard_ok_is_not_remembered_for_a_one_run_app() {
    let mut f = fx("keys", "python.exe", &["app.py"]);
    allow(&mut f, "dialog", false).unwrap();
    let (path, sha) = f.pinned();
    let err = f.reg.allow_keyboard(ID, &path, &sha, NOW).unwrap_err();
    assert!(err.to_string().contains("one run only"), "{err}");
    assert!(
        err.to_string()
            .contains("use a version that has its own program file"),
        "{err}"
    );
    assert!(f.module().keyboard.is_none());
}

/// The list code as it was computed before the arguments were part of it.
fn code_without_arguments(f: &Fx) -> String {
    let list = f.reg.permission_list(ID).unwrap();
    let mut items: Vec<(&str, _)> = list
        .items
        .iter()
        .map(|i| (i.raw.as_str(), i.state))
        .collect();
    items.sort_by(|a, b| a.0.cmp(b.0));
    let doc = serde_json::json!({
        "format": LIST_FORMAT,
        "id": ID,
        "version": list.version,
        "manifest_sha256": list.manifest_sha256,
        "exe_path": list.exe_path.to_lowercase(),
        "exe_sha256": list.exe_sha256,
        "items": items
            .iter()
            .map(|(r, s)| serde_json::json!({"raw": r, "state": s}))
            .collect::<Vec<_>>(),
    });
    foxglove_plugin_trust::sha256_hex(doc.to_string().as_bytes())
}

#[test]
fn an_app_without_arguments_keeps_its_list_code_and_its_remembered_ok() {
    let mut f = fx("no-args", "foxglove.exe", &[]);
    let list = f.reg.permission_list(ID).unwrap();
    assert_eq!(list.list_hash, code_without_arguments(&f));
    // A record written with the earlier code still matches after the change.
    allow(&mut f, "dialog", false).unwrap();
    let again = Registry::open(&f.home, true).unwrap();
    let b = again.inventory.modules[ID].bound.clone().unwrap();
    assert_eq!(b.list_hash, list.list_hash);
    assert!(b.run_args.is_empty());
    assert_eq!(again.binding_void(ID), None);
    assert!(again.inventory.modules[ID].one_run.is_none());
}

#[test]
fn the_list_code_is_for_the_program_and_every_argument() {
    let mut f = fx("code", "foxglove.exe", &["--profile", "work"]);
    let shown = f.reg.permission_list(ID).unwrap();
    assert_ne!(shown.list_hash, code_without_arguments(&f));
    let all: Vec<String> = shown.items.iter().map(|i| i.raw.clone()).collect();
    for other in [
        &["--profile", "home"][..],
        &["--profile"],
        &["work", "--profile"],
        &["--profile", "work", ""],
        &[],
    ] {
        f.readopt(other);
        assert_ne!(f.reg.permission_list(ID).unwrap().code(), shown.code());
        // An Allow of the list shown for the first command is refused for this one.
        let err = f
            .reg
            .approve_listed(ID, shown.code(), &all, &[], NOW, "cli", false)
            .unwrap_err();
        assert_eq!(err.id(), Some("permission.list_changed"), "{other:?}");
        assert!(f.module().bound.is_none());
    }
    f.readopt(&["--profile", "work"]);
    assert_eq!(f.reg.permission_list(ID).unwrap().code(), shown.code());
    // An ordinary app with arguments is bound to them, as before.
    let o = allow(&mut f, "cli", false).unwrap();
    assert_eq!(o.one_run, None);
    assert_eq!(
        f.module().bound.as_ref().unwrap().run_args,
        ["--profile", "work"]
    );
    f.readopt(&["--profile", "home"]);
    assert_eq!(f.reg.binding_void(ID), Some(VoidReason::Changed));
}
