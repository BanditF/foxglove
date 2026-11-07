//! The Screens page's art, sorted into its tiers (#516): what the keyboard keeps and comes
//! with, and every piece on this PC once with one badge. Pure: the page draws from these, and
//! the tests check them without a window or a keyboard.
//!
//! Pieces on this PC come from the library folder, the sets bundled with the app (the lighting
//! presets' and the plugins' art) and the source folders of frames (`art_sources`). The same
//! bytes in two places are one piece. A kept place can't be read back from the keyboard (its
//! report has a name, a length and, from firmware with #863, the CRC of its set, but no frame).
//! A place with a CRC matches the PC piece with those bytes and another place by content; one
//! without (older firmware) matches a piece by name and length, and two places that match the
//! same piece hold the same bytes.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::art_library;
use super::art_service::{Choice, JobKind, Retry, Snapshot};
use super::art_words::{showing, Badge, Showing};

/// Sets that ship with the app: the lighting presets' art (#273), the plugins' (#238, #259) and
/// the Bandit Motion set (movement and distortion styles of the Bandit emblem, made by
/// `tools/art-src/bandit/motion/bandit_motion.py`). The flag marks plugin art.
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
    (
        "bandit-scanline.bin",
        include_bytes!("../../resources/art/bandit-motion/bandit-scanline.bin"),
        false,
    ),
    (
        "bandit-glitch.bin",
        include_bytes!("../../resources/art/bandit-motion/bandit-glitch.bin"),
        false,
    ),
    (
        "bandit-cloak.bin",
        include_bytes!("../../resources/art/bandit-motion/bandit-cloak.bin"),
        false,
    ),
    (
        "bandit-drift.bin",
        include_bytes!("../../resources/art/bandit-motion/bandit-drift.bin"),
        false,
    ),
    (
        "bandit-ghost.bin",
        include_bytes!("../../resources/art/bandit-motion/bandit-ghost.bin"),
        false,
    ),
    (
        "bandit-compass.bin",
        include_bytes!("../../resources/art/bandit-motion/bandit-compass.bin"),
        false,
    ),
    (
        "bandit-signal-v2.bin",
        include_bytes!("../../resources/art/bandit-motion/bandit-signal-v2.bin"),
        false,
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
    /// Every name it goes by: [`PcPiece::name`] first, then those of the same bytes merged into
    /// it (#853: a library copy under another name leads [`PcPiece::origins`], so Keep names
    /// the place after that copy).
    pub names: Vec<String>,
    /// Library first when it is there too.
    pub origins: Vec<Origin>,
    /// The first frame as stored (see `art_ui::thumbnail`).
    pub image: Arc<egui::ColorImage>,
    /// A hash of the bytes: the piece's identity and its texture key.
    pub key: u64,
    pub bytes: usize,
    /// The CRCs a kept place holding it reports (#863; `art_library::blob_crcs`), of every copy
    /// merged into it.
    pub crcs: Vec<u32>,
    /// One of the wallpaper looks (`wallpapers.json` names it).
    pub wallpaper: bool,
    pub plugin: bool,
    /// Not made for this board's art screen (#340): the rule that fitted it.
    pub fitted: Option<String>,
}

impl PcPiece {
    /// Its library file, when it has one (a bundled or source piece is copied in first).
    pub fn file(&self) -> Option<&PathBuf> {
        self.origins.iter().find_map(|o| match o {
            Origin::Library(p) => Some(p),
            _ => None,
        })
    }

    /// Whether a kept place named `kept_name` (cut to the place's 16 bytes) holds this piece by
    /// one of its names.
    pub fn answers_to(&self, kept_name: &str) -> bool {
        self.names
            .iter()
            .any(|n| foxglove_link::protocol::art_name_fit(n) == kept_name)
    }

