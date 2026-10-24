//! "Match the wallpaper" for the right art (#60, #39): when Wallpaper Engine shows a wallpaper
//! that the art index knows, the right half shows its art.
//!
//! - Wallpaper Engine records each monitor's wallpaper in its `config.json`
//!   (`<user>.general.wallpaperconfig.selectedwallpapers.<monitor>.file`); a folder watch on it
//!   wakes us, so nothing polls.
//! - The index is `art\wallpapers.json` (format `foxglove-wallpaper-art` v1, from
//!   bandit-wallpapers): each wallpaper has path substrings to match and its right art, either
//!   a built-in set by name (selected, no push) or a library blob.
//! - Which monitor to follow is a setting; by default the first one that matches.

use std::path::PathBuf;

use serde::Deserialize;
use serde_json::Value;

use super::art_service::Choice;

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
    let order: Vec<usize> = if left.order.is_empty() { (0..frames.len()).collect() } else { left.order.clone() };
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
        let Some(sel) = user.pointer("/general/wallpaperconfig/selectedwallpapers").and_then(Value::as_object) else {
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

/// The indexed wallpaper on `monitor` (or the first monitor with a match when `None`).
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
            index.iter().find(|w| w.matches.iter().any(|m| file.contains(&m.to_lowercase())))
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

/// Wallpaper Engine's config.json, from Steam's install folder.
pub fn config_path() -> Option<PathBuf> {
    #[cfg(windows)]
    let steam = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER)
        .open_subkey(r"Software\Valve\Steam")
        .and_then(|k| k.get_value::<String, _>("SteamPath"))
        .ok()
        .map(PathBuf::from);
    #[cfg(not(windows))]
    let steam: Option<PathBuf> = None;
    let path = steam?.join("steamapps/common/wallpaper_engine/config.json");
    path.exists().then_some(path)
}

/// The indexed wallpaper on screen now, if any.
pub fn current(monitor: Option<&str>) -> Option<Wallpaper> {
    let text = std::fs::read_to_string(config_path()?).ok()?;
    find(&index(), &selected(&text), monitor).cloned()
}

/// The monitors Wallpaper Engine has wallpapers on, for the setting.
pub fn monitors() -> Vec<String> {
    config_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|t| selected(&t).into_iter().map(|(m, _)| m).collect())
        .unwrap_or_default()
}

/// Watches Wallpaper Engine's config and tells the art service about the wallpaper on screen.
pub fn start() {
    if super::repaint_trace::disabled("wallpaper") {
        return;
    }
    let Some(config) = config_path() else { return };
    super::services::spawn_named("wallpaper", move || {
        let (tx, rx) = std::sync::mpsc::channel::<()>();
        let _watch = super::screens_watch::settings(
            &config,
            std::sync::Arc::new(move || {
                let _ = tx.send(());
            }),
        );
        let mut last: Option<String> = None;
        // Check once at start, then on each change in Wallpaper Engine's folder.
        for () in std::iter::once(()).chain(rx) {
            let Ok(text) = std::fs::read_to_string(&config) else { continue };
            let index = index();
            let monitor = super::art_cycle::Cycle::load().wallpaper_monitor;
            let Some(w) = find(&index, &selected(&text), monitor.as_deref()) else { continue };
            if last.as_deref() == Some(w.id.as_str()) {
                continue;
            }
            last = Some(w.id.clone());
            if let Some(h) = super::art_service::handle() {
                h.wallpaper(w.clone());
            }
            super::screens::wallpaper_changed();
        }
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

    #[test]
    fn left_frames_parse_and_play_in_order() {
        let i: Index = serde_json::from_str(INDEX).unwrap();
        let left = i.wallpapers[0].left.clone().unwrap();
        assert_eq!((left.bin.as_str(), left.frames, left.fps), ("hud.left.bin", 1, 0));
        let left = Left { bin: "x".into(), frames: 3, fps: 4, order: vec![2, 0, 2] };
        let bytes = [0u8, 0, 1, 1, 2, 2];
        assert_eq!(frames_in_order(&bytes, &left, 2), Some(vec![vec![2, 2], vec![0, 0], vec![2, 2]]));
        // Wrong size or an order past the frames: no animation.
        assert_eq!(frames_in_order(&bytes[..5], &left, 2), None);
        assert_eq!(frames_in_order(&bytes, &Left { order: vec![3], ..left.clone() }, 2), None);
        // No order: every frame once.
        assert_eq!(frames_in_order(&bytes, &Left { order: vec![], ..left }, 2).map(|f| f.len()), Some(3));
    }

    const CONFIG: &str = r#"{"fuchs":{"general":{"wallpaperconfig":{"selectedwallpapers":{
        "//?/DISPLAY#DEL40EA#x":{"file":"C:/Program Files (x86)/Steam/steamapps/common/wallpaper_engine/projects/myprojects/bandit-fury-stars/index.html"},
        "Monitor0":{"file":"C:/Program Files (x86)/Steam/steamapps/workshop/content/431960/3000562427/scene.pkg"}}}}}}"#;

    #[test]
    fn finds_the_wallpaper_on_screen() {
        let index = parse_index(INDEX).unwrap();
        let sel = selected(CONFIG);
        assert_eq!(sel.len(), 2);
        assert_eq!(find(&index, &sel, None).map(|w| w.id.as_str()), Some("fury-stars"));
        assert_eq!(find(&index, &sel, Some("Monitor0")), None, "a workshop wallpaper not in the index");
        assert!(parse_index(r#"{"format":"other","version":1,"wallpapers":[]}"#).is_err());
    }
}
