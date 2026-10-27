//! Wallpapers as data (#562): any Wallpaper Engine wallpaper can describe itself to Foxglove
//! with a `foxglove.json` next to its `project.json` (format `foxglove-wallpaper` v1, spec in
//! docs/wallpaper-manifest.md). Nothing about any wallpaper is built into Foxglove; the Bandit
//! wallpapers ship their manifests like anyone else's.
//!
//! A manifest gives the wallpaper's id, title and pack, the Foxglove properties it reads
//! (`supports`: `clock`, `layer`, `gamemode`, `foxglove_live`), its palette, its motifs (the
//! Scenes page's lighting looks) and the right-screen art paired with it. From them:
//! - the Scenes page offers the motifs under "Wallpapers" ([`super::scene_looks::scenes`]);
//! - "On screen" and wallpaper sync (#493) know which monitor shows which wallpaper, and sync
//!   sends each only the properties it supports;
//! - the live feed's port reaches the wallpapers that support `foxglove_live`.
//!
//! **Where manifests are read:** each project folder in Wallpaper Engine's `projects/myprojects`
//! and `projects/defaultprojects`, each Workshop item in its Steam library's
//! `steamapps/workshop/content/431960`, and the folder of each wallpaper on screen. The wallpaper
//! service reads them at start, on Refresh, and when a watched wallpaper source changes; a
//! folder list is read again only when its folder changed. With no Wallpaper Engine and no
//! manifest, nothing shows and nothing is sent.

use std::path::{Component, Path, PathBuf};
use std::time::SystemTime;

use serde::Deserialize;

use foxglove_scene::Rgb;

use super::scene_looks::{
    self as looks, Art, FileLook, NamedPalette, Preset, Scenes, WallpaperLook, PALETTES,
};

/// The manifest's file name, next to the wallpaper's `project.json`.
pub const FILE: &str = "foxglove.json";
pub const FORMAT: &str = "foxglove-wallpaper";
pub const VERSION: u32 = 1;
/// The most motifs one wallpaper offers.
pub const MAX_MOTIFS: usize = 8;
/// The most folders read under one root (a Workshop library can be large).
const MAX_FOLDERS: usize = 4096;
/// A manifest larger than this isn't read.
const MAX_BYTES: u64 = 64 * 1024;

/// The Foxglove properties a wallpaper reads (wallpaper sync, `wallpaper_sync`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Supports {
    /// `clock`: the running scene's clock (`foxglove_scene`).
    pub clock: bool,
    /// `layer`: the keyboard's layer (`foxglove_layer`).
    pub layer: bool,
    /// `gamemode`: a GAME layer is on (`foxglove_game`).
    pub game: bool,
    /// `foxglove_live`: the live feed's port (`foxglove_live`, docs/live-feed.md).
    pub live: bool,
}

impl Supports {
    #[cfg(test)]
    pub const ALL: Supports = Supports {
        clock: true,
        layer: true,
        game: true,
        live: true,
    };

    /// From the manifest's list; names Foxglove doesn't know yet are skipped.
    pub fn from_names<S: AsRef<str>>(names: &[S]) -> Supports {
        let has = |n: &str| names.iter().any(|s| s.as_ref() == n);
        Supports {
            clock: has("clock"),
            layer: has("layer"),
            game: has("gamemode"),
            live: has("foxglove_live"),
        }
    }

    pub fn any(self) -> bool {
        self != Supports::default()
    }
}

/// One motif: a named lighting look that matches the wallpaper.
#[derive(Clone, Debug, PartialEq)]
pub struct Motif {
    pub name: String,
    /// What the look takes from the wallpaper (the preset's hover).
    pub about: String,
    pub look: looks::Look,
}

/// One wallpaper's manifest, read and checked.
#[derive(Clone, Debug, PartialEq)]
pub struct Manifest {
    pub id: String,
    pub title: String,
    pub pack: Option<String>,
    /// The wallpaper's folder (where its `project.json` and `foxglove.json` are).
    pub dir: PathBuf,
    pub supports: Supports,
    pub palette: Option<NamedPalette>,
    pub motifs: Vec<Motif>,
    pub art: Art,
}

