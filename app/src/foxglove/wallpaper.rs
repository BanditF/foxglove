//! "Match the wallpaper" for the right art (#60, #39): when the wallpaper on screen is one the
//! art index knows, the right half shows its art.
//!
//! - Wallpaper Engine is optional (#446, `wallpaper_engine.rs`). While it runs, it records each
//!   monitor's wallpaper in its `config.json`
//!   (`<user>.general.wallpaperconfig.selectedwallpapers.<monitor>.file`). Without it, or while
//!   it isn't running, the Windows desktop wallpaper stands in: its file
//!   (`HKCU\Control Panel\Desktop` › `WallPaper`) matches a wallpaper whose id is the file's
//!   name or whose `match` text is in its path.
//! - Folder watches wake us, so nothing polls: Wallpaper Engine's folder, and
//!   `%APPDATA%\Microsoft\Windows\Themes`, where Windows rewrites `TranscodedWallpaper` on each
//!   desktop wallpaper change.
//! - The index is `art\wallpapers.json` (format `foxglove-wallpaper-art` v1, from
//!   bandit-wallpapers): each wallpaper has path substrings to match and its right art, either
//!   a built-in set by name (selected, no push) or a library blob.
//! - Which monitor to follow is a setting; by default the first one that matches.
//! - Test hook: `FOXGLOVE_WALLPAPER_OVERRIDE=<file>` replaces Wallpaper Engine's config with
//!   that file, which holds one wallpaper id or Wallpaper Engine path. It's watched the same
//!   way, so rewriting it switches the wallpaper; the monitor setting doesn't apply.
//! - Wallpapers that describe themselves (`foxglove.json`, #562, `wallpaper_manifest`) are
//!   gathered on each change too: the Scenes page offers their looks and marks the ones on
//!   screen, and wallpaper sync (#493) reaches the monitors showing one that supports it.

use std::path::PathBuf;

use serde::Deserialize;
use serde_json::Value;

use super::art_service::Choice;
use super::wallpaper_manifest::Manifest;

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Wallpaper {
    pub id: String,
    pub title: String,
    #[serde(rename = "match")]
    pub matches: Vec<String>,
    pub right: Right,
    /// Frames for the left panel, when the wallpaper has them.
    #[serde(default)]
    pub left: Option<Left>,
}

/// The wallpaper's left-panel animation: `frames` 1-bit frames of the panel's size in `bin`
/// (1 = lit), played in `order` at `fps`.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Left {
    pub bin: String,
    pub frames: usize,
    pub fps: u32,
    #[serde(default)]
    pub order: Vec<usize>,
}

/// The frames of `left`, each `frame_bytes` long, in playing order; `None` if the file is
/// missing or the wrong size.
pub fn left_frames(left: &Left, frame_bytes: usize) -> Option<Vec<Vec<u8>>> {
    let bytes = std::fs::read(super::art_library::folder()?.join(&left.bin)).ok()?;
    frames_in_order(&bytes, left, frame_bytes)
}

fn frames_in_order(bytes: &[u8], left: &Left, frame_bytes: usize) -> Option<Vec<Vec<u8>>> {
    if frame_bytes == 0 || bytes.len() != left.frames * frame_bytes {
        return None;
    }
    let frames: Vec<Vec<u8>> = bytes.chunks(frame_bytes).map(<[u8]>::to_vec).collect();
    let order: Vec<usize> = if left.order.is_empty() {
        (0..frames.len()).collect()
    } else {
        left.order.clone()
    };
    order.iter().map(|&i| frames.get(i).cloned()).collect()
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Right {
    /// A built-in set's manifest name, when the art is built in.
    pub builtin: Option<String>,
    /// The blob in the art folder.
    pub library: String,
}

#[derive(Deserialize)]
struct Index {
    format: String,
    version: u32,
    wallpapers: Vec<Wallpaper>,
}

pub fn parse_index(text: &str) -> Result<Vec<Wallpaper>, String> {
    let i: Index = serde_json::from_str(text).map_err(|e| e.to_string())?;
    if i.format != "foxglove-wallpaper-art" || i.version != 1 {
        return Err("unknown wallpaper index format".into());
    }
    Ok(i.wallpapers)
}

fn index_path() -> Option<PathBuf> {
    Some(super::art_library::folder()?.join("wallpapers.json"))
}

pub fn index() -> Vec<Wallpaper> {
    index_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|t| parse_index(&t).ok())
        .unwrap_or_default()
}

