//! "Load a layout…" (#798, B3 of docs/design/device-focus-layouts-thumbnails-lights-declutter.md):
//! for a keyboard whose shape is guessed, the user loads a VIA definition, a QMK keyboard.json /
//! info.json or a KLE file, and the focused keyboard is drawn from it.
//!
//! - The file is read and parsed by `foxglove_via::layout_file`, bounded in size (0.2's VIA
//!   definition cap) and in keys; a bad file says why in one line.
//! - The layout's keys (not the file's path) are kept in the store's own `[layout_files]`
//!   section, by the keyboard's `vvvv:pppp`, with read-modify-write, so the file may move or go
//!   and the layout survives updates.
//! - It goes to `key_layout` on the ladder's "definition" step ([`key_layout::from_file`]), now
//!   and at every start, so it wins over a profile or a guess from the next frame and on every
//!   later connect, without a restart. It also wins over a VIA definition a scan hands over for
//!   the same board (user intent beats auto-detect) until it is forgotten; then the scan's
//!   definition draws the board again.
//! - Only drawn: nothing is sent to the keyboard.
//!
//! The file picker is a trait ([`Picker`]) so tests hand in a fake one: no dialog opens in them.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use foxglove_via::layout_file::{self, Format};
use serde::{Deserialize, Serialize};

use super::key_layout::{self, DefinitionFormat, KeyLayout, LayoutKey, LayoutSource, Supplied};

/// The store section.
const SECTION: &str = "layout_files";

/// The words on the drawing (#798). The link under a guessed drawing, after its muted note.
pub const LOAD: &str = "Load a layout\u{2026}";
pub const LOAD_HOVER: &str = "Draw this keyboard from a layout file: a VIA definition, a QMK keyboard.json / info.json, or a KLE layout (.json). Only the drawing changes; nothing is sent to the keyboard.";
/// Next to the chip of a layout loaded from a file.
pub const FORGET: &str = "Forget this layout";
pub const FORGET_HOVER: &str =
    "Stop drawing this keyboard from the file you loaded. The file itself isn't touched.";
/// The file dialog's title and its one filter.
const DIALOG_TITLE: &str = "Load a layout";
const FILTER: &str = "Layout files: VIA, QMK or KLE (.json)";

/// `[layout_files]`: each keyboard's loaded layout, by `vvvv:pppp`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Saved {
    pub keyboards: BTreeMap<String, Entry>,
}

/// One loaded layout.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Entry {
    /// `via`, `qmk` or `kle`.
    pub format: String,
    /// The layout's name (VIA's board name, QMK's `LAYOUT_*`, KLE's name or the file's).
    pub name: String,
    /// The file's name, for the chip's hover.
    pub file: String,
    /// Each key's `[x, y, w, h, r]` in key units, `r` in degrees about the key's centre.
    pub keys: Vec<[f32; 5]>,
}

fn id(vid: u16, pid: u16) -> String {
    format!("{vid:04x}:{pid:04x}")
}

fn parse_id(s: &str) -> Option<(u16, u16)> {
    let (v, p) = s.split_once(':')?;
    Some((
        u16::from_str_radix(v, 16).ok()?,
        u16::from_str_radix(p, 16).ok()?,
    ))
}

fn format_name(f: Format) -> &'static str {
    match f {
        Format::Via => "via",
        Format::Qmk => "qmk",
        Format::Kle => "kle",
    }
}

fn definition_format(s: &str) -> Option<DefinitionFormat> {
    match s {
        "via" => Some(DefinitionFormat::Via),
        "qmk" => Some(DefinitionFormat::QmkInfo),
        "kle" => Some(DefinitionFormat::Kle),
        _ => None,
    }
}

impl Entry {
    fn from_file(file: &str, loaded: &layout_file::LayoutFile) -> Entry {
        Entry {
            format: format_name(loaded.format).into(),
            name: loaded.layout.name.clone(),
            file: file.into(),
            keys: loaded
                .layout
                .keys
                .iter()
                .map(|k| [k.x, k.y, k.w, k.h, k.r])
                .collect(),
        }
    }

    /// The layout as `key_layout` takes it; `None` for an entry edited out of shape by hand.
    fn supplied(&self) -> Option<Supplied> {
        let format = definition_format(&self.format)?;
        let sane = |k: &&[f32; 5]| k.iter().all(|v| v.is_finite()) && k[2] > 0.0 && k[3] > 0.0;
        if self.keys.is_empty()
            || self.keys.len() > layout_file::MAX_KEYS
            || !self.keys.iter().all(|k| sane(&k))
        {
            return None;
        }
        let keys = self
            .keys
            .iter()
            .enumerate()
            .map(|(i, [x, y, w, h, r])| {
                let rect = egui::Rect::from_min_size(egui::pos2(*x, *y), egui::vec2(*w, *h));
                LayoutKey::new(rect, *r, i)
            })
            .collect();
        let name = if self.file.is_empty() || self.name == file_stem(&self.file) {
            self.name.clone()
        } else {
            format!("{} \u{b7} {}", self.name, self.file)
        };
        Some(Supplied {
            source: LayoutSource::Definition(format),
            layouts: vec![(name, keys)],
            active: 0,
        })
    }
}