    /// Where its polarity choice is kept (#666, `art_polarity`): its library file, else its
    /// folder of frames; a bundled set gets one once it is copied in.
    pub fn polarity_path(&self) -> Option<&PathBuf> {
        self.file().or_else(|| {
            self.origins.iter().find_map(|o| match o {
                Origin::Source(p) => Some(p),
                _ => None,
            })
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
    /// See [`PcPiece::crcs`].
    pub crcs: Vec<u32>,
    pub plugin: bool,
    /// Not made for this board's art screen (#340): the rule that fitted it.
    pub fitted: Option<String>,
}

/// What the scan read: the pieces, the names it couldn't read, and the pieces that can't go on
/// the art screen with the reason (#340).
pub struct Scanned {
    pub found: Vec<Found>,
    pub unreadable: Vec<String>,
    pub refused: Vec<(String, String)>,
}

/// Every library, bundled and source piece the scan read, plus what it couldn't use.
pub fn found(library: &art_library::Library) -> Scanned {
    found_for(library, &super::art_encode::ArtTarget::resolve())
}

/// [`found`] for the art screen `target`.
pub fn found_for(
    library: &art_library::Library,
    target: &Result<super::art_encode::ArtTarget, String>,
) -> Scanned {
    use super::art_encode::{Fit, DOES_NOT_FIT};
    let note = |fit: Fit| target.as_ref().ok().and_then(|t| fit.note(*t));
    let mut out = Vec::new();
    let mut unreadable = Vec::new();
    let mut refused = Vec::new();
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
                crcs: e.crcs.clone(),
                plugin: false,
                fitted: note(e.fit),
            }),
            Err(why) => match why.strip_prefix(DOES_NOT_FIT) {
                Some(why) => refused.push((name, why.to_string())),
                None => unreadable.push(name),
            },
        }
    }
    for b in bundled(target).iter() {
        let (file, _, plugin) = BUNDLED[b.index];
        let name = file.trim_end_matches(".bin").to_string();
        match &b.made {
            Ok((image, key, bytes, crc, fit)) => out.push(Found {
                origin: Origin::Bundled(b.index),
                name,
                image: image.clone(),
                bytes: *bytes,
                key: *key,
                crcs: vec![*crc],
                plugin,
                fitted: note(*fit),
            }),
            Err(why) => refused.push((name, why.clone())),
        }
    }
    for src in &library.sources {
        let name = src
            .folder
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        if let Err(why) = &src.blob {
            if let Some(why) = why.strip_prefix(DOES_NOT_FIT) {
                refused.push((name, why.to_string()));
            }
            continue;
        }
        let (Ok(blob), Some(image)) = (&src.blob, &src.preview) else {
            continue;
        };
        out.push(Found {
            origin: Origin::Source(src.folder.clone()),
            name,
            image: image.clone(),
            bytes: blob.len(),
            key: art_library::blob_hash(blob),
            crcs: vec![foxglove_link::protocol::blob_crc(blob)],
            plugin: false,
            fitted: src.fit.and_then(note),
        });
    }
    Scanned {
        found: out,
        unreadable,
        refused,
    }
}

/// A bundled set as made for the art screen: its picture, hash, size, CRC and fit, or why it
/// can't. (Keep copies it into the library first, so the paced CRC comes with that copy.)
struct Bundled {
    index: usize,
    made: Result<
        (
            Arc<egui::ColorImage>,
            u64,
            usize,
            u32,
            super::art_encode::Fit,
        ),
        String,
    >,
}

type Target = Result<super::art_encode::ArtTarget, String>;

