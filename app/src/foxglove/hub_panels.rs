//! Foxglove's panels in the hub (v3 H3.2, #1143 P8): three tray tiles (Keyboard, Keyboard lights,
//! Keyboard art) and one page ("Foxglove"), drawn from the `keyboard.*` facts the app already
//! mirrors to the hub (`keyboard_facts`) and nothing else. UX plan: `review/ux-p8-legacy-panels.md`.
//!
//! Read-only: each tile and each page section has one button, "Open Foxglove settings", which
//! sends a plain `ui.event`; the module opens its own settings window at the matching page
//! (`hub_link`, `settings_open`). Nothing is changed from here.
//!
//! The hub draws the frame states (off, starting, restarting, stopped, greyed cached values);
//! this draws only the bodies: waiting for the first facts, connected, not connected, lights off,
//! lights held by Windows or a scene, and a Foxglove page stopped by its panic guard (#1143 P1).
//! Everything here is pure (facts in, panels out), so each state has a window-free test and a
//! vector for the hub's render tests.

use foxglove_devices::facts::Value as Fact;
use foxglove_plugin::ui::{self, Button, Press};
use serde_json::{json, Value};

use super::shell::Page;

/// The panel ids, as `module.toml` declares them.
pub const TILE_KEYBOARD: &str = "keyboard";
pub const TILE_LIGHTS: &str = "lights";
pub const TILE_ART: &str = "art";
pub const PAGE: &str = "page";

/// The button's words, on every tile and section.
pub const OPEN: &str = "Open Foxglove settings";
/// The page's one line of introduction.
pub const INTRO: &str = "Your keyboard, its lights and its art, as the Foxglove app sees them.";
/// The notice while one of Foxglove's own pages is stopped by its guard.
pub const STOPPED: &str = "Foxglove's page stopped working.";

/// The most bytes of a name shown (a tile's text cap is 64; the cut adds "…").
const CUT: usize = 60;

/// Which of the three things a panel is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    Keyboard,
    Lights,
    Art,
}

impl Part {
    pub const ALL: [Part; 3] = [Part::Keyboard, Part::Lights, Part::Art];

    /// The panel (and section) title: not "Lights" and "Art", which the hub has of its own.
    pub fn title(self) -> &'static str {
        match self {
            Part::Keyboard => "Keyboard",
            Part::Lights => "Keyboard lights",
            Part::Art => "Keyboard art",
        }
    }

    fn tile_id(self) -> &'static str {
        match self {
            Part::Keyboard => TILE_KEYBOARD,
            Part::Lights => TILE_LIGHTS,
            Part::Art => TILE_ART,
        }
    }

    fn icon(self) -> &'static str {
        match self {
            Part::Keyboard => "keyboard",
            Part::Lights => "lights",
            Part::Art => "image",
        }
    }

    /// The settings page the button opens: art lives on Screens.
    pub fn settings_page(self) -> Page {
        match self {
            Part::Keyboard => Page::Keyboard,
            Part::Lights => Page::Lights,
            Part::Art => Page::Screens,
        }
    }

    /// The panel and node a `ui.event` click names, for [`Part::of_event`].
    fn button_node(self) -> &'static str {
        match self {
            Part::Keyboard => "go-keyboard",
            Part::Lights => "go-lights",
            Part::Art => "go-art",
        }
    }

    /// The part a click on `node` of `panel` is about: a tile's one button, or a page section's.
    pub fn of_event(panel: &str, node: &str) -> Option<Part> {
        Part::ALL.into_iter().find(|p| match panel {
            TILE_KEYBOARD | TILE_LIGHTS | TILE_ART => p.tile_id() == panel && node == "go",
            PAGE => node == p.button_node(),
            _ => false,
        })
    }

    /// What shows here once the keyboard reports it.
    fn waiting(self) -> &'static str {
        match self {
            Part::Keyboard => "Its state shows here once the keyboard reports it.",
            Part::Lights => "The lights show here once the keyboard reports them.",
            Part::Art => "The art shows here once the keyboard reports it.",
        }
    }
}

/// Which of Foxglove's own settings pages its panic guard has stopped (#1143 P1).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stopped {
    pub keyboard: bool,
    pub lights: bool,
    pub screens: bool,
}

