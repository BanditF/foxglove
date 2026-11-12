//! Other keyboards over VIA (#74), read-only, off by default: QMK keyboards found by the VIA
//! JSON definitions the user loads, and, with `generic` on (also off by default), any VIA or
//! Vial board on Raw HID. A Vial board gives its own definition; another board without one gets
//! lighting but no keymap. They join the device layer (facts,
//! the local API, plugins) next to the host-link keyboard. Nothing is written to them: the VIA
//! backend opens devices read-only, and reset/bootloader/factory commands never go out.
//! VIA/Vial writes stay off in 0.2 (#581, the `via-writes` feature), whatever a caller asks.
//!
//! Settings in the store's `[via]`; scanning happens on demand (Refresh, turning it on, adding a
//! definition), never on a timer.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex;

use foxglove_devices::{Device, Registry};
use foxglove_via::hid::ViaBackend;
use foxglove_via::vial::Unreadable;
use foxglove_via::{Definition, Source, Summary};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ViaSettings {
    pub enabled: bool,
    /// Also find VIA and Vial boards without a loaded definition (`ViaBackend::generic`). Off by
    /// default: opening one sends it VIA's protocol-version read, and a Vial board's definition
    /// reads.
    pub generic: bool,
    /// VIA JSON definitions (Keychron publishes them per keyboard).
    pub definitions: Vec<PathBuf>,
    /// Keyboards Foxglove leaves alone (by [`Known::id`]; `vvvv:pppp` from before #292 still
    /// counts), so VIA or the maker's app can use them (#74): their handles are closed and they
    /// aren't opened again until resumed.
    pub paused: Vec<String>,
    /// Keyboards seen before, so one that comes back on another port can be recognised (#292).
    pub known: Vec<Known>,
    /// A keyboard's id on a new port to the id it was first known by, after the user said
    /// "This is the same keyboard": its saved state (pause) follows it.
    pub aliases: BTreeMap<String, String>,
}

/// A keyboard Foxglove has seen.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Known {
    /// Its device id when first seen.
    pub id: String,
    pub name: String,
    /// `vvvv:pppp`: only a keyboard of the same model can be the same keyboard.
    pub model: String,
}

/// What a scan makes of one keyboard.
#[derive(Clone, Debug, PartialEq)]
pub struct Seen {
    /// The id its saved state is under: its own, or the one it was confirmed to be.
    pub id: String,
    /// Not known, but a known keyboard of the same model is missing: maybe it moved. The user
    /// is asked; until then it's treated as new but not remembered.
    pub maybe: Option<Known>,
    /// Known now for the first time (remembered by the scan).
    pub new: bool,
}

/// Recognises the keyboards present (`(id, model)`) against the saved ones. Pure, for the scan
/// and the tests.
pub fn recognise(
    present: &[(String, String)],
    known: &[Known],
    aliases: &BTreeMap<String, String>,
) -> Vec<Seen> {
    let canonical = |id: &str| aliases.get(id).cloned().unwrap_or_else(|| id.to_string());
    let present_ids: Vec<String> = present.iter().map(|(id, _)| canonical(id)).collect();
    present
        .iter()
        .map(|(id, model)| {
            let id = canonical(id);
            if known.iter().any(|k| k.id == id) {
                return Seen {
                    id,
                    maybe: None,
                    new: false,
                };
            }
            let missing = known
                .iter()
                .find(|k| &k.model == model && !present_ids.contains(&k.id))
                .cloned();
            Seen {
                new: missing.is_none(),
                maybe: missing,
                id,
            }
        })
        .collect()
}

pub fn settings() -> ViaSettings {
    super::store::section(STORE_OWNER, "via")
        .try_load()
        .unwrap_or_default()
}

pub fn save(s: &ViaSettings) -> Result<(), String> {
    super::store::section(STORE_OWNER, "via")
        .save(s)
        .map_err(|e| e.to_string())?;
    rescan();
    Ok(())
}

static REGISTRY: Mutex<Option<Registry>> = Mutex::new(None);
static STATUS: Mutex<Vec<Found>> = Mutex::new(Vec::new());
/// The boards whose layouts the last scan handed to `key_layout`, and from which step, so the
/// next scan can withdraw the ones that are gone.
static HANDED: Mutex<Vec<((u16, u16), super::key_layout::LayoutSource)>> = Mutex::new(Vec::new());

/// An open board's layout as `key_layout` takes it (#797): from its definition, laid out for the
/// layout options the board reported when it was opened. A loaded VIA definition is the
/// ladder's "definition" step, a loaded QMK layout too; a Vial board's own definition is the
/// keyboard's. None without a definition. Pure: it reads only what the open already read.
pub fn layout_of(
    summary: &Summary,
    layout: &foxglove_devices::Layout,
) -> Option<super::key_layout::Supplied> {
    use super::key_layout::{DefinitionFormat, LayoutKey, LayoutSource, Supplied};
    let (source, name) = match &summary.source {
        Source::Loaded => (
            LayoutSource::Definition(DefinitionFormat::Via),
            layout.name.clone(),
        ),
        Source::QmkLayout { layout } => (
            LayoutSource::Definition(DefinitionFormat::QmkInfo),
            layout.clone(),
        ),
        Source::Vial { .. } => (LayoutSource::Keyboard, layout.name.clone()),
        Source::Missing { .. } => return None,
    };
    if layout.keys.is_empty() {
        return None;
    }
    let name = if summary.layout_options.is_empty() {
        name
    } else {
        format!("{name} \u{b7} {}", summary.layout_options.join(", "))
    };
    let keys = layout
        .keys
        .iter()
        .enumerate()
        .map(|(i, k)| {
            let rect = egui::Rect::from_min_size(egui::pos2(k.x, k.y), egui::vec2(k.w, k.h));
            LayoutKey::new(rect, k.r, i)
        })
        .collect();
    Some(Supplied {
        source,
        layouts: vec![(name, keys)],
        active: 0,
    })
}