#[derive(Deserialize)]
struct FileManifest {
    format: String,
    version: u32,
    id: String,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    pack: Option<String>,
    #[serde(default)]
    supports: Vec<String>,
    #[serde(default)]
    palette: Option<FilePalette>,
    #[serde(default)]
    motifs: Vec<FileMotif>,
    #[serde(default)]
    art: FileArt,
}

#[derive(Deserialize)]
struct FilePalette {
    name: String,
    stops: Vec<String>,
}

#[derive(Deserialize)]
struct FileMotif {
    name: String,
    #[serde(default)]
    about: String,
    look: FileLook,
}

#[derive(Default, Deserialize)]
struct FileArt {
    builtin: Option<String>,
    file: Option<String>,
    library: Option<String>,
    bundled: Option<String>,
    fallback: Option<String>,
}

/// A name the page shows: trimmed, 1 to 40 characters.
fn shown_name(s: &str, what: &str) -> Result<String, String> {
    let t = s.trim();
    if t.is_empty() || t.chars().count() > looks::NAME_MAX {
        return Err(format!("{what} {s:?}: 1 to {} characters", looks::NAME_MAX));
    }
    Ok(t.to_string())
}

/// A borrowed name cut to the page's 40 characters, with an ellipsis when cut.
fn cut_to_fit(s: &str) -> String {
    if s.chars().count() <= looks::NAME_MAX {
        return s.to_string();
    }
    let mut out: String = s.chars().take(looks::NAME_MAX - 1).collect();
    out.push('…');
    out
}

/// An id: lower-case letters, digits, `.`, `_` and `-`, at most 64.
fn valid_id(id: &str) -> bool {
    let mut chars = id.chars();
    id.len() <= 64
        && chars
            .next()
            .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || ".-_".contains(c))
}

/// A plain file name in the art folder (no folders).
fn plain_file(name: &str) -> bool {
    let p = Path::new(name);
    !name.is_empty()
        && p.components().count() == 1
        && matches!(p.components().next(), Some(Component::Normal(_)))
}

/// A path inside the wallpaper's folder: relative, and never above it.
fn inside(dir: &Path, rel: &str) -> Option<PathBuf> {
    let p = Path::new(rel);
    let ok = !rel.is_empty() && p.components().all(|c| matches!(c, Component::Normal(_)));
    ok.then(|| dir.join(p))
}

/// Reads a manifest. `dir` is the wallpaper's folder; `project_title` is its `project.json`
/// title, used when the manifest has none.
pub fn parse(text: &str, dir: &Path, project_title: Option<&str>) -> Result<Manifest, String> {
    let f: FileManifest =
        serde_json::from_str(text.trim_start_matches('\u{feff}')).map_err(|e| e.to_string())?;
    if f.format != FORMAT || f.version != VERSION {
        return Err(format!("not {FORMAT} v{VERSION}"));
    }
    if !valid_id(&f.id) {
        return Err(format!("id {:?}: lower-case letters, digits, . _ -", f.id));
    }
    // The manifest's own title must be right; one borrowed from project.json (often long, as
    // Workshop titles are) is cut to fit, and a blank one gives way to the id.
    let title = match (f.title.as_deref(), project_title.map(str::trim)) {
        (Some(t), _) => shown_name(t, "title")?,
        (None, Some(t)) if !t.is_empty() => cut_to_fit(t),
        _ => f.id.clone(),
    };
    let pack = f
        .pack
        .as_deref()
        .map(|p| shown_name(p, "pack"))
        .transpose()?;
    let palette = match f.palette {
        Some(p) => {
            if p.stops.len() < 2 {
                return Err(format!("palette {} needs two colours", p.name));
            }
            let stops = p
                .stops
                .iter()
                .map(|s| looks::hex(s).map(Rgb))
                .collect::<Result<_, _>>()?;
            Some(NamedPalette {
                name: shown_name(&p.name, "palette")?,
                stops,
            })
        }
        None => None,
    };
    if f.motifs.len() > MAX_MOTIFS {
        return Err(format!("at most {MAX_MOTIFS} motifs"));
    }
    let mut motifs = Vec::new();
    for m in f.motifs {
        let name = shown_name(&m.name, "motif")?;
        let look = m.look.look().map_err(|e| format!("{name}: {e}"))?;
        let palette_ok = PALETTES.iter().any(|(n, _)| *n == look.palette)
            || palette.as_ref().is_some_and(|p| p.name == look.palette);
        if look.kind.uses_colour() || palette_ok {
            motifs.push(Motif {
                name,
                about: m.about.trim().to_string(),
                look,
            });
        } else {
            return Err(format!(
                "{name}: palette {} isn't Foxglove's or the manifest's",
                look.palette
            ));
        }
    }
    let a = f.art;
    for name in [&a.library, &a.bundled].into_iter().flatten() {
        if !plain_file(name) {
            return Err(format!("art {name:?}: a file name, no folders"));
        }
    }
    let file = match &a.file {
        Some(rel) => Some(
            inside(dir, rel)
                .ok_or_else(|| format!("art.file {rel:?}: inside the wallpaper's folder"))?,
        ),
        None => None,
    };
    Ok(Manifest {
        id: f.id,
        title,
        pack,
        dir: dir.to_path_buf(),
        supports: Supports::from_names(&f.supports),
        palette,
        motifs,
        art: Art {
            builtin: a.builtin,
            file,
            library: a.library,
            bundled: a.bundled,
            fallback: a.fallback,
        },
    })
}

