//! The PC sensors page (UX plan `ux-plan-omen-sensors-page.md`, slice 3b): one pure function from
//! what the sensors say to the page's tree, and the little state that turns a tree into the
//! hub's `ui.register` or `ui.patch`. No pipe, no clock, no window.
//!
//! Every string is built here (a hub `bind` can't format units), so no fact key, group word or
//! error id ever reaches the screen. A value that is missing reads "Not reported", never 0.

use std::collections::BTreeMap;

use foxglove_plugin::ui::{self, column, event_button, label, markdown, named, row, section};
use serde_json::{json, Value};

use crate::{Group, Metric};

/// The page panel the module declares (`[[ui.panels]]`).
pub const PANEL: &str = "page";
/// The tray tile (slice 3c): hidden until the user adds it from Edit tiles.
pub const TILE: &str = "tile";
/// Its title and icon, as `module.toml` declares them.
pub const TITLE: &str = "PC sensors";
pub const ICON: &str = "chart";

/// The id of the Try again button's node: its `ui.event` click restarts the sources once.
pub const RETRY: &str = "retry";

/// What `sensor.<group>.state` says; a state that was never published (or withdrawn) is
/// `Unknown`, so the page says "Reading…".
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum GroupState {
    #[default]
    Unknown,
    Ok,
    Unavailable,
}

impl GroupState {
    /// From the published text.
    pub fn from_text(t: &str) -> GroupState {
        match t {
            "ok" => GroupState::Ok,
            "unavailable" => GroupState::Unavailable,
            _ => GroupState::Unknown,
        }
    }
}

/// Everything the page shows.
#[derive(Clone, Debug, Default)]
pub struct PageInput {
    pub values: BTreeMap<Metric, i64>,
    pub states: BTreeMap<Group, GroupState>,
    /// `keyboard.game_mode` is on.
    pub game_mode: bool,
    /// The module may read `keyboard.game_mode` (its optional grant is on).
    pub game_mode_visible: bool,
}

impl PageInput {
    fn state(&self, g: Group) -> GroupState {
        self.states.get(&g).copied().unwrap_or_default()
    }
}

/// How one reading is drawn: the words, the tone, and the accessible name.
struct Shown {
    text: String,
    tone: &'static str,
    name: String,
    meter: f64,
}

const READING: &str = "Reading…";
const NOT_REPORTED: &str = "Not reported";

/// One reading. `what` is the row's name ("Processor"), `spoken` how a screen reader says the
/// value ("23 percent busy"), `text` how it is drawn ("23%").
fn shown(input: &PageInput, m: Metric, what: &str, fmt: impl Fn(i64) -> (String, String)) -> Shown {
    shown_of(input, m.group(), input.values.get(&m).copied(), what, fmt)
}

fn shown_of(
    input: &PageInput,
    g: Group,
    v: Option<i64>,
    what: &str,
    fmt: impl Fn(i64) -> (String, String),
) -> Shown {
    match (input.state(g), v) {
        (GroupState::Unknown, _) => Shown {
            text: READING.into(),
            tone: "off",
            name: format!("{what}, reading"),
            meter: 0.0,
        },
        (_, Some(v)) => {
            let (text, spoken) = fmt(v);
            Shown {
                text,
                tone: "neutral",
                name: format!("{what}, {spoken}"),
                meter: v.clamp(0, 100) as f64,
            }
        }
        (_, None) => Shown {
            text: NOT_REPORTED.into(),
            tone: "off",
            name: format!("{what}, not reported"),
            meter: 0.0,
        },
    }
}

fn percent(v: i64) -> String {
    format!("{v}%")
}

/// A name, a value and a meter: the Now rows.
fn now_row(id: &str, what: &str, s: Shown) -> Value {
    let meter = json!({
        "type": "meter",
        "id": format!("{id}-meter"),
        "props": {"value": s.meter, "max": 100, "name": s.name},
    });
    row(
        id,
        vec![
            label(&format!("{id}-name"), what),
            named(ui::value(&format!("{id}-value"), &s.text, s.tone), &s.name),
            meter,
        ],
    )
}

/// A name and a value: the graphics card rows.
fn detail_row(id: &str, what: &str, s: Shown) -> Value {
    row(
        id,
        vec![
            label(&format!("{id}-name"), what),
            named(ui::value(&format!("{id}-value"), &s.text, s.tone), &s.name),
        ],
    )
}

/// A `label` with a tone.
fn toned(id: &str, text: &str, tone: &str) -> Value {
    let mut n = label(id, text);
    n["props"]["tone"] = json!(tone);
    n
}

const ABOUT: &str = "Shows how busy your processor, graphics card and memory are, and your \
graphics card's temperature, speed, power and fan. It only reads: it never changes fan speed, \
performance or power. It reads only while this page is open or another app asks, and stops when \
nothing looks. Processor temperature isn't shown: Windows doesn't share it without an extra \
driver.";

