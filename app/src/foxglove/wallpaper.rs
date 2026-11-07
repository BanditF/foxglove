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
//! - "Lights follow the wallpaper" (#70) gets the accent of the wallpaper on screen from its
//!   manifest's palette on each of those changes (`accent::wallpaper_changed`); a wallpaper
//!   without one gives its picture's accent (#40, `wallpaper_image`), taken only while the
//!   lights follow the wallpaper and decoded again only when the picture changed.
//! - The Scenes page's "Wallpaper" palette (#40, `scene_looks::WALLPAPER`) is set on the same
//!   pass: the manifest's palette of the wallpaper on screen, else its picture's colours, read
//!   only once someone used the palette (its first use asks for one pass, `recheck_palette`).

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

/// Whether `text` from `source` can be read whole: Wallpaper Engine's config must parse, since
/// one read in the middle of its write would say no wallpaper is on screen (and wallpaper sync
/// would send everything again on the next read). The end of the write wakes us again.
fn readable(text: &str, source: &Source) -> bool {
    !matches!(source, Source::Engine(_))
        || serde_json::from_str::<Value>(text.trim_start_matches('\u{feff}')).is_ok()
}

/// Wallpaper Engine stops (unloads) wallpapers while another program is full-screen: its
/// "Other application fullscreen: Stop" (`<user>.general.user.playbackfullscreen` is `"stop"`),
/// so it loads them again, from their saved properties, when the program leaves (#586).
pub fn stops_under_fullscreen(config: &str) -> bool {
    let Ok(v) = serde_json::from_str::<Value>(config.trim_start_matches('\u{feff}')) else {
        return false;
    };
    v.as_object()
        .into_iter()
        .flat_map(|o| o.values())
        .filter_map(|user| user.pointer("/general/user/playbackfullscreen"))
        .any(|p| p.as_str() == Some("stop"))
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
            let supports = super::wallpaper_manifest::for_file(manifests, &file, false)?.supports;
            supports.any().then(|| super::wallpaper_sync::Target {
                version: super::wallpaper_manifest::folder_of(&file)
                    .and_then(|d| folder_version(&d)),
                location,
                file,
                supports,
            })
        })
        .collect()
}

/// A wallpaper folder's contents (`wallpaper_sync::Version`): the newest write time of the
/// folder and the files right in it, and how many entries it has. Installing it again (the folder
/// removed and copied anew) or updating it in place changes it; `None` when it can't be read.
pub fn folder_version(dir: &std::path::Path) -> Option<super::wallpaper_sync::Version> {
    let mut newest = std::fs::metadata(dir).and_then(|m| m.modified()).ok()?;
    let mut entries = 0u64;
    for e in std::fs::read_dir(dir).ok()?.flatten().take(MAX_ENTRIES) {
        entries += 1;
        if let Ok(t) = e.metadata().and_then(|m| m.modified()) {
            newest = newest.max(t);
        }
    }
    Some((newest, entries))
}

/// At most this many entries of a wallpaper folder are looked at for its version.
const MAX_ENTRIES: usize = 512;

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

/// What a wake can have changed: each watched file's (and wallpaper folder's) write time and
/// size, and whether Wallpaper Engine runs. The watches are on whole folders, so Wallpaper
/// Engine's own log.txt (beside its config, up to a line a second) wakes the service too; a wake
/// that changes none of these is skipped instead of looking for Wallpaper Engine and reading its
/// config again.
#[derive(Clone, Debug, PartialEq)]
struct Stamp {
    files: Vec<Option<(std::time::SystemTime, u64)>>,
    running: bool,
    /// The desktop wallpaper as the registry names it: Windows may rewrite its file before the
    /// registry, so the file alone could match while the name is still to change.
    desktop: Option<String>,
    /// The folders of the wallpapers on screen at the last pass, their contents
    /// ([`folder_version`]): a wallpaper installed again or updated is sent everything again.
    folders: Vec<Option<super::wallpaper_sync::Version>>,
}

