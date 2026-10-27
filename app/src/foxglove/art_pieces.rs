//! The Screens page's art, sorted into its tiers (#516): what the keyboard keeps and comes
//! with, and every piece on this PC once with one badge. Pure: the page draws from these, and
//! the tests check them without a window or a keyboard.
//!
//! Pieces on this PC come from the library folder, the sets bundled with the app (the lighting
//! presets' and the plugins' art) and the source folders of frames (`art_sources`). The same
//! bytes in two places are one piece. A kept place can't be read back from the keyboard (its
//! report has a name and a length), so a place matches a PC piece by both.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::art_library;
use super::art_service::{Choice, JobKind, Retry, Snapshot};
use super::art_words::{showing, Badge, Showing};

/// Sets that ship with the app: the lighting presets' art (#273) and the plugins' (#238, #259).
/// The flag marks plugin art.
pub const BUNDLED: &[(&str, &[u8], bool)] = &[
    (
        "fury.bin",
        include_bytes!("../../resources/lighting/art/fury.bin"),
        false,
    ),
    (
        "constellations.bin",
        include_bytes!("../../resources/lighting/art/constellations.bin"),
        false,
    ),
    (
        "hearth.bin",
        include_bytes!("../../resources/lighting/art/hearth.bin"),
        false,
    ),
    (
        "moon-garden.bin",
        include_bytes!("../../resources/lighting/art/moon-garden.bin"),
        false,
    ),
    (
        "bandit-focus.bin",
        include_bytes!("../../../assets/plugins/org.foxglove.focus-timer/focus.bin"),
        true,
    ),
    (
        "bandit-onair.bin",
        include_bytes!("../../../assets/plugins/org.foxglove.broadcast-status/onair.bin"),
        true,
    ),
];

/// Where a PC piece's bytes are.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Origin {
    Library(PathBuf),
    /// Index into [`BUNDLED`].
    Bundled(usize),
    /// A set folder under a source folder, converted on the scan thread.
    Source(PathBuf),
}

/// One piece on this PC.
#[derive(Clone)]
pub struct PcPiece {
    /// The file (or folder) name, as a kept place would be named after it.
    pub name: String,
    /// Library first when it is there too.
    pub origins: Vec<Origin>,
    /// The first frame as stored (see `art_ui::thumbnail`).
    pub image: Arc<egui::ColorImage>,
    /// A hash of the bytes: the piece's identity and its texture key.
    pub key: u64,
    pub bytes: usize,
    /// One of the wallpaper looks (`wallpapers.json` names it).
    pub wallpaper: bool,
    pub plugin: bool,
}

impl PcPiece {
    /// Its library file, when it has one (a bundled or source piece is copied in first).
    pub fn file(&self) -> Option<&PathBuf> {
        self.origins.iter().find_map(|o| match o {
            Origin::Library(p) => Some(p),
            _ => None,
        })
    }
}

/// A library file, a bundled set or a converted source folder, before merging.
pub struct Found {
    pub origin: Origin,
    pub name: String,
    pub image: Arc<egui::ColorImage>,
    pub bytes: usize,
    pub key: u64,
    pub plugin: bool,
}

/// Every library, bundled and source piece the scan read, plus the names it couldn't read.
pub fn found(library: &art_library::Library) -> (Vec<Found>, Vec<String>) {
    let mut out = Vec::new();
    let mut unreadable = Vec::new();
    for e in &library.entries {
        if !art_library::is_right_art_file(&e.path) {
            continue;
        }
        let name = super::art_service::file_name(&e.path);
        match &e.preview {
            Ok(image) => out.push(Found {
                origin: Origin::Library(e.path.clone()),
                name,
                image: image.clone(),
                bytes: e.bytes,
                key: e.hash,
                plugin: false,
            }),
            Err(_) => unreadable.push(name),
        }
    }
    for (i, image, key) in bundled() {
        let (file, bytes, plugin) = BUNDLED[*i];
        out.push(Found {
            origin: Origin::Bundled(*i),
            name: file.trim_end_matches(".bin").to_string(),
            image: image.clone(),
            bytes: bytes.len(),
            key: *key,
            plugin,
        });
    }
    for src in &library.sources {
        let (Ok(blob), Some(image)) = (&src.blob, &src.preview) else {
            continue;
        };
        out.push(Found {
            origin: Origin::Source(src.folder.clone()),
            name: src
                .folder
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
            image: image.clone(),
            bytes: blob.len(),
            key: art_library::blob_hash(blob),
            plugin: false,
        });
    }
    (out, unreadable)
}

