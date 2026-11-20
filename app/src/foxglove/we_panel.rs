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
pub const DISCONNECT_ALL: &str = "Disconnect all wallpapers";
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
        if l.reconnect && ui.button(RECONNECT_LABEL).clicked() {
            act = Some(Act::Reconnect);
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
            let c = ui
                .checkbox(&mut on, &r.title)
                .on_hover_text(format!("Let {} use your keyboard", r.title));
            if c.changed() {
                if let Some(k) = v.keys.get(r.slot) {
                    act = Some(Act::Allow(k.clone(), on));
                }
            }
            if let Some((word, t)) = &r.status {
                ui.label(egui::RichText::new(word).color(tone(t)));
            }
        });
        if r.reconnect {
            if let Some(h) = &r.help {
                ui.weak(h);
            }
            if ui
                .button(RECONNECT_LABEL)
                .on_hover_text(format!("Reconnect {}", r.title))
                .clicked()
            {
                act = Some(Act::Reconnect);
            }
            ui.weak(&r.capability);
        } else {
            ui.weak(&r.capability);
            if let Some(h) = &r.help {
                ui.weak(h);
            }
        }
    }
    ui.add_space(4.0);
    if v.disconnect_all && ui.button(DISCONNECT_ALL).clicked() {
        act = Some(Act::DisconnectAll);
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

/// The card's list from what Foxglove knows now. Wallpaper Engine counts as running (the list
/// must not scan processes every frame; a wallpaper that isn't on screen says so itself).
pub fn settings_card(ui: &mut egui::Ui) -> Option<String> {
    let v = view(&inputs_now(true));
    draw(ui, &v).and_then(|a| run(a).err())
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
    #[test]
    fn no_ui_string_talks_of_pasting_or_a_pairing_token() {
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
        for f in files {
            let name = f.file_name().unwrap().to_string_lossy().to_string();
            if name.starts_with("calibrate") || name == "tests.rs" || name == "theme_preview.rs" {
                continue;
            }
            let text = std::fs::read_to_string(&f).unwrap();
            // Only the code above the file's test module.
            let code = text.split("#[cfg(test)]").next().unwrap();
            for (n, line) in code.lines().enumerate() {
                let t = line.trim_start();
                if t.starts_with("//") {
                    continue;
                }
                for lit in line.split('"').skip(1).step_by(2) {
                    let low = lit.to_lowercase();
                    if !lit.starts_with("Foxglove:")
                        && (low.contains("paste ") || low.contains("pairing token"))
                    {
                        hits.push(format!("{name}:{}: {lit}", n + 1));
                    }
                }
            }
        }
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
