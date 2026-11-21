//! Foxglove's panels in the hub (v3 H3.2, #1143 P8): three tray tiles (Keyboard, Keyboard lights,
//! Keyboard art) and one page ("Foxglove"), drawn from the `keyboard.*` facts the app already
//! mirrors to the hub (`keyboard_facts`) and nothing else. UX plan: `review/ux-p8-legacy-panels.md`.
//!
//! Read-only: each tile and each page section has one button, "Open Foxglove settings", which
//! sends a plain `ui.event`; the module opens its own settings window at the matching page
//! (`hub_link`, `settings_open`). The one exception is the page's **Wallpaper Engine** section
//! (#1142, UX plan `review/ux-we-holder-1142.md` §3), shown while Wallpaper Engine is installed:
//! who holds the wallpapers paused, by the names the hub knows them by, and **Resume**, which
//! ends every hold (`hub_link` sends `lease.end` and the sync worker ends the local ones).
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
/// The intro while the page has the wallpaper section (#1142); the wallpaper app's own name
/// comes from data ([`Wallpaper::engine`]).
pub fn intro_we(engine: &str) -> String {
    format!("Your keyboard, its lights, its art and {engine}, as the Foxglove app sees them.")
}
/// The notice while one of Foxglove's own pages is stopped by its guard.
pub const STOPPED: &str = "Foxglove's page stopped working.";
/// The page's game mode row (#1234), after the layer: a status, never an action.
pub const GAME_ON: &str = "Game mode: on";
pub const GAME_OFF: &str = "Game mode: off";
/// The help under it while game mode is on: game mode frees, reserves and ends nothing (§8.9).
pub const GAME_HELP: &str =
    "Follows the GAME layer. It doesn't free the GPU: use Free GPU on the AI page or in the tray menu.";

/// The most bytes of a name shown (a tile's text cap is 64; the cut adds "…").
const CUT: usize = 60;

/// The Wallpaper Engine section's id, and its Resume button's node.
pub const WALLPAPER: &str = "wallpaper-engine";
pub const RESUME_NODE: &str = "we-resume";
/// The Resume button's words: they name what resumes, so a screen reader says it alone too.
pub const RESUME: &str = "Resume wallpapers";
/// The state line while a play failed and is owed (#1399): the wallpapers are still paused.
pub const RESUME_FAILED: &str = "Couldn't resume wallpapers";
/// The standalone tray's tooltip line for it: no wallpaper app's name, the tray has no data.
pub const RESUME_FAILED_TIP: &str = "Couldn't resume wallpapers. Try again.";
/// The most bytes of one caller's reason shown (in quotes, after the holder's name).
const REASON_CUT: usize = 48;
/// The most bytes of one holder's name shown (UX plan 5: names are cut at 32 with "...").
const NAME_CUT: usize = 32;

/// The section's words (UX plan `review/ux-plan-we-pause-holder.md` 2), all in one place. The
/// wallpaper app's name is never in them: it is data.
pub const NONE_PAUSING: &str = "No app is pausing your wallpapers";
pub const PLAYING: &str = "Playing";
pub const PAUSED_BY_YOU: &str = "Paused by you";
pub const NOT_RUNNING: &str = "Not running";
pub const FULL_SCREEN_SUB: &str = "They play again when the full-screen app closes.";
/// What the section calls itself when the data has no name for the wallpaper app.
pub const SECTION_FALLBACK: &str = "Wallpapers";
/// A holder the hub and the local list could not name.
pub const AN_APP: &str = "an app";