/// The bundled sets' pictures, decoded once.
fn bundled() -> &'static [(usize, Arc<egui::ColorImage>, u64)] {
    static DECODED: std::sync::OnceLock<Vec<(usize, Arc<egui::ColorImage>, u64)>> =
        std::sync::OnceLock::new();
    DECODED.get_or_init(|| {
        BUNDLED
            .iter()
            .enumerate()
            .filter_map(|(i, (_, bytes, _))| {
                let image = super::art_preview::first_frame(bytes).ok()?;
                Some((i, Arc::new(image), art_library::blob_hash(bytes)))
            })
            .collect()
    })
}

/// The library files `wallpapers.json` names (the wallpaper looks), read again only when the
/// file changes, not every frame.
pub fn wallpaper_files() -> Vec<String> {
    static CACHE: std::sync::Mutex<Option<(Option<std::time::SystemTime>, Vec<String>)>> =
        std::sync::Mutex::new(None);
    let modified = art_library::folder()
        .and_then(|f| std::fs::metadata(f.join("wallpapers.json")).ok())
        .and_then(|m| m.modified().ok());
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    match cache.as_ref() {
        Some((at, files)) if *at == modified => files.clone(),
        _ => {
            let files: Vec<String> = super::wallpaper::index()
                .into_iter()
                .map(|w| w.right.library.to_lowercase())
                .collect();
            *cache = Some((modified, files.clone()));
            files
        }
    }
}

/// The PC tier: each piece once (same bytes, or the same name, merge), sorted by name.
pub fn pc_pieces(found: Vec<Found>, is_wallpaper: &dyn Fn(&str) -> bool) -> Vec<PcPiece> {
    let mut out: Vec<PcPiece> = Vec::new();
    for f in found {
        let same = out
            .iter()
            .position(|p| p.key == f.key || p.name.eq_ignore_ascii_case(&f.name));
        match same {
            Some(i) => {
                let p = &mut out[i];
                // A library copy leads, so a click needs no copy first.
                if matches!(f.origin, Origin::Library(_)) {
                    p.origins.insert(0, f.origin);
                } else {
                    p.origins.push(f.origin);
                }
                p.plugin |= f.plugin;
            }
            None => out.push(PcPiece {
                wallpaper: is_wallpaper(&format!("{}.bin", f.name)),
                name: f.name,
                origins: vec![f.origin],
                image: f.image,
                key: f.key,
                bytes: f.bytes,
                plugin: f.plugin,
            }),
        }
    }
    out.sort_by_key(|p| p.name.to_lowercase());
    out
}

/// A kept place that holds a piece.
#[derive(Clone, Debug, PartialEq)]
pub struct Kept {
    pub slot: u8,
    pub name: String,
    /// The PC piece with this name and length, when this PC has it.
    pub piece: Option<usize>,
}

/// The keyboard's kept places, from its own report: `None` for a free one. Empty on firmware
/// without places.
pub fn kept(s: &Snapshot, pcs: &[PcPiece]) -> Vec<Option<Kept>> {
    let Some(st) = s.status else {
        return Vec::new();
    };
    (0..st.flash_slots.min(8))
        .map(|n| {
            let info = s.slots.iter().find(|i| i.slot == n && i.used)?;
            let piece = pcs.iter().position(|p| {
                digitalis::protocol::art_name_fit(&p.name) == info.name
                    && p.bytes == usize::from(info.len)
            });
            Some(Kept {
                slot: n,
                name: info.name.clone(),
                piece,
            })
        })
        .collect()
}

/// "Kept: used of total places", from the keyboard's report (the bits of `flash_used`).
pub fn meter(s: &Snapshot) -> Option<(usize, usize)> {
    let st = s.status.filter(|st| st.flash_slots > 0)?;
    let total = usize::from(st.flash_slots.min(8));
    let used = (0..total).filter(|n| st.flash_used & (1 << n) != 0).count();
    Some((used, total))
}

/// The first free place, `None` when the keyboard is full (or has none).
pub fn first_free(s: &Snapshot) -> Option<u8> {
    let st = s.status.filter(|st| st.flash_slots > 0)?;
    (0..st.flash_slots.min(8)).find(|n| st.flash_used & (1 << n) == 0)
}

/// The most a piece can be: the right screen's buffer, as the keyboard reports it.
pub fn capacity(s: &Snapshot) -> usize {
    s.status
        .map(|st| usize::from(st.right_capacity))
        .filter(|c| *c > 0)
        .unwrap_or(16 * 1024)
}