/// The bundled sets' pictures, made for `target` once (again when the art screen changes).
fn bundled(target: &Target) -> Arc<Vec<Bundled>> {
    static MADE: std::sync::Mutex<Option<(Target, Arc<Vec<Bundled>>)>> =
        std::sync::Mutex::new(None);
    let mut made = MADE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((_, sets)) = made.as_ref().filter(|(t, _)| t == target) {
        return sets.clone();
    }
    let sets: Arc<Vec<Bundled>> = Arc::new(
        BUNDLED
            .iter()
            .enumerate()
            .filter_map(|(index, (_, bytes, _))| {
                // As a library copy of it shows (Auto, #666), so the two are one piece; made for
                // the art screen as a library copy is (#340).
                let bytes = super::art_encode::normalise_blob(bytes, Default::default());
                let made = match super::art_encode::for_screen(&bytes, target) {
                    Ok((bytes, fit)) => Ok((
                        Arc::new(super::art_preview::first_frame(&bytes).ok()?),
                        art_library::blob_hash(&bytes),
                        bytes.len(),
                        foxglove_link::protocol::blob_crc(&bytes),
                        fit,
                    )),
                    Err(why) => Err(why
                        .strip_prefix(super::art_encode::DOES_NOT_FIT)
                        .unwrap_or(&why)
                        .to_string()),
                };
                Some(Bundled { index, made })
            })
            .collect(),
    );
    *made = Some((target.clone(), sets.clone()));
    sets
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
                if !p.names.contains(&f.name) {
                    p.names.push(f.name);
                }
                for crc in f.crcs {
                    if !p.crcs.contains(&crc) {
                        p.crcs.push(crc);
                    }
                }
            }
            None => out.push(PcPiece {
                wallpaper: is_wallpaper(&format!("{}.bin", f.name)),
                names: vec![f.name.clone()],
                name: f.name,
                origins: vec![f.origin],
                image: f.image,
                key: f.key,
                bytes: f.bytes,
                crcs: f.crcs,
                plugin: f.plugin,
                fitted: f.fitted,
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
    /// The PC piece it holds, when this PC has it: the one with its CRC, else (no CRC, or a
    /// piece paced or changed since) the one with its name and length.
    pub piece: Option<usize>,
    /// The CRC of its set, from firmware that reports it (#863).
    pub crc: Option<u32>,
}

impl Kept {
    /// Whether two places hold the same set: by CRC when both report one (#863), else by the
    /// PC piece both match.
    fn same_set(&self, other: &Kept) -> bool {
        match (self.crc, other.crc) {
            (Some(a), Some(b)) => a == b,
            _ => self.piece.is_some() && self.piece == other.piece,
        }
    }
}

/// How many kept places the keyboard has: its art report's count (at most 8), and never more
/// than the art screen's descriptor says (#832: 0 there hides the places). 0 without a report.
pub fn places(s: &Snapshot) -> u8 {
    let reported = s.status.map_or(0, |st| st.flash_slots.min(8));
    s.target.map_or(reported, |t| reported.min(t.slots))
}

/// The keyboard's kept places, from its own report: `None` for a free one. Empty on firmware
/// without places.
pub fn kept(s: &Snapshot, pcs: &[PcPiece]) -> Vec<Option<Kept>> {
    if s.status.is_none() {
        return Vec::new();
    }
    (0..places(s))
        .map(|n| {
            let info = s.slots.iter().find(|i| i.slot == n && i.used)?;
            let by_crc = info
                .crc
                .and_then(|crc| pcs.iter().position(|p| p.crcs.contains(&crc)));
            let piece = by_crc.or_else(|| {
                pcs.iter()
                    .position(|p| p.answers_to(&info.name) && p.bytes == usize::from(info.len))
            });
            Some(Kept {
                slot: n,
                name: info.name.clone(),
                piece,
                crc: info.crc,
            })
        })
        .collect()
}

/// The PC piece that holds a kept place's very bytes, for drawing its literal frames (the live
/// feed, `live_art`): with a CRC (#863) only the piece with that CRC, never a name-alike that
/// was paced or changed since; without one (older firmware), the piece with its name and length.
pub fn holding(info: &foxglove_link::protocol::SlotInfo, pcs: &[PcPiece]) -> Option<usize> {
    match info.crc {
        Some(crc) => pcs.iter().position(|p| p.crcs.contains(&crc)),
        None => pcs
            .iter()
            .position(|p| p.answers_to(&info.name) && p.bytes == usize::from(info.len)),
    }
}

/// A piece's bytes as made for the art screen and sent (the bytes a kept place holds): its
/// library file as pushed, else its bundled set or converted folder as made for the screen.
pub fn piece_blob(piece: &PcPiece, library: &art_library::Library) -> Option<Vec<u8>> {
    piece.origins.iter().find_map(|o| match o {
        Origin::Library(path) => super::art_service::pushed_bytes(path).ok(),
        Origin::Bundled(i) => {
            let (_, bytes, _) = BUNDLED.get(*i)?;
            let bytes = super::art_encode::normalise_blob(bytes, Default::default());
            let target = super::art_encode::ArtTarget::resolve();
            let (bytes, _) = super::art_encode::for_screen(&bytes, &target).ok()?;
            Some(super::art_encode::Pace::current().blob(bytes))
        }
        Origin::Source(folder) => library
            .sources
            .iter()
            .find(|s| &s.folder == folder)
            .and_then(|s| s.blob.as_ref().ok())
            .map(|b| b.to_vec()),
    })
}

/// Whether a kept place is the user's: places lent to the playlist (#522) change under it, so
/// they never count as keeping a piece.
fn yours(s: &Snapshot, k: &Kept) -> bool {
    !s.cycle.places.contains(&k.slot)
}

/// The kept place that already holds PC piece `index` (the same bytes: a place matches a piece
/// by CRC, else by name and length, and the same bytes on this PC are one piece), so Keep
/// refuses a second.
pub fn kept_as<'a>(s: &Snapshot, kept: &'a [Option<Kept>], index: usize) -> Option<&'a Kept> {
    kept.iter()
        .flatten()
        .find(|k| k.piece == Some(index) && yours(s, k))
}

