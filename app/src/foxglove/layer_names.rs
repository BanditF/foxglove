//! The keyboard's layer names (from ZMK Studio), remembered in the settings store's `[layers]`
//! section so the settings can show them while the keyboard isn't connected. Imported once
//! from `layers.ini`.

use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

use ini::Ini;
use serde::{Deserialize, Serialize};

static NAMES: OnceLock<Mutex<Vec<Option<String>>>> = OnceLock::new();

/// Layer index (as text: TOML keys are strings) -> name.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct Layers {
    names: BTreeMap<String, String>,
}

impl Layers {
    fn from_names(names: &[Option<String>]) -> Layers {
        let names = names
            .iter()
            .enumerate()
            .filter_map(|(i, n)| Some((i.to_string(), n.clone()?)))
            .collect();
        Layers { names }
    }

    fn to_names(&self) -> Vec<Option<String>> {
        let mut names = Vec::new();
        for (index, name) in &self.names {
            let Ok(index) = index.parse::<usize>() else {
                continue;
            };
            if names.len() <= index {
                names.resize(index + 1, None);
            }
            names[index] = Some(name.clone());
        }
        names
    }
}

fn names() -> &'static Mutex<Vec<Option<String>>> {
    NAMES.get_or_init(|| {
        let layers: Layers = super::store::load_or_migrate("layers", || {
            let ini = crate::settings::Settings::config_file_path()?.with_file_name("layers.ini");
            let conf = Ini::load_from_file(&ini).ok()?;
            let names = conf
                .section(Some("layers"))?
                .iter()
                .map(|(i, n)| (i.to_string(), n.to_string()))
                .collect();
            Some((Layers { names }, vec![ini]))
        });
        Mutex::new(layers.to_names())
    })
}

/// Stores the connected keyboard's names, writing only when they changed.
pub fn remember(new: &[Option<String>]) {
    let mut names = names().lock().unwrap_or_else(|e| e.into_inner());
    if names.as_slice() == new {
        return;
    }
    *names = new.to_vec();
    if let Err(e) = super::store::save("layers", &Layers::from_names(new)) {
        eprintln!("Foxglove: can't save the layer names: {e}");
    }
}

/// The names from the last connection.
pub fn remembered() -> Vec<Option<String>> {
    names().lock().unwrap_or_else(|e| e.into_inner()).clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_keep_their_indexes_with_gaps() {
        let names = vec![Some("Base".to_string()), None, Some("Nav".to_string())];
        let layers = Layers::from_names(&names);
        let back: Layers = toml::from_str(&toml::to_string(&layers).unwrap()).unwrap();
        assert_eq!(back.to_names(), names);
    }
}