const BLIND_TO_GAMES: &str = "PC sensors can't see when you're gaming, so it keeps updating once \
a second. To slow it down during games, allow \"see whether game mode is on\" on the Apps page.";

const NO_READINGS: &str = "Windows isn't giving any readings right now.";
const GPU_NOTE: &str = "This graphics card doesn't report these to Windows.";
const GPU_NOTE_ALL: &str =
    "This graphics card doesn't report these to Windows. Today only NVIDIA cards do.";

/// The page's tree for what the sensors say now (contract §5.2; one column).
pub fn page(i: &PageInput) -> Value {
    let cpu = shown(i, Metric::CpuLoad, "Processor", |v| {
        (percent(v), format!("{v} percent busy"))
    });
    let gpu = shown(i, Metric::GpuLoad, "Graphics card", |v| {
        (percent(v), format!("{v} percent busy"))
    });
    let ram = shown(i, Metric::RamUsed, "Memory", |v| {
        (format!("{v}% used"), format!("{v} percent used"))
    });

    let none = Group::ALL
        .iter()
        .all(|g| i.state(*g) == GroupState::Unavailable);
    let now = if none {
        vec![
            toned("now-none", NO_READINGS, "attention"),
            event_button(RETRY, "Try again"),
        ]
    } else {
        let rate = if i.game_mode {
            toned(
                "now-rate",
                "Game mode is on, so this updates every 10 seconds.",
                "off",
            )
        } else {
            toned(
                "now-rate",
                "Updates once a second while this page is open.",
                "neutral",
            )
        };
        vec![
            now_row("now-cpu", "Processor", cpu),
            now_row("now-gpu", "Graphics card", gpu),
            now_row("now-ram", "Memory", ram),
            rate,
        ]
    };

    let temp = shown(i, Metric::GpuTemp, "Graphics card temperature", |v| {
        (format!("{v} °C"), format!("{v} degrees Celsius"))
    });
    let clock = shown(i, Metric::GpuClock, "Graphics card speed", |v| {
        (format!("{v} MHz"), format!("{v} megahertz"))
    });
    let power = shown(i, Metric::GpuPower, "Graphics card power", |v| {
        (format!("{v} W"), format!("{v} watts"))
    });
    // RPM when present; else the percent with its words, so a percent never reads as RPM.
    let fan = match (
        i.values.get(&Metric::GpuFan).copied(),
        i.values.get(&Metric::GpuFanPct).copied(),
    ) {
        (Some(rpm), _) => shown_of(i, Group::Gpu, Some(rpm), "Graphics card fan", |v| {
            (format!("{v} RPM"), format!("{v} revolutions per minute"))
        }),
        (None, Some(p)) => shown_of(i, Group::Gpu, Some(p), "Graphics card fan", |v| {
            (
                format!("{v}% of full speed"),
                format!("{v} percent of full speed"),
            )
        }),
        (None, None) => shown_of(i, Group::Gpu, None, "Graphics card fan", |v| {
            (v.to_string(), v.to_string())
        }),
    };
    let missing_detail = [&temp, &clock, &power, &fan]
        .iter()
        .any(|s| s.text == NOT_REPORTED);
    let mut gpu_rows = vec![
        detail_row("gpu-temp", "Temperature", temp),
        detail_row("gpu-clock", "Speed", clock),
        detail_row("gpu-power", "Power", power),
        detail_row("gpu-fan", "Fan", fan),
    ];
    // The NVIDIA sentence only when the whole card is unavailable and Windows gives other
    // readings (so it isn't simply a bad moment); with some values present it is only "these";
    // with no readings at all the "Not reported" rows say enough.
    if missing_detail && !none {
        let note = if i.state(Group::Gpu) == GroupState::Unavailable {
            GPU_NOTE_ALL
        } else {
            GPU_NOTE
        };
        gpu_rows.push(toned("gpu-note", note, "off"));
    }

    let mut about = ABOUT.to_string();
    if !i.game_mode_visible {
        about.push_str("\n\n");
        about.push_str(BLIND_TO_GAMES);
    }

    column(
        "page",
        vec![
            section("now", "Now", now),
            section("gpu", "Graphics card", gpu_rows),
            section("about", "About", vec![markdown("about-text", &about)]),
        ],
    )
}

/// The page as `ui.register` takes it.
pub fn panel(i: &PageInput) -> Value {
    ui::panel(PANEL, "page", TITLE, ICON, page(i))
}