impl Stopped {
    /// Read from the guard.
    pub fn now() -> Stopped {
        let stopped = |p: Page| super::page_guard::stopped(&format!("page:{}", p.label()));
        Stopped {
            keyboard: stopped(Page::Keyboard),
            lights: stopped(Page::Lights),
            screens: stopped(Page::Screens),
        }
    }

    fn of(self, part: Part) -> bool {
        match part {
            Part::Keyboard => self.keyboard,
            Part::Lights => self.lights,
            Part::Art => self.screens,
        }
    }
}

/// What the module's link thread listens on: told when a page's guard stops or retries, so the
/// panels follow without a poll (set once by the link runner).
static GUARD_WAKE: std::sync::Mutex<Option<Box<dyn Fn() + Send>>> = std::sync::Mutex::new(None);

/// The link thread asks to be woken on a guard change.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn on_guard_change(f: impl Fn() + Send + 'static) {
    *GUARD_WAKE.lock().unwrap_or_else(|e| e.into_inner()) = Some(Box::new(f));
}

/// A page was stopped by its guard, or tried again (called from the UI thread).
pub fn guard_changed() {
    if let Some(f) = GUARD_WAKE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
    {
        f();
    }
}

/// Who draws the lights.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Keyboard,
    Windows,
    Scene,
}

/// The lights as the facts give them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Lights {
    pub on: Option<bool>,
    pub effect: Option<String>,
    pub brightness: Option<i64>,
    pub rainbow: bool,
    pub colour: Option<String>,
    pub source: Option<Source>,
}

/// Everything the panels say, read from the facts.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct View {
    /// `None`: no fact yet (the app just started).
    pub connected: Option<bool>,
    pub name: Option<String>,
    /// `usb` or `ble`.
    pub link: Option<String>,
    pub profile: Option<i64>,
    /// Percent by part, the left half first.
    pub batteries: Vec<(String, i64)>,
    pub layer: Option<i64>,
    pub layer_name: Option<String>,
    pub lights: Lights,
    /// The piece's name and where it comes from (`built_in`, `kept`, `sent`).
    pub art: Option<(String, String)>,
    pub stopped: Stopped,
}

impl View {
    /// The view of `facts` (every `keyboard.*` fact), with the guard's state.
    pub fn of(facts: &[(String, Fact)], stopped: Stopped) -> View {
        let get = |k: &str| facts.iter().find(|(key, _)| key == k).map(|(_, v)| v);
        let text = |k: &str| match get(k) {
            Some(Fact::Text(t)) if !t.is_empty() => Some(t.clone()),
            _ => None,
        };
        let int = |k: &str| match get(k) {
            Some(Fact::Int(i)) => Some(*i),
            _ => None,
        };
        let flag = |k: &str| match get(k) {
            Some(Fact::Bool(b)) => Some(*b),
            _ => None,
        };
        let mut batteries: Vec<(String, i64)> = facts
            .iter()
            .filter_map(|(k, v)| {
                let part = k.strip_prefix("keyboard.")?.strip_suffix(".battery")?;
                match v {
                    Fact::Int(p) if !part.is_empty() && !part.contains('.') => {
                        Some((part.to_string(), (*p).clamp(0, 100)))
                    }
                    _ => None,
                }
            })
            .collect();
        let rank = |p: &str| match p {
            "left" => 0,
            "right" => 1,
            _ => 2,
        };
        batteries.sort_by(|a, b| (rank(&a.0), &a.0).cmp(&(rank(&b.0), &b.0)));
        View {
            connected: flag("keyboard.connected"),
            name: text("keyboard.name"),
            link: text("keyboard.link"),
            profile: int("keyboard.profile"),
            batteries,
            layer: int("keyboard.layer"),
            layer_name: text("keyboard.layer_name"),
            lights: Lights {
                on: flag("keyboard.lighting.on"),
                effect: text("keyboard.lighting.effect"),
                brightness: int("keyboard.lighting.brightness"),
                rainbow: flag("keyboard.lighting.rainbow") == Some(true),
                colour: text("keyboard.lighting.color_name"),
                source: match text("keyboard.lighting.source").as_deref() {
                    Some("windows") => Some(Source::Windows),
                    Some("scene") => Some(Source::Scene),
                    Some("keyboard") => Some(Source::Keyboard),
                    _ => None,
                },
            },
            art: text("keyboard.art.name").zip(text("keyboard.art.from")),
            stopped,
        }
    }
}

