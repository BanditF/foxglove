//! Wallpaper Engine, an optional integration (#446). Foxglove works without it; with it, the
//! desk ties together. The rule every integration follows (docs/plugins-and-devices.md,
//! "Optional integrations"):
//! - find it the system way, once at start and when the user presses Refresh, never on a timer;
//! - absent: its features hide or show one short hint, with no error, no log line and no retry;
//! - where a feature has a Windows fallback, use it (here: the Windows desktop wallpaper).
//!
//! Where Wallpaper Engine is looked for, first match wins:
//! 1. a custom folder: `FOXGLOVE_WALLPAPER_ENGINE_DIR`, or `folder` in the store's
//!    `[wallpaper_engine]` section;
//! 2. Steam's own library, `<SteamPath>\steamapps\common\wallpaper_engine`;
//! 3. every Steam library in `steamapps\libraryfolders.vdf`, the ones listing app 431960 first,
//!    at the `installdir` its `appmanifest_431960.acf` names;
//! 4. Steam's uninstall entry, `Steam App 431960` › `InstallLocation`.
//!
//! **A lab or test run never touches the user's Wallpaper Engine (#1519).** A Foxglove whose
//! settings folder isn't the real one (`FOXGLOVE_CONFIG_DIR`, or any test build) is such a run;
//! that is decided once, at start, from how the process was started ([`reach`], by
//! `lab_run`'s rule). In it nothing
//! above is looked at: Wallpaper Engine is "not installed" and "not running", so no command is
//! built or run, no ticket is handed over and none of its folders is read or watched. The one
//! exception lets a lab case exercise the path: `FOXGLOVE_WALLPAPER_ENGINE_DIR` naming a
//! stand-in folder that is not an installed copy. Then only that folder counts: its program is
//! the only one run, and "running" means a program from that folder, never the user's copy.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// The wallpaper app's name as the section shows it: data, the one place it is spelled.
#[cfg_attr(test, allow(dead_code))]
pub const NAME: &str = "Wallpaper Engine";
/// Wallpaper Engine's Steam app id.
pub const APP_ID: &str = "431960";
/// Its programs, for "is it running".
const PROCESSES: &[&str] = &["wallpaper32.exe", "wallpaper64.exe"];

/// Whether Wallpaper Engine is here, as last found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Presence {
    /// Not found. Its features hide or hint; fallbacks run instead.
    Missing,
    /// Installed in `dir`. `running`: one of its programs ran when last looked.
    Installed { dir: PathBuf, running: bool },
}

impl Presence {
    /// Its `config.json` (each monitor's wallpaper), when installed. The file may not exist
    /// yet on a fresh install; watching its folder catches it.
    pub fn config(&self) -> Option<PathBuf> {
        match self {
            Presence::Installed { dir, .. } => Some(dir.join("config.json")),
            Presence::Missing => None,
        }
    }

    pub fn installed(&self) -> bool {
        matches!(self, Presence::Installed { .. })
    }

    pub fn running(&self) -> bool {
        matches!(self, Presence::Installed { running: true, .. })
    }

    /// The one short line a feature shows instead of an error; `None` when all is well.
    pub fn hint(&self) -> Option<&'static str> {
        match self {
            Presence::Missing => Some("Works best with Wallpaper Engine."),
            Presence::Installed { running: false, .. } => {
                Some("Wallpaper Engine isn't running, so this follows the Windows wallpaper.")
            }
            Presence::Installed { running: true, .. } => None,
        }
    }
}

/// What this run may do with Wallpaper Engine (#1519).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reach {
    /// A normal run: the installed program, found as the module docs say.
    Installed,
    /// A lab or test run given a stand-in folder: only that folder and the programs in it.
    StandIn(PathBuf),
    /// A lab or test run: nothing. Not installed, not running, no command.
    Off,
}