/// Hands the open boards' layouts (`handed`) to `key_layout` and withdraws the ones the last
/// scan handed over that are gone. Called once per scan, the only time a board's definition
/// and layout options are read; never on a timer.
fn hand_layouts(handed: Vec<((u16, u16), super::key_layout::Supplied)>) {
    let mut last = HANDED.lock().unwrap_or_else(|e| e.into_inner());
    let now: Vec<_> = handed.iter().map(|(id, l)| (*id, l.source)).collect();
    for (id, source) in last.iter().filter(|old| !now.contains(old)) {
        super::key_layout::withdraw(id.0, id.1, *source);
    }
    for ((vid, pid), layout) in handed {
        super::key_layout::from_definition(vid, pid, layout);
    }
    *last = now;
}

/// A keyboard found on the last scan, for the settings.
#[derive(Clone, Debug)]
pub struct Found {
    pub name: String,
    /// What it turned out to be when it answered VIA, else why not.
    pub opened: Result<Summary, String>,
    /// The id its saved state is under, for pausing it; none for a definition or scan error.
    pub key: Option<String>,
    pub paused: bool,
    /// Its id on this port, and the known keyboard it might be (asked in the settings).
    pub maybe: Option<(String, Known)>,
}

/// A keyboard's model: its USB vendor and product ids.
pub fn model(id: &foxglove_devices::DeviceId) -> String {
    format!("{:04x}:{:04x}", id.vid, id.pid)
}

/// The user's answer: the keyboard on this port is `known` (its saved state follows it), or
/// a different keyboard (remembered as itself).
pub fn same_keyboard(
    id: &str,
    name: &str,
    model: &str,
    same_as: Option<&Known>,
) -> Result<(), String> {
    let mut s = settings();
    match same_as {
        Some(k) => {
            s.aliases.insert(id.to_string(), k.id.clone());
        }
        None => s.known.push(Known {
            id: id.to_string(),
            name: name.to_string(),
            model: model.to_string(),
        }),
    }
    save(&s)
}

/// Pauses or resumes Foxglove for one keyboard; a rescan closes or reopens its handle.
pub fn set_paused(key: &str, paused: bool) -> Result<(), String> {
    let mut s = settings();
    s.paused.retain(|k| k != key);
    if paused {
        s.paused.push(key.to_string());
    }
    save(&s)
}

