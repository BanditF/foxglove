//! Shared art library, loaded away from the UI and tray threads.
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

#[derive(Clone)]
pub struct Entry {
    pub path: PathBuf,
    pub preview: Result<Arc<egui::ColorImage>, String>,
    /// The file's size and a hash of its bytes, so the art library view can show the same art
    /// in two places once (#59).
    pub bytes: usize,
    pub hash: u64,
    /// The CRCs a kept place reports for these bytes (#863, `SlotInfo::crc`): as made for the
    /// art screen, and as paced for it when that differs (`art_encode::Pace`, at scan time).
    pub crcs: Vec<u32>,
    /// How it was made for the art screen (#340): a blob made for another screen is fitted.
    pub fit: super::art_encode::Fit,
}

/// A hash of an art blob's bytes; equal blobs are the same art.
pub fn blob_hash(bytes: &[u8]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut h);
    h.finish()
}

/// The CRCs a kept place holding `bytes` (as made for the art screen) can report (#863): the
/// bytes' own, and, when `pace` changes them, those of the bytes as sent (`pushed_bytes`).
pub fn blob_crcs(bytes: &[u8], pace: super::art_encode::Pace) -> Vec<u32> {
    use foxglove_link::protocol::blob_crc;
    let made = blob_crc(bytes);
    let paced = blob_crc(&pace.blob(bytes.to_vec()));
    if paced == made {
        vec![made]
    } else {
        vec![made, paced]
    }
}
/// A set folder under one of the library's source folders, encoded on the scan thread.
#[derive(Clone)]
pub struct Source {
    pub folder: PathBuf,
    pub blob: Result<Arc<Vec<u8>>, String>,
    pub preview: Option<Arc<egui::ColorImage>>,
    /// How the set was fitted to the art screen (#340), when it encoded.
    pub fit: Option<super::art_encode::Fit>,
}

#[derive(Clone, Default)]
pub struct Library {
    pub entries: Vec<Entry>,
    pub sources: Vec<Source>,
    pub error: Option<String>,
}
static LIBRARY: OnceLock<Arc<Mutex<Library>>> = OnceLock::new();
static LOADING: AtomicBool = AtomicBool::new(false);
/// The art screen the source sets were last made for (#560, #340): when the keyboard's turns out
/// different (it connected after the first scan), they are made again.
type Target = Result<super::art_encode::ArtTarget, String>;
static MADE_FOR: Mutex<Option<Target>> = Mutex::new(None);
pub static PICK_FILE: AtomicBool = AtomicBool::new(false);
/// Counts finished scans, so what reads pieces by content (the live feed's kept places,
/// `live_art`) reads them again when the library changed under it.
static SCANS: AtomicU64 = AtomicU64::new(0);

/// How many scans have finished: a new value means the library may hold other pieces now.
pub fn generation() -> u64 {
    SCANS.load(Ordering::Relaxed)
}

/// A `.bin` the right screen can play. `<name>.left.bin` is a wallpaper's left-screen companion
/// (the wallpaper manifest names it): a different size, never right art, so it stays out.
pub fn is_right_art_file(path: &std::path::Path) -> bool {
    let is_bin = path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("bin"));
    let left = path
        .file_stem()
        .and_then(|s| s.to_str())
        .is_some_and(|s| s.to_ascii_lowercase().ends_with(".left"));
    is_bin && !left && !is_kept_copy(path)
}

/// The file name of a kept place's bytes read back from the keyboard (#878, `live_art`), by the
/// set's CRC: `kept-<crc>.bin` in this folder, kept out of the library's list.
pub fn kept_copy_name(crc: u32) -> String {
    format!("kept-{crc:08x}.bin")
}

/// Whether `path` is such a copy (`kept-` and eight hex digits).
pub fn is_kept_copy(path: &std::path::Path) -> bool {
    path.file_stem()
        .and_then(|s| s.to_str())
        .and_then(|s| s.strip_prefix("kept-"))
        .is_some_and(|hex| hex.len() == 8 && hex.chars().all(|c| c.is_ascii_hexdigit()))
}