impl Reach {
    /// Whether this run may start the program `exe`. Every spawn asks this last
    /// (`wallpaper_sync::run_cli_with`), so no caller gets around it.
    pub fn allows(&self, exe: &Path) -> bool {
        match self {
            Reach::Installed => true,
            Reach::StandIn(dir) => {
                foxglove_plugin::owner::inside(exe, dir, &foxglove_plugin::owner::NoRecords)
            }
            Reach::Off => false,
        }
    }
}

static REACH: OnceLock<Reach> = OnceLock::new();

/// This run's reach, decided once on first use (at start: the art page and the wallpaper
/// service both ask) and never again. One log line says so when it isn't the normal one.
pub fn reach() -> &'static Reach {
    REACH.get_or_init(|| {
        let lab = super::lab_run::lab_run();
        // Only a lab run needs to know where the installed copies are (to refuse them).
        let installed = if lab {
            candidates(&Places::installed())
        } else {
            Vec::new()
        };
        let reach = decide(lab, stand_in_env(), &installed);
        #[cfg(not(test))]
        match &reach {
            Reach::Installed => {}
            Reach::StandIn(_) => eprintln!(
                "Foxglove: wallpaper sync: a lab or test run; only the stand-in Wallpaper Engine folder is used, the installed one never"
            ),
            Reach::Off => eprintln!(
                "Foxglove: wallpaper sync: off for this run (a lab or test run); Wallpaper Engine is never run"
            ),
        }
        reach
    })
}

