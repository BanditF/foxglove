//! The allow list of Hideout's Wallpaper Engine section, as a model (WE handover S5a; UX plan
//! `review/ux-plan-we-panel.md` §2 and §3). Pure: what Foxglove knows goes in as [`Inputs`], a
//! [`View`] comes out, and [`nodes`] turns the view into the tree nodes S5b puts in the page's
//! Wallpaper Engine section. Nothing here reads the disk or the pairing store itself, so every
//! row of the plan's tables has a window-free test.
//!
//! The words are the plan's, exactly. The panel says "app" for programs and "wallpaper" only for
//! Wallpaper Engine projects, and never "token", "ticket", "pair" or "paste". A file path, a
//! ticket or a token never reaches a node id, a label or the view's words: a row is known to the
//! panel by its slot in [`View::keys`], a list Foxglove keeps.

use std::collections::BTreeSet;

use serde_json::Value;

use super::live_pairing::{Link, Links};
use super::live_wire::Why;
use super::wallpaper_manifest::{self as manifest, Manifest};
use super::wallpaper_sync::Target;
use foxglove_plugin::ui;

type Notify = Box<dyn Fn() + Send + Sync>;
static NOTIFY: std::sync::Mutex<Option<Notify>> = std::sync::Mutex::new(None);

/// The hub link registers how to say "the section changed" (it refreshes the panels).
pub fn on_change(f: impl Fn() + Send + Sync + 'static) {
    *NOTIFY.lock().unwrap_or_else(|e| e.into_inner()) = Some(Box::new(f));
}

/// Something the section shows changed: a switch, a socket, the wallpapers on screen. An event,
/// never a timer; the link drops a refresh that changes nothing.
pub fn changed() {
    if let Some(f) = NOTIFY.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        f();
    }
}

/// Every wallpaper key numbered this session, oldest first. Append-only: a key's number never
/// changes while Foxglove runs, so a click made against an older tree still means its own row.
static SLOTS: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

/// The slots with `keys` appended (those not numbered yet).
fn slots_with<'a>(keys: impl Iterator<Item = &'a String>) -> Vec<String> {
    let mut s = SLOTS.lock().unwrap_or_else(|e| e.into_inner());
    for k in keys {
        if !s.contains(k) {
            s.push(k.clone());
        }
    }
    s.clone()
}

/// The inputs from what Foxglove knows now (the app; tests build [`Inputs`] themselves).
pub fn inputs_now(engine_running: bool) -> Inputs {
    let saved = super::live_feed::settings();
    let allowed: BTreeSet<String> = saved.allowed.iter().cloned().collect();
    let asks = asks(
        &manifest::known(),
        &super::wallpaper_sync::targets_now(),
        &allowed,
    );
    Inputs {
        feed_on: saved.enabled,
        engine_running,
        slots: slots_with(asks.iter().map(|a| &a.key)),
        asks,
        links: super::live_pairing::links(super::live_pairing::store()),
        hold: super::live_feed::quiet_now().map(|w| match w {
            Why::Paused => Hold::Paused,
            Why::Away => Hold::Away,
            Why::Fullscreen => Hold::FullScreenGame,
        }),
    }
}

/// What a click or switch means once it is checked against the slots the hub was shown.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Act {
    Allow(String, bool),
    Reconnect,
    DisconnectAll,
}

/// Checks `node` against the slot keys and whether Disconnect all is in force. `Err` is the fixed
/// words: a node that isn't ours, a switch without a value, a slot the hub was never shown, and a
/// Reconnect while disconnected (Reconnect would undo Disconnect all).
pub fn plan(node: &str, value: Option<bool>, keys: &[String], held: bool) -> Result<Act, String> {
    let refused = || REFUSED.to_string();
    let Some(ev) = event(node, value) else {
        return Err(refused());
    };
    let key = |n: usize| keys.get(n).cloned().ok_or_else(refused);
    match ev {
        Event::Allow(n, on) => Ok(Act::Allow(key(n)?, on)),
        Event::Reconnect(n) if held => key(n).and_then(|_| Err(refused())),
        Event::Reconnect(n) => key(n).map(|_| Act::Reconnect),
        Event::ReconnectAll => Ok(Act::Reconnect),
        Event::DisconnectAll => Ok(Act::DisconnectAll),
    }
}

/// Does what a click or switch on `node` asks, against the slot keys the hub was last shown.
/// `Err` carries the words the hub shows beside the switch (never the store's own text).
pub fn handle(node: &str, value: Option<bool>, keys: &[String]) -> Result<(), String> {
    let held = super::live_pairing::links(super::live_pairing::store()).held;
    run(plan(node, value, keys, held)?)
}

/// The allow list's section, inside the Wallpaper Engine section.
pub const ASKS: &str = "we-asks";
/// A row's switch: `we-allow:<n>`, with `n` the row's slot in [`View::keys`].
pub const ALLOW: &str = "we-allow";
/// A row's Reconnect: `we-reconnect:<n>`; `we-reconnect:all` is the section line's.
pub const RECONNECT: &str = "we-reconnect";
/// The Disconnect all button's node.
pub const DISCONNECT_ALL_NODE: &str = "we-disconnect-all";

pub const ALLOW_LABEL: &str = "Allow";
pub const HEADER: &str = "Wallpapers that want your keyboard";
pub const RECONNECT_LABEL: &str = "Reconnect";
pub const RECONNECT_ALL_LABEL: &str = "Reconnect all wallpapers";
pub const DISCONNECT_ALL: &str = "Disconnect all wallpapers";
/// The section line when some other app tried to connect while every allowed wallpaper was
/// connected (UX review of #1334, finding 2). It claims no more than the refusal showed.
pub const FOREIGN_LINE: &str = "An app that isn't one of your wallpapers tried to connect. If a wallpaper stops showing your typing, press Reconnect.";
/// What the hub shows when the module refuses a flip (the raw error goes to the log only).
pub const REFUSED: &str = "Couldn't change this. Try again.";

/// Why the feed holds back for a moment (the feed's own reason).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hold {
    FullScreenGame,
    Paused,
    /// The session is locked or the display is off (the feed can't tell them apart).
    Away,
}

