//! Shared art library, loaded away from the UI and tray threads.
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

#[derive(Clone)]
pub struct Entry {
    pub path: PathBuf,
    pub preview: Result<Arc<egui::ColorImage>, String>,
}
#[derive(Clone, Default)]
pub struct Library {
    pub entries: Vec<Entry>,
    pub error: Option<String>,
}
static LIBRARY: OnceLock<Arc<Mutex<Library>>> = OnceLock::new();
static LOADING: AtomicBool = AtomicBool::new(false);
pub static PICK_FILE: AtomicBool = AtomicBool::new(false);

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
                    if !path
                        .extension()
                        .is_some_and(|e| e.eq_ignore_ascii_case("bin"))
                    {
                        continue;
                    }
                    use std::io::Read;
                    let preview = std::fs::File::open(&path)
                        .and_then(|f| {
                            let mut bytes = Vec::new();
                            f.take(65536).read_to_end(&mut bytes)?;
                            Ok(bytes)
                        })
                        .map_err(|e| e.to_string())
                        .and_then(|b| {
                            super::art_preview::first_frame(&b)
                                .map(Arc::new)
                                .map_err(str::to_owned)
                        });
                    library.entries.push(Entry { path, preview });
                }
                library.entries.sort_by(|a, b| a.path.cmp(&b.path));
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
pub fn delete(path: PathBuf) {
    let _ = std::thread::Builder::new()
        .name("foxglove-art-delete".into())
        .spawn(move || {
            if let Err(e) = std::fs::remove_file(path) {
                if let Some(state) = LIBRARY.get() {
                    state.lock().unwrap_or_else(|e| e.into_inner()).error = Some(e.to_string());
                }
            } else {
                refresh();
            }
        });
}