fn stand_in_env() -> Option<PathBuf> {
    std::env::var_os("FOXGLOVE_WALLPAPER_ENGINE_DIR")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

/// [`reach`]'s rule. `installed`: every place an installed copy may be; a stand-in that is one
/// of them, lies inside one or holds one is refused.
fn decide(lab: bool, stand_in: Option<PathBuf>, installed: &[PathBuf]) -> Reach {
    if !lab {
        return Reach::Installed;
    }
    match stand_in {
        Some(dir) if !installed.iter().any(|i| overlap(&dir, i)) => Reach::StandIn(dir),
        _ => Reach::Off,
    }
}

/// Whether one folder is the other or lies inside it, on resolved paths where they resolve
/// (a path that doesn't is compared as written), any case.
fn overlap(a: &Path, b: &Path) -> bool {
    let key = |p: &Path| {
        let p = std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
        let s = p.to_string_lossy().replace('/', "\\").to_lowercase();
        let s = s.strip_prefix(r"\\?\").unwrap_or(&s);
        s.trim_end_matches('\\').to_string()
    };
    let (a, b) = (key(a), key(b));
    let within = |x: &str, y: &str| {
        x == y || (x.starts_with(y) && x.as_bytes().get(y.len()) == Some(&b'\\'))
    };
    within(&a, &b) || within(&b, &a)
}

static PRESENCE: Mutex<Option<Presence>> = Mutex::new(None);

/// Wallpaper Engine as last found (found now on first use).
pub fn presence() -> Presence {
    let mut p = PRESENCE.lock().unwrap_or_else(|e| e.into_inner());
    p.get_or_insert_with(look).clone()
}

/// Looks again: at Refresh, and when a watched wallpaper source changes (an event).
pub fn refresh() -> Presence {
    let now = look();
    *PRESENCE.lock().unwrap_or_else(|e| e.into_inner()) = Some(now.clone());
    now
}

/// Whether one of Wallpaper Engine's programs runs right now (a process snapshot, read on
/// demand). Its command line starts Wallpaper Engine when it isn't running, so wallpaper sync
/// (#493) checks this before every push.
pub fn running_now() -> bool {
    running()
}

/// The program that takes Wallpaper Engine's command line (`-control ...`): the 64-bit one when
/// it is there.
pub fn cli_exe(presence: &Presence) -> Option<PathBuf> {
    cli_exe_as(reach(), presence)
}

/// [`cli_exe`] under `reach`: a program this run may not start is never handed out (#1519).
fn cli_exe_as(reach: &Reach, presence: &Presence) -> Option<PathBuf> {
    let Presence::Installed { dir, .. } = presence else {
        return None;
    };
    ["wallpaper64.exe", "wallpaper32.exe"]
        .iter()
        .map(|p| dir.join(p))
        .find(|p| p.is_file() && reach.allows(p))
}

/// The folder of the program Foxglove drives (#1445: the live feed pairs only with programs
/// inside it). The same record every push uses, never a name.
#[cfg_attr(test, allow(dead_code))]
pub fn host_dir() -> Option<PathBuf> {
    cli_exe(&presence())?.parent().map(Path::to_path_buf)
}

fn look() -> Presence {
    look_as(reach(), Places::system, running)
}

/// [`look`] under `reach` (#1519). Off: nothing is looked at. A stand-in: only its folder.
fn look_as(
    reach: &Reach,
    system: impl FnOnce() -> Places,
    running: impl FnOnce() -> bool,
) -> Presence {
    let places = match reach {
        Reach::Installed => system(),
        Reach::StandIn(dir) => Places {
            custom: vec![dir.clone()],
            ..Places::default()
        },
        Reach::Off => return Presence::Missing,
    };
    match find(&places) {
        Some(dir) => Presence::Installed {
            dir,
            running: running(),
        },
        None => Presence::Missing,
    }
}

fn running() -> bool {
    running_as(reach())
}

/// Whether Wallpaper Engine runs, as this run may see it (#1519): the user's copy is not the
/// lab's, so a lab run sees only a program started from its stand-in folder, or none.
#[cfg(target_os = "windows")]
fn running_as(reach: &Reach) -> bool {
    match reach {
        Reach::Installed => super::pc_lights::any_process_running(PROCESSES),
        Reach::StandIn(_) => super::pc_lights::process_ids(PROCESSES)
            .into_iter()
            .filter_map(foxglove_plugin::owner::image_and_birth)
            .any(|(image, _)| reach.allows(&image)),
        Reach::Off => false,
    }
}

#[cfg(not(target_os = "windows"))]
fn running_as(_reach: &Reach) -> bool {
    let _ = PROCESSES;
    false
}

/// Where to look; the system's values, or a test's.
#[derive(Clone, Debug, Default)]
pub struct Places {
    pub custom: Vec<PathBuf>,
    pub steam: Option<PathBuf>,
    pub uninstall: Vec<PathBuf>,
}

impl Places {
    /// Where an installed copy may be, without the custom folders.
    fn installed() -> Self {
        Places {
            custom: Vec::new(),
            steam: steam_path(),
            uninstall: uninstall_locations(),
        }
    }

    fn system() -> Self {
        #[derive(serde::Deserialize, Default)]
        #[serde(default)]
        struct Section {
            folder: Option<PathBuf>,
        }
        let mut custom: Vec<PathBuf> = stand_in_env().into_iter().collect();
        custom.extend(
            super::store::section(STORE_OWNER, "wallpaper_engine")
                .try_load::<Section>()
                .and_then(|s| s.folder),
        );
        Places {
            custom,
            ..Places::installed()
        }
    }
}

#[cfg(target_os = "windows")]
fn steam_path() -> Option<PathBuf> {
    winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER)
        .open_subkey(r"Software\Valve\Steam")
        .and_then(|k| k.get_value::<String, _>("SteamPath"))
        .ok()
        .map(PathBuf::from)
}

#[cfg(not(target_os = "windows"))]
fn steam_path() -> Option<PathBuf> {
    None
}

/// Steam's uninstall entry for the app, in both registry views.
#[cfg(target_os = "windows")]
fn uninstall_locations() -> Vec<PathBuf> {
    use winreg::enums::{HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_32KEY, KEY_WOW64_64KEY};
    let key = format!(r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\Steam App {APP_ID}");
    [KEY_WOW64_64KEY, KEY_WOW64_32KEY]
        .into_iter()
        .filter_map(|view| {
            winreg::RegKey::predef(HKEY_LOCAL_MACHINE)
                .open_subkey_with_flags(&key, KEY_READ | view)
                .and_then(|k| k.get_value::<String, _>("InstallLocation"))
                .ok()
        })
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .collect()
}

#[cfg(not(target_os = "windows"))]
fn uninstall_locations() -> Vec<PathBuf> {
    Vec::new()
}

/// A Wallpaper Engine folder: it has its config or one of its programs.
fn is_install(dir: &Path) -> bool {
    dir.join("config.json").is_file() || PROCESSES.iter().any(|p| dir.join(p).is_file())
}

/// The Wallpaper Engine folder, in the order the module docs give.
pub fn find(places: &Places) -> Option<PathBuf> {
    candidates(places).into_iter().find(|d| is_install(d))
}

/// Every folder Wallpaper Engine may be in, in the order the module docs give.
fn candidates(places: &Places) -> Vec<PathBuf> {
    let mut candidates: Vec<PathBuf> = places.custom.clone();
    if let Some(steam) = &places.steam {
        candidates.push(steam.join("steamapps/common/wallpaper_engine"));
        let vdf = ["steamapps/libraryfolders.vdf", "config/libraryfolders.vdf"]
            .iter()
            .find_map(|f| std::fs::read_to_string(steam.join(f)).ok())
            .unwrap_or_default();
        for lib in libraries(&vdf) {
            let apps = lib.join("steamapps");
            let installdir =
                std::fs::read_to_string(apps.join(format!("appmanifest_{APP_ID}.acf")))
                    .ok()
                    .and_then(|acf| install_dir(&acf))
                    .unwrap_or_else(|| "wallpaper_engine".into());
            candidates.push(apps.join("common").join(installdir));
        }
    }
    candidates.extend(places.uninstall.iter().cloned());
    candidates
}

/// The library folders in a `libraryfolders.vdf`, the ones listing Wallpaper Engine first.
/// Reads today's format (`"0" { "path" "…" "apps" { "431960" "…" } }`) and the old one
/// (`"1" "D:\\SteamLibrary"`).
pub fn libraries(vdf: &str) -> Vec<PathBuf> {
    let Some(Vdf::Obj(root)) = parse_vdf(vdf).into_iter().next().map(|(_, v)| v) else {
        return Vec::new();
    };
    let mut with: Vec<PathBuf> = Vec::new();
    let mut without: Vec<PathBuf> = Vec::new();
    for (key, value) in root {
        if !key.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        match value {
            Vdf::Str(path) => without.push(PathBuf::from(path)),
            Vdf::Obj(fields) => {
                let path = fields.iter().find_map(|(k, v)| match v {
                    Vdf::Str(s) if k.eq_ignore_ascii_case("path") => Some(PathBuf::from(s)),
                    _ => None,
                });
                let has = fields.iter().any(|(k, v)| match v {
                    Vdf::Obj(apps) if k.eq_ignore_ascii_case("apps") => {
                        apps.iter().any(|(id, _)| id == APP_ID)
                    }
                    _ => false,
                });
                if let Some(path) = path {
                    if has { &mut with } else { &mut without }.push(path);
                }
            }
        }
    }
    with.extend(without);
    with
}

/// The `installdir` an `appmanifest_431960.acf` names.
pub fn install_dir(acf: &str) -> Option<String> {
    let Some((_, Vdf::Obj(state))) = parse_vdf(acf).into_iter().next() else {
        return None;
    };
    state.into_iter().find_map(|(k, v)| match v {
        Vdf::Str(s) if k.eq_ignore_ascii_case("installdir") && !s.is_empty() => Some(s),
        _ => None,
    })
}

/// A Valve KeyValues ("VDF") value.
#[derive(Clone, Debug, PartialEq)]
enum Vdf {
    Str(String),
    Obj(Vec<(String, Vdf)>),
}

/// A small KeyValues text parser: quoted or bare tokens, `{ }` blocks, `//` comments and the
/// `\\ \" \n \t` escapes. Malformed input gives what parsed before the fault.
fn parse_vdf(text: &str) -> Vec<(String, Vdf)> {
    let toks = vdf_tokens(text);
    let mut at = 0;
    vdf_block(&toks, &mut at, 0)
}

#[derive(Debug, PartialEq)]
enum VdfTok {
    Open,
    Close,
    Text(String),
}

fn vdf_tokens(text: &str) -> Vec<VdfTok> {
    let mut out = Vec::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '{' => out.push(VdfTok::Open),
            '}' => out.push(VdfTok::Close),
            '/' if chars.peek() == Some(&'/') => {
                for c in chars.by_ref() {
                    if c == '\n' {
                        break;
                    }
                }
            }
            '"' => {
                let mut s = String::new();
                while let Some(c) = chars.next() {
                    match c {
                        '"' => break,
                        '\\' => match chars.next() {
                            Some('n') => s.push('\n'),
                            Some('t') => s.push('\t'),
                            Some(other) => s.push(other),
                            None => break,
                        },
                        other => s.push(other),
                    }
                }
                out.push(VdfTok::Text(s));
            }
            c if c.is_whitespace() => {}
            c => {
                let mut s = c.to_string();
                while let Some(&n) = chars.peek() {
                    if n.is_whitespace() || n == '{' || n == '}' || n == '"' {
                        break;
                    }
                    s.push(n);
                    chars.next();
                }
                out.push(VdfTok::Text(s));
            }
        }
    }
    out
}