/// The manifest in `dir`, if it has one. A broken one is logged once per read and skipped.
pub fn read(dir: &Path) -> Option<Manifest> {
    let path = dir.join(FILE);
    let meta = std::fs::metadata(&path).ok().filter(|m| m.is_file())?;
    if meta.len() > MAX_BYTES {
        eprintln!("Foxglove: {} is over 64 KiB, skipped", path.display());
        return None;
    }
    let text = std::fs::read_to_string(&path).ok()?;
    let project = dir.join("project.json");
    let small = std::fs::metadata(&project).is_ok_and(|m| m.len() <= 4 * MAX_BYTES);
    let title = small
        .then(|| std::fs::read_to_string(&project).ok())
        .flatten()
        .and_then(|t| {
            serde_json::from_str::<serde_json::Value>(t.trim_start_matches('\u{feff}')).ok()
        })
        .and_then(|v| v.get("title").and_then(|t| t.as_str()).map(str::to_string));
    match parse(&text, dir, title.as_deref()) {
        Ok(m) => Some(m),
        Err(e) => {
            eprintln!("Foxglove: {} skipped: {e}", path.display());
            None
        }
    }
}

/// Where wallpapers live for this Wallpaper Engine: its own projects and its Steam library's
/// Workshop items. None without it.
pub fn roots(engine: &super::wallpaper_engine::Presence) -> Vec<PathBuf> {
    let super::wallpaper_engine::Presence::Installed { dir, .. } = engine else {
        return Vec::new();
    };
    let mut out = vec![
        dir.join("projects").join("myprojects"),
        dir.join("projects").join("defaultprojects"),
    ];
    // <library>\steamapps\common\wallpaper_engine -> <library>\steamapps\workshop\content\431960
    if let Some(steamapps) = dir.parent().and_then(Path::parent) {
        out.push(
            steamapps
                .join("workshop")
                .join("content")
                .join(super::wallpaper_engine::APP_ID),
        );
    }
    out
}

/// Every manifest in the folders right under each root, in root order, then by folder name.
pub fn scan(roots: &[PathBuf]) -> Vec<Manifest> {
    let mut out = Vec::new();
    for root in roots {
        let Ok(entries) = std::fs::read_dir(root) else {
            continue;
        };
        let mut dirs: Vec<PathBuf> = entries
            .filter_map(Result::ok)
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
            .map(|e| e.path())
            .take(MAX_FOLDERS)
            .collect();
        dirs.sort();
        out.extend(dirs.iter().filter_map(|d| read(d)));
    }
    out
}

/// The roots' folder lists, read again only when a root changed (a project added or removed
/// changes its folder's time) or on Refresh.
#[derive(Default)]
pub struct Scanner {
    stamps: Vec<(PathBuf, Option<SystemTime>)>,
    found: Vec<Manifest>,
}

impl Scanner {
    pub fn scan(&mut self, roots: &[PathBuf], force: bool) -> &[Manifest] {
        let stamps: Vec<_> = roots
            .iter()
            .map(|r| {
                (
                    r.clone(),
                    std::fs::metadata(r).and_then(|m| m.modified()).ok(),
                )
            })
            .collect();
        if force || stamps != self.stamps {
            self.found = scan(roots);
            self.stamps = stamps;
        }
        &self.found
    }
}