/// "Wallpapers paused by Arete".
pub fn paused_by(who: &str) -> String {
    format!("Wallpapers paused by {who}")
}
/// "Try again, or press Play in <wallpaper app>." (#1399)
pub fn resume_failed_sub(engine: &str) -> String {
    format!("Try again, or press Play in {engine}.")
}
/// "Press Play in <wallpaper app>."
pub fn press_play(engine: &str) -> String {
    format!("Press Play in {engine}.")
}
/// The text with its first letter capitalised, for the start of a sentence ("an app" -> "An app").
pub fn sentence_start(text: &str) -> String {
    let mut c = text.chars();
    match c.next() {
        Some(f) => f.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}
/// "<wallpaper app> isn't running."
pub fn isnt_running(engine: &str) -> String {
    format!("{} isn't running.", sentence_start(engine))
}
/// "Resumed. Arete can pause them again."
pub fn resumed(who: &str) -> String {
    format!("Resumed. {who} can pause them again.")
}
/// "Arete stopped, so its pause ended."
pub fn stopped(who: &str) -> String {
    format!("{who} stopped, so its pause ended.")
}
/// The one line after a Resume or a stop ("Resumed. Arete can pause them again."), shared by the
/// hub page and the standalone tray's tooltip. Real app names show as given; only the "an app"
/// fallback is capitalised for the start of a sentence.
pub fn after_words(after: &After) -> String {
    // Real names show as given; only the "an app" fallback takes a capital at a sentence start.
    let first = |words: String| {
        if words == AN_APP {
            sentence_start(&words)
        } else {
            words
        }
    };
    match after {
        After::Resumed(names) => resumed(&first(holder_words(names))),
        After::Stopped(Some(n)) => stopped(&first(holder_words(std::slice::from_ref(n)))),
        After::Stopped(None) => stopped(&sentence_start(AN_APP)),
    }
}
/// "Resume wallpapers, paused by Arete": the button's accessible name.
pub fn resume_name(who: &str) -> String {
    format!("{RESUME}, paused by {who}")
}
/// The state line's node (a status: polite, announced when its words change).
pub const STATE_NODE: &str = "we-state";

/// What just happened, once nothing holds the wallpapers paused (`we_after`).
#[derive(Clone, Debug, PartialEq)]
pub enum After {
    /// The user pressed Resume while these apps held them.
    Resumed(Vec<String>),
    /// The last app holding them stopped (its name, when known).
    Stopped(Option<String>),
}

/// Wallpaper Engine as the section shows it (#1142). `None` in [`View`]: not installed, no
/// section.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Wallpaper {
    pub running: bool,
    /// A paired wallpaper reports itself paused: the user's own pause.
    pub user_paused: bool,
    /// A full-screen app is in front: a play waits for it to close.
    pub full_screen: bool,
    /// Who holds it paused, by the names the hub knows them by (`module.foxglove.lease_holders`).
    pub holders: Vec<String>,
    /// The wallpaper app's name, from data (`wallpaper_engine::NAME`).
    pub engine: String,
    /// A paired wallpaper reports itself playing (so "Playing" is known, not guessed).
    pub playing: bool,
    /// What just happened, for the one line after a Resume or a stop.
    pub after: Option<After>,
    /// The holders' own reasons (`wallpaper_engine.paused_by`), shown in quotes after the names.
    pub reasons: Vec<String>,
    /// A play failed and is owed (#1399, `wallpaper_engine.play_owed`): paused, nobody holds it.
    pub play_owed: bool,
    /// The allow list (S5b); `None` before Foxglove has read it (tests).
    pub allow: Option<super::we_panel::Inputs>,
}

impl Wallpaper {
    /// The wallpaper app's name for the page; a generic noun while the data gives none.
    fn engine_name(&self) -> String {
        if self.engine.is_empty() {
            "your wallpaper app".into()
        } else {
            self.engine.clone()
        }
    }

    pub(super) fn held(&self) -> bool {
        !self.holders.is_empty() || !self.reasons.is_empty()
    }
}

/// Whether a click on `node` of `panel` is the Wallpaper Engine section's Resume.
pub fn is_resume(panel: &str, node: &str) -> bool {
    panel == PAGE && node == RESUME_NODE
}