fn vdf_block(toks: &[VdfTok], at: &mut usize, depth: usize) -> Vec<(String, Vdf)> {
    let mut out = Vec::new();
    while let Some(tok) = toks.get(*at) {
        *at += 1;
        let key = match tok {
            VdfTok::Close => break,
            VdfTok::Open => continue,
            VdfTok::Text(k) => k.clone(),
        };
        match toks.get(*at) {
            Some(VdfTok::Text(v)) => {
                *at += 1;
                out.push((key, Vdf::Str(v.clone())));
            }
            Some(VdfTok::Open) if depth < 16 => {
                *at += 1;
                out.push((key, Vdf::Obj(vdf_block(toks, at, depth + 1))));
            }
            _ => break,
        }
    }
    out
}

/// The feature that owns this file's store sections.
const STORE_OWNER: &str = "wallpaper-engine";

#[cfg(test)]
mod tests {
    use super::*;

    /// A temp folder for one test, removed at the end.
    struct Temp(PathBuf);
    impl Temp {
        fn new(name: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("foxglove-we-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Temp(dir)
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn vdf_for(libs: &[(&Path, bool)]) -> String {
        let mut s = String::from("\"libraryfolders\"\n{\n");
        for (i, (path, has)) in libs.iter().enumerate() {
            let path = path.display().to_string().replace('\\', "\\\\");
            let apps = if *has {
                "\"228980\"\t\t\"1\"\n\t\t\t\"431960\"\t\t\"123\""
            } else {
                "\"228980\"\t\t\"1\""
            };
            s += &format!(
                "\t\"{i}\"\n\t{{\n\t\t\"path\"\t\t\"{path}\"\n\t\t\"label\"\t\t\"\"\n\t\t\"apps\"\n\t\t{{\n\t\t\t{apps}\n\t\t}}\n\t}}\n"
            );
        }
        s + "}\n"
    }

    #[test]
    fn parses_both_libraryfolders_formats() {
        let vdf = vdf_for(&[
            (Path::new(r"C:\Program Files (x86)\Steam"), false),
            (Path::new(r"D:\SteamLibrary"), true),
        ]);
        assert_eq!(
            libraries(&vdf),
            vec![
                PathBuf::from(r"D:\SteamLibrary"),
                PathBuf::from(r"C:\Program Files (x86)\Steam")
            ],
            "the library listing app 431960 comes first"
        );
        let old = "\"LibraryFolders\"\n{\n\t\"TimeNextStatsReport\"\t\t\"1700000000\"\n\t\"ContentStatsID\"\t\t\"-1\"\n\t\"1\"\t\t\"E:\\\\Games\\\\Steam\"\n}\n";
        assert_eq!(libraries(old), vec![PathBuf::from(r"E:\Games\Steam")]);
        assert!(libraries("").is_empty());
        assert!(libraries("\"libraryfolders\" { \"0\" { \"path\" ").is_empty());
        let acf = "\"AppState\"\n{\n\t\"appid\"\t\t\"431960\"\n\t// comment\n\t\"installdir\"\t\t\"wallpaper_engine\"\n}\n";
        assert_eq!(install_dir(acf).as_deref(), Some("wallpaper_engine"));
        assert_eq!(install_dir("garbage"), None);
    }

    #[test]
    fn wallpaper_engine_not_installed() {
        let t = Temp::new("missing");
        let steam = t.0.join("Steam");
        std::fs::create_dir_all(steam.join("steamapps")).unwrap();
        std::fs::write(
            steam.join("steamapps/libraryfolders.vdf"),
            vdf_for(&[(&steam, false)]),
        )
        .unwrap();
        let places = Places {
            steam: Some(steam),
            ..Places::default()
        };
        assert_eq!(find(&places), None);
        assert_eq!(find(&Places::default()), None, "no Steam at all");
        let missing = Presence::Missing;
        assert_eq!(missing.config(), None);
        assert_eq!(missing.hint(), Some("Works best with Wallpaper Engine."));
    }

    #[test]
    fn wallpaper_engine_in_a_second_library() {
        let t = Temp::new("second");
        let steam = t.0.join("Steam");
        let lib = t.0.join("D").join("SteamLibrary");
        std::fs::create_dir_all(steam.join("steamapps")).unwrap();
        std::fs::write(
            steam.join("steamapps/libraryfolders.vdf"),
            vdf_for(&[(&steam, false), (&lib, true)]),
        )
        .unwrap();
        let we = lib.join("steamapps/common/wallpaper_engine");
        std::fs::create_dir_all(&we).unwrap();
        std::fs::write(we.join("wallpaper64.exe"), b"").unwrap();
        let places = Places {
            steam: Some(steam.clone()),
            ..Places::default()
        };
        assert_eq!(find(&places), Some(we.clone()));
        // A custom folder wins over Steam.
        let custom = t.0.join("Custom WE");
        std::fs::create_dir_all(&custom).unwrap();
        std::fs::write(custom.join("config.json"), b"{}").unwrap();
        let places = Places {
            custom: vec![custom.clone()],
            steam: Some(steam),
            ..Places::default()
        };
        assert_eq!(find(&places), Some(custom));
        // Only the uninstall entry knows it.
        let places = Places {
            uninstall: vec![we.clone()],
            ..Places::default()
        };
        assert_eq!(find(&places), Some(we));
    }

    /// A folder that looks like Wallpaper Engine: its config and its 64-bit program.
    fn stand_in(t: &Temp, name: &str) -> PathBuf {
        let dir = t.0.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("config.json"), b"{}").unwrap();
        std::fs::write(dir.join("wallpaper64.exe"), b"").unwrap();
        dir
    }

    /// #1519: how the process was started decides, nothing a script has to remember.
    #[test]
    fn a_lab_run_reaches_a_stand_in_folder_or_nothing() {
        let t = Temp::new("reach");
        let installed = stand_in(&t, "Steam/steamapps/common/wallpaper_engine");
        let fake = stand_in(&t, "stand-in");
        let known = [installed.clone()];
        // A normal run: unchanged, whatever the variable says.
        assert_eq!(decide(false, None, &known), Reach::Installed);
        assert_eq!(decide(false, Some(fake.clone()), &known), Reach::Installed);
        // A lab run: nothing, or the stand-in.
        assert_eq!(decide(true, None, &known), Reach::Off);
        assert_eq!(
            decide(true, Some(fake.clone()), &known),
            Reach::StandIn(fake.clone())
        );
        // Never the installed copy: itself, in another spelling, a folder in it or one holding it.
        let upper = PathBuf::from(installed.display().to_string().to_uppercase());
        for dir in [
            installed.clone(),
            upper,
            installed.join("bin"),
            t.0.join("Steam"),
            t.0.clone(),
        ] {
            assert_eq!(
                decide(true, Some(dir.clone()), &known),
                Reach::Off,
                "{dir:?}"
            );
        }
        // A sibling whose name starts the same is another folder.
        let sibling = stand_in(&t, "Steam/steamapps/common/wallpaper_engine_fake");
        assert_eq!(
            decide(true, Some(sibling.clone()), &known),
            Reach::StandIn(sibling)
        );
        // Every test build is a lab run.
        assert!(crate::foxglove::lab_run::lab_run());
        assert_ne!(*reach(), Reach::Installed);
    }

    #[test]
    fn a_lab_run_sees_no_installed_or_running_wallpaper_engine() {
        let t = Temp::new("blind");
        let installed = stand_in(&t, "installed");
        let fake = stand_in(&t, "stand-in");
        let system = || Places {
            custom: vec![installed.clone()],
            ..Places::default()
        };
        // Off: nowhere is looked at and nothing is asked whether it runs.
        let nowhere = || -> Places { panic!("a lab run looked for the installed program") };
        let unasked = || -> bool { panic!("a lab run asked whether the user's copy runs") };
        assert_eq!(look_as(&Reach::Off, nowhere, unasked), Presence::Missing);
        assert!(!running_as(&Reach::Off));
        // A stand-in: only its folder, never the system's places.
        let only = Reach::StandIn(fake.clone());
        assert_eq!(
            look_as(&only, nowhere, || false),
            Presence::Installed {
                dir: fake.clone(),
                running: false
            }
        );
        // No program from this test's folder runs, whatever else does on this PC.
        assert!(!running_as(&only));
        // A normal run: as before.
        let found = look_as(&Reach::Installed, system, || true);
        assert_eq!(
            found,
            Presence::Installed {
                dir: installed.clone(),
                running: true
            }
        );
        // The program is handed out only where this run may start it.
        let exe = Some(installed.join("wallpaper64.exe"));
        assert_eq!(cli_exe_as(&Reach::Installed, &found), exe);
        assert_eq!(cli_exe_as(&Reach::Off, &found), None);
        assert_eq!(cli_exe_as(&only, &found), None, "not the stand-in's");
        let stood = look_as(&only, nowhere, || false);
        assert_eq!(
            cli_exe_as(&only, &stood),
            Some(fake.join("wallpaper64.exe"))
        );
        assert!(only.allows(&fake.join("wallpaper64.exe")));
        assert!(!only.allows(&installed.join("wallpaper64.exe")));
        assert!(!Reach::Off.allows(&fake.join("wallpaper64.exe")));
        // This test build itself: nothing installed, nothing running, no program, no folder.
        if *reach() == Reach::Off {
            assert_eq!(presence(), Presence::Missing);
            assert_eq!(refresh(), Presence::Missing);
            assert!(!running_now());
            assert_eq!(host_dir(), None);
        }
    }

    #[test]
    fn wallpaper_engine_installed_but_not_running() {
        let p = Presence::Installed {
            dir: PathBuf::from(r"D:\SteamLibrary\steamapps\common\wallpaper_engine"),
            running: false,
        };
        assert!(p.installed() && !p.running());
        assert_eq!(
            p.config(),
            Some(
                PathBuf::from(r"D:\SteamLibrary\steamapps\common\wallpaper_engine")
                    .join("config.json")
            )
        );
        assert!(p.hint().is_some_and(|h| h.contains("Windows wallpaper")));
        let p = Presence::Installed {
            dir: PathBuf::new(),
            running: true,
        };
        assert_eq!(p.hint(), None);
    }
}