/// `text` cut on a whole character to fit a tile's text cap, with "…".
pub fn cut(text: &str) -> String {
    if text.len() <= CUT {
        return text.to_string();
    }
    let mut end = CUT;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", text[..end].trim_end())
}

/// "77 %", as the knob line has it.
fn percent(p: i64) -> String {
    format!("{p} %")
}

fn capital(word: &str) -> String {
    let mut c = word.chars();
    c.next()
        .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
        .unwrap_or_default()
}

/// "Left 77 % · Right 91 %": only the parts reported.
fn battery_line(v: &View) -> Option<String> {
    (!v.batteries.is_empty()).then(|| {
        v.batteries
            .iter()
            .map(|(part, p)| format!("{} {}", capital(part), percent(*p)))
            .collect::<Vec<_>>()
            .join(" · ")
    })
}

fn link_line(v: &View) -> String {
    match v.link.as_deref() {
        Some("usb") => "Connected over USB".into(),
        Some("ble") => "Connected over Bluetooth".into(),
        _ => "Connected".into(),
    }
}

/// What a tile or a section says: the glance, its tone and the line under it.
struct Glance {
    value: String,
    tone: &'static str,
    sub: String,
}

const NOT_CONNECTED: &str = "Not connected";
const TURN_ON: &str = "Turn it on or plug it in.";

fn glance(part: Part, v: &View) -> Glance {
    let waiting = || Glance {
        value: "Waiting for the keyboard".into(),
        tone: "neutral",
        sub: part.waiting().into(),
    };
    match v.connected {
        None => return waiting(),
        Some(false) => {
            return Glance {
                value: NOT_CONNECTED.into(),
                tone: "off",
                sub: TURN_ON.into(),
            }
        }
        Some(true) => {}
    }
    match part {
        Part::Keyboard => Glance {
            value: link_line(v),
            tone: "ok",
            sub: battery_line(v)
                .unwrap_or_else(|| "Battery shows once the keyboard reports it.".into()),
        },
        Part::Lights => lights_glance(v).unwrap_or_else(waiting),
        Part::Art => match &v.art {
            Some((name, from)) => Glance {
                value: cut(name),
                tone: "ok",
                sub: art_from(from).into(),
            },
            None => waiting(),
        },
    }
}

fn art_from(from: &str) -> &'static str {
    match from {
        "kept" => "Kept on the keyboard",
        "sent" => "Sent from this PC",
        _ => "Built-in set",
    }
}

fn lights_glance(v: &View) -> Option<Glance> {
    let l = &v.lights;
    if l.on? == false {
        return Some(Glance {
            value: "Off".into(),
            tone: "off",
            sub: "The keyboard's lights are off.".into(),
        });
    }
    let effect = cut(l.effect.as_deref()?);
    let value = match l.brightness {
        Some(b) => format!("{effect} · {}", percent(b)),
        None => effect,
    };
    let sub = match l.source {
        Some(Source::Windows) => "Windows Dynamic Lighting is drawing them".to_string(),
        Some(Source::Scene) => format!("{} is running", cut(l.effect.as_deref()?)),
        _ if l.rainbow => "Makes its own colours".into(),
        _ => cut(l.colour.as_deref().unwrap_or("Custom colour")),
    };
    Some(Glance {
        value,
        tone: "active",
        sub,
    })
}

fn open_button(node: &'static str) -> Button {
    Button::new(OPEN, Press::Event(node))
}

/// A tile: the glance, its line (the guard's notice instead when the page is stopped), the button.
fn tile(part: Part, v: &View) -> Value {
    let g = glance(part, v);
    // A tile has one label (the tile rules): the guard's notice takes the line's place.
    let line = if v.stopped.of(part) {
        stopped_label("sub")
    } else {
        ui::label("sub", &g.sub)
    };
    let mut kids = vec![ui::value("glance", &g.value, g.tone), line];
    kids.push(ui::button("go", &open_button("go")));
    ui::panel(
        part.tile_id(),
        "tile",
        part.title(),
        part.icon(),
        ui::column("tile", kids),
    )
}

fn stopped_label(id: &str) -> Value {
    json!({"type": "label", "id": id, "props": {"text": STOPPED, "tone": "attention"}})
}

fn meter(id: &str, percent: i64) -> Value {
    json!({"type": "meter", "id": id, "props": {"value": percent, "max": 100}})
}