fn file_stem(file: &str) -> String {
    Path::new(file)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

// The keyboards with a loaded layout, and their formats: a copy of `[layout_files]`'s keys, so
// the page asks no store each frame.
#[cfg(not(test))]
static LOADED: std::sync::Mutex<BTreeMap<String, DefinitionFormat>> =
    std::sync::Mutex::new(BTreeMap::new());

#[cfg(not(test))]
fn with_loaded<R>(f: impl FnOnce(&mut BTreeMap<String, DefinitionFormat>) -> R) -> R {
    f(&mut LOADED.lock().unwrap_or_else(|e| e.into_inner()))
}

// Tests run side by side, each with its own (as `key_layout`'s handed-over layouts are).
#[cfg(test)]
thread_local! {
    static LOADED: std::cell::RefCell<BTreeMap<String, DefinitionFormat>> =
        const { std::cell::RefCell::new(BTreeMap::new()) };
}

#[cfg(test)]
fn with_loaded<R>(f: impl FnOnce(&mut BTreeMap<String, DefinitionFormat>) -> R) -> R {
    LOADED.with_borrow_mut(f)
}

/// Hands every saved layout to `key_layout`, once at start: each keyboard is drawn from its
/// loaded layout as soon as it connects. A store that can't be read hands nothing over.
pub fn start() {
    match super::store::section(STORE_OWNER, SECTION).load::<Saved>() {
        Ok(saved) => hand_over_all(&saved.unwrap_or_default()),
        Err(e) => eprintln!("Foxglove: can't read the loaded layouts: {e}"),
    }
}

fn hand_over_all(saved: &Saved) {
    for (kb, entry) in &saved.keyboards {
        let (Some((vid, pid)), Some(layout)) = (parse_id(kb), entry.supplied()) else {
            eprintln!("Foxglove: [{SECTION}] {kb}: not a layout; skipped");
            continue;
        };
        let format = definition_format(&entry.format);
        with_loaded(|l| l.insert(kb.clone(), format.unwrap_or(DefinitionFormat::Via)));
        key_layout::from_file(vid, pid, layout);
    }
}

/// Loads the layout file at `path` for keyboard (vid, pid): parsed (bounded), saved in
/// `[layout_files]`, and handed to `key_layout`, so it's drawn from the next frame. `Err` is the
/// one line the page shows.
pub fn load(vid: u16, pid: u16, path: &Path) -> Result<(), String> {
    let file = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "the file".into());
    let one_line = |why: String| {
        let why = why.split_whitespace().collect::<Vec<_>>().join(" ");
        let why: String = why.chars().take(160).collect();
        format!("Couldn't load {file}: {why}.")
    };
    let loaded = layout_file::read(path).map_err(one_line)?;
    let entry = Entry::from_file(&file, &loaded);
    let supplied = entry
        .supplied()
        .ok_or_else(|| one_line("it has no keys Foxglove can draw".into()))?;
    let format = definition_format(&entry.format).unwrap_or(DefinitionFormat::Via);
    super::store::section(STORE_OWNER, SECTION)
        .update::<Saved>(|s| {
            s.keyboards.insert(id(vid, pid), entry);
        })
        .map_err(|e| format!("Couldn't save the layout: {e}."))?;
    with_loaded(|l| l.insert(id(vid, pid), format));
    key_layout::from_file(vid, pid, supplied);
    eprintln!(
        "Foxglove: keyboard {}: layout loaded from {file}",
        id(vid, pid)
    );
    Ok(())
}

/// Forgets keyboard (vid, pid)'s loaded layout: out of `[layout_files]` and withdrawn, so a
/// scanned VIA definition, else the next source down, draws it again.
pub fn forget(vid: u16, pid: u16) -> Result<(), String> {
    super::store::section(STORE_OWNER, SECTION)
        .update::<Saved>(|s| {
            s.keyboards.remove(&id(vid, pid));
        })
        .map_err(|e| format!("Couldn't forget the layout: {e}."))?;
    with_loaded(|l| l.remove(&id(vid, pid)));
    key_layout::forget_file(vid, pid);
    Ok(())
}

/// Whether keyboard (vid, pid) has a layout loaded from a file. From the copy; asks nothing.
pub fn loaded(vid: u16, pid: u16) -> bool {
    with_loaded(|l| l.contains_key(&id(vid, pid)))
}