impl Hold {
    fn words(self) -> &'static str {
        match self {
            Hold::FullScreenGame => "Nothing is sent while a full-screen game runs.",
            Hold::Paused => "Nothing is sent while Foxglove is paused.",
            Hold::Away => "Nothing is sent while the PC is locked or the screen is off.",
        }
    }
}

/// One wallpaper whose manifest asks for the live feed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ask {
    /// Its folder key (`wallpaper_manifest::key_of_dir`): what the allow list and the pairing
    /// store know it by. Never drawn.
    pub key: String,
    /// The manifest's title, 40 characters at most.
    pub title: String,
    /// It shows on a monitor now.
    pub on_screen: bool,
    /// The user's switch.
    pub allowed: bool,
    /// It can't take the new connection yet (Q4: its manifest lacks the marker S3 adds).
    pub needs_update: bool,
}

/// Everything the section is drawn from.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Inputs {
    /// "Live keyboard feed for wallpapers" is on.
    pub feed_on: bool,
    /// Wallpaper Engine is running.
    pub engine_running: bool,
    pub asks: Vec<Ask>,
    pub links: Links,
    /// The feed's own reason for sending nothing right now, if any.
    pub hold: Option<Hold>,
    /// Every wallpaper key the panel has numbered this session, oldest first (append-only, so a
    /// key's number never changes while Foxglove runs). A key missing here is numbered after
    /// these, in ask order.
    pub slots: Vec<String>,
}

/// The asks, from the scanned manifests, the monitors' targets and the saved allow list. Only
/// wallpapers whose manifest supports `foxglove_live` ask.
pub fn asks(manifests: &[Manifest], targets: &[Target], allowed: &BTreeSet<String>) -> Vec<Ask> {
    manifests
        .iter()
        .filter(|m| m.supports.live)
        .map(|m| {
            let key = manifest::key_of_dir(&m.dir);
            Ask {
                on_screen: targets
                    .iter()
                    .any(|t| t.supports.live && manifest::key_of_file(&t.file) == key),
                allowed: allowed.contains(&key),
                // The manifest has no marker yet (S3 adds `foxglove_ticket`): nothing is
                // told it needs an update until then.
                needs_update: false,
                title: m.title.clone(),
                key,
            }
        })
        .collect()
}

/// A tone word the hub draws (`ok`, `active`, `off`, `attention`).
pub type Tone = &'static str;

/// One row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    /// The row's stable number: what its nodes' ids carry (never its position).
    pub slot: usize,
    pub title: String,
    pub allowed: bool,
    /// The status word beside the title; none while the section line says it.
    pub status: Option<(String, Tone)>,
    /// Under the capability line.
    pub help: Option<String>,
    /// "{T} can see which key positions you press and when, never the letters."
    pub capability: String,
    /// Offer Reconnect on this row.
    pub reconnect: bool,
}

/// The one line above the list, and whether it carries Reconnect.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    pub text: String,
    pub reconnect: bool,
}

/// The section's body.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct View {
    pub line: Option<Line>,
    pub rows: Vec<Row>,
    /// The folder key behind each slot (the append-only list Foxglove keeps; the nodes carry only
    /// the slot number, which never changes for a key while Foxglove runs).
    pub keys: Vec<String>,
    /// Show **Disconnect all wallpapers**.
    pub disconnect_all: bool,
}

/// "{T} can see which key positions you press and when, never the letters."
pub fn capability(title: &str) -> String {
    format!("{title} can see which key positions you press and when, never the letters.")
}

fn old_way_line(n: usize) -> String {
    if n == 1 {
        "1 wallpaper is still connected the old way. Allow it here to keep it working.".into()
    } else {
        format!(
            "{n} wallpapers are still connected the old way. Allow them here to keep them working."
        )
    }
}

/// Whether the line about another app shows: the pairing store saw a refusal with every allowed
/// wallpaper connected, and the rows agree (each allowed wallpaper on screen is connected, so
/// the refusal cannot be one of them reloading). The pairing flag is the event; this is the
/// second look before words go on screen.
fn shared_app(i: &Inputs, order: &[&Ask], link: &dyn Fn(&Ask) -> Link) -> bool {
    i.links.foreign
        && i.feed_on
        && i.engine_running
        && !i.links.held
        && order
            .iter()
            .filter(|a| a.allowed && a.on_screen && !a.needs_update)
            .all(|a| link(a) == Link::Connected)
}

/// The section's view (plan §3). At most one section line shows, the first that holds from the
/// top of §3.1's table; rows are allowed first, then off, on-screen first, then by title, and
/// the order never depends on a status.
pub fn view(i: &Inputs) -> View {
    let mut order: Vec<&Ask> = i.asks.iter().collect();
    order.sort_by_key(|a| {
        (
            !a.allowed,
            !a.on_screen,
            a.title.to_lowercase(),
            a.key.clone(),
        )
    });
    let mut slots = i.slots.clone();
    for a in &i.asks {
        if !slots.contains(&a.key) {
            slots.push(a.key.clone());
        }
    }
    let link = |a: &Ask| i.links.by_dir.get(&a.key).copied().unwrap_or(Link::Idle);
    let say = |s: &str, t: Tone| Some((s.to_string(), t));

    let rows: Vec<Row> = order
        .iter()
        .map(|a| {
            let t = &a.title;
            let mut r = Row {
                slot: slots.iter().position(|k| *k == a.key).unwrap_or(0),
                title: t.clone(),
                allowed: a.allowed,
                status: None,
                help: None,
                capability: capability(t),
                reconnect: false,
            };
            if a.needs_update {
                r.status = say("Needs an update", "attention");
                r.help = Some(format!(
                    "Update {t} in Wallpaper Engine, then it can connect."
                ));
            } else if !i.feed_on || !i.engine_running {
                // The section line says why nothing connects.
            } else if !a.allowed {
                r.status = say("Off", "off");
            } else if i.links.held {
                r.status = say("Disconnected", "off");
            } else if !a.on_screen {
                r.status = say("Not on screen", "off");
                r.help = Some("Connects when it's on screen.".into());
            } else {
                match link(a) {
                    Link::Connected => {
                        r.status = say("Connected", "ok");
                        r.help = i.hold.map(|h| h.words().to_string());
                    }
                    Link::Reconnecting => r.status = say("Reconnecting\u{2026}", "active"),
                    Link::Capped => {
                        r.status = say("Not connected", "attention");
                        r.help = Some(format!("{t} isn't getting your keyboard."));
                        r.reconnect = true;
                    }
                    Link::Connecting | Link::Idle => {
                        r.status = say("Connecting\u{2026}", "active");
                    }
                }
            }
            r
        })
        .collect();

    let any_live = order.iter().any(|a| {
        a.allowed
            && !a.needs_update
            && matches!(
                link(a),
                Link::Connected | Link::Connecting | Link::Reconnecting
            )
    });
    let line = if !i.feed_on {
        Some((
            "The live keyboard feed is off. Wallpapers you allow connect when you turn it on.",
            false,
        ))
    } else if !i.engine_running {
        Some(("Wallpaper Engine isn't running.", false))
    } else if i.links.held {
        Some((
            "All wallpapers are disconnected. They stay allowed.",
            true,
        ))
    } else if shared_app(i, &order, &link) {
        Some((FOREIGN_LINE, true))
    } else if i.links.old_way == 0 && i.asks.is_empty() {
        Some((
            "No wallpaper has asked for your keyboard yet. Wallpapers that can show your typing appear here.",
            false,
        ))
    } else {
        None
    }
    .map(|(text, reconnect)| Line {
        text: text.to_string(),
        reconnect,
    })
    .or_else(|| {
        (i.links.old_way > 0).then(|| Line {
            text: old_way_line(i.links.old_way),
            reconnect: false,
        })
    });
    // The button must tell the truth: old-way sockets count, and it is hidden when none exists.
    let disconnect_all =
        i.feed_on && i.engine_running && !i.links.held && (any_live || i.links.old_way > 0);
    View {
        line,
        rows,
        keys: slots,
        disconnect_all,
    }
}