/// Whether the job running now sends or keeps this piece.
pub fn sending(s: &Snapshot, piece: &PcPiece) -> bool {
    let Some(job) = s.job.as_ref().filter(|_| s.busy) else {
        return false;
    };
    let file = match &job.again {
        Some(Retry::Choose(Choice::File(p))) if job.kind == JobKind::Show => p,
        Some(Retry::Keep { file, .. }) => file,
        _ => return false,
    };
    piece.file() == Some(file) || super::art_service::file_name(file) == piece.name
}

/// A PC piece's one badge.
pub fn badge(s: &Snapshot, piece: &PcPiece, kept: &[Option<Kept>], index: usize) -> Badge {
    if sending(s, piece) {
        let pct = s.push_progress.map_or(0, |(done, total)| {
            if total == 0 {
                0
            } else {
                (u32::from(done) * 100 / u32::from(total)) as u8
            }
        });
        return Badge::Sending(pct);
    }
    if piece.bytes > capacity(s) {
        return Badge::TooBig;
    }
    if kept.iter().flatten().any(|k| k.piece == Some(index)) {
        return Badge::OnKeyboard;
    }
    Badge::ThisPc
}

/// The PC tier's filters.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Filter {
    #[default]
    All,
    NotOnKeyboard,
    Wallpaper,
    Plugins,
}