pub fn found() -> Vec<Found> {
    STATUS.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Loads the definitions, VIA JSON or a QMK keyboard.json / info.json (told apart by their
/// contents); returns the ones that parse and the errors for the rest.
fn definitions(s: &ViaSettings) -> (Vec<Definition>, Vec<String>) {
    let mut ok = Vec::new();
    let mut errors = Vec::new();
    for path in &s.definitions {
        match Definition::load(path) {
            Ok(d) => ok.push(d),
            Err(e) => errors.push(format!("{}: {e}", path.display())),
        }
    }
    (ok, errors)
}

/// What a scan does with one keyboard it found.
#[derive(Clone, Debug, PartialEq)]
pub struct Planned {
    /// Its device id on this port.
    pub own: String,
    /// The id its saved state (pause) is under: its own, or the one it was confirmed to be.
    pub key: String,
    /// Listed but never opened, so another app has it to itself.
    pub paused: bool,
    /// The known keyboard it might be (asked in the settings).
    pub maybe: Option<Known>,
}

/// Decides, for the keyboards found (`(id, model, name)`), what a scan does: the key their state
/// is under, whether they're paused, the known keyboard each might be, and the new ones to
/// remember. Pure, for the scan and the tests.
pub fn plan(found: &[(String, String, String)], s: &ViaSettings) -> (Vec<Planned>, Vec<Known>) {
    let present: Vec<(String, String)> = found
        .iter()
        .map(|(id, model, _)| (id.clone(), model.clone()))
        .collect();
    let mut learned = Vec::new();
    let planned = found
        .iter()
        .zip(recognise(&present, &s.known, &s.aliases))
        .map(|((own, model, name), seen)| {
            if seen.new {
                learned.push(Known {
                    id: own.clone(),
                    name: name.clone(),
                    model: model.clone(),
                });
            }
            let paused = s.paused.contains(&seen.id) || s.paused.contains(model);
            Planned {
                own: own.clone(),
                key: seen.id,
                paused,
                maybe: seen.maybe,
            }
        })
        .collect();
    (planned, learned)
}

/// Finds and opens (read-only) the VIA keyboards matching the definitions; closes them all
/// when turned off. Runs on a background thread.
pub fn rescan() {
    super::services::spawn_named("via-scan", || {
        let s = settings();
        let mut guard = REGISTRY.lock().unwrap_or_else(|e| e.into_inner());
        *guard = None; // closes what was open
        if !s.enabled {
            STATUS.lock().unwrap_or_else(|e| e.into_inner()).clear();
            drop(guard);
            hand_layouts(Vec::new());
            super::keyboard_focus::report(super::keyboard_focus::Source::Via, Vec::new());
            return;
        }
        let (defs, errors) = definitions(&s);
        let mut registry = Registry::new(super::devices::bus().clone());
        let mut backend = ViaBackend::new(defs);
        if s.generic {
            backend = backend.generic();
        }
        // #723: only the hidden dev switch (a via-writes build with FOXGLOVE_VIA_WRITES=1) opens
        // boards for writes; no setting does.
        if foxglove_via::writes_enabled() {
            backend = backend.allow_writes();
        }
        let summaries = backend.opened();
        let _ = registry.add_backend(Box::new(backend));
        let report = registry.discover();
        let mut found: Vec<Found> = errors
            .into_iter()
            .map(|e| Found {
                name: "definition".into(),
                opened: Err(e),
                key: None,
                paused: false,
                maybe: None,
            })
            .collect();
        found.extend(report.errors.iter().map(|(_, e)| Found {
            name: "scan".into(),
            opened: Err(e.to_string()),
            key: None,
            paused: false,
            maybe: None,
        }));
        let present: Vec<(String, String, String)> = report
            .devices
            .iter()
            .map(|e| {
                (
                    e.info.id.to_string(),
                    model(&e.info.id),
                    e.info.name.clone(),
                )
            })
            .collect();
        let (planned, learned) = plan(&present, &s);
        let mut layouts = Vec::new();
        // The keyboards it opened, for the focus (#799): read-only, but they can be focused.
        let mut seen = Vec::new();
        for (entry, p) in report.devices.iter().zip(planned) {
            let maybe = p.maybe.map(|k| (p.own, k));
            let key = p.key;
            // Paused: listed, never opened, so another app has it to itself.
            if p.paused {
                found.push(Found {
                    name: entry.info.name.clone(),
                    opened: Err("paused".into()),
                    key: Some(key),
                    paused: true,
                    maybe,
                });
                continue;
            }
            let opened = registry
                .open(&entry.info.id)
                .map(|d| {
                    let summary = summaries.get(&entry.info.id).unwrap_or_else(|| Summary {
                        firmware: d.info().firmware.clone(),
                        lighting: None,
                        lighting_asked: true,
                        source: Source::Missing { why: None },
                        layers: None,
                        layout_options: Vec::new(),
                    });
                    // The layout the open already worked out; nothing more is asked.
                    let layout = d.keymap().and_then(|k| k.layout().ok());
                    if let Some(l) = layout.and_then(|l| layout_of(&summary, &l)) {
                        layouts.push(((entry.info.id.vid, entry.info.id.pid), l));
                    }
                    summary
                })
                .map_err(|e| super::devices::describe(&e));
            if opened.is_ok() {
                seen.push(super::keyboard_focus::Seen {
                    key: key.clone(),
                    name: entry.info.name.clone(),
                    vid: entry.info.id.vid,
                    pid: entry.info.id.pid,
                    device: Some(entry.info.id.to_string()),
                });
            }
            if let Ok(Summary {
                source: Source::Missing { why: Some(why) },
                ..
            }) = &opened
            {
                eprintln!(
                    "Foxglove: {}: its Vial definition couldn't be read: {why}",
                    entry.info.name
                );
            }
            found.push(Found {
                name: entry.info.name.clone(),
                opened,
                key: Some(key),
                paused: false,
                maybe,
            });
        }
        if !learned.is_empty() {
            let mut s = settings();
            s.known.extend(learned);
            if let Err(e) = super::store::section(STORE_OWNER, "via").save(&s) {
                eprintln!("Foxglove: remembering VIA keyboards: {e}");
            }
        }
        *STATUS.lock().unwrap_or_else(|e| e.into_inner()) = found;
        *guard = Some(registry);
        // Handed over after the registry is released: a new layout rebuilds the Scenes room,
        // which may look at the open boards.
        drop(guard);
        hand_layouts(layouts);
        super::keyboard_focus::report(super::keyboard_focus::Source::Via, seen);
    });
}

/// Runs `f` on an open VIA keyboard by its id.
pub fn with_device<R>(id: &str, f: impl FnOnce(&dyn Device) -> R) -> Option<R> {
    let guard = REGISTRY.lock().unwrap_or_else(|e| e.into_inner());
    let registry = guard.as_ref()?;
    let entry = registry
        .list()
        .into_iter()
        .find(|e| e.info.id.to_string() == id)?;
    registry.device(&entry.info.id).map(f)
}

/// The open VIA keyboards, for `devices.list`.
pub fn devices() -> Vec<(foxglove_devices::DeviceInfo, foxglove_devices::Caps)> {
    let guard = REGISTRY.lock().unwrap_or_else(|e| e.into_inner());
    let Some(registry) = guard.as_ref() else {
        return vec![];
    };
    registry
        .list()
        .into_iter()
        .filter_map(|e| {
            registry
                .device(&e.info.id)
                .map(|d| (e.info.clone(), d.caps()))
        })
        .collect()
}

/// A board's lighting channel name; else that none answered, or, for a model the catalog says
/// has none (never asked), that it has none.
pub fn lighting_text(s: &Summary) -> &'static str {
    match (s.lighting, s.lighting_asked) {
        (Some(c), _) => c.label(),
        (None, true) => "none answered",
        (None, false) => "none (this model has none)",
    }
}

/// Where a board's definition came from, as the settings say it: a read failure in plain
/// words ([`Unreadable::plain`]).
pub fn source_text(s: &Source) -> String {
    match s {
        Source::Missing { why: Some(why) } => format!(
            "the keyboard's own couldn't be read ({}), so no keymap",
            Unreadable::of(why).plain()
        ),
        Source::QmkLayout { .. } => "from a loaded QMK layout".into(),
        s => source_detail(s),
    }
}

/// [`source_text`] with a read failure's technical reason, for the health view and the log.
pub fn source_detail(s: &Source) -> String {
    match s {
        Source::Loaded => "from a loaded file".into(),
        Source::QmkLayout { layout } => format!("from a loaded QMK layout ({layout})"),
        Source::Vial { .. } => "read from the keyboard (Vial)".into(),
        Source::Missing { why: None } => "none, so no keymap: add its VIA definition".into(),
        Source::Missing { why: Some(why) } => {
            format!("the keyboard's own couldn't be read: {why}")
        }
    }
}

/// One line on what an open board is: firmware, lighting channel, definition source.
pub fn summary_line(s: &Summary) -> String {
    format!(
        "Firmware: {} · Lighting: {} · Definition: {}",
        s.firmware.as_deref().unwrap_or("unknown"),
        lighting_text(s),
        source_text(&s.source)
    )
}

/// The health view's row per open VIA board (#74): where its definition came from. A loaded or
/// Vial definition is OK; none is a warning (lighting only, no keymap); a Vial definition that
/// couldn't be read is a failure. Paused and failed opens have no row.
pub fn board_rows(found: &[Found]) -> Vec<super::health::HealthRow> {
    use super::health::{HealthRow, Level};
    found
        .iter()
        .filter(|f| !f.paused)
        .filter_map(|f| {
            let s = f.opened.as_ref().ok()?;
            let level = match &s.source {
                Source::Loaded | Source::QmkLayout { .. } | Source::Vial { .. } => Level::Ok,
                Source::Missing { why: None } => Level::Warn,
                Source::Missing { why: Some(_) } => Level::Fail,
            };
            Some(HealthRow::new(
                "via",
                format!("{} · Definition", f.name),
                level,
                format!(
                    "{}; {}; lighting: {}",
                    source_detail(&s.source),
                    s.firmware.as_deref().unwrap_or("firmware unknown"),
                    lighting_text(s)
                ),
            ))
        })
        .collect()
}

/// The health view's rows for VIA: writes (#581), off in 0.2 (the `via-writes` feature), so
/// "VIA/Vial writes: off (0.2)", then one per open board ([`board_rows`]). Registered with
/// `health::register` as "via_writes".
pub fn health_rows() -> Vec<super::health::HealthRow> {
    let mut rows = vec![writes_row(foxglove_via::writes_enabled())];
    rows.extend(board_rows(&found()));
    rows
}

/// "VIA/Vial writes: off (0.2)", or a warning while the hidden dev switch has them on (#723).
fn writes_row(on: bool) -> super::health::HealthRow {
    use super::health::{HealthRow, Level};
    if on {
        HealthRow::new(
            "via",
            "VIA/Vial writes",
            Level::Warn,
            "ON: dev test switch FOXGLOVE_VIA_WRITES=1 (#723), not for daily use",
        )
    } else {
        HealthRow::new("via", "VIA/Vial writes", Level::Ok, "off (0.2)")
    }
}

/// Opens the configured VIA keyboards at start-up, if turned on. Asks the write switch first,
/// so the log says at start-up when the dev switch has writes on (#723).
pub fn start() {
    foxglove_via::writes_enabled();
    if settings().enabled && !super::repaint_trace::disabled("via") {
        rescan();
    }
}

/// The feature that owns this file's store sections.
const STORE_OWNER: &str = "qmk.via";

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use foxglove_via::client::Channel;

    fn known(id: &str) -> Known {
        Known {
            id: id.into(),
            name: "K5 Pro".into(),
            model: "3434:0250".into(),
        }
    }

    #[test]
    fn a_keyboard_on_a_new_port_is_asked_about_and_an_answer_sticks() {
        let model = "3434:0250".to_string();
        let mut aliases = BTreeMap::new();
        // First time: new, remembered.
        let seen = recognise(&[("port-a".into(), model.clone())], &[], &aliases);
        assert_eq!(
            seen,
            [Seen {
                id: "port-a".into(),
                maybe: None,
                new: true
            }]
        );
        let k = vec![known("port-a")];
        // Back on its port: known.
        assert_eq!(
            recognise(&[("port-a".into(), model.clone())], &k, &aliases)[0],
            Seen {
                id: "port-a".into(),
                maybe: None,
                new: false
            }
        );
        // On another port while port-a is away: maybe the same one.
        let moved = recognise(&[("port-b".into(), model.clone())], &k, &aliases);
        assert_eq!(
            moved[0].maybe.as_ref().map(|k| k.id.as_str()),
            Some("port-a")
        );
        assert!(!moved[0].new, "not remembered until the user answers");
        // "This is the same keyboard": its state is under port-a's id from now on.
        aliases.insert("port-b".into(), "port-a".into());
        assert_eq!(
            recognise(&[("port-b".into(), model.clone())], &k, &aliases)[0],
            Seen {
                id: "port-a".into(),
                maybe: None,
                new: false
            }
        );
        // Two of the same model at once: the one on a new port is a different keyboard.
        let both = recognise(
            &[
                ("port-a".into(), model.clone()),
                ("port-c".into(), model.clone()),
            ],
            &k,
            &BTreeMap::new(),
        );
        assert_eq!(
            both[1],
            Seen {
                id: "port-c".into(),
                maybe: None,
                new: true
            }
        );
        // Another model is never asked about.
        assert!(recognise(&[("x".into(), "1234:0001".into())], &k, &BTreeMap::new())[0].new);
    }

    /// Two identical boards with no usable serial (#292), as the VIA backend names them: by
    /// where they're plugged in (`usb:<container id>@<location path>`).
    fn twin(port: u8, serial: Option<&str>) -> (String, String, String) {
        let path = format!(
            "usb:3c2e5d1f9a8b4c6d8e7f0a1b2c3d4e5f@PCIROOT(0)#PCI(1400)#USBROOT(0)#USB({port})"
        );
        let id = foxglove_devices::DeviceId::new(
            foxglove_devices::BackendId("via".into()),
            0x3434,
            0x0250,
            serial,
            &path,
        );
        (id.to_string(), model(&id), "K5 Pro".into())
    }

    #[test]
    fn twin_boards_without_a_serial_are_two_keyboards_each_with_its_own_pause() {
        // No serial, and the placeholder some firmware sends: both fall back to the location.
        let (a, b) = (twin(1, None), twin(2, Some("000000")));
        assert_ne!(a.0, b.0, "told apart by the port");
        assert_eq!(
            a.0,
            twin(1, Some("0")).0,
            "an untrusted serial doesn't change the id"
        );
        let mut s = ViaSettings::default();
        let (planned, learned) = plan(&[a.clone(), b.clone()], &s);
        assert_eq!(learned.len(), 2, "both remembered");
        assert!(planned
            .iter()
            .all(|p| !p.paused && p.maybe.is_none() && p.key == p.own));
        s.known = learned;
        // Pausing one leaves its twin alone.
        s.paused.push(a.0.clone());
        let (planned, learned) = plan(&[a.clone(), b.clone()], &s);
        assert!(learned.is_empty());
        assert_eq!(
            planned.iter().map(|p| p.paused).collect::<Vec<_>>(),
            [true, false]
        );
        // A model-wide pause from before #292 still covers both.
        let legacy = ViaSettings {
            paused: vec!["3434:0250".into()],
            ..s.clone()
        };
        assert!(plan(&[a, b], &legacy).0.iter().all(|p| p.paused));
    }

    #[test]
    fn a_twin_on_another_port_is_asked_about_and_this_is_the_same_keyboard_carries_its_pause() {
        let (a, b, moved) = (twin(1, None), twin(2, None), twin(3, None));
        let known = |t: &(String, String, String)| Known {
            id: t.0.clone(),
            name: t.2.clone(),
            model: t.1.clone(),
        };
        let mut s = ViaSettings {
            known: vec![known(&a), known(&b)],
            paused: vec![a.0.clone()],
            ..Default::default()
        };
        // A moved to port 3 while B stays: maybe A, not remembered, and not paused yet.
        let (planned, learned) = plan(&[moved.clone(), b.clone()], &s);
        assert!(learned.is_empty(), "not remembered until the user answers");
        assert_eq!(
            planned[0].maybe.as_ref().map(|k| k.id.clone()),
            Some(a.0.clone())
        );
        assert!(!planned[0].paused && planned[1].maybe.is_none() && !planned[1].paused);
        // "This is the same keyboard": its state (the pause) follows it to port 3.
        s.aliases.insert(moved.0.clone(), a.0.clone());
        let (planned, _) = plan(&[moved.clone(), b.clone()], &s);
        assert_eq!(
            (planned[0].key.as_str(), planned[0].paused),
            (a.0.as_str(), true)
        );
        // "A different keyboard" instead: remembered as itself, its own (unpaused) state.
        let s = ViaSettings {
            known: vec![known(&a), known(&b), known(&moved)],
            paused: vec![a.0.clone()],
            ..Default::default()
        };
        let (planned, _) = plan(&[moved.clone(), b], &s);
        assert_eq!(
            (
                planned[0].key.as_str(),
                planned[0].paused,
                planned[0].maybe.is_none()
            ),
            (moved.0.as_str(), false, true)
        );
    }

    #[test]
    fn pause_and_same_keyboard_answers_are_saved_in_the_settings_store() {
        // A temp settings folder (FOXGLOVE_CONFIG_DIR), never the real one; VIA stays off, so
        // the rescan these trigger opens nothing.
        let dir = super::super::store::test_config_dir();
        let (a, moved) = (twin(1, None), twin(3, None));
        let known_a = Known {
            id: a.0.clone(),
            name: a.2.clone(),
            model: a.1.clone(),
        };
        super::super::store::section(STORE_OWNER, "via")
            .save(&ViaSettings {
                known: vec![known_a.clone()],
                ..Default::default()
            })
            .unwrap();
        set_paused(&a.0, true).unwrap();
        same_keyboard(&moved.0, &moved.2, &moved.1, Some(&known_a)).unwrap();
        let s = settings();
        assert_eq!(s.paused, std::slice::from_ref(&a.0));
        assert_eq!(s.aliases.get(&moved.0), Some(&a.0));
        assert!(
            std::fs::read_to_string(dir.join("foxglove.toml"))
                .unwrap()
                .contains("[via"),
            "in the temp store"
        );
        assert!(
            plan(&[moved], &s).0[0].paused,
            "the pause follows the answer"
        );
        set_paused(&a.0, false).unwrap();
        assert!(settings().paused.is_empty(), "resumed");
    }

    #[cfg(not(feature = "via-writes"))]
    #[test]
    fn the_health_view_says_via_writes_are_off() {
        let rows = health_rows();
        assert_eq!(rows[0].line(), "VIA/Vial writes: off (0.2)");
        assert_eq!(rows[0].level, super::super::health::Level::Ok);
    }

    /// #723: with the dev switch on, the health view warns.
    #[test]
    fn the_health_view_warns_while_the_dev_switch_has_writes_on() {
        let row = writes_row(true);
        assert_eq!(row.level, super::super::health::Level::Warn);
        assert!(
            row.line().contains("FOXGLOVE_VIA_WRITES=1"),
            "{}",
            row.line()
        );
        assert_eq!(writes_row(false).line(), "VIA/Vial writes: off (0.2)");
    }

    /// #723: writes can't be turned on from the settings: `[via]` has no writes key, and one
    /// written into the store by hand is ignored (dropped on the next save).
    #[test]
    fn no_setting_turns_writes_on() {
        let s: ViaSettings = toml::from_str("enabled = true\nwrites = true\n").unwrap();
        assert!(s.enabled);
        let saved = toml::to_string(&s).unwrap();
        assert!(!saved.contains("writes"), "{saved}");
    }

    /// Boards a generic scan finds (#74), one per definition source, for the tests and the
    /// settings preview: a Vial board, a board with a loaded file, a VIA board with none, a Vial
    /// board whose definition doesn't unpack, and a paused one.
    pub(crate) fn sample_found() -> Vec<Found> {
        let board = |name: &str, firmware: &str, lighting, source, layers| Found {
            name: name.into(),
            opened: Ok(Summary {
                firmware: Some(firmware.into()),
                lighting,
                lighting_asked: true,
                source,
                layers,
                layout_options: vec![],
            }),
            key: Some(format!("via:{name}")),
            paused: false,
            maybe: None,
        };
        vec![
            board(
                "Vial macropad",
                "VIA protocol 9, Vial protocol 6",
                Some(Channel::RgbMatrix),
                Source::Vial { protocol: 6 },
                Some(4),
            ),
            board(
                "QMK keyboard",
                "VIA protocol 12",
                Some(Channel::RgbMatrix),
                Source::Loaded,
                Some(4),
            ),
            board(
                "VIA numpad",
                "VIA protocol 12",
                Some(Channel::Backlight),
                Source::Missing { why: None },
                None,
            ),
            board(
                "Vial split",
                "VIA protocol 9, Vial protocol 6",
                None,
                Source::Missing {
                    why: Some(
                        "the Vial definition doesn't unpack: xz: invalid check (CRC64)".into(),
                    ),
                },
                None,
            ),
            Found {
                name: "Paused board".into(),
                opened: Err("paused".into()),
                key: Some("via:paused".into()),
                paused: true,
                maybe: None,
            },
        ]
    }

    #[test]
    fn each_board_says_its_firmware_lighting_and_definition_source() {
        let found = sample_found();
        let line = |i: usize| summary_line(found[i].opened.as_ref().unwrap());
        assert_eq!(
            line(0),
            "Firmware: VIA protocol 9, Vial protocol 6 · Lighting: RGB Matrix · Definition: read \
             from the keyboard (Vial)"
        );
        assert_eq!(
            line(1),
            "Firmware: VIA protocol 12 · Lighting: RGB Matrix · Definition: from a loaded file"
        );
        assert_eq!(
            line(2),
            "Firmware: VIA protocol 12 · Lighting: Backlight · Definition: none, so no keymap: \
             add its VIA definition"
        );
        assert_eq!(
            line(3),
            "Firmware: VIA protocol 9, Vial protocol 6 · Lighting: none answered · Definition: \
             the keyboard's own couldn't be read (its stored data was damaged), so no keymap"
        );
    }

    /// A model the catalog says has no lighting (`lighting = "none"`, never asked) says so in
    /// both the settings line and the health row; one that was asked keeps "none answered".
    #[test]
    fn a_model_without_lighting_says_so_instead_of_none_answered() {
        let board = |lighting_asked| Found {
            name: "Keychron K5 Pro (ANSI, white)".into(),
            opened: Ok(Summary {
                firmware: Some("VIA protocol 11".into()),
                lighting: None,
                lighting_asked,
                source: Source::Loaded,
                layers: Some(4),
                layout_options: vec![],
            }),
            key: Some("via:k5".into()),
            paused: false,
            maybe: None,
        };
        let never = board(false);
        assert_eq!(
            summary_line(never.opened.as_ref().unwrap()),
            "Firmware: VIA protocol 11 · Lighting: none (this model has none) · Definition: from a loaded file"
        );
        assert!(board_rows(&[never])[0]
            .line()
            .ends_with("; VIA protocol 11; lighting: none (this model has none)"));
        let asked = board(true);
        assert!(summary_line(asked.opened.as_ref().unwrap()).contains("Lighting: none answered"));
        assert!(board_rows(&[asked])[0]
            .line()
            .ends_with("; lighting: none answered"));
    }

    /// A QMK keyboard.json in the definitions list loads like a VIA file (#74) and the board
    /// says where its layout came from: the settings line plainly, the health row with the
    /// layout's name.
    #[test]
    fn a_qmk_keyboard_json_loads_and_says_so() {
        let dir = std::env::temp_dir().join(format!("foxglove-via-qmk-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("keyboard.json");
        std::fs::write(
            &path,
            r#"{"keyboard_name": "Fox 2", "usb": {"vid": "0xFEED", "pid": "0x0002"},
                "matrix_size": {"rows": 1, "cols": 2},
                "layouts": {"LAYOUT": {"layout": [
                    {"matrix": [0, 0], "x": 0, "y": 0}, {"matrix": [0, 1], "x": 1, "y": 0}]}}}"#,
        )
        .unwrap();
        let s = ViaSettings {
            definitions: vec![path],
            ..Default::default()
        };
        let (defs, errors) = definitions(&s);
        let _ = std::fs::remove_dir_all(&dir);
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(
            (defs[0].name.as_str(), defs[0].qmk_layout.as_deref()),
            ("Fox 2", Some("LAYOUT"))
        );
        let source = Source::QmkLayout {
            layout: "LAYOUT".into(),
        };
        let found = Found {
            name: "Fox 2".into(),
            opened: Ok(Summary {
                firmware: Some("VIA protocol 11".into()),
                lighting: None,
                lighting_asked: true,
                source: source.clone(),
                layers: Some(4),
                layout_options: vec![],
            }),
            key: Some("via:fox2".into()),
            paused: false,
            maybe: None,
        };
        assert!(summary_line(found.opened.as_ref().unwrap())
            .ends_with("· Definition: from a loaded QMK layout"));
        let rows = board_rows(&[found]);
        assert_eq!(rows[0].level, super::super::health::Level::Ok);
        assert!(rows[0].line().contains("from a loaded QMK layout (LAYOUT)"));
    }

    /// The definition source is a health signal: a row per open board, none for a paused one.
    #[test]
    fn the_definition_source_is_a_health_row_per_open_board() {
        use super::super::health::Level;
        let rows = board_rows(&sample_found());
        let got: Vec<(&str, Level)> = rows.iter().map(|r| (r.item.as_str(), r.level)).collect();
        assert_eq!(
            got,
            [
                ("Vial macropad · Definition", Level::Ok),
                ("QMK keyboard · Definition", Level::Ok),
                ("VIA numpad · Definition", Level::Warn),
                ("Vial split · Definition", Level::Fail),
            ]
        );
        assert!(rows.iter().all(|r| r.subsystem == "via"));
        assert_eq!(
            rows[0].detail,
            "read from the keyboard (Vial); VIA protocol 9, Vial protocol 6; lighting: RGB Matrix"
        );
        assert_eq!(
            rows[3].detail,
            "the keyboard's own couldn't be read: the Vial definition doesn't unpack: xz: \
             invalid check (CRC64); VIA protocol 9, Vial protocol 6; lighting: none answered",
            "the health view keeps the technical reason"
        );
        assert!(board_rows(&[]).is_empty());
    }

    /// The settings say each kind of Vial read failure in plain words (#74).
    #[test]
    fn a_vial_read_failure_is_said_in_plain_words() {
        let say = |why: &str| {
            source_text(&Source::Missing {
                why: Some(why.into()),
            })
        };
        let plain = |p: &str| format!("the keyboard's own couldn't be read ({p}), so no keymap");
        for (why, phrase) in [
            (
                "the Vial definition doesn't unpack: xz: invalid check",
                "its stored data was damaged",
            ),
            (
                "the Vial definition isn't UTF-8 text",
                "its stored data was damaged",
            ),
            (
                "the Vial definition is 70000 bytes, over the 65536-byte cap",
                "it's too large",
            ),
            (
                "the Vial definition is 2097152 bytes decompressed, over the 1048576-byte cap",
                "it's too large",
            ),
            ("operation timed out", "it took too long to answer"),
            ("the Vial definition is empty", "it has none stored"),
            (
                "the Vial definition isn't an XZ stream Foxglove reads",
                "it's stored in a format Foxglove doesn't read",
            ),
            (
                "the Vial definition doesn't parse: missing field `matrix`",
                "Foxglove doesn't understand its layout",
            ),
            ("unsupported operation", "it stopped answering Vial's reads"),
            ("busy: in use", "something went wrong reading it"),
        ] {
            assert_eq!(say(why), plain(phrase), "{why}");
        }
    }

    /// Generic discovery is off by default and in stores from before it existed.
    #[test]
    fn generic_discovery_is_off_by_default() {
        assert!(!ViaSettings::default().generic);
        let old: ViaSettings = toml::from_str("enabled = true\n").unwrap();
        assert!(old.enabled && !old.generic);
        let on = ViaSettings {
            enabled: true,
            generic: true,
            ..Default::default()
        };
        let text = toml::to_string(&on).unwrap();
        assert!(text.contains("generic = true"), "{text}");
        assert_eq!(toml::from_str::<ViaSettings>(&text).unwrap(), on);
    }

    #[test]
    fn the_saved_state_round_trips() {
        let mut s = ViaSettings {
            enabled: true,
            paused: vec!["port-a".into(), "3434:0250".into()],
            known: vec![known("port-a")],
            ..Default::default()
        };
        s.aliases.insert("port-b".into(), "port-a".into());
        let text = toml::to_string(&s).unwrap();
        assert_eq!(toml::from_str::<ViaSettings>(&text).unwrap(), s);
        // A store from before #292 (no known, no aliases) still loads.
        let old: ViaSettings =
            toml::from_str("enabled = true\npaused = [\"3434:0250\"]\n").unwrap();
        assert!(old.known.is_empty() && old.aliases.is_empty());
    }

    /// A Keychron K5 Pro shaped VIA definition (#797): full size, 108 keys (the 104 of ANSI and
    /// four more over the number pad), drawn the way VIA definitions draw them, with one layout
    /// option as #770's fixtures have them: "Split Backspace", whose second choice (two 1u keys)
    /// sits right of the board.
    pub(crate) fn k5_pro_definition() -> String {
        use serde_json::{json, Value};
        let mut col = 0u16;
        let mut row = 0u16;
        let mut rows: Vec<Value> = Vec::new();
        // Each row: (gap before, width, height, option) per key; gaps and sizes in key units.
        let k = |gap: f64, w: f64| (gap, w, 1.0, "");
        let mut push = |keys: Vec<(f64, f64, f64, &str)>, y: f64, row: &mut u16, col: &mut u16| {
            let mut items = Vec::new();
            for (i, (gap, w, h, option)) in keys.into_iter().enumerate() {
                let mut o = serde_json::Map::new();
                if i == 0 && y != 0.0 {
                    o.insert("y".into(), json!(y));
                }
                if gap != 0.0 {
                    o.insert("x".into(), json!(gap));
                }
                if w != 1.0 {
                    o.insert("w".into(), json!(w));
                }
                if h != 1.0 {
                    o.insert("h".into(), json!(h));
                }
                if !o.is_empty() {
                    items.push(Value::Object(o));
                }
                let label = if option.is_empty() {
                    format!("{row},{col}")
                } else {
                    format!("{row},{col}\n\n\n{option}")
                };
                items.push(json!(label));
                *col += 1;
            }
            rows.push(Value::Array(items));
            *row += 1;
            *col = 0;
        };
        let ones = |n: usize, first_gap: f64| {
            (0..n)
                .map(|i| k(if i == 0 { first_gap } else { 0.0 }, 1.0))
                .collect::<Vec<_>>()
        };
        // Esc, F1-F12 in threes of four, the three over the arrows, four over the number pad.
        let mut r0 = vec![k(0.0, 1.0)];
        r0.extend(ones(4, 1.0));
        r0.extend(ones(4, 0.5));
        r0.extend(ones(4, 0.5));
        r0.extend(ones(3, 0.25));
        r0.extend(ones(4, 0.25));
        // ` 1-0 - = and Backspace (2u, or two 1u keys right of the board), Ins Home PgUp, the pad.
        let mut r1 = ones(13, 0.0);
        r1.push((0.0, 2.0, 1.0, "0,0"));
        r1.extend(ones(3, 0.25));
        r1.extend(ones(4, 0.25));
        r1.push((0.25, 1.0, 1.0, "0,1"));
        r1.push((0.0, 1.0, 1.0, "0,1"));
        let mut r2 = vec![k(0.0, 1.5)];
        r2.extend(ones(12, 0.0));
        r2.push(k(0.0, 1.5));
        r2.extend(ones(3, 0.25));
        r2.extend(ones(3, 0.25));
        r2.push((0.0, 1.0, 2.0, ""));
        let mut r3 = vec![k(0.0, 1.75)];
        r3.extend(ones(11, 0.0));
        r3.push(k(0.0, 2.25));
        r3.extend(ones(3, 3.5));
        let mut r4 = vec![k(0.0, 2.25)];
        r4.extend(ones(10, 0.0));
        r4.push(k(0.0, 2.75));
        r4.push(k(1.25, 1.0));
        r4.extend(ones(3, 1.25));
        r4.push((0.0, 1.0, 2.0, ""));
        let mut r5: Vec<_> = (0..3).map(|_| k(0.0, 1.25)).collect();
        r5.push(k(0.0, 6.25));
        r5.extend((0..4).map(|_| k(0.0, 1.25)));
        r5.extend(ones(3, 0.25));
        r5.push(k(0.25, 2.0));
        r5.push(k(0.0, 1.0));
        push(r0, 0.0, &mut row, &mut col);
        push(r1, 0.25, &mut row, &mut col);
        push(r2, 0.0, &mut row, &mut col);
        push(r3, 0.0, &mut row, &mut col);
        push(r4, 0.0, &mut row, &mut col);
        push(r5, 0.0, &mut row, &mut col);
        json!({
            "name": "Keychron K5 Pro",
            "vendorId": "0x3434",
            "productId": "0x0250",
            "matrix": {"rows": 6, "cols": 23},
            "menus": ["qmk_rgb_matrix"],
            "layouts": {"labels": ["Split Backspace"], "keymap": rows}
        })
        .to_string()
    }

    /// What an open of the K5 Pro reports for a layout options value (as `ViaDevice::open`
    /// works it out): its summary and its layout.
    pub(crate) fn k5_pro_opened(value: u32) -> (Summary, foxglove_devices::Layout) {
        let d = Definition::parse(&k5_pro_definition()).unwrap();
        let summary = Summary {
            firmware: Some("VIA protocol 12".into()),
            lighting: Some(Channel::RgbMatrix),
            lighting_asked: true,
            source: Source::Loaded,
            layers: Some(4),
            layout_options: d.chosen_names(value),
        };
        (summary, d.layout_for(value))
    }

    fn count(l: &crate::foxglove::key_layout::KeyLayout) -> usize {
        l.parts.iter().map(Vec::len).sum()
    }

    /// #797: a K5 Pro definition draws its full-size layout, with the chip "Layout from its VIA
    /// definition", and follows its layout options value.
    #[test]
    fn a_k5_pro_definition_draws_its_full_size_layout_and_follows_its_layout_options() {
        use crate::foxglove::key_layout::{resolve, LayoutSource};
        let (summary, layout) = k5_pro_opened(0);
        let supplied = layout_of(&summary, &layout).unwrap();
        let drawn = resolve(None, &[supplied], 0);
        assert_eq!(count(&drawn), 108, "the whole board, not a guess");
        assert_eq!(drawn.source.chip(), "Layout from its VIA definition");
        assert!(matches!(drawn.source, LayoutSource::Definition(_)));
        assert_eq!(drawn.source.dot(), Some(crate::foxglove::theme::Dot::Ok));
        assert!(
            drawn.size.x > 22.0 && drawn.size.x < 23.0,
            "{:?}",
            drawn.size
        );
        assert!((drawn.size.y - 6.25).abs() < 0.01, "{:?}", drawn.size);
        assert_eq!(drawn.name.as_deref(), Some("Keychron K5 Pro"));
        // Split Backspace on: two 1u keys where the 2u one was.
        let (summary, layout) = k5_pro_opened(1);
        let split = resolve(None, &[layout_of(&summary, &layout).unwrap()], 0);
        assert_eq!(count(&split), 109);
        assert_eq!(split.source.chip(), "Layout from its VIA definition");
        assert_eq!(
            split.name.as_deref(),
            Some("Keychron K5 Pro \u{b7} Split Backspace")
        );
        let hover = crate::foxglove::key_layout::hover(&split);
        assert!(
            hover.starts_with("Drawn from the VIA definition you loaded"),
            "{hover}"
        );
        let backspace = |l: &crate::foxglove::key_layout::KeyLayout| {
            let mut row: Vec<(f32, f32)> = l.parts[0]
                .iter()
                .filter(|k| {
                    (k.rect.min.y - 1.25).abs() < 0.01 && k.rect.min.x > 12.5 && k.rect.min.x < 15.0
                })
                .map(|k| (k.rect.min.x, k.rect.width()))
                .collect();
            row.sort_by(|a, b| a.0.total_cmp(&b.0));
            row
        };
        assert_eq!(backspace(&drawn), [(13.0, 2.0)]);
        assert_eq!(backspace(&split), [(13.0, 1.0), (14.0, 1.0)]);
        // The ladder: the definition beats the guess, and the keyboard beats it.
        assert_eq!(
            drawn.size, split.size,
            "the split keys sit in the 2u key's place"
        );
    }

    /// A loaded QMK keyboard.json says "from a QMK layout", named by its `LAYOUT_*`; a Vial
    /// board's own definition is the keyboard's; a board with no definition hands nothing over.
    #[test]
    fn qmk_layouts_and_vial_definitions_have_their_own_chips() {
        use crate::foxglove::key_layout::resolve;
        let (mut summary, layout) = k5_pro_opened(0);
        summary.source = Source::QmkLayout {
            layout: "LAYOUT_108_ansi".into(),
        };
        summary.layout_options.clear();
        let qmk = resolve(None, &[layout_of(&summary, &layout).unwrap()], 0);
        assert_eq!(qmk.source.chip(), "Layout from a QMK layout");
        assert_eq!(qmk.name.as_deref(), Some("LAYOUT_108_ansi"));
        assert_eq!(count(&qmk), 108);
        summary.source = Source::Vial { protocol: 6 };
        let vial = resolve(None, &[layout_of(&summary, &layout).unwrap()], 0);
        assert_eq!(vial.source.chip(), "Layout from the keyboard");
        summary.source = Source::Missing { why: None };
        assert_eq!(layout_of(&summary, &layout), None);
    }

    /// A scan hands the open boards' layouts over and withdraws the ones gone (a definition
    /// removed, a board paused or closed); the host-link keyboard stays the connected one.
    #[test]
    fn a_scan_hands_layouts_over_and_withdraws_the_gone_ones() {
        use crate::foxglove::key_layout;
        key_layout::set_for_test(0x1d50, 0x615e, None);
        let (summary, layout) = k5_pro_opened(0);
        let k5 = layout_of(&summary, &layout).unwrap();
        hand_layouts(vec![((0x3434, 0x0250), k5.clone())]);
        let focused = key_layout::tests::handed_for(0x3434, 0x0250);
        assert_eq!(focused, vec![k5]);
        assert_eq!(key_layout::tests::connected_now(), Some((0x1d50, 0x615e)));
        hand_layouts(Vec::new());
        assert!(key_layout::tests::handed_for(0x3434, 0x0250).is_empty());
    }

    /// #797, nothing is written: the layout comes from what the read-only open already read.
    /// The code that hands it over reaches no VIA write path, and the scan opens boards without
    /// asking for writes.
    #[test]
    fn handing_layouts_over_reaches_no_via_write_path() {
        let src = include_str!("via_host.rs");
        let body = |from: &str, to: &str| {
            let start = src.find(from).unwrap();
            let end = src[start..].find(to).unwrap() + start;
            &src[start..end]
        };
        let handing = body("pub fn layout_of(", "/// Loads the definitions");
        let scan = body("pub fn rescan()", "/// Runs `f` on an open VIA keyboard");
        // #723: the scan asks for writes only behind the hidden dev switch, never a setting.
        let gated = "if foxglove_via::writes_enabled() {\n            backend = backend.allow_writes();\n        }";
        let scan = scan.replace('\r', "");
        assert_eq!(scan.matches("allow_writes").count(), 1);
        assert!(
            scan.contains(gated),
            "allow_writes only behind the dev switch"
        );
        let scan = scan.replace(gated, "");
        for code in [handing, scan.as_str()] {
            for write in [
                "set_binding",
                "set_effect",
                "set_color",
                "set_speed",
                "set_value",
                "set_layout_options",
                "save_lighting",
                "allow_writes",
                "with_writes",
                ".send(",
                "lighting()",
            ] {
                assert!(!code.contains(write), "{write} reached");
            }
        }
        assert!(scan.contains("ViaBackend::new(defs)"));
    }
}