/// "Arete", "Arete and Render queue", "Arete and 2 more"; "an app" when the hub named nobody
/// (an older hub, or a caller of the local API).
pub fn holder_words(names: &[String]) -> String {
    // A caller that gave no name is stored as "An app": mid-sentence it is "an app".
    let cut = |n: &String| {
        if n.eq_ignore_ascii_case(AN_APP) {
            AN_APP.to_string()
        } else {
            cut_to(n, NAME_CUT)
        }
    };
    match names {
        [] => AN_APP.into(),
        [a] => cut(a),
        [a, b] => format!("{} and {}", cut(a), cut(b)),
        [a, rest @ ..] => format!("{} and {} more", cut(a), rest.len()),
    }
}

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
    /// `keyboard.game_mode` (#1149): a GAME layer is active. `None`: not reported.
    pub game_mode: Option<bool>,
    pub lights: Lights,
    /// The piece's name and where it comes from (`built_in`, `kept`, `sent`).
    pub art: Option<(String, String)>,
    pub stopped: Stopped,
    /// Wallpaper Engine (#1142); `None` while it isn't installed.
    pub wallpaper: Option<Wallpaper>,
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
            game_mode: flag("keyboard.game_mode"),
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
            wallpaper: None,
        }
    }

    /// The view with Wallpaper Engine's section (#1142).
    pub fn with_wallpaper(mut self, w: Option<Wallpaper>) -> View {
        self.wallpaper = w;
        self
    }
}

/// `text` cut on a whole character to fit a tile's text cap, with "…".
pub fn cut(text: &str) -> String {
    cut_to(text, CUT)
}