/// `found` plus the manifests of the folders on screen, each wallpaper once (the first of an id
/// wins: our own projects, then the Workshop).
pub fn gather(found: &[Manifest], on_screen: &[PathBuf]) -> Vec<Manifest> {
    let mut out: Vec<Manifest> = Vec::new();
    let extra: Vec<Manifest> = on_screen
        .iter()
        .filter(|d| !found.iter().any(|m| same_dir(&m.dir, d)))
        .filter_map(|d| read(d))
        .collect();
    for m in found.iter().cloned().chain(extra) {
        if !out.iter().any(|o| o.id == m.id) {
            out.push(m);
        }
    }
    out
}

fn norm(p: &str) -> String {
    p.trim()
        .replace('\\', "/")
        .trim_end_matches('/')
        .to_lowercase()
}

fn same_dir(a: &Path, b: &Path) -> bool {
    norm(&a.to_string_lossy()) == norm(&b.to_string_lossy())
}

/// The folder of a wallpaper file as Wallpaper Engine records it (`.../<project>/index.html`).
pub fn folder_of(file: &str) -> Option<PathBuf> {
    let f = file.trim().replace('\\', "/");
    let parent = Path::new(&f).parent()?;
    (!parent.as_os_str().is_empty()).then(|| parent.to_path_buf())
}

/// The wallpaper a monitor shows: the manifest in the file's folder, else, with `by_name`, one
/// whose id is the whole text or the file's name (the test hook, a desktop image named after a
/// wallpaper; never for Wallpaper Engine's own monitors).
pub fn for_file<'a>(manifests: &'a [Manifest], file: &str, by_name: bool) -> Option<&'a Manifest> {
    let dir = folder_of(file);
    let text = file.trim().to_lowercase();
    let stem = Path::new(&text.replace('\\', "/"))
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_string();
    manifests
        .iter()
        .find(|m| dir.as_deref().is_some_and(|d| same_dir(&m.dir, d)))
        .or_else(|| {
            by_name
                .then(|| manifests.iter().find(|m| m.id == text || m.id == stem))
                .flatten()
        })
}

/// The Scenes page's palettes and looks from the manifests. A palette named like one already
/// taken (Foxglove's, or another wallpaper's with other colours) and a motif named like a
/// preset already offered get the wallpaper's title after their name.
pub fn scenes_of(manifests: &[Manifest]) -> Scenes {
    let mut palettes: Vec<NamedPalette> = Vec::new();
    let mut looks_out: Vec<WallpaperLook> = Vec::new();
    let mut taken: Vec<String> = looks::effect_presets()
        .into_iter()
        .map(|p| p.name.to_lowercase())
        .collect();
    for m in manifests {
        let mut own = m.palette.clone();
        if let Some(p) = own.as_mut() {
            let builtin = PALETTES.iter().any(|(n, _)| *n == p.name);
            let other = palettes.iter().find(|q| q.name == p.name);
            if builtin || other.is_some_and(|q| q.stops != p.stops) {
                p.name = format!("{} ({})", p.name, m.title);
            }
            if !palettes.iter().any(|q| q.name == p.name) {
                palettes.push(p.clone());
            }
        }
        for motif in &m.motifs {
            let mut look = motif.look.clone();
            if let (Some(p), Some(orig)) = (&own, &m.palette) {
                if look.palette == orig.name {
                    look.palette = p.name.clone();
                }
            }
            let mut name = motif.name.clone();
            if taken.contains(&name.to_lowercase()) {
                name = format!("{name} ({})", m.title);
            }
            if taken.contains(&name.to_lowercase()) {
                continue;
            }
            taken.push(name.to_lowercase());
            looks_out.push(WallpaperLook {
                wallpaper: m.id.clone(),
                title: m.title.clone(),
                pack: m.pack.clone(),
                motif: motif.about.clone(),
                preset: Preset { name, look },
                art: m.art.clone(),
            });
        }
    }
    Scenes {
        palettes,
        looks: looks_out,
    }
}