pub fn folder() -> Option<PathBuf> {
    crate::settings::Settings::config_file_path().map(|p| p.with_file_name("art"))
}
pub fn snapshot() -> Library {
    let state = LIBRARY.get_or_init(|| {
        let state = Arc::new(Mutex::new(Library::default()));
        scan(state.clone());
        state
    });
    let made_for = MADE_FOR.lock().unwrap_or_else(|e| e.into_inner()).clone();
    if made_for.is_some_and(|t| t != super::art_encode::ArtTarget::resolve()) {
        scan(state.clone());
    }
    state.lock().unwrap_or_else(|e| e.into_inner()).clone()
}
pub fn refresh() {
    let _ = snapshot();
    if let Some(state) = LIBRARY.get() {
        scan(state.clone());
    }
}
fn scan(state: Arc<Mutex<Library>>) {
    if LOADING.swap(true, Ordering::Relaxed) {
        return;
    }
    let target = super::art_encode::ArtTarget::resolve();
    *MADE_FOR.lock().unwrap_or_else(|e| e.into_inner()) = Some(target.clone());
    if std::thread::Builder::new()
        .name("foxglove-art-library".into())
        .spawn(move || {
            let mut library = Library::default();
            // As Keep would send them now (#863: the CRCs a kept place reports).
            let pace = super::art_encode::Pace::current();
            let result = (|| -> Result<(), String> {
                let folder = folder().ok_or("No settings directory")?;
                std::fs::create_dir_all(&folder).map_err(|e| e.to_string())?;
                for entry in std::fs::read_dir(folder).map_err(|e| e.to_string())? {
                    let path = entry.map_err(|e| e.to_string())?.path();
                    if !is_right_art_file(&path) {
                        continue;
                    }
                    use std::io::Read;
                    // As shown and sent: normalised to the piece's polarity (#666), made for
                    // the art screen (#340).
                    let read = std::fs::File::open(&path)
                        .and_then(|f| {
                            let mut bytes = Vec::new();
                            f.take(65536).read_to_end(&mut bytes)?;
                            Ok(super::art_polarity::blob(&path, &bytes))
                        })
                        .map_err(|e| e.to_string())
                        .and_then(|b| super::art_encode::for_screen(&b, &target));
                    let (bytes, hash, crcs, fit) = read.as_ref().map_or(
                        (0, 0, Vec::new(), super::art_encode::Fit::AsMade),
                        |(b, fit)| (b.len(), blob_hash(b), blob_crcs(b, pace), *fit),
                    );
                    let preview = read.and_then(|(b, _)| {
                        super::art_preview::first_frame(&b)
                            .map(Arc::new)
                            .map_err(str::to_owned)
                    });
                    library.entries.push(Entry {
                        path,
                        preview,
                        bytes,
                        hash,
                        crcs,
                        fit,
                    });
                }
                library.entries.sort_by(|a, b| a.path.cmp(&b.path));
                // Source folders (#59): every set folder under them, converted for the view.
                // Made for the connected keyboard's art screen (#560).
                for source in super::art_sources::load().sources {
                    for folder in super::art_sources::sets_in(&source) {
                        let polarity = super::art_polarity::of(&folder); // #666
                        let made = target
                            .as_ref()
                            .map_err(|e| format!("{}{e}", super::art_encode::DOES_NOT_FIT))
                            .and_then(|t| super::art_encode::encode_set(&folder, *t, polarity));
                        let preview = made
                            .as_ref()
                            .ok()
                            .and_then(|(b, _)| super::art_preview::first_frame(b).ok())
                            .map(Arc::new);
                        let fit = made.as_ref().ok().map(|(_, fit)| *fit);
                        library.sources.push(Source {
                            folder,
                            blob: made.map(|(b, _)| Arc::new(b)),
                            preview,
                            fit,
                        });
                    }
                }
                Ok(())
            })();
            library.error = result.err();
            *state.lock().unwrap_or_else(|e| e.into_inner()) = library;
            LOADING.store(false, Ordering::Relaxed);
            SCANS.fetch_add(1, Ordering::Relaxed);
            // A kept place's piece may be here now (the live feed's frames, `live_art`).
            super::live_feed::art_changed();
        })
        .is_err()
    {
        LOADING.store(false, Ordering::Relaxed);
    }
}
/// Moves a library file to the Recycle Bin (never a hard delete on Windows), then rescans.
pub fn delete(path: PathBuf) {
    let _ = std::thread::Builder::new()
        .name("foxglove-art-delete".into())
        .spawn(move || {
            if let Err(e) = recycle(&path) {
                if let Some(state) = LIBRARY.get() {
                    state.lock().unwrap_or_else(|e| e.into_inner()).error = Some(e.to_string());
                }
            } else {
                refresh();
            }
        });
}

#[cfg(target_os = "windows")]
fn recycle(path: &std::path::Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::UI::Shell::{
        SHFileOperationW, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOERRORUI, FOF_SILENT, FO_DELETE,
        SHFILEOPSTRUCTW,
    };
    // A double-NUL-terminated list of one path.
    let mut from: Vec<u16> = path.as_os_str().encode_wide().collect();
    from.extend([0, 0]);
    let mut op = SHFILEOPSTRUCTW {
        wFunc: FO_DELETE,
        pFrom: windows::core::PCWSTR(from.as_ptr()),
        fFlags: (FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_NOERRORUI | FOF_SILENT).0 as u16,
        ..Default::default()
    };
    // SAFETY: `from` outlives the call and is double-NUL terminated, as the API requires.
    let code = unsafe { SHFileOperationW(&mut op) };
    if code != 0 || op.fAnyOperationsAborted.as_bool() {
        return Err(std::io::Error::other(format!(
            "couldn't move it to the Recycle Bin (code {code})"
        )));
    }
    Ok(())
}

#[cfg(not(target_os = "windows"))]
fn recycle(path: &std::path::Path) -> std::io::Result<()> {
    std::fs::remove_file(path)
}

#[cfg(test)]
mod tests {
    use super::is_right_art_file;
    use std::path::Path;

    #[test]
    fn left_companions_are_not_right_art() {
        assert!(is_right_art_file(Path::new("hearth-moongarden.bin")));
        assert!(is_right_art_file(Path::new("HUD.BIN")));
        assert!(!is_right_art_file(Path::new("hearth-moongarden.left.bin")));
        assert!(!is_right_art_file(Path::new("Hud.LEFT.BIN")));
        assert!(!is_right_art_file(Path::new("hud.left"))); // not a .bin at all
        assert!(!is_right_art_file(Path::new("notes.txt")));
        assert!(
            is_right_art_file(Path::new("left.bin")),
            "a set named left is still right art"
        );
    }

    /// #878: a kept place's bytes read back are kept by CRC beside the library, not in it.
    #[test]
    fn kept_copies_stay_out_of_the_library() {
        let name = super::kept_copy_name(0x788A_EFC8);
        assert_eq!(name, "kept-788aefc8.bin");
        assert!(!is_right_art_file(Path::new(&name)));
        assert!(super::is_kept_copy(Path::new(&name)));
        assert!(
            is_right_art_file(Path::new("kept-fox.bin")),
            "a piece named kept-"
        );
        assert!(is_right_art_file(Path::new("kept-788aefc8x.bin")));
    }
}
