use super::*;
use std::cell::RefCell;

/// The old Run value in memory.
#[derive(Default)]
struct FakeRun(RefCell<Option<String>>);

impl RunKey for FakeRun {
    fn get(&self) -> std::io::Result<Option<String>> {
        Ok(self.0.borrow().clone())
    }
    fn set(&self, command: &str) -> std::io::Result<()> {
        *self.0.borrow_mut() = Some(command.into());
        Ok(())
    }
    fn remove(&self) -> std::io::Result<()> {
        *self.0.borrow_mut() = None;
        Ok(())
    }
}

/// The shortcut in memory, counting writes.
#[derive(Default)]
struct FakeEntry(RefCell<Option<PathBuf>>, RefCell<u32>);

impl Entry for FakeEntry {
    fn target(&self) -> std::io::Result<Option<PathBuf>> {
        Ok(self.0.borrow().clone())
    }
    fn create(&self, exe: &Path) -> std::io::Result<()> {
        *self.1.borrow_mut() += 1;
        *self.0.borrow_mut() = Some(exe.into());
        Ok(())
    }
    fn remove(&self) -> std::io::Result<()> {
        *self.1.borrow_mut() += 1;
        *self.0.borrow_mut() = None;
        Ok(())
    }
}

/// A StartupApproved value in memory: the bytes Windows would hold.
#[derive(Default)]
struct FakeApproved(RefCell<Option<[u8; 12]>>);

impl Approved for FakeApproved {
    fn get(&self) -> std::io::Result<Option<bool>> {
        Ok(self.0.borrow().map(|b| approved_enabled(&b)))
    }
    fn set(&self, enabled: bool) -> std::io::Result<()> {
        *self.0.borrow_mut() = Some(approved_bytes(enabled, 0x01DC_0000_0000_0000));
        Ok(())
    }
    fn clear(&self) -> std::io::Result<()> {
        *self.0.borrow_mut() = None;
        Ok(())
    }
}

#[derive(Default)]
struct Rig {
    entry: FakeEntry,
    approved: FakeApproved,
    run: FakeRun,
    run_approved: FakeApproved,
}

impl Rig {
    fn places(&self) -> Places<'_> {
        Places {
            entry: &self.entry,
            approved: &self.approved,
            run: &self.run,
            run_approved: &self.run_approved,
        }
    }
    /// Windows' own toggle in Settings › Apps › Startup.
    fn windows_toggle(&self, on: bool) {
        self.approved.set(on).unwrap();
    }
    fn writes(&self) -> u32 {
        *self.entry.1.borrow()
    }
}

const ME_FILE: &str = r"C:\Program Files\Foxglove\foxglove.exe";
const OTHER_FILE: &str = r"C:\Users\fuchs\splitkeeb\app-main\foxglove-9f29911.exe";
const GONE_FILE: &str = r"C:\worktrees\_targets\oncall-673-app\release\foxglove.exe";
const DEV: Option<&str> = Some("a dev build");

fn me() -> PathBuf {
    PathBuf::from(ME_FILE)
}

/// Only these files exist, for `reconcile`.
fn only(files: &'static [&'static str]) -> impl Fn(&Path) -> bool {
    move |p| files.iter().any(|f| Path::new(f) == p)
}

fn fresh() -> AutostartSettings {
    AutostartSettings::default()
}

#[test]
fn on_by_default() {
    assert!(AutostartSettings::default().start_with_windows);
    let parsed: AutostartSettings = toml::from_str("").unwrap();
    assert!(
        parsed.start_with_windows,
        "a store without the section means on"
    );
    assert!(!parsed.made);
    let old: AutostartSettings = toml::from_str("start_with_windows = false").unwrap();
    assert!(
        !old.start_with_windows && !old.made,
        "0.2's store still reads"
    );
}