/// `text` cut on a whole character to at most `max` bytes before the "…".
pub fn cut_to(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    let mut end = max;
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
    if !l.on? {
        return Some(Glance {
            value: "Off".into(),
            tone: "off",
            sub: "The keyboard's lights are off.".into(),
        });
    }
    // The brightness follows in the same value: room for it under the 64-byte cap.
    let effect = cut_to(l.effect.as_deref()?, 48);
    let value = match l.brightness {
        Some(b) => format!("{effect} · {}", percent(b)),
        None => effect,
    };
    let sub = match l.source {
        Some(Source::Windows) => "Windows Dynamic Lighting is drawing them".to_string(),
        Some(Source::Scene) => "A Foxglove scene is drawing them".into(),
        _ if l.rainbow => "Makes its own colours".into(),
        _ => cut(l
            .colour
            .as_deref()
            .unwrap_or("The colour shows once the keyboard reports it.")),
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

/// A meter, named in words for a screen reader (the label is its name and what it reads).
fn meter(id: &str, percent: i64, words: &str) -> Value {
    json!({"type": "meter", "id": id, "props": {"value": percent, "max": 100, "label": words}})
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
            // Game mode on the page only (#1234, Hub pick B): words, no tone or glyph, so it
            // never pairs with the AI tile's "Reserved for gaming".
            match v.game_mode {
                Some(true) => {
                    out.push(line("game", GAME_ON));
                    out.push(line("game-help", GAME_HELP));
                }
                Some(false) => out.push(line("game", GAME_OFF)),
                None => {}
            }
            if v.batteries.is_empty() {
                out.push(line(
                    "battery",
                    "Battery shows once the keyboard reports it.",
                ));
            }
            for (part_name, p) in &v.batteries {
                out.push(meter(
                    &id(&format!("meter-{part_name}")),
                    *p,
                    &format!("{} battery {}", capital(part_name), percent(*p)),
                ));
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
        out.push(meter(
            &id("meter-brightness"),
            b,
            &format!("Brightness: {}", percent(b)),
        ));
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
        Some(Source::Scene) => {
            out.push(ui::label(&id("source"), "A Foxglove scene is drawing them"))
        }
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

/// The wallpaper section (#1142, UX plan `review/ux-plan-we-pause-holder.md` 2): what holds the
/// wallpapers and Resume while an app does. A reason is the app's own words, so it comes after
/// the name for it, in quotes and cut; Resume is offered only while an app's pause holds (never
/// over the user's). The wallpaper app's name is data; every other word is a constant above.
fn wallpaper_section(w: &Wallpaper) -> Value {
    let id = |s: &str| format!("we-{s}");
    let engine = w.engine_name();
    let who = holder_words(&w.holders);
    // The words the state node announces, when they are not its value (the after-lines).
    let mut say: Option<String> = None;
    let (value, tone, sub, resume) = if !w.running {
        let sub = if !w.held() {
            isnt_running(&engine)
        } else if w.holders.len() == 1 {
            format!(
                "{} {}'s pause ended with it.",
                isnt_running(&engine),
                cut_to(&w.holders[0], NAME_CUT)
            )
        } else if w.holders.is_empty() {
            format!("{} The pause ended with it.", isnt_running(&engine))
        } else {
            format!("{} Their pauses ended with it.", isnt_running(&engine))
        };
        (NOT_RUNNING.to_string(), "off", Some(sub), false)
    } else if w.user_paused {
        let sub = if w.held() {
            let first = w
                .holders
                .first()
                .map_or_else(|| "An app".into(), |n| cut_to(n, NAME_CUT));
            format!("{first} is waiting too. It won't press Play for you.")
        } else {
            press_play(&engine)
        };
        (PAUSED_BY_YOU.into(), "off", Some(sub), false)
    } else if w.play_owed && !w.held() {
        (
            RESUME_FAILED.into(),
            "active",
            Some(resume_failed_sub(&engine)),
            true,
        )
    } else if w.held() {
        let sub = if w.full_screen {
            Some(FULL_SCREEN_SUB.to_string())
        } else {
            let quoted: Vec<String> = w
                .reasons
                .iter()
                .take(2)
                .map(|r| format!("\u{201c}{}\u{201d}", cut_to(r, REASON_CUT)))
                .collect();
            let mut line = quoted.join(" \u{b7} ");
            if w.reasons.len() > 2 {
                line.push_str(" \u{b7} \u{2026}");
            }
            (!quoted.is_empty()).then_some(line)
        };
        (paused_by(&who), "active", sub, true)
    } else if let Some(after) = &w.after {
        let sub = match after {
            After::Resumed(_) if w.full_screen => {
                format!("Resumed. {FULL_SCREEN_SUB}")
            }
            other => after_words(other),
        };
        say = Some(sub.clone());
        (NONE_PAUSING.into(), "ok", Some(sub), false)
    } else if w.playing {
        (PLAYING.into(), "ok", None, false)
    } else {
        (NONE_PAUSING.into(), "ok", None, false)
    };
    let mut state = ui::value(&id("state"), &value, tone);
    state["props"]["live"] = Value::Bool(true);
    if let Some(words) = &say {
        state = ui::named(state, words);
    }
    let mut kids = vec![state];
    if let Some(s) = sub {
        kids.push(ui::label(&id("sub"), &s));
    }
    if resume {
        let mut button = ui::button("", &Button::new(RESUME, Press::Event(RESUME_NODE)));
        // Owed (#1399): no holder to name, the plain label says it (the state line says why).
        if !(w.play_owed && !w.held()) {
            button = ui::named(button, &resume_name(&who));
        }
        // The button goes once pressed: the focus moves to the state line, which then says so.
        button["props"]["focus_after"] = Value::String(STATE_NODE.into());
        kids.push(button);
    }
    if let Some(inputs) = &w.allow {
        let mut inputs = inputs.clone();
        inputs.engine_running = w.running;
        // "<wallpaper app> isn't running." is already the section's line above.
        kids.extend(
            super::we_panel::nodes(&super::we_panel::view(&inputs))
                .into_iter()
                .filter(|n| w.running || n["id"] != "we-line"),
        );
    }
    ui::section(
        WALLPAPER,
        if w.engine.is_empty() {
            SECTION_FALLBACK
        } else {
            &w.engine
        },
        kids,
    )
}

/// The three tiles and the page, for `ui.register`.
pub fn panels(v: &View) -> Vec<Value> {
    let mut out: Vec<Value> = Part::ALL.into_iter().map(|p| tile(p, v)).collect();
    let intro = match &v.wallpaper {
        Some(w) => intro_we(&w.engine_name()),
        None => INTRO.to_string(),
    };
    let mut page = vec![ui::label("intro", &intro)];
    page.extend(Part::ALL.into_iter().map(|p| section(p, v)));
    if let Some(w) = &v.wallpaper {
        page.push(wallpaper_section(w));
    }
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
#[cfg(test)]
pub fn declared() -> Vec<String> {
    let mut ids: Vec<String> = Part::ALL.iter().map(|p| p.tile_id().to_string()).collect();
    ids.push(PAGE.into());
    ids
}

#[cfg(test)]
mod tests;