/// The tray tile's tree (contract §5.5: one `value`, one `label`, one `meter`, no button; a click
/// opens the page). The value is the busiest of processor, graphics card and memory ("Graphics
/// card 87%"), the label the other two. A reading that isn't there says "Reading…" or "Not
/// reported", never 0.
pub fn tile(i: &PageInput) -> Value {
    let all = [
        (Metric::CpuLoad, "Processor", "busy"),
        (Metric::GpuLoad, "Graphics card", "busy"),
        (Metric::RamUsed, "Memory", "used"),
    ];
    let busiest = all
        .iter()
        .filter_map(|(m, n, w)| i.values.get(m).map(|v| (*v, *m, *n, *w)))
        .max_by_key(|(v, ..)| *v);
    let sub: Option<String>;
    let (text, tone, spoken, meter) = match busiest {
        Some((v, busy, name, word)) => {
            let rest: Vec<String> = all
                .iter()
                .filter(|(m, ..)| *m != busy)
                .map(|(m, n, _)| match (i.values.get(m), i.state(m.group())) {
                    (Some(v), _) => format!("{n} {v}%"),
                    (None, GroupState::Unknown) => format!("{n} reading…"),
                    (None, _) => format!("{n} not reported"),
                })
                .collect();
            sub = Some(rest.join(" · "));
            (
                format!("{name} {v}%"),
                "neutral",
                format!("{name}, {v} percent {word}"),
                v.clamp(0, 100) as f64,
            )
        }
        None => {
            let none = Group::ALL
                .iter()
                .all(|g| i.state(*g) == GroupState::Unavailable);
            // The sub line is always there, so a reading arriving is a patch, not a new shape.
            sub = Some("Processor · Graphics card · Memory".to_string());
            if none {
                (
                    NOT_REPORTED.to_string(),
                    "off",
                    "PC sensors, not reported".to_string(),
                    0.0,
                )
            } else {
                (
                    READING.to_string(),
                    "off",
                    "PC sensors, reading".to_string(),
                    0.0,
                )
            }
        }
    };
    let mut kids = vec![ui::value("glance", &text, tone)];
    if let Some(sub) = &sub {
        kids.push(label("sub", sub));
    }
    kids.push(named(
        json!({"type": "meter", "id": "meter", "props": {"value": meter, "max": 100}}),
        &spoken,
    ));
    column("tile", kids)
}

/// The tile as `ui.register` takes it.
pub fn tile_panel(i: &PageInput) -> Value {
    ui::panel(TILE, "tile", TITLE, ICON, tile(i))
}

/// What to send the hub for a new tree.
#[derive(Clone, Debug, PartialEq)]
pub enum PageOut {
    /// `ui.register` with this panel (the first time, or when the tree's shape changed).
    Register(Value),
    /// `ui.patch` ops for the nodes whose text changed.
    Patch(Vec<Value>),
}

/// The tree the hub holds for one panel, so each new tree costs one patch of what changed (or
/// nothing).
#[derive(Default)]
pub struct PageView {
    shown: Option<Value>,
    tile: bool,
}

impl PageView {
    /// A view of the tray tile.
    pub fn tile() -> PageView {
        PageView {
            shown: None,
            tile: true,
        }
    }

    /// `None` when the hub already holds `tree`.
    pub fn next(&mut self, tree: Value) -> Option<PageOut> {
        let out = match &self.shown {
            Some(old) if *old == tree => return None,
            Some(old) => match ui::diff(old, &tree) {
                Some(ops) => PageOut::Patch(ops),
                None => PageOut::Register(self.panel_of(&tree)),
            },
            None => PageOut::Register(self.panel_of(&tree)),
        };
        self.shown = Some(tree);
        Some(out)
    }

    fn panel_of(&self, tree: &Value) -> Value {
        if self.tile {
            ui::panel(TILE, "tile", TITLE, ICON, tree.clone())
        } else {
            ui::panel(PANEL, "page", TITLE, ICON, tree.clone())
        }
    }
}

/// What to send the hub for new trees of both panels.
#[derive(Clone, Debug, PartialEq)]
pub enum PanelsOut {
    /// `ui.register` with both panels (a `ui.register` replaces the module's set).
    Register(Vec<Value>),
    /// `ui.patch` of one panel.
    Patch {
        panel: &'static str,
        ops: Vec<Value>,
    },
}

/// The tile and the page the hub holds.
pub struct PanelsView {
    tile: PageView,
    page: PageView,
}

impl Default for PanelsView {
    fn default() -> Self {
        PanelsView::new()
    }
}

impl PanelsView {
    pub fn new() -> PanelsView {
        PanelsView {
            tile: PageView::tile(),
            page: PageView::default(),
        }
    }

    /// Nothing when the hub already holds both trees; one register of both panels when either
    /// changed shape; otherwise a patch for each panel whose text changed.
    pub fn next(&mut self, tile: Value, page: Value) -> Vec<PanelsOut> {
        let t = self.tile.next(tile);
        let p = self.page.next(page);
        if matches!(t, Some(PageOut::Register(_))) || matches!(p, Some(PageOut::Register(_))) {
            let panels = [&self.tile, &self.page]
                .iter()
                .filter_map(|v| v.shown.as_ref().map(|s| v.panel_of(s)))
                .collect();
            return vec![PanelsOut::Register(panels)];
        }
        let mut out = Vec::new();
        if let Some(PageOut::Patch(ops)) = t {
            out.push(PanelsOut::Patch { panel: TILE, ops });
        }
        if let Some(PageOut::Patch(ops)) = p {
            out.push(PanelsOut::Patch { panel: PANEL, ops });
        }
        out
    }
}