/// The view as tree nodes, for the Wallpaper Engine section's children (S5b). Node ids carry a
/// row index only.
pub fn nodes(v: &View) -> Vec<Value> {
    let mut out = Vec::new();
    if let Some(l) = &v.line {
        out.push(ui::label("we-line", &l.text));
        if l.reconnect {
            out.push(ui::named(
                ui::event_button(&format!("{RECONNECT}:all"), RECONNECT_LABEL),
                "Reconnect all wallpapers",
            ));
        }
    }
    if !v.rows.is_empty() {
        let rows = v
            .rows
            .iter()
            .map(|r| {
                let n = r.slot;
                // The switch comes first, so it sits at one x whatever the status word is.
                let mut head = vec![
                    ui::named(
                        ui::toggle(&format!("{ALLOW}:{n}"), ALLOW_LABEL, r.allowed),
                        &format!("Let {} use your keyboard", r.title),
                    ),
                    ui::value(&format!("we-title:{n}"), &r.title, "neutral"),
                ];
                if let Some((word, tone)) = &r.status {
                    head.push(ui::value(&format!("we-status:{n}"), word, tone));
                }
                let help = r
                    .help
                    .as_ref()
                    .map(|h| ui::label(&format!("we-help:{n}"), h));
                let button = r.reconnect.then(|| {
                    ui::named(
                        ui::event_button(&format!("{RECONNECT}:{n}"), RECONNECT_LABEL),
                        &format!("Reconnect {}", r.title),
                    )
                });
                let cap = ui::label(&format!("we-cap:{n}"), &r.capability);
                let mut kids = vec![ui::row(&format!("we-head:{n}"), head)];
                if r.reconnect {
                    // The line that needs the user comes before the boilerplate.
                    kids.extend(help);
                    kids.extend(button);
                    kids.push(cap);
                } else {
                    kids.push(cap);
                    kids.extend(help);
                }
                ui::column(&format!("we-row:{n}"), kids)
            })
            .collect();
        out.push(ui::section(ASKS, HEADER, rows));
    }
    if v.disconnect_all {
        out.push(ui::event_button(DISCONNECT_ALL_NODE, DISCONNECT_ALL));
    }
    out
}

/// The same list in Foxglove's own Settings (no hub): one checkbox per wallpaper, the status,
/// Reconnect and Disconnect all, from the same [`View`] the Hideout section draws. Returns what
/// the user did this frame; [`run`] does it.
pub fn draw(ui: &mut egui::Ui, v: &View) -> Option<Act> {
    let pal = super::theme::palette(ui);
    let tone = |t: Tone| match t {
        "ok" => pal.ok,
        "attention" => pal.warn,
        "active" => pal.accent,
        _ => pal.muted,
    };
    let mut act = None;
    if let Some(l) = &v.line {
        ui.weak(&l.text);
        if l.reconnect {
            let b = ui.button(RECONNECT_LABEL);
            b.widget_info(|| {
                egui::WidgetInfo::labeled(egui::WidgetType::Button, true, RECONNECT_ALL_LABEL)
            });
            if b.on_hover_text(RECONNECT_ALL_LABEL).clicked() {
                act = Some(Act::Reconnect);
            }
        }
    }
    if !v.rows.is_empty() {
        ui.add_space(4.0);
        ui.strong(HEADER);
    }
    for r in &v.rows {
        ui.add_space(4.0);
        let mut on = r.allowed;
        ui.horizontal(|ui| {
            let name = format!("Let {} use your keyboard", r.title);
            let c = ui.checkbox(&mut on, &r.title).on_hover_text(&name);
            // The accessible name is the plan's, and the status and what it sees ride along.
            c.widget_info(|| {
                egui::WidgetInfo::selected(egui::WidgetType::Checkbox, true, r.allowed, &name)
            });
            let said = match &r.status {
                Some((word, _)) => format!("{word}. {}", r.capability),
                None => r.capability.clone(),
            };
            ui.ctx()
                .accesskit_node_builder(c.id, |n| n.set_description(said));
            if c.changed() {
                if let Some(k) = v.keys.get(r.slot) {
                    act = Some(Act::Allow(k.clone(), on));
                }
            }
            if let Some((word, t)) = &r.status {
                ui.label(egui::RichText::new(word).color(tone(t)));
            }
        });
        // What it sees, then why it isn't connected, then the button that helps.
        ui.weak(&r.capability);
        if let Some(h) = &r.help {
            ui.weak(h);
        }
        if r.reconnect {
            let name = format!("Reconnect {}", r.title);
            let b = ui.button(RECONNECT_LABEL);
            b.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &name));
            if b.on_hover_text(&name).clicked() {
                act = Some(Act::Reconnect);
            }
        }
    }
    if v.disconnect_all {
        ui.add_space(8.0);
        ui.separator();
        if ui.button(DISCONNECT_ALL).clicked() {
            act = Some(Act::DisconnectAll);
        }
    }
    act
}