impl Filter {
    pub const ALL: [(Filter, &'static str); 4] = [
        (Filter::All, "All"),
        (Filter::NotOnKeyboard, "Not on keyboard"),
        (Filter::Wallpaper, "Wallpaper looks"),
        (Filter::Plugins, "From plugins"),
    ];

    pub fn keeps(self, piece: &PcPiece, badge: Badge, search: &str) -> bool {
        let shown = match self {
            Filter::All => true,
            Filter::NotOnKeyboard => badge != Badge::OnKeyboard,
            Filter::Wallpaper => piece.wallpaper,
            Filter::Plugins => piece.plugin,
        };
        shown && (search.is_empty() || piece.name.to_lowercase().contains(&search.to_lowercase()))
    }
}

/// Whether this PC piece is the one on the right screen now: sent from its file, or shown
/// from the kept place it matches.
pub fn on_screen(s: &Snapshot, piece: &PcPiece, kept: &[Option<Kept>], index: usize) -> bool {
    let Some(st) = s.status else {
        return false;
    };
    match showing(&st) {
        Showing::Sent => s.showing_file().is_some_and(|f| piece.file() == Some(f)),
        Showing::Kept(n) => kept
            .iter()
            .flatten()
            .any(|k| k.slot == n && k.piece == Some(index)),
        _ => false,
    }
}

/// The picture of what the right screen shows, with its texture key, when Foxglove has one:
/// a built-in's from the firmware, a kept place's from its PC piece, a sent file's.
pub fn showing_picture(s: &Snapshot, pcs: &[PcPiece]) -> Option<(u64, Arc<egui::ColorImage>)> {
    let st = s.status?;
    match showing(&st) {
        Showing::BuiltIn(n) => {
            super::art_builtins::picture(n, st.right_set_count).map(|p| (p.key, p.image))
        }
        Showing::Kept(n) => {
            let k = kept(s, pcs).into_iter().flatten().find(|k| k.slot == n)?;
            let p = &pcs[k.piece?];
            Some((p.key, p.image.clone()))
        }
        Showing::Sent => {
            let file = s.showing_file()?;
            let p = pcs.iter().find(|p| p.file() == Some(file))?;
            Some((p.key, p.image.clone()))
        }
        Showing::Other(_) => None,
    }
}

/// The name of what the right screen shows now, and its badge.
pub fn showing_name(s: &Snapshot) -> String {
    let Some(st) = s.status else {
        return "…".into();
    };
    match showing(&st) {
        Showing::Kept(n) => super::art_service::name_of(&Choice::Slot(n), s),
        Showing::BuiltIn(n) => super::art_service::name_of(&Choice::BuiltIn(n), s),
        Showing::Sent => s.showing_file().map_or_else(
            || "A piece from this PC".into(),
            |p| super::art_service::file_name(p),
        ),
        Showing::Other(_) => "…".into(),
    }
}

/// The library file for a piece: itself, or a bundled or source piece copied in first (so it
/// can be sent, kept, or used by a layer).
pub fn to_library(piece: &PcPiece, library: &art_library::Library) -> Result<PathBuf, String> {
    if let Some(p) = piece.file() {
        return Ok(p.clone());
    }
    match piece.origins.first() {
        Some(Origin::Bundled(i)) => {
            let (name, bytes, _) = BUNDLED.get(*i).ok_or("no such set")?;
            write_to_library(name, bytes)
        }
        Some(Origin::Source(folder)) => {
            let blob = library
                .sources
                .iter()
                .find(|s| &s.folder == folder)
                .and_then(|s| s.blob.as_ref().ok())
                .ok_or("that folder isn't converted yet")?;
            write_to_library(&format!("{}.bin", piece.name), blob)
        }
        _ => Err("no file".into()),
    }
}

/// Add art… (#520): copies a .bin from anywhere into the library (a new name when one is
/// taken by other art) and returns where it went.
pub fn add_file(from: &Path) -> Result<PathBuf, String> {
    let bytes = std::fs::read(from).map_err(|e| format!("reading {}: {e}", from.display()))?;
    if super::art_preview::first_frame(&bytes).is_err() {
        return Err(format!("{} is not an art blob", from.display()));
    }
    let folder = art_library::folder().ok_or("No settings directory")?;
    if from.parent() == Some(folder.as_path()) {
        return Ok(from.to_path_buf());
    }
    let stem = super::art_service::file_name(from);
    let mut name = format!("{stem}.bin");
    for n in 2.. {
        match std::fs::read(folder.join(&name)) {
            Ok(there) if there != bytes => name = format!("{stem} ({n}).bin"),
            _ => break,
        }
    }
    write_to_library(&name, &bytes)
}

/// A set that ships with Foxglove ([`BUNDLED`], by file name) as a library file, copied in
/// first when it isn't there yet (a scene's wallpaper art, #488).
pub fn bundled_file(name: &str) -> Result<PathBuf, String> {
    let (name, bytes, _) = BUNDLED
        .iter()
        .find(|b| b.0 == name)
        .ok_or_else(|| format!("no bundled set {name}"))?;
    write_to_library(name, bytes)
}

fn write_to_library(name: &str, bytes: &[u8]) -> Result<PathBuf, String> {
    let folder = art_library::folder().ok_or("No settings directory")?;
    let path = folder.join(name);
    if path.exists() {
        return Ok(path);
    }
    std::fs::create_dir_all(&folder).map_err(|e| e.to_string())?;
    std::fs::write(&path, bytes).map_err(|e| format!("couldn't copy {name}: {e}"))?;
    // Only a new file changes the library.
    art_library::refresh();
    Ok(path)
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use digitalis::protocol::{ArtStatus, SlotInfo};

    fn image() -> Arc<egui::ColorImage> {
        Arc::new(egui::ColorImage::new([2, 2], vec![egui::Color32::WHITE; 4]))
    }

    fn found(origin: Origin, name: &str, key: u64, bytes: usize) -> Found {
        Found {
            plugin: matches!(origin, Origin::Bundled(4 | 5)),
            origin,
            name: name.into(),
            image: image(),
            bytes,
            key,
        }
    }

    fn lib(n: &str) -> Origin {
        Origin::Library(PathBuf::from(format!("art/{n}.bin")))
    }

    pub fn status(used: u8) -> ArtStatus {
        ArtStatus {
            link: 2,
            right_state: 0,
            right_set: 3,
            right_set_count: 17,
            right_capacity: 16384,
            flash_slots: 8,
            flash_used: used,
            ..Default::default()
        }
    }

    fn slot(n: u8, name: &str, len: u16) -> SlotInfo {
        SlotInfo {
            slot: n,
            used: true,
            writes: 1,
            len,
            name: name.into(),
        }
    }

    /// Each piece once: the library copy of a bundled set is one piece, led by the file.
    #[test]
    fn every_piece_shows_once() {
        let pcs = pc_pieces(
            vec![
                found(Origin::Bundled(0), "fury", 1, 900),
                found(lib("fury"), "fury", 1, 900),
                found(lib("Radar"), "Radar", 2, 3000),
                found(lib("radar"), "radar", 9, 3000),
                found(Origin::Bundled(4), "bandit-focus", 3, 800),
                found(Origin::Source("src/Orbit".into()), "Orbit", 4, 5000),
            ],
            &|f| f == "Radar.bin",
        );
        let names: Vec<_> = pcs.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["bandit-focus", "fury", "Orbit", "Radar"]);
        assert_eq!(pcs[1].origins, vec![lib("fury"), Origin::Bundled(0)]);
        assert_eq!(pcs[1].file(), Some(&PathBuf::from("art/fury.bin")));
        assert!(pcs[0].plugin && !pcs[1].plugin);
        assert!(pcs[3].wallpaper && !pcs[2].wallpaper);
        assert_eq!(pcs[2].file(), None, "a source folder needs a copy first");
    }

