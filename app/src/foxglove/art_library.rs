//! Shared art library, loaded away from the UI and tray threads.
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

#[derive(Clone)]
pub struct Entry {
    pub path: PathBuf,
    pub preview: Result<Arc<egui::ColorImage>, String>,
    /// The file's size and a hash of its bytes, so the art library view can show the same art
    /// in two places once (#59).
    pub bytes: usize,
    pub hash: u64,
}

/// A hash of an art blob's bytes; equal blobs are the same art.
pub fn blob_hash(bytes: &[u8]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut h);
    h.finish()
}
/// A set folder under one of the library's source folders, encoded on the scan thread.
#[derive(Clone)]
pub struct Source {
    pub folder: PathBuf,
    pub blob: Result<Arc<Vec<u8>>, String>,
    pub preview: Option<Arc<egui::ColorImage>>,
}

#[derive(Clone, Default)]
pub struct Library {
    pub entries: Vec<Entry>,
    pub sources: Vec<Source>,
    pub error: Option<String>,
}
static LIBRARY: OnceLock<Arc<Mutex<Library>>> = OnceLock::new();
static LOADING: AtomicBool = AtomicBool::new(false);
pub static PICK_FILE: AtomicBool = AtomicBool::new(false);

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
    is_bin && !left
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
    if std::thread::Builder::new()
        .name("foxglove-art-library".into())
        .spawn(move || {
            let mut library = Library::default();
            let result = (|| -> Result<(), String> {
                let folder = folder().ok_or("No settings directory")?;
                std::fs::create_dir_all(&folder).map_err(|e| e.to_string())?;
                for entry in std::fs::read_dir(folder).map_err(|e| e.to_string())? {
                    let path = entry.map_err(|e| e.to_string())?.path();
                    if !is_right_art_file(&path) {
                        continue;
                    }
                    use std::io::Read;
                    let read = std::fs::File::open(&path).and_then(|f| {
                        let mut bytes = Vec::new();
                        f.take(65536).read_to_end(&mut bytes)?;
                        Ok(bytes)
                    });
                    let (bytes, hash) = read.as_ref().map_or((0, 0), |b| (b.len(), blob_hash(b)));
                    let preview = read.map_err(|e| e.to_string()).and_then(|b| {
                        super::art_preview::first_frame(&b)
                            .map(Arc::new)
                            .map_err(str::to_owned)
                    });
                    library.entries.push(Entry {
                        path,
                        preview,
                        bytes,
                        hash,
                    });
                }
                library.entries.sort_by(|a, b| a.path.cmp(&b.path));
                // Source folders (#59): every set folder under them, converted for the view.
                // Made for the connected keyboard's art screen (#560).
                let target = super::art_encode::ArtTarget::current();
                for source in super::art_sources::load().sources {
                    for folder in super::art_sources::sets_in(&source) {
                        let blob = super::art_encode::encode_folder(&folder, target);
                        let preview = blob
                            .as_ref()
                            .ok()
                            .and_then(|b| super::art_preview::first_frame(b).ok())
                            .map(Arc::new);
                        library.sources.push(Source {
                            folder,
                            blob: blob.map(Arc::new),
                            preview,
                        });
                    }
                }
                Ok(())
            })();
            library.error = result.err();
            *state.lock().unwrap_or_else(|e| e.into_inner()) = library;
            LOADING.store(false, Ordering::Relaxed);
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
}