/// Does `act`, then tells the panels. The error is the fixed words.
pub fn run(act: Act) -> Result<(), String> {
    match act {
        Act::Allow(key, on) => super::live_feed::allow_wallpaper(&key, on),
        Act::Reconnect => {
            super::live_feed::reconnect();
            Ok(())
        }
        Act::DisconnectAll => {
            super::live_feed::disconnect_all();
            Ok(())
        }
    }
}

/// Is Wallpaper Engine running, for the card. A process snapshot is read when the card opens
/// (not drawn in the last second) and after each click: never per frame and never on a timer.
fn engine_running_for_card(after_click: bool) -> bool {
    static SEEN: std::sync::Mutex<Option<(std::time::Instant, bool)>> = std::sync::Mutex::new(None);
    let mut s = SEEN.lock().unwrap_or_else(|e| e.into_inner());
    let now = std::time::Instant::now();
    let fresh =
        matches!(*s, Some((at, _)) if now.duration_since(at) < std::time::Duration::from_secs(1));
    let running = match *s {
        Some((_, r)) if fresh && !after_click => r,
        _ => {
            #[cfg(not(test))]
            let r = super::wallpaper_engine::running_now();
            #[cfg(test)]
            let r = true;
            r
        }
    };
    *s = Some((now, running));
    running
}

/// The card's list from what Foxglove knows now; `Some` is what a click did.
pub fn settings_card(ui: &mut egui::Ui) -> Option<Result<(), String>> {
    let v = view(&inputs_now(engine_running_for_card(false)));
    let r = draw(ui, &v).map(run);
    if r.is_some() {
        engine_running_for_card(true);
    }
    r
}

/// What a click or a switch on `node` means (S5b's router uses it).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    Allow(usize, bool),
    Reconnect(usize),
    ReconnectAll,
    DisconnectAll,
}