/// The wallpaper service gathered `found`: the Scenes page's looks follow. True when they
/// changed (the page repaints).
pub fn set(found: &[Manifest]) -> bool {
    looks::set_scenes(scenes_of(found))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::foxglove::scene_looks::Kind;

    /// A third-party pack, not ours: two wallpapers from "Tidepool".
    pub(crate) const REEF: &str = r##"{
        "format": "foxglove-wallpaper", "version": 1, "id": "tidepool.reef",
        "pack": "Tidepool", "supports": ["clock", "foxglove_live", "weather"],
        "palette": { "name": "Reef", "stops": ["#0fb5ae", "#f2e8cf", "#0b2545"] },
        "motifs": [ { "name": "Reef drift", "about": "Kelp swaying in teal and sand",
                      "look": { "kind": "drift", "palette": "Reef", "speed": 3, "width_mm": 180 } } ],
        "art": { "file": "keyboard/right.bin", "fallback": "Bandit_Emblem" },
        "added_in_v2": true
    }"##;
    pub(crate) const LIGHTHOUSE: &str = r##"{
        "format": "foxglove-wallpaper", "version": 1, "id": "tidepool.lighthouse",
        "title": "Lighthouse", "pack": "Tidepool",
        "motifs": [ { "name": "Lighthouse beam", "about": "The lamp turning over the bay",
                      "look": { "kind": "scanline", "colour": "#fff1c1", "background": "off",
                                "speed": 2, "direction": "right", "width_mm": 60 } } ]
    }"##;

    pub(crate) fn fake_manifests() -> Vec<Manifest> {
        vec![
            parse(
                REEF,
                Path::new("C:/WE/projects/myprojects/reef"),
                Some("Reef"),
            )
            .unwrap(),
            parse(
                LIGHTHOUSE,
                Path::new("D:/Lib/steamapps/workshop/content/431960/777"),
                None,
            )
            .unwrap(),
        ]
    }

    /// What the page offers in tests (`scene_looks::scenes`).
    pub(crate) fn fake_scenes() -> Scenes {
        scenes_of(&fake_manifests())
    }

    #[test]
    fn a_third_party_manifest_reads() {
        let [reef, light] = &fake_manifests()[..] else {
            panic!()
        };
        assert_eq!(
            (reef.title.as_str(), reef.pack.as_deref()),
            ("Reef", Some("Tidepool"))
        );
        assert_eq!(
            reef.supports,
            Supports {
                clock: true,
                live: true,
                ..Supports::default()
            },
            "names Foxglove doesn't know are skipped"
        );
        assert_eq!(
            reef.art.file.as_deref(),
            Some(Path::new(
                "C:/WE/projects/myprojects/reef/keyboard/right.bin"
            ))
        );
        assert_eq!(reef.motifs[0].look.kind, Kind::Drift);
        assert_eq!(light.title, "Lighthouse");
        // A borrowed project.json title is cut to fit, a blank one gives way to the id.
        let long = "A Very Long Workshop Title That Goes On And On Forever";
        let m = parse(
            LIGHTHOUSE
                .replace("\"title\": \"Lighthouse\",", "")
                .as_str(),
            Path::new("x"),
            Some(long),
        )
        .unwrap();
        assert_eq!(m.title.chars().count(), 40);
        assert!(m.title.ends_with('…'));
        let m = parse(
            LIGHTHOUSE
                .replace("\"title\": \"Lighthouse\",", "")
                .as_str(),
            Path::new("x"),
            Some("  "),
        )
        .unwrap();
        assert_eq!(m.title, "tidepool.lighthouse");
        assert!(
            !light.supports.any(),
            "a wallpaper that reads nothing from Foxglove"
        );
        assert_eq!(light.art, Art::default());
    }

    #[test]
    fn a_bad_manifest_is_refused() {
        let dir = Path::new("x");
        let ok = |t: &str| parse(t, dir, None);
        assert!(ok(REEF).is_ok());
        for (bad, why) in [
            (REEF.replace("\"version\": 1", "\"version\": 2"), "version"),
            (
                REEF.replace("foxglove-wallpaper", "foxglove-scenes"),
                "format",
            ),
            (REEF.replace("tidepool.reef", "Tidepool Reef"), "id"),
            (REEF.replace("#0fb5ae", "#0fb5a"), "hex"),
            (
                REEF.replace("\"palette\": \"Reef\"", "\"palette\": \"Kelp\""),
                "an unknown palette",
            ),
            (REEF.replace("\"speed\": 3", "\"speed\": 0"), "speed"),
            (
                REEF.replace("keyboard/right.bin", "../../right.bin"),
                "art above the folder",
            ),
            (
                REEF.replace("keyboard/right.bin", "C:/right.bin"),
                "art elsewhere",
            ),
            (
                REEF.replace("\"fallback\"", "\"library\": \"a/b.bin\", \"fallback\""),
                "library folders",
            ),
        ] {
            assert!(ok(&bad).is_err(), "{why}");
        }
    }

    #[test]
    fn names_taken_get_the_wallpapers_title() {
        let mut ms = fake_manifests();
        let mut twin = ms[0].clone();
        twin.id = "other.reef".into();
        twin.title = "Other".into();
        twin.palette.as_mut().unwrap().stops.reverse();
        ms.push(twin);
        let mut steel = ms[1].clone();
        steel.id = "steel".into();
        steel.title = "Mill".into();
        steel.motifs[0].name = "Steel sweep".into();
        ms.push(steel);
        let s = scenes_of(&ms);
        let names: Vec<&str> = s.looks.iter().map(|w| w.preset.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "Reef drift",
                "Lighthouse beam",
                "Reef drift (Other)",
                "Steel sweep (Mill)"
            ]
        );
        assert_eq!(s.palettes[1].name, "Reef (Other)");
        assert_eq!(s.looks[2].preset.look.palette, "Reef (Other)");
    }

    /// A Wallpaper Engine install with our projects and a Workshop item: the manifests are
    /// found, a folder without one is skipped, and the folder on screen is matched.
    #[test]
    fn manifests_are_found_in_wallpaper_engine_folders() {
        let base = std::env::temp_dir().join(format!("foxglove-manifests-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let we = base.join("steamapps/common/wallpaper_engine");
        let reef = we.join("projects/myprojects/reef");
        let item = base.join("steamapps/workshop/content/431960/777");
        let plain = we.join("projects/myprojects/someone-elses");
        for d in [&reef, &item, &plain] {
            std::fs::create_dir_all(d).unwrap();
        }
        std::fs::write(reef.join(FILE), REEF).unwrap();
        std::fs::write(reef.join("project.json"), r#"{"title":"Reef"}"#).unwrap();
        std::fs::write(item.join(FILE), LIGHTHOUSE).unwrap();
        std::fs::write(plain.join("project.json"), r#"{"title":"Plain"}"#).unwrap();
        let engine = super::super::wallpaper_engine::Presence::Installed {
            dir: we.clone(),
            running: true,
        };
        let roots = roots(&engine);
        let mut scanner = Scanner::default();
        let found = scanner.scan(&roots, false).to_vec();
        let ids: Vec<&str> = found.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["tidepool.reef", "tidepool.lighthouse"]);
        assert_eq!(found[0].title, "Reef", "from project.json");
        let shown = format!("{}/index.html", reef.display()).replace('\\', "/");
        assert_eq!(
            for_file(&found, &shown, false).map(|m| m.id.as_str()),
            Some("tidepool.reef")
        );
        let other = format!("{}/index.html", plain.display());
        assert_eq!(
            for_file(&found, &other, true),
            None,
            "no manifest, not ours to know"
        );
        assert_eq!(
            for_file(&found, "tidepool.lighthouse", true).map(|m| m.title.as_str()),
            Some("Lighthouse")
        );
        // A wallpaper on screen elsewhere is read from its own folder.
        let away = base.join("elsewhere/reef2");
        std::fs::create_dir_all(&away).unwrap();
        std::fs::write(
            away.join(FILE),
            REEF.replace("tidepool.reef", "tidepool.reef2"),
        )
        .unwrap();
        let all = gather(&found, &[away.clone(), reef.clone()]);
        assert_eq!(all.len(), 3);
        // No Wallpaper Engine: nowhere to look, nothing found.
        assert!(super::roots(&super::super::wallpaper_engine::Presence::Missing).is_empty());
        assert!(scan(&[base.join("nothing")]).is_empty());
        assert!(scenes_of(&[]).looks.is_empty());
        let _ = std::fs::remove_dir_all(&base);
    }
}
