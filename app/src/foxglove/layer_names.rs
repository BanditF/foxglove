//! The keyboard's layer names (from ZMK Studio), remembered in `layers.ini` next to the settings,
//! so the settings can show them while the keyboard isn't connected.

use std::sync::{Mutex, OnceLock};

use ini::Ini;

static NAMES: OnceLock<Mutex<Vec<Option<String>>>> = OnceLock::new();

fn path() -> Option<std::path::PathBuf> {
    crate::settings::Settings::config_file_path().map(|p| p.with_file_name("layers.ini"))
}

fn names() -> &'static Mutex<Vec<Option<String>>> {
    NAMES.get_or_init(|| {
        let loaded = path()
            .and_then(|p| Ini::load_from_file(p).ok())
            .and_then(|conf| {
                let section = conf.section(Some("layers"))?;
                let mut names = Vec::new();
                for (index, name) in section.iter() {
                    let Ok(index) = index.parse::<usize>() else {
                        continue;
                    };
                    if names.len() <= index {
                        names.resize(index + 1, None);
                    }
                    names[index] = Some(name.to_string());
                }
                Some(names)
            })
            .unwrap_or_default();
        Mutex::new(loaded)
    })
}

/// Stores the connected keyboard's names, writing the file only when they changed.
pub fn remember(new: &[Option<String>]) {
    let mut names = names().lock().unwrap();
    if names.as_slice() == new {
        return;
    }
    *names = new.to_vec();
    let mut conf = Ini::new();
    let mut section = conf.with_section(Some("layers"));
    for (index, name) in new.iter().enumerate() {
        if let Some(name) = name {
            section.set(index.to_string(), name.as_str());
        }
    }
    if let Some(p) = path() {
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = conf.write_to_file(p);
    }
}

/// The names from the last connection.
pub fn remembered() -> Vec<Option<String>> {
    names().lock().unwrap().clone()
}