/// The node's meaning; `None` for a node that isn't ours or an index that isn't a number.
/// `value` is the toggle's new state.
pub fn event(node: &str, value: Option<bool>) -> Option<Event> {
    if node == DISCONNECT_ALL_NODE {
        return Some(Event::DisconnectAll);
    }
    let (kind, n) = node.split_once(':')?;
    match (kind, n) {
        (RECONNECT, "all") => Some(Event::ReconnectAll),
        (RECONNECT, n) => n.parse().ok().map(Event::Reconnect),
        (ALLOW, n) => Some(Event::Allow(n.parse().ok()?, value?)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn ask(key: &str, title: &str, on_screen: bool, allowed: bool) -> Ask {
        Ask {
            key: key.into(),
            title: title.into(),
            on_screen,
            allowed,
            needs_update: false,
        }
    }

    fn links(pairs: &[(&str, Link)]) -> Links {
        Links {
            by_dir: pairs
                .iter()
                .map(|(k, l)| (k.to_string(), *l))
                .collect::<BTreeMap<_, _>>(),
            ..Links::default()
        }
    }

    fn base(asks: Vec<Ask>, links: Links) -> Inputs {
        Inputs {
            feed_on: true,
            engine_running: true,
            asks,
            links,
            hold: None,
            slots: Vec::new(),
        }
    }

    fn one(link: Link, on_screen: bool) -> View {
        view(&base(
            vec![ask("hud", "Bandit HUD", on_screen, true)],
            links(&[("hud", link)]),
        ))
    }

    fn status(v: &View) -> Option<(&str, Tone)> {
        v.rows[0].status.as_ref().map(|(s, t)| (s.as_str(), *t))
    }

    fn two_connected() -> Inputs {
        let mut i = base(
            vec![
                ask("hud", "Bandit HUD", true, true),
                ask("courier", "Courier", true, true),
            ],
            links(&[("hud", Link::Connected), ("courier", Link::Connected)]),
        );
        i.links.foreign = true;
        i
    }

    #[test]
    fn another_app_gets_one_section_line_with_reconnect_and_no_row_changes() {
        let i = two_connected();
        let v = view(&i);
        let l = v.line.clone().unwrap();
        assert_eq!(
            l.text,
            "An app that isn't one of your wallpapers tried to connect. If a wallpaper stops showing your typing, press Reconnect."
        );
        assert!(l.reconnect);
        let mut calm = i.clone();
        calm.links.foreign = false;
        assert_eq!(v.rows, view(&calm).rows, "rows stay plain Connected");
        assert!(v
            .rows
            .iter()
            .all(|r| r.status.as_ref().unwrap().0 == "Connected"));
        assert!(v.rows.iter().all(|r| !r.reconnect && r.help.is_none()));
        // The same Reconnect the plan's section line has, named for the screen reader.
        let tree = nodes(&v);
        let btn = tree
            .iter()
            .find(|n| n["id"] == "we-reconnect:all")
            .expect("the section's Reconnect");
        assert_eq!(btn["props"]["name"], "Reconnect all wallpapers");
    }

    #[test]
    fn the_line_about_another_app_needs_every_allowed_wallpaper_connected() {
        let mut i = two_connected();
        // One wallpaper is reloading: the refusal may be its own.
        i.links.by_dir.insert("courier".into(), Link::Reconnecting);
        assert_eq!(view(&i).line, None);
        i.links.by_dir.insert("courier".into(), Link::Capped);
        assert_eq!(view(&i).line, None);
        // A wallpaper that is off, or not on screen, does not hold the line back.
        let mut j = two_connected();
        j.asks.push(ask("neon", "Neon Rain", true, false));
        j.asks.push(ask("atlas", "Atlas", false, true));
        assert!(view(&j).line.unwrap().text.starts_with("An app that isn't"));
        // Not while disconnected, paused-off, or without the engine: those lines say why.
        j.links.held = true;
        assert!(view(&j).line.unwrap().text.starts_with("All wallpapers"));
        j.links.held = false;
        j.engine_running = false;
        assert_eq!(
            view(&j).line.unwrap().text,
            "Wallpaper Engine isn't running."
        );
    }

    #[test]
    fn the_line_about_another_app_uses_no_forbidden_words() {
        let words = FOREIGN_LINE.to_lowercase();
        for bad in ["token", "ticket", "pair", "paste", "shared", "using"] {
            assert!(!words.contains(bad), "{bad}");
        }
    }

    #[test]
    fn section_lines_follow_the_plans_table_top_down() {
        let mut i = base(vec![ask("hud", "Bandit HUD", true, true)], Links::default());
        assert_eq!(view(&i).line, None);
        i.links.old_way = 1;
        assert_eq!(
            view(&i).line.unwrap().text,
            "1 wallpaper is still connected the old way. Allow it here to keep it working."
        );
        i.links.old_way = 3;
        assert_eq!(
            view(&i).line.unwrap().text,
            "3 wallpapers are still connected the old way. Allow them here to keep them working."
        );
        i.links.held = true;
        let l = view(&i).line.unwrap();
        assert_eq!(
            l.text,
            "All wallpapers are disconnected. They stay allowed."
        );
        assert!(l.reconnect, "the line carries Reconnect");
        i.engine_running = false;
        let l = view(&i).line.unwrap();
        assert_eq!(l.text, "Wallpaper Engine isn't running.");
        assert!(!l.reconnect);
        i.feed_on = false;
        assert_eq!(
            view(&i).line.unwrap().text,
            "The live keyboard feed is off. Wallpapers you allow connect when you turn it on."
        );
        let none = view(&base(vec![], Links::default()));
        assert_eq!(
            none.line.unwrap().text,
            "No wallpaper has asked for your keyboard yet. Wallpapers that can show your typing appear here."
        );
        assert!(!none.disconnect_all);
        assert_eq!(nodes(&view(&base(vec![], Links::default()))).len(), 1);
    }

    #[test]
    fn every_row_state_has_its_words_tone_help_and_button() {
        let v = one(Link::Connected, true);
        assert_eq!(status(&v), Some(("Connected", "ok")));
        assert_eq!(v.rows[0].help, None);
        assert!(v.disconnect_all);

        // The feed's own reason, in its own words.
        for (h, text) in [
            (
                Hold::FullScreenGame,
                "Nothing is sent while a full-screen game runs.",
            ),
            (Hold::Paused, "Nothing is sent while Foxglove is paused."),
            (
                Hold::Away,
                "Nothing is sent while the PC is locked or the screen is off.",
            ),
        ] {
            let mut i = base(
                vec![ask("hud", "Bandit HUD", true, true)],
                links(&[("hud", Link::Connected)]),
            );
            i.hold = Some(h);
            let v = view(&i);
            assert_eq!(status(&v), Some(("Connected", "ok")));
            assert_eq!(v.rows[0].help.as_deref(), Some(text));
        }

        assert_eq!(
            status(&one(Link::Connecting, true)),
            Some(("Connecting\u{2026}", "active"))
        );
        assert_eq!(
            status(&one(Link::Idle, true)),
            Some(("Connecting\u{2026}", "active"))
        );
        assert_eq!(
            status(&one(Link::Reconnecting, true)),
            Some(("Reconnecting\u{2026}", "active"))
        );

        let v = one(Link::Capped, true);
        assert_eq!(status(&v), Some(("Not connected", "attention")));
        assert_eq!(
            v.rows[0].help.as_deref(),
            Some("Bandit HUD isn't getting your keyboard.")
        );
        assert!(v.rows[0].reconnect);
        assert!(!v.disconnect_all, "nothing is connected");

        let v = one(Link::Idle, false);
        assert_eq!(status(&v), Some(("Not on screen", "off")));
        assert_eq!(
            v.rows[0].help.as_deref(),
            Some("Connects when it's on screen.")
        );

        let v = view(&base(
            vec![ask("neon", "Neon Rain (Workshop)", false, false)],
            Links::default(),
        ));
        assert_eq!(status(&v), Some(("Off", "off")));
        assert!(!v.rows[0].allowed && v.rows[0].help.is_none() && !v.rows[0].reconnect);

        // Wallpaper Engine not running: the switches stay, the status words go.
        let mut i = base(
            vec![
                ask("hud", "Bandit HUD", true, true),
                ask("neon", "Neon", false, false),
            ],
            Links::default(),
        );
        i.engine_running = false;
        let v = view(&i);
        assert!(v.rows.iter().all(|r| r.status.is_none() && !r.reconnect));
        assert!(!v.disconnect_all);

        // Disconnect all in force.
        let mut i = base(
            vec![ask("hud", "Bandit HUD", true, true)],
            links(&[("hud", Link::Idle)]),
        );
        i.links.held = true;
        let v = view(&i);
        assert_eq!(status(&v), Some(("Disconnected", "off")));
        assert!(
            !v.rows[0].reconnect && v.rows[0].help.is_none(),
            "the section has Reconnect"
        );
        assert!(!v.disconnect_all);

        // Too old for the handover, whatever the switch says.
        for allowed in [false, true] {
            let mut a = ask("hud", "Bandit HUD", true, allowed);
            a.needs_update = true;
            let v = view(&base(vec![a], Links::default()));
            assert_eq!(status(&v), Some(("Needs an update", "attention")));
            assert_eq!(
                v.rows[0].help.as_deref(),
                Some("Update Bandit HUD in Wallpaper Engine, then it can connect.")
            );
        }
    }

    #[test]
    fn every_row_says_what_allow_gives_even_when_off() {
        let v = view(&base(
            vec![
                ask("a", "Courier", true, false),
                ask("b", "Bandit HUD", true, true),
            ],
            Links::default(),
        ));
        for r in &v.rows {
            assert_eq!(
                r.capability,
                format!(
                    "{} can see which key positions you press and when, never the letters.",
                    r.title
                )
            );
        }
    }

    #[test]
    fn old_way_sockets_make_disconnect_all_show() {
        let mut i = base(
            vec![ask("hud", "Bandit HUD", true, false)],
            Links::default(),
        );
        assert!(!view(&i).disconnect_all);
        i.links.old_way = 1;
        assert!(view(&i).disconnect_all);
    }

    #[test]
    fn rows_are_allowed_first_then_on_screen_then_title_and_never_move_with_status() {
        let asks = vec![
            ask("e", "Neon Rain (Workshop)", false, false),
            ask("d", "Zed", true, false),
            ask("c", "Courier", false, true),
            ask("b", "Bandit HUD", true, true),
            ask("a", "Atlas", true, true),
        ];
        let titles = |v: &View| v.rows.iter().map(|r| r.title.clone()).collect::<Vec<_>>();
        let a = view(&base(asks.clone(), Links::default()));
        assert_eq!(
            titles(&a),
            [
                "Atlas",
                "Bandit HUD",
                "Courier",
                "Zed",
                "Neon Rain (Workshop)"
            ]
        );
        assert_eq!(
            a.keys,
            ["e", "d", "c", "b", "a"],
            "slots follow first sight, never the sort"
        );
        let b = view(&base(
            asks,
            links(&[
                ("a", Link::Capped),
                ("b", Link::Connected),
                ("c", Link::Reconnecting),
            ]),
        ));
        assert_eq!(titles(&a), titles(&b));
    }

    #[test]
    fn nodes_carry_an_index_never_a_path_and_events_read_it_back() {
        let hud = "d:/steam/workshop/content/431960/123";
        let v = view(&base(
            vec![
                ask(hud, "Bandit HUD", true, true),
                ask("d:/projects/courier", "Courier", false, false),
            ],
            links(&[(hud, Link::Capped)]),
        ));
        let tree = serde_json::to_string(&nodes(&v)).unwrap();
        for private in ["steam", "workshop", "431960", "d:/"] {
            assert!(
                !tree.to_lowercase().contains(private),
                "{private} in {tree}"
            );
        }
        for id in ["we-allow:0", "we-allow:1", "we-reconnect:0", "we-asks"] {
            assert!(tree.contains(&format!("\"id\":\"{id}\"")), "{id}");
        }
        assert!(
            !tree.contains("we-reconnect:1"),
            "the second row has no button"
        );
        assert!(!tree.contains(DISCONNECT_ALL_NODE), "nothing is connected");

        // Each switch and button has its own accessible name.
        let named: Vec<(String, String)> = {
            let mut acc = Vec::new();
            fn walk(n: &Value, v: &mut Vec<(String, String)>) {
                if let Some(name) = n["props"]["name"].as_str() {
                    v.push((n["id"].as_str().unwrap().into(), name.into()));
                }
                for c in n["children"].as_array().into_iter().flatten() {
                    walk(c, v);
                }
            }
            for n in nodes(&v) {
                walk(&n, &mut acc);
            }
            acc
        };
        let has = |id: &str, name: &str| named.iter().any(|(i, n)| i == id && n == name);
        assert!(has("we-allow:0", "Let Bandit HUD use your keyboard"));
        assert!(has("we-allow:1", "Let Courier use your keyboard"));
        assert!(has("we-reconnect:0", "Reconnect Bandit HUD"));
        let mut all = view(&base(vec![], Links::default()));
        all.line = Some(Line {
            text: "x".into(),
            reconnect: true,
        });
        assert!(nodes(&all)
            .iter()
            .any(|n| n["props"]["name"] == "Reconnect all wallpapers"));

        assert_eq!(event("we-allow:3", Some(true)), Some(Event::Allow(3, true)));
        assert_eq!(event("we-allow:3", None), None, "a switch without a value");
        assert_eq!(event("we-allow:x", Some(true)), None);
        assert_eq!(event("we-reconnect:2", None), Some(Event::Reconnect(2)));
        assert_eq!(event("we-reconnect:all", None), Some(Event::ReconnectAll));
        assert_eq!(event(DISCONNECT_ALL_NODE, None), Some(Event::DisconnectAll));
        assert_eq!(event("we-resume", None), None);
    }

    /// The panel never uses the pairing words (plan §3.2), whatever the state.
    #[test]
    fn no_word_of_the_old_setup_shows() {
        let mut texts = Vec::new();
        for (feed, engine, held, old, link, on_screen, allowed) in [
            (true, true, false, 0usize, Link::Capped, true, true),
            (true, true, true, 2, Link::Idle, true, true),
            (true, true, false, 1, Link::Connected, false, false),
            (false, true, false, 0, Link::Idle, true, true),
            (true, false, false, 0, Link::Idle, true, false),
        ] {
            let mut i = base(
                vec![ask("hud", "Bandit HUD", on_screen, allowed)],
                links(&[("hud", link)]),
            );
            (i.feed_on, i.engine_running) = (feed, engine);
            (i.links.held, i.links.old_way) = (held, old);
            i.hold = Some(Hold::Away);
            texts.push(serde_json::to_string(&nodes(&view(&i))).unwrap());
        }
        texts.push(REFUSED.into());
        for t in texts {
            let t = t.to_lowercase();
            for word in ["token", "ticket", "pair", "paste"] {
                assert!(!t.contains(word), "{word} in {t}");
            }
        }
    }

    #[test]
    fn a_rows_number_never_changes_when_the_rows_reorder() {
        let mut i = base(
            vec![
                ask("a", "Atlas", true, false),
                ask("b", "Bandit", true, false),
            ],
            Links::default(),
        );
        let first = view(&i);
        assert_eq!(first.keys, ["a", "b"]);
        let slot = |v: &View, t: &str| v.rows.iter().find(|r| r.title == t).unwrap().slot;
        assert_eq!((slot(&first, "Atlas"), slot(&first, "Bandit")), (0, 1));
        // Bandit is allowed: it sorts first, and a new manifest appears between them.
        i.slots = first.keys.clone();
        i.asks[1].allowed = true;
        i.asks.push(ask("0", "Aardvark", false, false));
        let second = view(&i);
        assert_eq!(second.rows[0].title, "Bandit");
        assert_eq!((slot(&second, "Atlas"), slot(&second, "Bandit")), (0, 1));
        assert_eq!(second.keys, ["a", "b", "0"]);
        let ids = serde_json::to_string(&nodes(&second)).unwrap();
        assert!(
            ids.contains("we-allow:0") && ids.contains("we-allow:1") && ids.contains("we-allow:2")
        );
        // A click made against the first tree still means the same wallpaper.
        assert_eq!(
            plan("we-allow:0", Some(true), &second.keys, false),
            Ok(Act::Allow("a".into(), true))
        );
    }

    #[test]
    fn the_switch_is_first_in_the_row() {
        let v = one(Link::Connected, true);
        let rows = serde_json::to_string(&nodes(&v)).unwrap();
        let (sw, title, st) = (
            rows.find("we-allow:0").unwrap(),
            rows.find("we-title:0").unwrap(),
            rows.find("we-status:0").unwrap(),
        );
        assert!(sw < title && title < st);
    }

    #[test]
    fn refusals_are_fixed_words_and_never_reach_the_store() {
        let keys = vec!["a".to_string()];
        for (node, value, held) in [
            ("we-allow:0", None, false),
            ("we-allow:x", Some(true), false),
            ("we-allow:9", Some(true), false),
            ("we-mystery:0", None, false),
            ("we-reconnect:9", None, false),
            ("we-reconnect:0", None, true),
            ("we-nothing", None, false),
        ] {
            assert_eq!(
                plan(node, value, &keys, held),
                Err(REFUSED.to_string()),
                "{node} {value:?} {held}"
            );
        }
        assert_eq!(
            plan("we-reconnect:all", None, &keys, true),
            Ok(Act::Reconnect)
        );
        assert_eq!(
            plan("we-reconnect:0", None, &keys, false),
            Ok(Act::Reconnect)
        );
        assert_eq!(
            plan(DISCONNECT_ALL_NODE, None, &keys, false),
            Ok(Act::DisconnectAll)
        );
    }

    /// S5c: no string the app shows tells the user to paste a pairing token (the Calibrate
    /// copy-paste is another feature, exempt by path; log lines start with "Foxglove:").
    /// The code above a file's test module: cut at the `#[cfg(test)]` that comes right before
    /// `mod ...`, not at the first one (a single test-only fn would hide the rest).
    fn non_test_code(text: &str) -> String {
        let text = text.replace("\r\n", "\n");
        match text.find("#[cfg(test)]\nmod ") {
            Some(i) => text[..i].to_string(),
            None => text,
        }
    }

    /// Words a user must never read (plan 3.2): paste, pair, token, ticket in any form.
    fn forbidden_word(lit: &str) -> bool {
        lit.to_lowercase()
            .split(|c: char| !c.is_alphabetic())
            .any(|w| {
                matches!(
                    w,
                    "paste"
                        | "pasted"
                        | "pastes"
                        | "pasting"
                        | "pair"
                        | "pairs"
                        | "paired"
                        | "pairing"
                        | "token"
                        | "tokens"
                        | "ticket"
                        | "tickets"
                )
            })
    }

    /// Every string literal in `code` that holds a forbidden word, with its line. Log lines
    /// (starting "Foxglove:") are not UI. Handles escapes, continued literals, raw strings,
    /// char literals and comments.
    fn forbidden_literals(code: &str) -> Vec<(usize, String)> {
        let c: Vec<char> = code.chars().collect();
        let (mut i, mut line, mut hits) = (0, 1, Vec::new());
        while i < c.len() {
            let raw_start = c[i] == 'r'
                && (i == 0 || !(c[i - 1].is_alphanumeric() || c[i - 1] == '_'))
                && matches!(c.get(i + 1), Some('"') | Some('#'));
            if c[i] == '\n' {
                line += 1;
            } else if c[i] == '/' && c.get(i + 1) == Some(&'/') {
                while i < c.len() && c[i] != '\n' {
                    i += 1;
                }
                continue;
            } else if c[i] == '\'' {
                // 'x' or '\x' is a char literal; a lifetime has no closing quote.
                if c.get(i + 1) == Some(&'\\') {
                    i += 3;
                } else if c.get(i + 2) == Some(&'\'') {
                    i += 2;
                }
            } else if c[i] == '"' || raw_start {
                let raw = c[i] == 'r';
                let mut j = i;
                let mut hashes = 0;
                if raw {
                    j += 1;
                    while c.get(j) == Some(&'#') {
                        hashes += 1;
                        j += 1;
                    }
                    if c.get(j) != Some(&'"') {
                        i += 1;
                        continue;
                    }
                }
                let start_line = line;
                j += 1;
                let mut lit = String::new();
                while j < c.len() {
                    if !raw && c[j] == '\\' {
                        if c.get(j + 1) == Some(&'\n') {
                            line += 1;
                            // A continued line: the next line's indent is skipped.
                            lit.push(' ');
                        }
                        j += 2;
                        continue;
                    }
                    if c[j] == '"' && (0..hashes).all(|k| c.get(j + 1 + k) == Some(&'#')) {
                        break;
                    }
                    if c[j] == '\n' {
                        line += 1;
                    }
                    lit.push(c[j]);
                    j += 1;
                }
                if !lit.starts_with("Foxglove:") && forbidden_word(&lit) {
                    hits.push((start_line, lit));
                }
                i = j + 1 + hashes;
                continue;
            }
            i += 1;
        }
        hits
    }

    /// Screen readers get the plan's names: "Let {T} use your keyboard" for the switch, "Reconnect
    /// {T}" for a row's button, "Reconnect all wallpapers" for the section's.
    #[test]
    fn the_card_gives_screen_readers_the_plans_names() {
        let mut i = Inputs {
            feed_on: true,
            engine_running: true,
            asks: vec![
                ask("hud", "Bandit HUD", true, true),
                ask("courier", "Courier", true, true),
            ],
            links: links(&[("hud", Link::Connected), ("courier", Link::Capped)]),
            hold: None,
            slots: Vec::new(),
        };
        i.links.held = false;
        let mut v = view(&i);
        v.line = Some(Line {
            text: "Held.".into(),
            reconnect: true,
        });
        let ctx = crate::foxglove::theme::headless::ctx(crate::foxglove::theme::Choice::Dark);
        ctx.enable_accesskit();
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(600.0, 800.0),
            )),
            ..Default::default()
        };
        let mut out = ctx.run_ui(raw, |ui| {
            draw(ui, &v);
        });
        out.textures_delta.clear();
        let tree = out.platform_output.accesskit_update.expect("a tree");
        let names: Vec<String> = tree
            .nodes
            .iter()
            .filter_map(|(_, n)| n.label().map(str::to_owned))
            .collect();
        for want in [
            "Let Bandit HUD use your keyboard",
            "Let Courier use your keyboard",
            "Reconnect Courier",
            "Reconnect all wallpapers",
        ] {
            assert!(names.iter().any(|n| n == want), "{want} in {names:#?}");
        }
        let described = tree
            .nodes
            .iter()
            .any(|(_, n)| n.description().is_some_and(|d| d.starts_with("Connected.")));
        assert!(described, "the status rides on the switch");
    }

    #[test]
    fn the_guard_sees_what_it_should() {
        // The old paste line, back in a card, is caught even after a test-only fn.
        let src = "#[cfg(test)]\nfn handshake() {}\nfn group() { ui.weak(\"Paste it into the wallpaper.\"); }\n#[cfg(test)]\nmod tests { fn t() { \"token\"; } }\n";
        let code = non_test_code(src);
        assert!(code.contains("fn group"), "{code}");
        assert!(!code.contains("mod tests"));
        assert_eq!(forbidden_literals(&code).len(), 1);
        // Whole words, continuation lines and log lines.
        assert!(forbidden_word("a paired wallpaper"));
        assert!(forbidden_word("Pasted."));
        assert!(!forbidden_word("a pastel colour"));
        let two = "let a = \"first \\\n    pairing token\";";
        assert_eq!(forbidden_literals(two).len(), 1);
        let log = "eprintln!(\"Foxglove: live feed: new pairing token\");";
        assert!(forbidden_literals(log).is_empty());
        assert!(forbidden_literals("// \"paste\"\nlet c = '\"'; let n = 1;").is_empty());
    }

    #[test]
    fn no_ui_string_talks_of_pasting_pairing_tokens_or_tickets() {
        fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            for e in std::fs::read_dir(dir).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    walk(&p, out);
                } else if p.extension().is_some_and(|x| x == "rs") {
                    out.push(p);
                }
            }
        }
        let mut files = Vec::new();
        walk(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
            &mut files,
        );
        let mut hits = Vec::new();
        let mut card_seen = false;
        for f in files {
            let name = f.file_name().unwrap().to_string_lossy().to_string();
            if name.starts_with("calibrate") || name == "tests.rs" || name == "theme_preview.rs" {
                continue;
            }
            let code = non_test_code(&std::fs::read_to_string(&f).unwrap());
            // Wire words, log text, secret masking and Bluetooth pairing are not the panel's.
            let exempt = |lit: &str| {
                matches!(
                    (name.as_str(), lit.split_whitespace().next().unwrap_or("")),
                    ("app_log.rs", "token")
                        | ("diagnostics.rs", "2026-10-25")
                        | ("live_feed.rs", "token")
                        | ("live_wire.rs", "Tickets({}" | "wrong" | "live" | "token")
                        | ("wallpaper_sync.rs", "foxglove_ticket")
                        | ("consumer.rs" | "keyboard.rs", "Paste")
                        | ("zmk_rpc.rs", "Failed")
                )
            };
            if name == "live_feed.rs" {
                card_seen = code.contains("pub fn group(");
            }
            for (n, lit) in forbidden_literals(&code) {
                if exempt(&lit) {
                    continue;
                }
                hits.push(format!("{name}:{n}: {lit}"));
            }
        }
        assert!(card_seen, "the card's code in live_feed.rs is scanned");
        assert!(hits.is_empty(), "{hits:#?}");
    }

    #[test]
    fn no_manifest_no_ask() {
        assert!(asks(&[], &[], &BTreeSet::new()).is_empty());
    }

    /// Review images (S5a): with `WE_REVIEW_OUT` set, writes every §3 state's tree as JSON for the
    /// window-free renderer in `hideout-panels`. Without it this only checks the trees build.
    #[test]
    fn writes_the_review_scenarios() {
        let hud = "d:/we/hud";
        let mk = |feed: bool, engine: bool, asks: Vec<Ask>, l: Links, hold| Inputs {
            feed_on: feed,
            engine_running: engine,
            asks,
            links: l,
            hold,
            slots: Vec::new(),
        };
        let three = |a: bool, b: bool, c: bool| {
            vec![
                ask(hud, "Bandit HUD", true, a),
                ask("d:/we/courier", "Courier", false, b),
                ask("d:/we/neon", "Neon Rain (Workshop)", false, c),
            ]
        };
        let mut held = links(&[]);
        held.held = true;
        let mut old = links(&[]);
        old.old_way = 2;
        let mut upd = three(false, false, false);
        upd[2].needs_update = true;
        let scenarios: Vec<(&str, Inputs)> = vec![
            (
                "01-connected-and-not-on-screen",
                mk(
                    true,
                    true,
                    three(true, true, false),
                    links(&[(hud, Link::Connected)]),
                    None,
                ),
            ),
            (
                "02-held-by-a-game",
                mk(
                    true,
                    true,
                    three(true, false, false),
                    links(&[(hud, Link::Connected)]),
                    Some(Hold::FullScreenGame),
                ),
            ),
            (
                "03-connecting-and-reconnecting",
                mk(
                    true,
                    true,
                    vec![
                        ask(hud, "Bandit HUD", true, true),
                        ask("d:/we/courier", "Courier", true, true),
                    ],
                    links(&[
                        (hud, Link::Connecting),
                        ("d:/we/courier", Link::Reconnecting),
                    ]),
                    None,
                ),
            ),
            (
                "04-needs-you-reconnect",
                mk(
                    true,
                    true,
                    three(true, false, false),
                    links(&[(hud, Link::Capped)]),
                    None,
                ),
            ),
            ("05-needs-an-update", mk(true, true, upd, links(&[]), None)),
            (
                "06-after-disconnect-all",
                mk(true, true, three(true, true, false), held, None),
            ),
            (
                "07-old-way-still-connected",
                mk(true, true, three(false, false, false), old, None),
            ),
            (
                "08-engine-not-running",
                mk(true, false, three(true, false, false), links(&[]), None),
            ),
            (
                "09-feed-off",
                mk(false, true, three(true, false, false), links(&[]), None),
            ),
            (
                "10-nothing-asked-yet",
                mk(true, true, vec![], links(&[]), None),
            ),
        ];
        let out: Vec<Value> = scenarios
            .iter()
            .map(|(name, i)| {
                let n = nodes(&view(i));
                assert!(!n.is_empty(), "{name}");
                serde_json::json!({"name": name, "nodes": n})
            })
            .collect();
        if let Some(path) = std::env::var_os("WE_REVIEW_OUT") {
            std::fs::write(path, serde_json::to_string_pretty(&out).unwrap()).unwrap();
        }
    }
}