/// Each monitor's wallpaper file from Wallpaper Engine's config, sorted by monitor key.
pub fn selected(config: &str) -> Vec<(String, String)> {
    let Ok(v) = serde_json::from_str::<Value>(config.trim_start_matches('\u{feff}')) else {
        return vec![];
    };
    let mut out = Vec::new();
    for user in v.as_object().into_iter().flat_map(|o| o.values()) {
        let Some(sel) = user
            .pointer("/general/wallpaperconfig/selectedwallpapers")
            .and_then(Value::as_object)
        else {
            continue;
        };
        for (monitor, w) in sel {
            if let Some(file) = w.get("file").and_then(Value::as_str) {
                out.push((monitor.clone(), file.replace('\\', "/")));
            }
        }
    }
    out.sort();
    out
}

/// The indexed wallpaper on `monitor` (or the first monitor with a match when `None`). A
/// wallpaper matches by id (the whole entry, or the file's name without its extension) or by a
/// `match` text in the path.
pub fn find<'a>(
    index: &'a [Wallpaper],
    selected: &[(String, String)],
    monitor: Option<&str>,
) -> Option<&'a Wallpaper> {
    selected
        .iter()
        .filter(|(m, _)| monitor.is_none_or(|want| want == m))
        .find_map(|(_, file)| {
            let file = file.to_lowercase();
            let stem = std::path::Path::new(file.trim())
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or_default();
            index.iter().find(|w| {
                w.id.eq_ignore_ascii_case(file.trim())
                    || w.id.eq_ignore_ascii_case(stem)
                    || w.matches.iter().any(|m| file.contains(&m.to_lowercase()))
            })
        })
}

/// What to show for a wallpaper on this keyboard: its built-in set when the firmware has it
/// (by manifest name), else its library blob.
pub fn choice(w: &Wallpaper, set_count: u8) -> Option<Choice> {
    let builtin = w.right.builtin.as_ref().and_then(|name| {
        let sets = super::art_manifest::sets(set_count)?;
        let set = sets.iter().find(|s| &s.name == name)?;
        i8::try_from(set.index).ok().map(Choice::BuiltIn)
    });
    builtin.or_else(|| {
        let file = super::art_library::folder()?.join(&w.right.library);
        file.exists().then_some(Choice::File(file))
    })
}

/// Wallpaper Engine's config.json, when Wallpaper Engine is installed (in any Steam library or
/// a custom folder, #446).
pub fn config_path() -> Option<PathBuf> {
    super::wallpaper_engine::presence().config()
}

/// The test hook's file, when `FOXGLOVE_WALLPAPER_OVERRIDE` is set.
fn override_path() -> Option<PathBuf> {
    std::env::var_os("FOXGLOVE_WALLPAPER_OVERRIDE")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

/// Where the wallpaper on screen is read from.
#[derive(Clone, Debug, PartialEq)]
enum Source {
    /// The test hook's file: one wallpaper id or path.
    Override(PathBuf),
    /// Wallpaper Engine's config.json, while Wallpaper Engine runs.
    Engine(PathBuf),
    /// The Windows desktop wallpaper: without Wallpaper Engine, or while it isn't running.
    Desktop,
}

/// Where to read the wallpaper on screen: the test hook, Wallpaper Engine while it runs, or
/// the Windows desktop wallpaper.
fn source(engine: &super::wallpaper_engine::Presence) -> Source {
    source_with(engine, override_path())
}

fn source_with(engine: &super::wallpaper_engine::Presence, over: Option<PathBuf>) -> Source {
    if let Some(p) = over {
        return Source::Override(p);
    }
    match engine.config() {
        Some(config) if engine.running() => Source::Engine(config),
        _ => Source::Desktop,
    }
}

/// The file Windows rewrites when the desktop wallpaper changes; watching its folder is the
/// change event.
fn desktop_marker() -> Option<PathBuf> {
    let appdata = std::env::var_os("APPDATA").filter(|v| !v.is_empty())?;
    Some(PathBuf::from(appdata).join(r"Microsoft\Windows\Themes\TranscodedWallpaper"))
}

/// The desktop wallpaper's file, as Windows records it.
#[cfg(windows)]
fn desktop_wallpaper() -> Option<String> {
    winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER)
        .open_subkey(r"Control Panel\Desktop")
        .and_then(|k| k.get_value::<String, _>("WallPaper"))
        .ok()
        .filter(|s| !s.trim().is_empty())
}

