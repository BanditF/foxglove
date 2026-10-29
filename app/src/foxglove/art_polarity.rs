//! Which way round each piece's ink is (#666): the art picker's Auto / As is / Invert per piece,
//! by its library file or its folder of frames, in the store's `[art.polarity]`. Auto is the
//! default and isn't stored.
//!
//! Every piece is normalised before it is shown or sent, so that its drawing is the ink and the
//! keyboard's panel polarity (dark or light, Screens) alone decides how it looks: a folder of
//! frames when it is encoded (`art_encode::encode_folder`), a blob file (the user's, a plugin's,
//! a bundled one) by its ink flag (`art_encode::normalise_blob`). A folder's blob copied into the
//! library is stored As is, as its encode already applied the folder's choice.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use super::art_encode::{self, Polarity};

const SECTION: &str = "art.polarity";

/// The overrides: a path (as Foxglove names it) to its polarity; Auto is never stored.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Overrides {
    pub pieces: BTreeMap<String, Polarity>,
}

impl Overrides {
    pub fn of(&self, path: &Path) -> Polarity {
        self.pieces.get(&key(path)).copied().unwrap_or_default()
    }

    /// Sets `path`'s polarity; true when it changed.
    pub fn set(&mut self, path: &Path, polarity: Polarity) -> bool {
        let k = key(path);
        let before = self.pieces.get(&k).copied().unwrap_or_default();
        if polarity == Polarity::Auto {
            self.pieces.remove(&k);
        } else {
            self.pieces.insert(k, polarity);
        }
        before != polarity
    }
}

fn key(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

static CACHE: Mutex<Option<Overrides>> = Mutex::new(None);

fn with<T>(f: impl FnOnce(&mut Overrides) -> T) -> T {
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    let o = cache.get_or_insert_with(|| super::store::try_load(SECTION).unwrap_or_default());
    f(o)
}

/// The polarity chosen for the piece at `path` (a library file or a folder of frames).
pub fn of(path: &Path) -> Polarity {
    with(|o| o.of(path))
}

/// Chooses `polarity` for the piece at `path` and saves it; the art library is converted again
/// so the pictures follow. Returns whether it changed.
pub fn set(path: &Path, polarity: Polarity) -> bool {
    let changed = with(|o| {
        let changed = o.set(path, polarity);
        if changed {
            if let Err(e) = super::store::save(SECTION, o) {
                eprintln!("Foxglove: can't save the art's polarity: {e}");
            }
        }
        changed
    });
    if changed {
        super::art_library::refresh();
    }
    changed
}

/// Records `polarity` for `path` without converting the library again (a file just written).
pub fn note(path: &Path, polarity: Polarity) {
    with(|o| {
        if o.set(path, polarity) {
            if let Err(e) = super::store::save(SECTION, o) {
                eprintln!("Foxglove: can't save the art's polarity: {e}");
            }
        }
    });
}

/// A blob file's bytes as shown and sent: normalised to its polarity.
pub fn blob(path: &Path, bytes: &[u8]) -> Vec<u8> {
    art_encode::normalise_blob(bytes, of(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_is_the_default_and_never_stored() {
        let mut o = Overrides::default();
        let p = Path::new(r"C:\art\fox.bin");
        assert_eq!(o.of(p), Polarity::Auto);
        assert!(o.set(p, Polarity::Invert));
        assert_eq!(o.of(p), Polarity::Invert);
        assert!(!o.set(p, Polarity::Invert), "no change");
        assert!(o.set(p, Polarity::Auto));
        assert!(o.pieces.is_empty(), "auto isn't stored");
        assert!(!o.set(p, Polarity::Auto));
    }

    #[test]
    fn the_store_section_round_trips() {
        let mut o = Overrides::default();
        o.set(Path::new(r"C:\art\fox.bin"), Polarity::AsIs);
        o.set(Path::new(r"D:\src\Bandit idle"), Polarity::Invert);
        let text = toml::to_string(&o).unwrap();
        assert!(text.contains("as-is") && text.contains("invert"), "{text}");
        assert_eq!(toml::from_str::<Overrides>(&text).unwrap(), o);
    }
}