/// The PC piece whose bytes come from `file` (a library file, or a folder of frames).
pub fn piece_of_file(pcs: &[PcPiece], file: &Path) -> Option<usize> {
    pcs.iter().position(|p| {
        p.origins.iter().any(|o| match o {
            Origin::Library(f) | Origin::Source(f) => f == file,
            Origin::Bundled(_) => false,
        })
    })
}

/// A kept place holding the same piece as another: `slot` can be freed, `of_slot` stays kept.
#[derive(Clone, Debug, PartialEq)]
pub struct Duplicate {
    pub slot: u8,
    pub name: String,
    pub of_slot: u8,
    pub of_name: String,
}

/// Whether kept place `slot` is in use: on the screen now, shown after a restart, or a layer's
/// art.
fn in_use(s: &Snapshot, slot: u8) -> bool {
    s.pinned == Some(Choice::Slot(slot))
        || s.cycle
            .layers
            .iter()
            .any(|l| l.art == super::art_cycle::Item::Slot(slot))
        || s.status
            .is_some_and(|st| showing(&st) == Showing::Kept(slot))
}

/// The user's kept places that hold a set another of them already holds: the same CRC when the
/// firmware reports it (#863; no PC piece needed), else the same PC piece. Of each set the one
/// in use stays (else the first); the rest are duplicates, each with the place that stays.
pub fn duplicates(s: &Snapshot, kept: &[Option<Kept>]) -> Vec<Duplicate> {
    let mine: Vec<&Kept> = kept
        .iter()
        .flatten()
        .filter(|k| (k.piece.is_some() || k.crc.is_some()) && yours(s, k))
        .collect();
    let mut out = Vec::new();
    let mut seen: Vec<u8> = Vec::new();
    for k in &mine {
        if seen.contains(&k.slot) {
            continue;
        }
        let same: Vec<&&Kept> = mine.iter().filter(|o| o.same_set(k)).collect();
        seen.extend(same.iter().map(|o| o.slot));
        let stays = same.iter().find(|o| in_use(s, o.slot)).unwrap_or(&same[0]);
        out.extend(
            same.iter()
                .filter(|o| o.slot != stays.slot)
                .map(|o| Duplicate {
                    slot: o.slot,
                    name: o.name.clone(),
                    of_slot: stays.slot,
                    of_name: stays.name.clone(),
                }),
        );
    }
    out.sort_by_key(|d| d.slot);
    out
}

/// "Kept: used of total places", from the keyboard's report (the bits of `flash_used`).
pub fn meter(s: &Snapshot) -> Option<(usize, usize)> {
    let st = s.status.filter(|_| places(s) > 0)?;
    let total = usize::from(places(s));
    let used = (0..total).filter(|n| st.flash_used & (1 << n) != 0).count();
    Some((used, total))
}

/// The first free place, `None` when the keyboard is full (or has none). Places lent to the
/// playlist (#522) are its own, never offered.
pub fn first_free(s: &Snapshot) -> Option<u8> {
    let st = s.status.filter(|_| places(s) > 0)?;
    (0..places(s)).find(|n| st.flash_used & (1 << n) == 0 && !s.cycle.places.contains(n))
}

/// The art buffer of firmware that doesn't report one (the art relay's 16 KiB).
pub const DEFAULT_CAPACITY: usize = 16 * 1024;