/// A page section's rows; the button comes after them.
fn rows(part: Part, v: &View) -> Vec<Value> {
    let id = |s: &str| format!("{}-{s}", part.tile_id());
    let line = |k: &str, text: &str| ui::label(&id(k), text);
    match v.connected {
        None => {
            return vec![line("waiting", part.waiting())];
        }
        Some(false) => return vec![line("away", NOT_CONNECTED)],
        Some(true) => {}
    }
    let mut out = Vec::new();
    match part {
        Part::Keyboard => {
            if let Some(name) = &v.name {
                out.push(line("name", &cut(name)));
            }
            out.push(line(
                "link",
                &match (v.link.as_deref(), v.profile) {
                    (Some("ble"), Some(p)) => format!("{}, profile {p}", link_line(v)),
                    _ => link_line(v),
                },
            ));
            match (&v.layer_name, v.layer) {
                (Some(n), _) => out.push(line("layer", &format!("Layer {}", cut(n)))),
                (None, Some(n)) => out.push(line("layer", &format!("Layer {n}"))),
                _ => {}
            }
            if v.batteries.is_empty() {
                out.push(line(
                    "battery",
                    "Battery shows once the keyboard reports it.",
                ));
            }
            for (part_name, p) in &v.batteries {
                out.push(line(
                    &format!("battery-{part_name}"),
                    &format!("{} battery {}", capital(part_name), percent(*p)),
                ));
                out.push(meter(&id(&format!("meter-{part_name}")), *p));
            }
        }
        Part::Lights => match lights_rows(part, v) {
            Some(rows) => out.extend(rows),
            None => out.push(line("waiting", part.waiting())),
        },
        Part::Art => match &v.art {
            Some((name, from)) => {
                out.push(line("name", &cut(name)));
                out.push(line("from", art_from(from)));
            }
            None => out.push(line("waiting", part.waiting())),
        },
    }
    out
}

fn lights_rows(part: Part, v: &View) -> Option<Vec<Value>> {
    let l = &v.lights;
    let id = |s: &str| format!("{}-{s}", part.tile_id());
    if !l.on? {
        return Some(vec![ui::label(&id("off"), "Lights: Off")]);
    }
    let mut out = vec![ui::label(
        &id("effect"),
        &format!("Effect: {}", cut(l.effect.as_deref()?)),
    )];
    if let Some(b) = l.brightness {
        out.push(ui::label(
            &id("brightness"),
            &format!("Brightness: {}", percent(b)),
        ));
        out.push(meter(&id("meter-brightness"), b));
    }
    if l.rainbow {
        out.push(ui::label(&id("colour"), "Colour: makes its own colours"));
    } else if let Some(c) = &l.colour {
        out.push(ui::label(&id("colour"), &format!("Colour: {}", cut(c))));
    }
    match l.source {
        Some(Source::Windows) => out.push(ui::label(
            &id("source"),
            "Windows Dynamic Lighting is drawing them",
        )),
        Some(Source::Scene) => out.push(ui::label(
            &id("source"),
            &format!("{} is running", cut(l.effect.as_deref()?)),
        )),
        _ => {}
    }
    Some(out)
}

/// One section of the page: its rows, the guard's notice, the button.
fn section(part: Part, v: &View) -> Value {
    let mut kids = rows(part, v);
    if v.stopped.of(part) {
        kids.push(stopped_label(&format!("{}-stopped", part.tile_id())));
    }
    kids.push(ui::button("", &open_button(part.button_node())));
    ui::section(part.tile_id(), part.title(), kids)
}

/// The three tiles and the page, for `ui.register`.
pub fn panels(v: &View) -> Vec<Value> {
    let mut out: Vec<Value> = Part::ALL.into_iter().map(|p| tile(p, v)).collect();
    let mut page = vec![ui::label("intro", INTRO)];
    page.extend(Part::ALL.into_iter().map(|p| section(p, v)));
    out.push(ui::panel(
        PAGE,
        "page",
        "Foxglove",
        "keyboard",
        ui::column("page", page),
    ));
    out
}

/// The panels as `ui.register` sends them: the same set [`panels`] gives for a view of nothing
/// (what the manifest declares).
pub fn declared() -> Vec<String> {
    let mut ids: Vec<String> = Part::ALL.iter().map(|p| p.tile_id().to_string()).collect();
    ids.push(PAGE.into());
    ids
}

#[cfg(test)]
mod tests;