#[cfg(not(windows))]
fn desktop_wallpaper() -> Option<String> {
    None
}

/// The text the source holds: a file's contents, or the desktop wallpaper's path.
fn read(source: &Source) -> Option<String> {
    match source {
        Source::Override(p) | Source::Engine(p) => std::fs::read_to_string(p).ok(),
        Source::Desktop => desktop_wallpaper(),
    }
}

/// Each monitor's wallpaper from `text`. The override file and the desktop wallpaper are one
/// entry with no monitor.
fn on_screen(text: &str, source: &Source) -> Vec<(String, String)> {
    let one = |name: &str| {
        let w = text.trim_start_matches('\u{feff}').trim();
        if w.is_empty() {
            vec![]
        } else {
            vec![(name.to_string(), w.replace('\\', "/"))]
        }
    };
    match source {
        Source::Override(_) => one("override"),
        Source::Desktop => one("desktop"),
        Source::Engine(_) => selected(text),
    }
}

/// The indexed wallpaper in `text` for the monitor setting, which names a Wallpaper Engine
/// monitor and so applies only to its config.
fn find_in(
    index: &[Wallpaper],
    text: &str,
    source: &Source,
    monitor: Option<&str>,
) -> Option<Wallpaper> {
    let monitor = monitor.filter(|_| matches!(source, Source::Engine(_)));
    find(index, &on_screen(text, source), monitor).cloned()
}

/// The monitors whose wallpaper reads Foxglove's properties (its manifest's `supports`):
/// Wallpaper Engine's key, the wallpaper's file and what it supports. None from any other
/// source, since only Wallpaper Engine's command line reaches a wallpaper.
fn sync_targets(
    manifests: &[Manifest],
    text: &str,
    source: &Source,
) -> Vec<super::wallpaper_sync::Target> {
    if !matches!(source, Source::Engine(_)) {
        return Vec::new();
    }
    selected(text)
        .into_iter()
        .filter_map(|(location, file)| {
            let supports = super::wallpaper_manifest::for_file(manifests, &file)?.supports;
            supports.any().then_some(super::wallpaper_sync::Target {
                location,
                file,
                supports,
            })
        })
        .collect()
}

/// The indexed wallpaper on screen now, if any.
pub fn current(monitor: Option<&str>) -> Option<Wallpaper> {
    let source = source(&super::wallpaper_engine::presence());
    let text = read(&source)?;
    find_in(&index(), &text, &source, monitor)
}

/// The monitors Wallpaper Engine has wallpapers on, for the setting; none without it.
pub fn monitors() -> Vec<String> {
    config_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|t| selected(&t).into_iter().map(|(m, _)| m).collect())
        .unwrap_or_default()
}

/// The folders to watch: the test hook's alone, or Wallpaper Engine's (when installed) and the
/// desktop wallpaper's.
fn watched(engine: &super::wallpaper_engine::Presence) -> Vec<PathBuf> {
    watched_with(engine, override_path(), desktop_marker())
}

fn watched_with(
    engine: &super::wallpaper_engine::Presence,
    over: Option<PathBuf>,
    desktop: Option<PathBuf>,
) -> Vec<PathBuf> {
    match over {
        Some(p) => vec![p],
        None => [engine.config(), desktop].into_iter().flatten().collect(),
    }
}

/// The ids of the wallpapers with a manifest on screen, on any monitor (the Scenes page marks
/// their looks, #488, #562). Set by the wallpaper service on each change.
static ON_SCREEN: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