/// The most a piece can be: the art screen's largest blob from its descriptor (#832), else the
/// buffer the art status reports, else [`DEFAULT_CAPACITY`].
pub fn capacity(s: &Snapshot) -> usize {
    s.target
        .map(|t| usize::from(t.max_blob))
        .filter(|c| *c > 0)
        .or_else(|| {
            s.status
                .map(|st| usize::from(st.right_capacity))
                .filter(|c| *c > 0)
        })
        .unwrap_or(DEFAULT_CAPACITY)
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
    let name = super::art_service::file_name(file);
    piece.file() == Some(file) || piece.names.contains(&name)
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
            let file = write_to_library(&format!("{}.bin", piece.name), blob)?;
            // The encode applied the folder's polarity already (#666): the copy goes as it is.
            super::art_polarity::note(&file, super::art_encode::Polarity::AsIs);
            Ok(file)
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
    use foxglove_link::protocol::{ArtStatus, SlotInfo};

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
            crcs: Vec::new(),
            fitted: None,
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
            crc: None,
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

    /// #853: the same bytes under two names are one piece, named after the first file while its
    /// newest library copy leads (and Keep names the place after that copy). The kept place
    /// finds the piece's picture by any of its names; a place no piece answers to stays unknown.
    #[test]
    fn a_kept_place_finds_its_piece_by_any_name() {
        let pcs = pc_pieces(
            vec![
                found(lib("hearth-moongarden"), "hearth-moongarden", 7, 3281),
                found(lib("moon-garden"), "moon-garden", 7, 3281),
                found(Origin::Bundled(3), "moon-garden", 7, 3281),
                found(lib("Radar"), "Radar", 1, 3000),
            ],
            &|_| false,
        );
        assert_eq!(pcs.len(), 2, "one piece for the same bytes");
        let garden = &pcs[0];
        assert_eq!(garden.name, "hearth-moongarden");
        assert_eq!(garden.names, ["hearth-moongarden", "moon-garden"]);
        assert_eq!(
            garden.file(),
            Some(&PathBuf::from("art/moon-garden.bin")),
            "the newest library copy leads, so Keep names the place moon-garden"
        );
        let s = Snapshot {
            status: Some(status(0b111)),
            slots: vec![
                slot(0, "hearth-moongarde", 3281),
                slot(1, "moon-garden", 3281),
                slot(2, "lost-piece", 3281),
            ],
            ..Default::default()
        };
        let k = kept(&s, &pcs);
        let pieces: Vec<_> = k
            .iter()
            .take(3)
            .map(|p| p.as_ref().unwrap().piece)
            .collect();
        assert_eq!(
            pieces,
            [Some(0), Some(0), None],
            "the first place by its 16-byte cut name, the second by the merged name,              the third truly unknown"
        );
        assert_eq!(badge(&s, garden, &k, 0), Badge::OnKeyboard);
        let s = Snapshot {
            status: Some(ArtStatus {
                // Kept place 1, the one named after the merged copy.
                right_set: 18,
                ..status(0b11)
            }),
            ..s
        };
        assert_eq!(showing_picture(&s, &pcs).map(|p| p.0), Some(7));
    }

    /// Two kept places holding the same bytes (hearth-moongarden and moon-garden, one piece on
    /// this PC): Keep finds the piece already kept, and the second place is the duplicate
    /// unless only it is in use. A place lent to the playlist never counts.
    #[test]
    fn the_same_bytes_kept_twice_are_a_duplicate() {
        let pcs = pc_pieces(
            vec![
                found(lib("hearth-moongarden"), "hearth-moongarden", 7, 3281),
                found(lib("moon-garden"), "moon-garden", 7, 3281),
                found(lib("Radar"), "Radar", 1, 3000),
            ],
            &|_| false,
        );
        let s = Snapshot {
            status: Some(status(0b111)),
            slots: vec![
                slot(0, "hearth-moongarde", 3281),
                slot(1, "Radar", 3000),
                slot(2, "moon-garden", 3281),
            ],
            ..Default::default()
        };
        let k = kept(&s, &pcs);
        assert_eq!(kept_as(&s, &k, 0).map(|k| k.slot), Some(0));
        assert_eq!(kept_as(&s, &k, 1).map(|k| k.slot), Some(1));
        assert_eq!(
            piece_of_file(&pcs, Path::new("art/moon-garden.bin")),
            Some(0)
        );
        assert_eq!(piece_of_file(&pcs, Path::new("art/other.bin")), None);
        let dup = Duplicate {
            slot: 2,
            name: "moon-garden".into(),
            of_slot: 0,
            of_name: "hearth-moongarde".into(),
        };
        assert_eq!(duplicates(&s, &k), [dup]);

        // Only the second is used (after a restart): the first is the one to free.
        let pinned = Snapshot {
            pinned: Some(Choice::Slot(2)),
            ..s.clone()
        };
        let d = duplicates(&pinned, &kept(&pinned, &pcs));
        assert_eq!((d[0].slot, d[0].of_slot), (0, 2));

        // Lent to the playlist: not the user's, so no duplicate and nothing kept by it.
        let mut lent = s.clone();
        lent.cycle.places = vec![0];
        let k = kept(&lent, &pcs);
        assert!(duplicates(&lent, &k).is_empty());
        assert_eq!(kept_as(&lent, &k, 0).map(|k| k.slot), Some(2));

        // Different bytes under different names are no duplicate.
        let one = Snapshot {
            slots: vec![slot(0, "moon-garden", 3281), slot(1, "Radar", 3000)],
            status: Some(status(0b11)),
            ..Default::default()
        };
        assert!(duplicates(&one, &kept(&one, &pcs)).is_empty());
    }

    /// A found piece with the CRCs a place holding it reports (#863).
    fn found_crc(origin: Origin, name: &str, key: u64, bytes: usize, crc: u32) -> Found {
        Found {
            crcs: vec![crc],
            ..found(origin, name, key, bytes)
        }
    }

    fn slot_crc(n: u8, name: &str, len: u16, crc: u32) -> SlotInfo {
        SlotInfo {
            crc: Some(crc),
            ..slot(n, name, len)
        }
    }

    /// #863: a place that reports its CRC finds the piece with those bytes whatever it is
    /// named, so Keep refuses that piece and the screen shows its picture. A CRC no piece has
    /// (a piece paced or changed since) falls back to the name and length, as a place without
    /// one (older firmware) does.
    #[test]
    fn a_kept_place_with_a_crc_finds_its_piece_by_content() {
        let pcs = pc_pieces(
            vec![
                found_crc(lib("orbit"), "orbit", 1, 3000, 0x1111),
                found_crc(lib("radar"), "radar", 2, 3000, 0x2222),
            ],
            &|_| false,
        );
        let s = Snapshot {
            status: Some(status(0b111)),
            slots: vec![
                // Named after a copy this PC no longer has, same length as both pieces.
                slot_crc(0, "renamed", 3000, 0x2222),
                // Named after orbit but holding radar's bytes: the CRC wins over the name.
                slot_crc(1, "orbit", 3000, 0x2222),
                // A CRC no piece has: radar by name and length.
                slot_crc(2, "radar", 3000, 0x9999),
            ],
            ..Default::default()
        };
        let k = kept(&s, &pcs);
        let pieces: Vec<_> = k
            .iter()
            .take(3)
            .map(|p| p.as_ref().unwrap().piece)
            .collect();
        assert_eq!(pieces, [Some(1), Some(1), Some(1)]);
        assert_eq!(k[0].as_ref().unwrap().crc, Some(0x2222));
        assert_eq!(kept_as(&s, &k, 1).map(|k| k.slot), Some(0));
        assert_eq!(kept_as(&s, &k, 0), None, "orbit's bytes are kept nowhere");
        assert_eq!(badge(&s, &pcs[0], &k, 0), Badge::ThisPc);
        // The screen plays place 0 (set_count 17 + 0): radar's picture, found by CRC alone.
        let showing = Snapshot {
            status: Some(ArtStatus {
                right_set: 17,
                ..status(0b111)
            }),
            ..s.clone()
        };
        assert_eq!(showing_picture(&showing, &pcs).map(|p| p.0), Some(2));
        // The same places from firmware without CRCs: by name and length only.
        let old = Snapshot {
            slots: s
                .slots
                .iter()
                .map(|i| SlotInfo {
                    crc: None,
                    ..i.clone()
                })
                .collect(),
            ..s.clone()
        };
        let pieces: Vec<_> = kept(&old, &pcs)
            .iter()
            .take(3)
            .map(|p| p.as_ref().unwrap().piece)
            .collect();
        assert_eq!(pieces, [None, Some(0), Some(1)]);
    }

    /// #863: places with CRCs are duplicates when their CRCs match, even of a piece this PC
    /// doesn't have; the same name and length with different CRCs is no duplicate.
    #[test]
    fn places_with_the_same_crc_are_duplicates() {
        let pcs = pc_pieces(
            vec![found_crc(lib("radar"), "radar", 2, 3000, 0x2222)],
            &|_| false,
        );
        let s = Snapshot {
            status: Some(status(0b1111)),
            slots: vec![
                slot_crc(0, "lost-piece", 4000, 0x7777),
                slot_crc(1, "radar", 3000, 0x2222),
                slot_crc(2, "lost-copy", 4000, 0x7777),
                // radar by name and length (a CRC no piece has), but other bytes than place 1.
                slot_crc(3, "radar", 3000, 0x3333),
            ],
            ..Default::default()
        };
        let k = kept(&s, &pcs);
        assert_eq!(k[0].as_ref().unwrap().piece, None, "no piece on this PC");
        assert_eq!(k[3].as_ref().unwrap().piece, Some(0), "by name and length");
        let dup = Duplicate {
            slot: 2,
            name: "lost-copy".into(),
            of_slot: 0,
            of_name: "lost-piece".into(),
        };
        assert_eq!(duplicates(&s, &k), [dup]);
        // In use, the second copy stays and the first goes.
        let pinned = Snapshot {
            pinned: Some(Choice::Slot(2)),
            ..s.clone()
        };
        let d = duplicates(&pinned, &kept(&pinned, &pcs));
        assert_eq!(
            d.iter().map(|d| (d.slot, d.of_slot)).collect::<Vec<_>>(),
            [(0, 2)]
        );
        // Lent to the playlist: never a duplicate.
        let mut lent = s.clone();
        lent.cycle.places = vec![2];
        assert!(duplicates(&lent, &kept(&lent, &pcs)).is_empty());
    }

    /// A piece's CRCs are every merged copy's (#863), so a place holding any of them finds it.
    #[test]
    fn a_merged_piece_keeps_every_copys_crc() {
        let pcs = pc_pieces(
            vec![
                found_crc(lib("fury"), "fury", 1, 900, 0xAAAA),
                found_crc(Origin::Bundled(0), "fury", 1, 900, 0xAAAA),
                found_crc(lib("Fury"), "Fury", 5, 900, 0xBBBB),
            ],
            &|_| false,
        );
        assert_eq!(pcs.len(), 1);
        assert_eq!(pcs[0].crcs, [0xAAAA, 0xBBBB]);
    }

    /// The CRCs a place reports for a library piece (#863): the piece's own, and as paced for
    /// a still e-paper screen, which sends its first frame only. A fast screen with no limit
    /// sends the bytes as they are: one CRC, the one BEGIN announces.
    #[test]
    fn a_piece_has_the_crc_it_is_sent_with() {
        use super::super::art_encode::Pace;
        use super::super::boards::Refresh;
        use foxglove_link::protocol::blob_crc;
        let bytes = super::super::art_encode::normalise_blob(BUNDLED[0].1, Default::default());
        assert_eq!(
            art_library::blob_crcs(&bytes, Pace::default()),
            [blob_crc(&bytes)]
        );
        let still = Pace {
            refresh: Refresh::EpaperPartial,
            ..Pace::default()
        };
        let sent = still.blob(bytes.clone());
        assert_ne!(
            sent, bytes,
            "fury is animated: e-paper gets its first frame only"
        );
        assert_eq!(
            art_library::blob_crcs(&bytes, still),
            [blob_crc(&bytes), blob_crc(&sent)]
        );
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

    /// #832: the art screen's descriptor wins: its largest blob over the status's buffer, and
    /// 0 places there hides them (the meter, the list, the free place).
    #[test]
    fn the_descriptor_sets_capacity_and_places() {
        use foxglove_link::protocol::ArtLimits;
        let pcs = pc_pieces(
            vec![
                found(lib("Radar"), "Radar", 1, 20000),
                found(lib("Orbit"), "Orbit", 2, 40000),
            ],
            &|_| false,
        );
        let target = |max_blob, slots| {
            Some(ArtLimits {
                max_blob,
                slots,
                refresh: ArtLimits::REFRESH_FAST,
                min_frame_ms: 0,
            })
        };
        let mut s = Snapshot {
            status: Some(status(0b1)),
            ..Default::default()
        };
        // v1 firmware: the status's 16 KiB.
        assert_eq!(capacity(&s), 16384);
        assert_eq!(super::super::art_ui::kib(capacity(&s)), "16 KiB");
        // A synthetic 32 KiB target: a 20 000-byte piece fits, a 40 000-byte one is over 32 KiB.
        s.target = target(32768, 8);
        assert_eq!(capacity(&s), 32768);
        assert_eq!(super::super::art_ui::kib(capacity(&s)), "32 KiB");
        let k = kept(&s, &pcs);
        // Sorted: Orbit, Radar.
        assert_eq!(badge(&s, &pcs[0], &k, 0), Badge::TooBig);
        assert_eq!(badge(&s, &pcs[1], &k, 1), Badge::ThisPc);
        // A blob of 0 says nothing: the status's buffer, then the default.
        s.target = target(0, 8);
        assert_eq!(capacity(&s), 16384);
        s.status = Some(ArtStatus {
            right_capacity: 0,
            ..status(0)
        });
        assert_eq!(capacity(&s), DEFAULT_CAPACITY);

        // Places: the report's 8, unless the descriptor says fewer.
        s.status = Some(status(0b1));
        s.target = target(16384, 8);
        assert_eq!((places(&s), kept(&s, &pcs).len()), (8, 8));
        assert_eq!(meter(&s), Some((1, 8)));
        s.target = target(16384, 0);
        assert_eq!(places(&s), 0);
        assert!(kept(&s, &pcs).is_empty(), "no places: the list is hidden");
        assert_eq!((meter(&s), first_free(&s)), (None, None));
        s.target = None;
        assert_eq!(places(&s), 8);
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

    /// Every bundled set fits a pushed set (the right half's 16 KiB) with the art budget's
    /// headroom (12 KiB), and its file name is the name the library shows.
    #[test]
    fn bundled_sets_fit_the_push_budget() {
        for (file, bytes, _) in BUNDLED {
            assert!(bytes.starts_with(b"NVA1"), "{file} is an NVA1 blob");
            assert!(bytes.len() <= 12 * 1024, "{file}: {} bytes", bytes.len());
            assert!(file.ends_with(".bin"), "{file}");
        }
    }

    /// #340: on another board's art screen the bundled (Corne-made) sets are fitted and marked
    /// "not made for this board"; on the Corne they aren't; on a screen that can't take art they
    /// are listed with the reason instead.
    #[test]
    fn bundled_art_is_made_for_the_board() {
        use super::super::art_encode::ArtTarget;
        let lib = art_library::Library::default();
        let corne = found_for(&lib, &Ok(ArtTarget::FIRMWARE));
        assert_eq!(corne.found.len(), BUNDLED.len());
        assert!(corne.found.iter().all(|f| f.fitted.is_none()));
        assert!(corne.found.iter().all(|f| f.image.size == [140, 68]));
        assert_eq!(corne.found[0].bytes, BUNDLED[0].1.len(), "untouched");

        let tall = ArtTarget {
            width: 88,
            height: 184,
            rotation: 0,
        };
        let other = found_for(&lib, &Ok(tall));
        assert_eq!(other.found.len(), BUNDLED.len());
        assert!(other.found.iter().all(|f| f.image.size == [88, 184]));
        assert!(other
            .found
            .iter()
            .all(|f| f.fitted.as_deref().is_some_and(|n| n.contains("88x184"))));
        assert!(other.refused.is_empty());

        let why = "The art screen is 320x80; art can be at most 255x255.".to_string();
        let none = found_for(&lib, &Err(why.clone()));
        assert!(none.found.is_empty());
        assert_eq!(none.refused.len(), BUNDLED.len());
        assert!(none.refused.iter().all(|(_, w)| *w == why));
    }
}