/// Picks a file. The app's is an in-window dialog showing only `.json` files; tests hand in a
/// fake one, so no dialog ever opens in them.
pub trait Picker {
    /// Opens the picker.
    fn open(&mut self);
    /// Runs it for this frame.
    fn update(&mut self, ctx: &egui::Context);
    /// The file picked since the last call, once.
    fn take(&mut self) -> Option<PathBuf>;
}

struct Dialog(egui_file_dialog::FileDialog);

impl Dialog {
    fn new() -> Dialog {
        Dialog(
            egui_file_dialog::FileDialog::new()
                .title(DIALOG_TITLE)
                .add_file_filter_extensions(FILTER, vec!["json"])
                .default_file_filter(FILTER),
        )
    }
}

impl Picker for Dialog {
    fn open(&mut self) {
        self.0.pick_file();
    }
    fn update(&mut self, ctx: &egui::Context) {
        self.0.update(ctx);
    }
    fn take(&mut self) -> Option<PathBuf> {
        self.0.take_picked()
    }
}

#[derive(Default)]
struct UiState {
    /// Made the first time "Load a layout…" is clicked.
    picker: Option<Box<dyn Picker>>,
    /// The keyboard the open picker loads for: the one focused when it was opened.
    target: Option<(u16, u16)>,
    /// The last load's problem, for that keyboard.
    message: Option<((u16, u16), String)>,
}

thread_local! { static UI: std::cell::RefCell<UiState> = std::cell::RefCell::new(UiState::default()); }

/// Hands in the picker the next "Load a layout…" opens (tests' fake one).
#[cfg(test)]
pub fn set_picker(picker: Box<dyn Picker>) {
    UI.with_borrow_mut(|s| {
        *s = UiState {
            picker: Some(picker),
            ..UiState::default()
        }
    });
}

/// The guessed drawing's muted note and its "Load a layout…" link, and the last load's problem
/// in one line. `target` is the keyboard a loaded layout is for (the focused one); without one
/// there's no link. Runs the picker, and loads what it picked.
pub fn note_row(ui: &mut egui::Ui, layout: &KeyLayout, target: Option<(u16, u16)>) {
    UI.with_borrow_mut(|state| {
        if let Some(p) = state.picker.as_mut() {
            p.update(ui.ctx());
            if let (Some(path), Some((vid, pid))) = (p.take(), state.target) {
                state.message = load(vid, pid, &path).err().map(|m| ((vid, pid), m));
            }
        }
        if let Some(note) = layout.source.note() {
            ui.horizontal_wrapped(|ui| {
                ui.weak(note);
                if let Some(t) = target {
                    let r = ui.small_button(LOAD).on_hover_text(LOAD_HOVER);
                    #[cfg(test)]
                    LAST_LOAD.set(Some(r.rect));
                    if r.clicked() {
                        state.target = Some(t);
                        state.message = None;
                        state
                            .picker
                            .get_or_insert_with(|| Box::new(Dialog::new()))
                            .open();
                    }
                }
            });
        }
        if let Some((_, m)) = state.message.as_ref().filter(|(k, _)| Some(*k) == target) {
            ui.colored_label(ui.visuals().error_fg_color, m);
        }
    });
}

/// "Forget this layout", next to the chip of a layout loaded from a file for `target`.
pub fn forget_link(ui: &mut egui::Ui, layout: &KeyLayout, target: Option<(u16, u16)>) {
    let Some((vid, pid)) = target else {
        return;
    };
    if !matches!(layout.source, LayoutSource::Definition(_)) || !loaded(vid, pid) {
        return;
    }
    let r = ui.small_button(FORGET).on_hover_text(FORGET_HOVER);
    #[cfg(test)]
    LAST_FORGET.set(Some(r.rect));
    if r.clicked() {
        let m = forget(vid, pid).err().map(|m| ((vid, pid), m));
        UI.with_borrow_mut(|s| s.message = m);
    }
}

#[cfg(test)]
thread_local! {
    /// Where the last frame drew the links, for the click tests.
    pub static LAST_LOAD: std::cell::Cell<Option<egui::Rect>> = const { std::cell::Cell::new(None) };
    pub static LAST_FORGET: std::cell::Cell<Option<egui::Rect>> = const { std::cell::Cell::new(None) };
}

/// Shows `m` as the last load's problem for `target`, for the previews.
#[cfg(test)]
pub fn show_problem(target: (u16, u16), m: &str) {
    UI.with_borrow_mut(|s| s.message = Some((target, m.to_string())));
}

/// The message the page shows for `target`, for the tests.
#[cfg(test)]
pub fn message_for(target: (u16, u16)) -> Option<String> {
    UI.with_borrow(|s| {
        s.message
            .as_ref()
            .filter(|(k, _)| *k == target)
            .map(|(_, m)| m.clone())
    })
}

#[cfg(test)]
#[path = "layout_files_tests.rs"]
mod tests;

/// The feature that owns this file's store sections.
const STORE_OWNER: &str = "foxglove.keymap";