/// The wallpapers with a manifest on screen now (ids), as the wallpaper service last saw them.
pub fn on_screen_ids() -> Vec<String> {
    ON_SCREEN.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Every wallpaper with a manifest in `text`, each once, in monitor order.
fn ids_on_screen(manifests: &[Manifest], text: &str, source: &Source) -> Vec<String> {
    let mut ids: Vec<String> = Vec::new();
    for (_, file) in on_screen(text, source) {
        if let Some(m) = super::wallpaper_manifest::for_file(manifests, &file) {
            if !ids.contains(&m.id) {
                ids.push(m.id.clone());
            }
        }
    }
    ids
}

/// The folders of the wallpapers on screen, where a manifest may be.
fn folders_on_screen(text: &str, source: &Source) -> Vec<PathBuf> {
    on_screen(text, source)
        .iter()
        .filter_map(|(_, file)| super::wallpaper_manifest::folder_of(file))
        .collect()
}

/// Refresh asks for Wallpaper Engine's folders to be read again, changed or not.
static RESCAN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Wakes the wallpaper service: a watched folder changed, or the user pressed Refresh.
static WAKE: std::sync::Mutex<Option<std::sync::mpsc::Sender<()>>> = std::sync::Mutex::new(None);

/// Refresh (#446): look for Wallpaper Engine again, on the wallpaper service's thread, which
/// also watches its folder from now on if it was installed or moved. Without the service,
/// just look again.
pub fn refresh() {
    RESCAN.store(true, std::sync::atomic::Ordering::Relaxed);
    let sent = WAKE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .is_some_and(|tx| tx.send(()).is_ok());
    if !sent {
        super::wallpaper_engine::refresh();
    }
}

/// Watches the wallpaper sources and tells the art service about the wallpaper on screen. Each
/// change (or Refresh) looks for Wallpaper Engine again: whether it's running now, and where
/// it is, so its folder is watched once it's installed or moved. Without Wallpaper Engine this
/// watches only the desktop wallpaper, quietly.
pub fn start() {
    if super::repaint_trace::disabled("wallpaper") {
        return;
    }
    super::services::spawn_named("wallpaper", move || {
        let (tx, rx) = std::sync::mpsc::channel::<()>();
        *WAKE.lock().unwrap_or_else(|e| e.into_inner()) = Some(tx.clone());
        let watch = |paths: &[PathBuf]| -> Vec<_> {
            paths
                .iter()
                .filter_map(|p| {
                    let tx = tx.clone();
                    super::screens_watch::settings(
                        p,
                        std::sync::Arc::new(move || {
                            let _ = tx.send(());
                        }),
                    )
                })
                .collect()
        };
        let engine = super::wallpaper_engine::presence();
        let mut paths = watched(&engine);
        let mut watches = watch(&paths);
        let mut last: Option<String> = None;
        let mut scanner = super::wallpaper_manifest::Scanner::default();
        // Check once at start, then on each change in a watched folder or Refresh.
        for (n, ()) in std::iter::once(()).chain(rx.iter()).enumerate() {
            let engine = if n == 0 {
                engine.clone()
            } else {
                super::wallpaper_engine::refresh()
            };
            let now = watched(&engine);
            if now != paths {
                // Stop the old watches before starting the new ones.
                watches.clear();
                watches = watch(&now);
                paths = now;
            }
            let source = source(&engine);
            let text = read(&source);
            // The wallpapers that describe themselves (#562): Wallpaper Engine's folders, read
            // again when they changed, and the folders on screen.
            let force = RESCAN.swap(false, std::sync::atomic::Ordering::Relaxed);
            let found = scanner.scan(&super::wallpaper_manifest::roots(&engine), force);
            let shown = text
                .as_deref()
                .map_or_else(Vec::new, |t| folders_on_screen(t, &source));
            let manifests = super::wallpaper_manifest::gather(found, &shown);
            let mut repaint = super::wallpaper_manifest::set(&manifests);
            let Some(text) = text else {
                if repaint {
                    super::scene_run::repaint_page();
                }
                continue;
            };
            // Wallpaper sync (#493): the monitors showing a wallpaper that reads Foxglove's
            // properties get them; a new wallpaper gets them at once.
            super::wallpaper_sync::wallpapers(sync_targets(&manifests, &text, &source));
            let ids = ids_on_screen(&manifests, &text, &source);
            repaint |= {
                let mut on = ON_SCREEN.lock().unwrap_or_else(|e| e.into_inner());
                std::mem::replace(&mut *on, ids.clone()) != ids
            };
            if repaint {
                super::scene_run::repaint_page();
            }
            let index = index();
            let monitor = super::art_cycle::Cycle::load().wallpaper_monitor;
            let Some(w) = find_in(&index, &text, &source, monitor.as_deref()) else {
                continue;
            };
            if last.as_deref() == Some(w.id.as_str()) {
                continue;
            }
            last = Some(w.id.clone());
            if let Some(h) = super::art_service::handle() {
                h.wallpaper(w.clone());
            }
            super::screens::wallpaper_changed();
        }
        drop(watches);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    const INDEX: &str = r#"{"format":"foxglove-wallpaper-art","version":1,"wallpapers":[
        {"id":"hud","title":"HUD","workshop_id":null,"match":["projects/myprojects/bandit-hud/"],
         "right":{"builtin":"Bandit_Idle","library":"hud.bin","bytes":10},"left":{"bin":"hud.left.bin","frames":1,"fps":0,"order":[0]}},
        {"id":"fury-stars","title":"Stars","workshop_id":null,"match":["projects/myprojects/bandit-fury-stars/"],
         "right":{"builtin":null,"library":"fury-stars.bin","bytes":10},"left":{"bin":"x","frames":1,"fps":0,"order":[0]}}]}"#;

    /// Only monitors showing a wallpaper whose manifest supports Foxglove's properties get
    /// wallpaper sync, with what it supports; "On screen" knows every wallpaper with a manifest.
    /// The wallpapers are a fake third-party pack (#562).
    #[test]
    fn wallpapers_are_known_by_their_manifests() {
        use super::super::wallpaper_manifest::{tests::fake_manifests, Supports};
        let ms = fake_manifests();
        let config = r#"{"me":{"general":{"wallpaperconfig":{"selectedwallpapers":{
            "//?/DISPLAY#DEL40EA#1":{"file":"C:/WE/projects/myprojects/reef/index.html"},
            "//?/DISPLAY#SAM0E3C#2":{"file":"D:/Lib/steamapps/workshop/content/431960/777/scene.pkg"},
            "Monitor0":{"file":"C:/WE/workshop/content/431960/3000562427/scene.pkg"}}}}}}"#;
        let engine = Source::Engine(PathBuf::new());
        let targets = sync_targets(&ms, config, &engine);
        assert_eq!(
            targets.len(),
            1,
            "the lighthouse reads nothing, the other has no manifest"
        );
        assert_eq!(targets[0].location, "//?/DISPLAY#DEL40EA#1");
        assert_eq!(
            targets[0].supports,
            Supports {
                clock: true,
                live: true,
                ..Supports::default()
            }
        );
        let o = Source::Override(PathBuf::new());
        assert!(sync_targets(&ms, "tidepool.reef", &o).is_empty());
        assert!(sync_targets(&ms, config, &Source::Desktop).is_empty());
        // The Scenes page's "On screen": every wallpaper with a manifest, once each, from any
        // source.
        assert_eq!(
            ids_on_screen(&ms, config, &engine),
            ["tidepool.reef", "tidepool.lighthouse"]
        );
        let twice = config.replace(
            "D:/Lib/steamapps/workshop/content/431960/777/scene.pkg",
            "C:/WE/projects/myprojects/reef/index.html",
        );
        assert_eq!(ids_on_screen(&ms, &twice, &engine), ["tidepool.reef"]);
        assert_eq!(ids_on_screen(&ms, "tidepool.reef", &o), ["tidepool.reef"]);
        assert!(ids_on_screen(&ms, "nothing", &o).is_empty());
        // No manifests: no wallpaper is known, nothing is synced.
        assert!(ids_on_screen(&[], config, &engine).is_empty());
        assert!(sync_targets(&[], config, &engine).is_empty());
        assert_eq!(
            folders_on_screen(config, &engine)[0],
            PathBuf::from("C:/WE/projects/myprojects/reef")
        );
    }

    #[test]
    fn override_file_names_a_wallpaper_by_id_or_path() {
        let index = parse_index(INDEX).unwrap();
        let o = Source::Override(PathBuf::new());
        let id = |text: &str| find_in(&index, text, &o, Some("Monitor9")).map(|w| w.id);
        assert_eq!(id("fury-stars\r\n").as_deref(), Some("fury-stars"));
        assert_eq!(id("\u{feff}HUD").as_deref(), Some("hud"));
        assert_eq!(
            id(r"C:\WE\projects\myprojects\bandit-hud\index.html").as_deref(),
            Some("hud")
        );
        assert_eq!(id("nothing"), None);
        assert_eq!(id(""), None);
    }

    #[test]
    fn left_frames_parse_and_play_in_order() {
        let i: Index = serde_json::from_str(INDEX).unwrap();
        let left = i.wallpapers[0].left.clone().unwrap();
        assert_eq!(
            (left.bin.as_str(), left.frames, left.fps),
            ("hud.left.bin", 1, 0)
        );
        let left = Left {
            bin: "x".into(),
            frames: 3,
            fps: 4,
            order: vec![2, 0, 2],
        };
        let bytes = [0u8, 0, 1, 1, 2, 2];
        assert_eq!(
            frames_in_order(&bytes, &left, 2),
            Some(vec![vec![2, 2], vec![0, 0], vec![2, 2]])
        );
        // Wrong size or an order past the frames: no animation.
        assert_eq!(frames_in_order(&bytes[..5], &left, 2), None);
        assert_eq!(
            frames_in_order(
                &bytes,
                &Left {
                    order: vec![3],
                    ..left.clone()
                },
                2
            ),
            None
        );
        // No order: every frame once.
        assert_eq!(
            frames_in_order(
                &bytes,
                &Left {
                    order: vec![],
                    ..left
                },
                2
            )
            .map(|f| f.len()),
            Some(3)
        );
    }

    const CONFIG: &str = r#"{"fuchs":{"general":{"wallpaperconfig":{"selectedwallpapers":{
        "//?/DISPLAY#DEL40EA#x":{"file":"C:/Program Files (x86)/Steam/steamapps/common/wallpaper_engine/projects/myprojects/bandit-fury-stars/index.html"},
        "Monitor0":{"file":"C:/Program Files (x86)/Steam/steamapps/workshop/content/431960/3000562427/scene.pkg"}}}}}}"#;

    #[test]
    fn finds_the_wallpaper_on_screen() {
        let index = parse_index(INDEX).unwrap();
        let sel = selected(CONFIG);
        assert_eq!(sel.len(), 2);
        assert_eq!(
            find(&index, &sel, None).map(|w| w.id.as_str()),
            Some("fury-stars")
        );
        assert_eq!(
            find(&index, &sel, Some("Monitor0")),
            None,
            "a workshop wallpaper not in the index"
        );
        assert!(parse_index(r#"{"format":"other","version":1,"wallpapers":[]}"#).is_err());
    }

    /// Without Wallpaper Engine, or while it isn't running, the Windows desktop wallpaper
    /// stands in. It matches by the file's name or a `match` text, and the monitor setting (a
    /// Wallpaper Engine monitor) doesn't apply.
    #[test]
    fn the_desktop_wallpaper_stands_in() {
        use super::super::wallpaper_engine::Presence;
        let index = parse_index(INDEX).unwrap();
        let desktop = Some(PathBuf::from(r"C:\AppData\Themes\TranscodedWallpaper"));
        let source = |p: &Presence| source_with(p, None);
        let watched = |p: &Presence| watched_with(p, None, desktop.clone());
        assert_eq!(source(&Presence::Missing), Source::Desktop);
        assert_eq!(watched(&Presence::Missing).len(), 1, "the desktop only");
        let o = PathBuf::from("override.txt");
        assert_eq!(
            source_with(&Presence::Missing, Some(o.clone())),
            Source::Override(o.clone())
        );
        assert_eq!(
            watched_with(&Presence::Missing, Some(o.clone()), desktop.clone()),
            vec![o]
        );
        let we = PathBuf::from(r"D:\SteamLibrary\steamapps\common\wallpaper_engine");
        let stopped = Presence::Installed {
            dir: we.clone(),
            running: false,
        };
        assert_eq!(source(&stopped), Source::Desktop);
        assert_eq!(watched(&stopped).len(), 2, "its folder and the desktop");
        let running = Presence::Installed {
            dir: we.clone(),
            running: true,
        };
        assert_eq!(source(&running), Source::Engine(we.join("config.json")));
        let d = Source::Desktop;
        let id = |text: &str| find_in(&index, text, &d, Some("Monitor0")).map(|w| w.id);
        assert_eq!(
            id(r"C:\Users\me\Pictures\Fury-Stars.png").as_deref(),
            Some("fury-stars")
        );
        assert_eq!(
            id(r"D:\WE\projects\myprojects\bandit-hud\preview.jpg").as_deref(),
            Some("hud")
        );
        assert_eq!(id(r"C:\Windows\Web\Wallpaper\Windows\img0.jpg"), None);
        assert_eq!(id(""), None);
    }
}