#[test]
fn the_approved_bytes_are_windows_own() {
    let on = approved_bytes(true, 0x1122_3344_5566_7788);
    assert_eq!(
        on,
        [2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
        "enabled: 02, no time"
    );
    let off = approved_bytes(false, 0x1122_3344_5566_7788);
    assert_eq!(off[..4], [3, 0, 0, 0]);
    assert_eq!(
        off[4..],
        0x1122_3344_5566_7788u64.to_le_bytes(),
        "disabled: 03 and the FILETIME"
    );
    for (first, enabled) in [(2, true), (6, true), (1, false), (3, false), (7, false)] {
        assert_eq!(approved_enabled(&[first, 0, 0, 0]), enabled, "{first:02x}");
    }
    assert!(approved_enabled(&[]), "an empty value is no flag: on");
}

#[test]
fn first_start_makes_the_shortcut_and_turns_it_on() {
    let rig = Rig::default();
    let mut s = fresh();
    let r = reconcile(&mut s, &me(), None, &rig.places(), &only(&[ME_FILE])).unwrap();
    assert_eq!(r, Startup::Written);
    assert_eq!(live(&rig.places()).unwrap(), Live::On(me()));
    assert!(s.start_with_windows && s.made);
}

// App → Windows.

#[test]
fn the_switch_writes_what_windows_toggle_writes() {
    let rig = Rig::default();
    apply(true, &me(), &rig.places()).unwrap();
    assert_eq!(live(&rig.places()).unwrap(), Live::On(me()));
    assert_eq!(rig.approved.0.borrow().unwrap()[0], 2);
    apply(false, &me(), &rig.places()).unwrap();
    assert_eq!(
        live(&rig.places()).unwrap(),
        Live::Disabled(me()),
        "the shortcut stays, turned off"
    );
    assert_eq!(rig.approved.0.borrow().unwrap()[0], 3);
    apply(true, &me(), &rig.places()).unwrap();
    assert_eq!(live(&rig.places()).unwrap(), Live::On(me()));
    assert_eq!(rig.writes(), 1, "one shortcut, made once");
}

#[test]
fn the_switch_claims_the_shortcut_from_another_exe() {
    let rig = Rig::default();
    rig.entry.create(Path::new(OTHER_FILE)).unwrap();
    apply(true, &me(), &rig.places()).unwrap();
    assert_eq!(live(&rig.places()).unwrap(), Live::On(me()));
}

// Windows → app.

#[test]
fn turned_off_in_windows_shows_off_and_is_never_turned_back_on() {
    let rig = Rig::default();
    let mut s = fresh();
    reconcile(&mut s, &me(), None, &rig.places(), &only(&[ME_FILE])).unwrap();
    rig.windows_toggle(false);
    for _ in 0..3 {
        let r = reconcile(&mut s, &me(), None, &rig.places(), &only(&[ME_FILE])).unwrap();
        assert_eq!(r, Startup::DisabledInWindows);
        assert!(!s.start_with_windows, "the switch shows off");
    }
    assert_eq!(
        rig.approved.get().unwrap(),
        Some(false),
        "still off in Windows"
    );
    rig.windows_toggle(true);
    let r = reconcile(&mut s, &me(), None, &rig.places(), &only(&[ME_FILE])).unwrap();
    assert_eq!(r, Startup::Unchanged);
    assert!(
        s.start_with_windows,
        "turned on in Windows: the switch shows on"
    );
}

#[test]
fn a_removed_shortcut_is_not_made_again() {
    let rig = Rig::default();
    let mut s = fresh();
    reconcile(&mut s, &me(), None, &rig.places(), &only(&[ME_FILE])).unwrap();
    rig.entry.remove().unwrap();
    let writes = rig.writes();
    let r = reconcile(&mut s, &me(), None, &rig.places(), &only(&[ME_FILE])).unwrap();
    assert_eq!(r, Startup::RemovedOutside);
    assert!(!s.start_with_windows);
    let r = reconcile(&mut s, &me(), None, &rig.places(), &only(&[ME_FILE])).unwrap();
    assert_eq!((r, rig.writes()), (Startup::Off, writes), "and never after");
}

#[test]
fn off_in_settings_writes_nothing_at_startup() {
    let rig = Rig::default();
    let mut s = AutostartSettings {
        start_with_windows: false,
        made: false,
    };
    let r = reconcile(&mut s, &me(), None, &rig.places(), &only(&[ME_FILE])).unwrap();
    assert_eq!((r, rig.writes()), (Startup::Off, 0));
}

// The repair (#684) and other installs.

#[test]
fn startup_repairs_a_shortcut_whose_file_is_gone_keeping_its_toggle() {
    let rig = Rig::default();
    rig.entry.create(Path::new(GONE_FILE)).unwrap();
    rig.windows_toggle(true);
    let mut s = AutostartSettings {
        start_with_windows: true,
        made: true,
    };
    let r = reconcile(&mut s, &me(), None, &rig.places(), &only(&[ME_FILE])).unwrap();
    assert_eq!(r, Startup::Replaced(GONE_FILE.into()));
    assert_eq!(live(&rig.places()).unwrap(), Live::On(me()));
    // Off in Windows, a stale shortcut is left as it is: no repair turns it back on.
    let rig = Rig::default();
    rig.entry.create(Path::new(GONE_FILE)).unwrap();
    rig.windows_toggle(false);
    let r = reconcile(&mut s, &me(), None, &rig.places(), &only(&[ME_FILE])).unwrap();
    assert_eq!(r, Startup::DisabledInWindows);
    assert_eq!(rig.entry.target().unwrap(), Some(PathBuf::from(GONE_FILE)));
}

#[test]
fn startup_never_takes_over_another_existing_exe() {
    for dev in [None, DEV] {
        let rig = Rig::default();
        rig.entry.create(Path::new(OTHER_FILE)).unwrap();
        let mut s = fresh();
        let r = reconcile(
            &mut s,
            &me(),
            dev,
            &rig.places(),
            &only(&[ME_FILE, OTHER_FILE]),
        )
        .unwrap();
        assert_eq!(r, Startup::KeptOther(OTHER_FILE.into()));
        assert_eq!(rig.writes(), 1, "only the test's own");
        // Another install's old Run value, with no shortcut: that's its entry, no second one.
        let rig = Rig::default();
        let other = command(Path::new(OTHER_FILE));
        rig.run.set(&other).unwrap();
        let r = reconcile(
            &mut s,
            &me(),
            dev,
            &rig.places(),
            &only(&[ME_FILE, OTHER_FILE]),
        )
        .unwrap();
        assert_eq!((r, rig.writes()), (Startup::KeptOther(other), 0));
    }
}

#[test]
fn a_dev_build_never_writes_at_startup() {
    let rig = Rig::default();
    let mut s = fresh();
    let r = reconcile(&mut s, &me(), DEV, &rig.places(), &only(&[ME_FILE])).unwrap();
    assert_eq!((r, rig.writes()), (Startup::Skipped("a dev build"), 0));
    rig.run.set(&command(&me())).unwrap();
    let r = reconcile(&mut s, &me(), DEV, &rig.places(), &only(&[ME_FILE])).unwrap();
    assert_eq!(r, Startup::Skipped("a dev build"));
    assert!(
        rig.run.get().unwrap().is_some(),
        "the Run value isn't moved by a dev build either"
    );
}

// The move from 0.2's Run value.

#[test]
fn startup_moves_the_run_value_to_the_shortcut() {
    let rig = Rig::default();
    rig.run.set(&command(&me())).unwrap();
    let mut s = fresh();
    let r = reconcile(&mut s, &me(), None, &rig.places(), &only(&[ME_FILE])).unwrap();
    assert_eq!(r, Startup::Moved(false));
    assert_eq!(rig.run.get().unwrap(), None);
    assert_eq!(live(&rig.places()).unwrap(), Live::On(me()));
    assert!(s.made && s.start_with_windows);
}

#[test]
fn a_run_value_off_in_windows_moves_and_stays_off() {
    let rig = Rig::default();
    rig.run.set(&command(&me())).unwrap();
    rig.run_approved.set(false).unwrap();
    let mut s = fresh();
    let r = reconcile(&mut s, &me(), None, &rig.places(), &only(&[ME_FILE])).unwrap();
    assert_eq!(r, Startup::Moved(true));
    assert_eq!(live(&rig.places()).unwrap(), Live::Disabled(me()));
    assert_eq!(
        rig.run_approved.get().unwrap(),
        None,
        "the old toggle goes with the value"
    );
    assert!(!s.start_with_windows);
}

#[test]
fn a_run_value_whose_file_is_gone_moves_too() {
    let rig = Rig::default();
    rig.run.set(&command(Path::new(GONE_FILE))).unwrap();
    let mut s = fresh();
    let r = reconcile(&mut s, &me(), None, &rig.places(), &only(&[ME_FILE])).unwrap();
    assert_eq!(r, Startup::Moved(false));
    assert_eq!(live(&rig.places()).unwrap(), Live::On(me()));
}

// Uninstall.

#[test]
fn uninstall_removes_only_this_exes_entries() {
    let rig = Rig::default();
    apply(true, &me(), &rig.places()).unwrap();
    rig.run.set(&command(&me())).unwrap();
    assert!(cleanup(&me(), &rig.places()).unwrap());
    assert_eq!(live(&rig.places()).unwrap(), Live::Missing);
    assert_eq!(
        (rig.run.get().unwrap(), rig.approved.get().unwrap()),
        (None, None)
    );
    assert!(
        !cleanup(&me(), &rig.places()).unwrap(),
        "nothing left to remove"
    );
    rig.entry.create(Path::new(OTHER_FILE)).unwrap();
    assert!(
        !cleanup(&me(), &rig.places()).unwrap(),
        "another install's shortcut stays"
    );
    assert_eq!(rig.entry.target().unwrap(), Some(PathBuf::from(OTHER_FILE)));
}

#[test]
fn dev_paths_are_recognised() {
    let dev = |p: &str| dev_build(Path::new(p), false).is_some();
    assert!(dev(
        r"C:\Users\fuchs\splitkeeb\worktrees\_targets\oncall-673-app\release\foxglove.exe"
    ));
    assert!(dev(
        r"C:\Users\fuchs\splitkeeb\foxglove\app\target\release\foxglove.exe"
    ));
    assert!(dev(
        r"C:\Users\fuchs\splitkeeb\worktrees\autostart\app\Target\release\foxglove.exe"
    ));
    assert!(!dev(ME_FILE));
    assert!(!dev(OTHER_FILE));
    assert!(!dev(
        r"C:\Users\fuchs\AppData\Local\Programs\Foxglove\foxglove.exe"
    ));
    assert!(
        dev_build(Path::new(ME_FILE), true).is_some(),
        "debug builds are dev builds"
    );
}

#[test]
fn the_run_value_target_is_the_quoted_path() {
    let me = me();
    let quoted = command(&me);
    assert_eq!(target(&quoted), me);
    assert_eq!(target(&format!("{quoted} --tray")), me);
    assert_eq!(target(&format!("  {ME_FILE} ")), me);
    assert!(
        same(r#""c:\program files\foxglove\FOXGLOVE.EXE""#, &quoted),
        "case and quotes don't matter"
    );
}

#[test]
fn the_log_line_says_what_and_why() {
    let line = |r: Startup| describe(&Ok(r), &me());
    assert!(line(Startup::Written).contains("made the Startup shortcut"));
    assert!(line(Startup::Replaced(GONE_FILE.into())).contains("no longer exists"));
    assert!(line(Startup::KeptOther(OTHER_FILE.into())).contains("left alone"));
    assert!(line(Startup::Unchanged).contains("nothing to do"));
    assert!(line(Startup::DisabledInWindows).contains("Windows' startup list"));
    assert!(line(Startup::Moved(true)).contains("off in Windows"));
    assert!(line(Startup::Skipped("a dev build")).contains("not written"));
    assert!(line(Startup::Off).starts_with("Foxglove: Start with Windows: off"));
}

/// The real shortcut and registry code, on a temp folder and a test key
/// (`HKCU\Software\FoxgloveTest\autostart-<pid>`): never the user's Startup folder, Run key or
/// StartupApproved values.
#[cfg(windows)]
#[test]
fn the_windows_places_round_trip_on_a_test_root() {
    use super::windows_places::Owned;
    let pid = std::process::id();
    let base = format!(r"Software\FoxgloveTest\autostart-{pid}");
    let dir = std::env::temp_dir().join(format!("foxglove-autostart-test-{pid}"));
    let owned = Owned::at(dir.clone(), &base);
    assert!(
        owned.approved.key.starts_with(r"Software\FoxgloveTest\"),
        "{}",
        owned.approved.key
    );
    assert!(
        owned.run.key.starts_with(r"Software\FoxgloveTest\"),
        "{}",
        owned.run.key
    );
    let exe = std::env::current_exe().unwrap();
    let p = owned.places();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        assert_eq!(live(&p).unwrap(), Live::Missing);
        // App → Windows.
        apply(true, &exe, &p).unwrap();
        assert!(dir.join(SHORTCUT).is_file());
        match live(&p).unwrap() {
            Live::On(t) => assert!(same_path(&t, &exe), "{} vs {}", t.display(), exe.display()),
            other => panic!("{other:?}"),
        }
        apply(false, &exe, &p).unwrap();
        assert!(matches!(live(&p).unwrap(), Live::Disabled(_)));
        // Windows → app: Windows' own disabled bytes (01 too) read as off.
        let (key, _) = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER)
            .create_subkey(&owned.approved.key)
            .unwrap();
        let bytes = vec![1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        key.set_raw_value(
            SHORTCUT,
            &winreg::RegValue {
                bytes: bytes.into(),
                vtype: winreg::enums::REG_BINARY,
            },
        )
        .unwrap();
        let mut s = AutostartSettings {
            start_with_windows: true,
            made: true,
        };
        let r = reconcile(&mut s, &exe, None, &p, &|f| f.is_file()).unwrap();
        assert_eq!(r, Startup::DisabledInWindows);
        assert!(!s.start_with_windows);
        // The move from a Run value, then uninstall.
        owned.entry.remove().unwrap();
        owned.approved.clear().unwrap();
        p.run.set(&command(&exe)).unwrap();
        let mut s = AutostartSettings::default();
        assert_eq!(
            reconcile(&mut s, &exe, None, &p, &|f| f.is_file()).unwrap(),
            Startup::Moved(false)
        );
        assert_eq!(p.run.get().unwrap(), None);
        assert!(cleanup(&exe, &p).unwrap());
        assert_eq!(live(&p).unwrap(), Live::Missing);
    }));
    let _ = std::fs::remove_dir_all(&dir);
    let _ = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER).delete_subkey_all(&base);
    if let Err(e) = result {
        std::panic::resume_unwind(e);
    }
}