    /// One badge per piece: sending, too big, on the keyboard (a kept place matching it by
    /// name and length), or this PC.
    #[test]
    fn one_badge_per_piece() {
        let pcs = pc_pieces(
            vec![
                found(lib("Radar"), "Radar", 1, 3000),
                found(lib("Orbit"), "Orbit", 2, 4000),
                found(lib("Standby"), "Standby", 3, 20000),
                found(lib("Uplink"), "Uplink", 4, 5000),
            ],
            &|_| false,
        );
        let mut s = Snapshot {
            status: Some(status(0b11)),
            slots: vec![slot(0, "Radar", 3000), slot(1, "Orbit", 999)],
            ..Default::default()
        };
        let k = kept(&s, &pcs);
        assert_eq!(k.len(), 8);
        assert_eq!(
            k[0].as_ref().unwrap().piece,
            Some(1),
            "Radar: same name and length"
        );
        assert_eq!(
            k[1].as_ref().unwrap().piece,
            None,
            "Orbit: another length, another piece"
        );
        assert!(k[2].is_none());
        let badges: Vec<Badge> = (0..pcs.len()).map(|i| badge(&s, &pcs[i], &k, i)).collect();
        // Sorted: Orbit, Radar, Standby, Uplink.
        assert_eq!(
            badges,
            [
                Badge::ThisPc,
                Badge::OnKeyboard,
                Badge::TooBig,
                Badge::ThisPc
            ]
        );
        s.busy = true;
        s.push_progress = Some((3000, 5000));
        s.job = Some(super::super::art_service::Job {
            kind: JobKind::Keep,
            name: "Uplink".into(),
            quiet: false,
            again: Some(Retry::Keep {
                file: "art/Uplink.bin".into(),
                slot: 2,
            }),
        });
        assert_eq!(badge(&s, &pcs[3], &k, 3), Badge::Sending(60));
        assert_eq!(badge(&s, &pcs[0], &k, 0), Badge::ThisPc);
        assert!(!Filter::NotOnKeyboard.keeps(&pcs[1], Badge::OnKeyboard, ""));
        assert!(Filter::All.keeps(&pcs[1], Badge::OnKeyboard, "rad"));
        assert!(!Filter::All.keeps(&pcs[1], Badge::OnKeyboard, "orb"));
    }

    /// The meter and the free place come from the keyboard's report, never a guess.
    #[test]
    fn the_meter_counts_the_keyboards_places() {
        let s = |used| Snapshot {
            status: Some(status(used)),
            ..Default::default()
        };
        assert_eq!(meter(&s(0b1_1111)), Some((5, 8)));
        assert_eq!(first_free(&s(0b1_1111)), Some(5));
        assert_eq!(meter(&s(0xff)), Some((8, 8)));
        assert_eq!(first_free(&s(0xff)), None, "full: the replace choice");
        assert_eq!(first_free(&s(0b1111_1110)), Some(0));
        let old = Snapshot {
            status: Some(ArtStatus {
                flash_slots: 0,
                ..status(0)
            }),
            ..Default::default()
        };
        assert_eq!(meter(&old), None);
        assert_eq!(first_free(&old), None);
        assert_eq!(meter(&Snapshot::default()), None, "no keyboard");
    }

    #[test]
    fn the_piece_on_screen_is_found() {
        let pcs = pc_pieces(
            vec![
                found(lib("Radar"), "Radar", 1, 3000),
                found(lib("Orbit"), "Orbit", 2, 4000),
            ],
            &|_| false,
        );
        let mut s = Snapshot {
            status: Some(ArtStatus {
                right_set: 17,
                ..status(0b1)
            }),
            slots: vec![slot(0, "Radar", 3000)],
            ..Default::default()
        };
        let k = kept(&s, &pcs);
        assert!(on_screen(&s, &pcs[1], &k, 1), "kept place 1 shows Radar");
        assert!(!on_screen(&s, &pcs[0], &k, 0));
        assert_eq!(showing_name(&s), "Radar");
        assert_eq!(showing_picture(&s, &pcs).map(|p| p.0), Some(1));
        s.status = Some(ArtStatus {
            right_state: 2,
            right_set: -1,
            ..status(0b1)
        });
        s.sent = Some(("art/Orbit.bin".into(), 2));
        let k = kept(&s, &pcs);
        assert!(on_screen(&s, &pcs[0], &k, 0), "Orbit sent from this PC");
        assert_eq!(showing_name(&s), "Orbit");
    }
}