fn stamp(paths: &[PathBuf], running: bool, desktop: Option<String>, folders: &[PathBuf]) -> Stamp {
    Stamp {
        desktop,
        folders: folders.iter().map(|d| folder_version(d)).collect(),
        files: paths
            .iter()
            .map(|p| {
                let m = std::fs::metadata(p).ok()?;
                Some((m.modified().ok()?, m.len()))
            })
            .collect(),
        running,
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
    let by_name = !matches!(source, Source::Engine(_));
    for (_, file) in on_screen(text, source) {
        if let Some(m) = super::wallpaper_manifest::for_file(manifests, &file, by_name) {
            if !ids.contains(&m.id) {
                ids.push(m.id.clone());
            }
        }
    }
    ids
}

/// The accent of the wallpaper on screen, from its manifest's palette ("Lights follow the
/// wallpaper", #70): on the monitor the setting names (Wallpaper Engine's only, as for the art),
/// else the first monitor whose wallpaper has one. None when no wallpaper on screen has a
/// palette, so the lights fall back to the Windows accent.
fn accent_on_screen(
    manifests: &[Manifest],
    text: &str,
    source: &Source,
    monitor: Option<&str>,
) -> Option<[u8; 3]> {
    manifest_on_screen(manifests, text, source, monitor, |m| m.accent.map(|c| c.0))
}

/// The palette of the wallpaper on screen, from its manifest, for the Scenes page's "Wallpaper"
/// palette (#40): chosen as [`accent_on_screen`] chooses.
fn palette_on_screen(
    manifests: &[Manifest],
    text: &str,
    source: &Source,
    monitor: Option<&str>,
) -> Option<Vec<foxglove_scene::Rgb>> {
    manifest_on_screen(manifests, text, source, monitor, |m| {
        m.palette.as_ref().map(|p| p.stops.clone())
    })
}

/// `get` of the first wallpaper on screen whose manifest gives it: on the monitor the setting
/// names (Wallpaper Engine's only), else on any monitor.
fn manifest_on_screen<T>(
    manifests: &[Manifest],
    text: &str,
    source: &Source,
    monitor: Option<&str>,
    get: impl Fn(&Manifest) -> Option<T>,
) -> Option<T> {
    let engine = matches!(source, Source::Engine(_));
    let monitor = monitor.filter(|_| engine);
    on_screen(text, source)
        .iter()
        .filter(|(m, _)| monitor.is_none_or(|want| want == m))
        .find_map(|(_, file)| {
            get(super::wallpaper_manifest::for_file(
                manifests, file, !engine,
            )?)
        })
}

/// What the Scenes page's "Wallpaper" palette gets from a pass: the manifest's palette, else,
/// once the palette is `wanted`, the picture's stops (or none), else nothing read.
fn scene_palette(
    manifest: Option<Vec<foxglove_scene::Rgb>>,
    wanted: bool,
    picture: impl FnOnce() -> Option<Vec<super::wallpaper_image::Swatch>>,
) -> super::scene_looks::OnScreen {
    use super::scene_looks::OnScreen;
    match manifest {
        Some(stops) => OnScreen::Known(Some(stops)),
        None if wanted => {
            OnScreen::Known(picture().and_then(|p| super::wallpaper_image::stops(&p)))
        }
        None => OnScreen::Unread,
    }
}

/// The pictures that stand for the wallpaper on screen (#40), best first, for its accent when it
/// has no manifest palette: on the monitor the setting names (Wallpaper Engine's only), else the
/// first monitor that has one. For the desktop wallpaper, Windows' copy (`desktop`) comes last, for a file
/// the registry names that is gone.
fn pictures_on_screen(
    text: &str,
    source: &Source,
    monitor: Option<&str>,
    desktop: Option<&std::path::Path>,
) -> Vec<PathBuf> {
    let monitor = monitor.filter(|_| matches!(source, Source::Engine(_)));
    let fallback = desktop.filter(|_| matches!(source, Source::Desktop));
    on_screen(text, source)
        .iter()
        .filter(|(m, _)| monitor.is_none_or(|want| want == m))
        .map(|(_, file)| super::wallpaper_image::candidates(file, fallback))
        .find(|c| !c.is_empty())
        .unwrap_or_default()
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

/// The lights started following the wallpaper (#40): the next pass reads the picture on screen,
/// changed or not.
static RECHECK: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The Scenes page's "Wallpaper" palette was first used (#40): the next pass reads the picture on
/// screen, changed or not.
static PALETTE_DUE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

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

/// The lights started following the wallpaper (#40): the wallpaper service takes the accent of
/// the wallpaper on screen (its picture's, which it reads only while they follow it) and wakes the
/// light follower. False without the service: the caller wakes the follower itself.
pub fn recheck() -> bool {
    RECHECK.store(true, std::sync::atomic::Ordering::Relaxed);
    WAKE.lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .is_some_and(|tx| tx.send(()).is_ok())
}

/// The pass found `p` for the "Wallpaper" palette: a running scene with it takes the new
/// colours. True when it changed (the page repaints).
fn set_scene_palette(p: super::scene_looks::OnScreen) -> bool {
    let changed = super::scene_looks::set_wallpaper_palette(p);
    if changed {
        super::scenes_ui::wallpaper_palette_changed();
    }
    changed
}

/// The Scenes page's "Wallpaper" palette was first used (#40): the wallpaper service makes one
/// pass for it. Without the service, nothing: the palette shows as Bandit.
pub fn recheck_palette() {
    PALETTE_DUE.store(true, std::sync::atomic::Ordering::Relaxed);
    if let Some(tx) = WAKE.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        let _ = tx.send(());
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
    // Supervised: a panic (a wallpaper's manifest, a config Wallpaper Engine wrote oddly)
    // restarts the service, which reads everything afresh.
    super::services::spawn_supervised("wallpaper", || {
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
        // Wallpaper Engine's project folders (a wallpaper installed again: its folder removed and
        // copied anew) and the folders of the wallpapers on screen (updated in place), so a
        // reloaded wallpaper is sent its properties again (`wallpaper_sync`).
        let watch_folders = |dirs: &[PathBuf], contents: bool| -> Vec<_> {
            dirs.iter()
                .filter_map(|d| {
                    let tx = tx.clone();
                    super::screens_watch::entries(
                        d,
                        contents,
                        std::sync::Arc::new(move || {
                            let _ = tx.send(());
                        }),
                    )
                })
                .collect()
        };
        let mut root_watches = (Vec::<PathBuf>::new(), Vec::new());
        let mut shown_watches = (
            Vec::<(PathBuf, Option<super::wallpaper_sync::Version>)>::new(),
            Vec::new(),
        );
        let mut shown_folders: Vec<PathBuf> = Vec::new();
        let mut last: Option<String> = None;
        let mut scanner = super::wallpaper_manifest::Scanner::default();
        let mut seen: Option<Stamp> = None;
        // The wallpaper folders the manifests come from: a wallpaper added or removed there
        // changes their write time, so it counts as a change too.
        let mut roots = super::wallpaper_manifest::roots(&engine);
        // The picture on screen and its accent (#40), decoded again only when it changed.
        let mut pictures = super::wallpaper_image::Images::default();
        // The lights just started following the wallpaper (`recheck`): wake the follower once
        // the accent is known, even when it didn't change.
        let mut follower_due = false;
        // Check once at start, then on each change in a watched folder or Refresh.
        for (n, ()) in std::iter::once(()).chain(rx.iter()).enumerate() {
            let now = stamp(
                &[paths.as_slice(), roots.as_slice()].concat(),
                super::wallpaper_engine::running_now(),
                desktop_wallpaper(),
                &shown_folders,
            );
            let refresh = RESCAN.load(std::sync::atomic::Ordering::Relaxed);
            let recheck = RECHECK.swap(false, std::sync::atomic::Ordering::Relaxed);
            if recheck {
                // Whatever this pass finds, the follower runs after it (`recheck`).
                follower_due = true;
            }
            let palette_due = PALETTE_DUE.swap(false, std::sync::atomic::Ordering::Relaxed);
            if n > 0 && !refresh && !recheck && !palette_due && seen.as_ref() == Some(&now) {
                continue;
            }
            seen = Some(now);
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
                // Other files: the next wake compares against them.
                seen = None;
            }
            let source = source(&engine);
            let text = read(&source);
            // The wallpapers that describe themselves (#562): Wallpaper Engine's folders, read
            // again when they changed, and the folders on screen.
            let force = RESCAN.swap(false, std::sync::atomic::Ordering::Relaxed);
            roots = super::wallpaper_manifest::roots(&engine);
            let found = scanner.scan(&roots, force);
            let shown = text
                .as_deref()
                .map_or_else(Vec::new, |t| folders_on_screen(t, &source));
            if root_watches.0 != roots {
                root_watches.1.clear();
                root_watches = (roots.clone(), watch_folders(&roots, false));
            }
            // Watched again when a folder was copied anew (the old watch went with the old one).
            let versions: Vec<_> = shown
                .iter()
                .map(|d| (d.clone(), folder_version(d)))
                .collect();
            if shown_watches.0 != versions {
                shown_watches.1.clear();
                shown_watches = (versions, watch_folders(&shown, true));
            }
            shown_folders.clone_from(&shown);
            let manifests = super::wallpaper_manifest::gather(found, &shown);
            let mut repaint = super::wallpaper_manifest::set(&manifests);
            if repaint {
                // A scene that started before its wallpaper was found takes its palette, name
                // and art now.
                super::scenes_ui::wallpapers_changed();
            }
            let palette_wanted = super::scene_looks::wallpaper_palette_wanted();
            let Some(text) = text else {
                super::accent::wallpaper_changed(None);
                repaint |= set_scene_palette(scene_palette(None, palette_wanted, || None));
                if std::mem::take(&mut follower_due) {
                    super::accent::wake();
                }
                if repaint {
                    super::scene_run::repaint_page();
                }
                continue;
            };
            if !readable(&text, &source) {
                if repaint {
                    super::scene_run::repaint_page();
                }
                continue;
            }
            let monitor = super::art_cycle::Cycle::load().wallpaper_monitor;
            // The picture on screen's palette (#40), decoded only when asked for and changed.
            let mut picture = || {
                let desktop = desktop_marker();
                let found =
                    pictures_on_screen(&text, &source, monitor.as_deref(), desktop.as_deref());
                pictures.palette(&found)
            };
            // Lights follow the wallpaper (#70): the accent on screen now, from a manifest's
            // palette, else (#40) from the picture, read only while the lights follow it.
            let accent =
                accent_on_screen(&manifests, &text, &source, monitor.as_deref()).or_else(|| {
                    super::accent::wallpaper_enabled()
                        .then(|| picture().and_then(|p| super::wallpaper_image::accent(&p)))
                        .flatten()
                        .map(|c| c.0)
                });
            super::accent::wallpaper_changed(accent);
            // The Scenes page's "Wallpaper" palette (#40): the manifest's palette, else the
            // picture's, read only once someone used it (the same decode as the accent's).
            repaint |= set_scene_palette(scene_palette(
                palette_on_screen(&manifests, &text, &source, monitor.as_deref()),
                palette_wanted,
                picture,
            ));
            if std::mem::take(&mut follower_due) {
                super::accent::wake();
            }
            // Wallpaper sync (#493): the monitors showing a wallpaper that reads Foxglove's
            // properties get them; a new wallpaper gets them at once.
            super::wallpaper_sync::wallpapers(
                sync_targets(&manifests, &text, &source),
                stops_under_fullscreen(&text),
            );
            let ids = ids_on_screen(&manifests, &text, &source);
            repaint |= {
                let mut on = ON_SCREEN.lock().unwrap_or_else(|e| e.into_inner());
                std::mem::replace(&mut *on, ids.clone()) != ids
            };
            if repaint {
                super::scene_run::repaint_page();
            }
            let index = index();
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
        drop(root_watches);
        drop(shown_watches);
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

    /// The playback setting behind #586's reloads, as Wallpaper Engine writes it.
    #[test]
    fn the_stop_setting_under_full_screen_programs_is_read() {
        let stop = "\u{feff}{\"me\":{\"general\":{\"user\":{\"playbackfullscreen\":\"stop\",\"playbackmaximized\":\"stop\"}}}}";
        assert!(stops_under_fullscreen(stop));
        let pause = r#"{"me":{"general":{"user":{"playbackfullscreen":"pause"}}}}"#;
        assert!(!stops_under_fullscreen(pause));
        assert!(!stops_under_fullscreen(r#"{"me":{"general":{}}}"#));
        assert!(!stops_under_fullscreen("{half-written"));
    }

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
        // Lights follow the wallpaper (#70): the first monitor's wallpaper with a palette, or
        // the monitor the setting names; a wallpaper without a manifest gives none.
        assert_eq!(
            accent_on_screen(&ms, config, &engine, None),
            Some([0x0f, 0xb5, 0xae])
        );
        assert_eq!(
            accent_on_screen(&ms, config, &engine, Some("//?/DISPLAY#SAM0E3C#2")),
            None,
            "the lighthouse has no palette"
        );
        assert_eq!(
            accent_on_screen(&ms, config, &engine, Some("Monitor0")),
            None,
            "no manifest"
        );
        assert_eq!(
            accent_on_screen(&ms, "tidepool.reef", &o, Some("Monitor0")),
            Some([0x0f, 0xb5, 0xae]),
            "the test hook is matched by id, and the monitor setting doesn't apply"
        );
        assert_eq!(
            accent_on_screen(&ms, "C:/Pictures/beach.jpg", &Source::Desktop, None),
            None
        );
        // The Scenes page's "Wallpaper" palette (#40): the same wallpaper's manifest palette.
        let reef = ms.iter().find(|m| m.id == "tidepool.reef").unwrap();
        let stops = reef.palette.as_ref().unwrap().stops.clone();
        assert_eq!(
            palette_on_screen(&ms, config, &engine, None),
            Some(stops.clone())
        );
        assert_eq!(
            palette_on_screen(&ms, config, &engine, Some("//?/DISPLAY#SAM0E3C#2")),
            None
        );
        assert_eq!(
            palette_on_screen(&ms, "tidepool.reef", &o, None),
            Some(stops)
        );
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

    /// #40: the picture for the wallpaper on screen, on the monitor the setting names (Wallpaper
    /// Engine's) or the first; Windows' copy stands in last for the desktop wallpaper only.
    #[test]
    fn the_picture_on_screen_is_its_file_or_its_preview() {
        let dir = std::env::temp_dir().join(format!("foxglove-wp-pictures-{}", std::process::id()));
        let reef = dir.join("reef");
        std::fs::create_dir_all(&reef).unwrap();
        std::fs::write(reef.join("project.json"), r#"{"preview":"preview.jpg"}"#).unwrap();
        let reef_file = reef.join("index.html").to_string_lossy().replace('\\', "/");
        let config = format!(
            r#"{{"me":{{"general":{{"wallpaperconfig":{{"selectedwallpapers":{{
            "Monitor0":{{"file":"C:/WE/workshop/content/431960/1/scene.pkg"}},
            "Monitor1":{{"file":"{reef_file}"}}}}}}}}}}}}"#
        );
        let engine = Source::Engine(PathBuf::new());
        let copy = PathBuf::from(r"C:\AppData\Themes\TranscodedWallpaper");
        assert_eq!(
            pictures_on_screen(&config, &engine, None, Some(&copy)),
            vec![reef.join("preview.jpg")],
            "the first monitor with a picture; Windows' copy isn't Wallpaper Engine's"
        );
        assert!(pictures_on_screen(&config, &engine, Some("Monitor0"), Some(&copy)).is_empty());
        assert_eq!(
            pictures_on_screen(&config, &engine, Some("Monitor1"), Some(&copy)),
            vec![reef.join("preview.jpg")]
        );
        let desktop = r"C:\Users\me\Pictures\beach.jpg";
        assert_eq!(
            pictures_on_screen(desktop, &Source::Desktop, Some("Monitor1"), Some(&copy)),
            vec![
                PathBuf::from("C:/Users/me/Pictures/beach.jpg"),
                copy.clone()
            ],
            "the monitor setting doesn't apply to the desktop"
        );
        assert!(pictures_on_screen("", &Source::Desktop, None, Some(&copy)).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// #40: the "Wallpaper" palette takes the manifest's palette without reading the picture;
    /// else the picture only once the palette is wanted, and a grey one gives no colours.
    #[test]
    fn the_scene_palette_reads_the_picture_only_when_wanted() {
        use super::super::scene_looks::OnScreen;
        use super::super::wallpaper_image::{palette, Swatch};
        use foxglove_scene::Rgb;
        let reef = vec![Rgb([0x0f, 0xb5, 0xae]), Rgb([0x0b, 0x25, 0x45])];
        let read = std::cell::Cell::new(0);
        let teal = || {
            read.set(read.get() + 1);
            Some(palette(&[Rgb([0x0f, 0xb5, 0xae]); 16]))
        };
        assert_eq!(
            scene_palette(Some(reef.clone()), true, teal),
            OnScreen::Known(Some(reef.clone()))
        );
        assert_eq!(scene_palette(None, false, teal), OnScreen::Unread);
        assert_eq!(read.get(), 0, "no picture read");
        let OnScreen::Known(Some(stops)) = scene_palette(None, true, teal) else {
            panic!("the picture's stops");
        };
        assert_eq!((read.get(), stops.len()), (1, 2));
        let grey = || {
            Some(vec![Swatch {
                colour: Rgb([128, 128, 128]),
                share: 1.0,
            }])
        };
        assert_eq!(scene_palette(None, true, grey), OnScreen::Known(None));
        assert_eq!(scene_palette(None, true, || None), OnScreen::Known(None));
    }

    /// A config read while Wallpaper Engine writes it is skipped, not taken as "no wallpapers"
    /// (which made wallpaper sync send every monitor the same values again).
    #[test]
    fn a_half_written_config_is_skipped() {
        let engine = Source::Engine(PathBuf::new());
        let whole = r#"{"me":{"general":{"wallpaperconfig":{"selectedwallpapers":{}}}}}"#;
        assert!(readable(whole, &engine));
        assert!(readable(&format!("\u{feff}{whole}"), &engine));
        assert!(!readable(&whole[..whole.len() / 2], &engine));
        assert!(!readable("", &engine));
        // The override file and the desktop wallpaper are plain text.
        assert!(readable("fury-stars", &Source::Override(PathBuf::new())));
        assert!(readable(r"C:\Pictures\a.jpg", &Source::Desktop));
    }

    /// Wallpaper Engine's log.txt beside its config wakes the service; only a change to a
    /// watched file, or Wallpaper Engine starting or quitting, is worth a look.
    #[test]
    fn only_a_change_to_what_is_read_counts() {
        let dir = std::env::temp_dir().join(format!("foxglove-wp-stamp-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let config = dir.join("config.json");
        std::fs::write(&config, "{}").unwrap();
        let paths = vec![config.clone(), dir.join("missing")];
        let before = stamp(&paths, true, None, &[]);
        std::fs::write(dir.join("log.txt"), "icon create err: 1008.").unwrap();
        assert_eq!(stamp(&paths, true, None, &[]), before, "its log isn't read");
        assert_ne!(stamp(&paths, false, None, &[]), before, "it quit");
        assert_ne!(
            stamp(&paths, true, Some("C:/a.jpg".into()), &[]),
            before,
            "the desktop wallpaper's name changed"
        );
        std::fs::write(&config, r#"{"me":{}}"#).unwrap();
        assert_ne!(stamp(&paths, true, None, &[]), before, "its config changed");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A wallpaper installed again (its folder removed and copied anew, the files keeping their
    /// times, as tools/build.py --install does) or updated in place has a new version, and the
    /// service's stamp sees it, so wallpaper sync sends it its properties (the theme) again.
    #[test]
    fn a_wallpaper_folder_copied_anew_has_a_new_version() {
        let root = std::env::temp_dir().join(format!("foxglove-wp-version-{}", std::process::id()));
        let src = root.join("dist");
        let dest = root.join("myprojects").join("bandit-live-board");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("index.html"), "<html>").unwrap();
        std::fs::write(src.join("project.json"), "{}").unwrap();
        let install = || {
            let _ = std::fs::remove_dir_all(&dest);
            std::fs::create_dir_all(&dest).unwrap();
            for f in ["index.html", "project.json"] {
                std::fs::copy(src.join(f), dest.join(f)).unwrap();
            }
        };
        install();
        let first = folder_version(&dest).expect("a version");
        assert_eq!(first.1, 2);
        assert_eq!(folder_version(&dest), Some(first), "nothing changed");
        let shown = vec![dest.clone()];
        let before = stamp(&[], true, None, &shown);
        std::thread::sleep(std::time::Duration::from_millis(30));
        install();
        let again = folder_version(&dest).unwrap();
        assert_ne!(again, first, "copied anew");
        assert_ne!(stamp(&[], true, None, &shown), before);
        std::thread::sleep(std::time::Duration::from_millis(30));
        std::fs::write(dest.join("main.js"), "// new").unwrap();
        assert_ne!(folder_version(&dest).unwrap(), again, "updated in place");
        assert_eq!(folder_version(&root.join("gone")), None);
        let _ = std::fs::remove_dir_all(&root);
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
