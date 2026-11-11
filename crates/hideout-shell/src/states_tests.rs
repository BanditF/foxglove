// SPDX-License-Identifier: MIT
//! The states #1086 publishes, on every surface (#983, #1049; docs/hideout-supervisor-ux.md's
//! state table, docs/hideout-revoked-ux.md, contract §7.6/§7.8): waiting (root cause once),
//! needs a newer Hideout, quarantined, updating, Update ready and Update waiting for you, on the
//! tab, the page, the tray tile, the Modules row and the tray. Window-free; the review sheets
//! (one per surface) go to `HIDEOUT_REVIEW_DIR`.

use super::*;
use crate::marks::Marks;
use crate::modules_list::{page_ui, row_in, Does, Installed, Row};

const LINK: &str = "mod.link";

fn manifest(id: &str, name: &str) -> hideout_contract::module::Module {
    let toml = format!(
        "id = \"{id}\"\nname = \"{name}\"\nversion = \"0.5.0\"\nkind = \"service\"\n\
         family = \"third-party\"\nhub = \"1.0\"\nrun = [\"x.exe\"]\ndescription = \"x\"\n"
    );
    hideout_contract::module::Module::parse(&toml, &Default::default()).expect("manifest parses")
}

/// Art's page with these facts, and Link (Art's provider) known by name with a page.
fn art_in(dir: &std::path::Path, facts: &[(&str, serde_json::Value)]) -> (Hub, Asked) {
    let (mut hub, asked) = supervised(dir, &[]);
    hub.panels
        .register(LINK, LINK, None, &params("link", "Link"));
    hub.panels.set_manifest(manifest(LINK, "Link"));
    for (k, v) in facts {
        hub.panels.set_fact(&format!("module.{SUP}.{k}"), v.clone());
    }
    (hub, asked)
}

fn waiting(dir: &std::path::Path) -> (Hub, Asked) {
    art_in(
        dir,
        &[
            ("state", serde_json::json!("waiting")),
            ("waiting", serde_json::json!(LINK)),
            ("reason", serde_json::json!(words::waiting_reason("Link"))),
        ],
    )
}

fn needs_hub(dir: &std::path::Path, previous: Option<&str>) -> (Hub, Asked) {
    art_in(
        dir,
        &[
            ("state", serde_json::json!("needs-hub")),
            (
                "previous",
                previous.map_or(serde_json::Value::Null, |v| serde_json::json!(v)),
            ),
        ],
    )
}

fn quarantined(dir: &std::path::Path, previous: Option<&str>) -> (Hub, Asked) {
    let (mut hub, asked) = art_in(dir, &[]);
    hub.panels.quarantine(SUP);
    for (k, v) in [
        ("state", serde_json::json!("quarantined")),
        ("reason", serde_json::json!(words::QUARANTINED_LINE)),
        (
            "previous",
            previous.map_or(serde_json::Value::Null, |v| serde_json::json!(v)),
        ),
    ] {
        hub.panels.set_fact(&format!("module.{SUP}.{k}"), v);
    }
    (hub, asked)
}

fn updating(dir: &std::path::Path) -> (Hub, Asked) {
    art_in(
        dir,
        &[
            ("state", serde_json::json!("updating")),
            ("reason", serde_json::json!(words::PLACE_KEPT)),
        ],
    )
}

fn update_busy(dir: &std::path::Path) -> (Hub, Asked) {
    art_in(
        dir,
        &[
            ("state", serde_json::json!("update-busy")),
            (
                "reason",
                serde_json::json!(words::update_busy("a flash running")),
            ),
        ],
    )
}

fn update_waiting(dir: &std::path::Path) -> (Hub, Asked) {
    art_in(
        dir,
        &[
            ("state", serde_json::json!("update-waiting")),
            (
                "reason",
                serde_json::json!(words::update_asks(Some("see every key you press"), false)),
            ),
        ],
    )
}

/// Where a drawn text is (its rect).
fn text_rect(out: &egui::FullOutput, text: &str) -> Option<egui::Rect> {
    fn walk(s: &epaint::Shape, text: &str) -> Option<egui::Rect> {
        match s {
            epaint::Shape::Text(t) if t.galley.text() == text => {
                Some(t.galley.rect.translate(t.pos.to_vec2()))
            }
            epaint::Shape::Vec(all) => all.iter().find_map(|s| walk(s, text)),
            _ => None,
        }
    }
    out.shapes.iter().find_map(|c| walk(&c.shape, text))
}

/// A click on the drawn text `text` (a button's label), through the real frames.
fn click(ctx: &egui::Context, hub: &mut Hub, text: &str) {
    let out = frames(ctx, hub, vec![]);
    let at = text_rect(&out, text)
        .unwrap_or_else(|| panic!("{text:?} not drawn: {:?}", drawn(&out)))
        .center();
    let press = |down| Event::PointerButton {
        pos: at,
        button: egui::PointerButton::Primary,
        pressed: down,
        modifiers: Modifiers::NONE,
    };
    for ev in [
        vec![Event::PointerMoved(at)],
        vec![press(true)],
        vec![press(false)],
    ] {
        let _ = ctx.run(input(1100.0, 640.0, ev), |c| hub.frame(c));
    }
}

fn art_tab(hub: &Hub) -> Look {
    hub.look(hub.tabs.position(&TabId::panel(SUP, "art")).unwrap())
}

fn asked(a: &Asked) -> Vec<String> {
    a.0.lock().unwrap().clone()
}

/// Waiting (root cause once): "Waiting for Link", Link's reason, Go to Link, which opens Link's
/// page. No crash bar, no red mark, the page stays live; the tab has the ring with the words.
#[test]
fn waiting_names_the_root_once_and_goes_to_it() {
    let ctx = ctx();
    let dir = temp_dir("st-waiting");
    let (mut hub, rec) = waiting(&dir);
    let out = frames(&ctx, &mut hub, vec![]);
    let t = drawn(&out);
    for s in ["Waiting for Link", "Link is not running.", "Go to Link"] {
        assert!(t.iter().any(|x| x == s), "{s}: {t:?}");
    }
    assert!(
        t.iter().any(|x| x == "Art page from Art."),
        "the page greyed: {t:?}"
    );
    assert!(
        !t.iter()
            .any(|x| x.contains('\u{26A0}') || x.contains("stopped unexpectedly")),
        "{t:?}"
    );
    assert_eq!(
        hub.panels.panel(&PanelKey::new(SUP, "art")).unwrap().state,
        PanelState::Live
    );
    let l = art_tab(&hub);
    assert_eq!(l.state, TabState::Waiting("Waiting for Link".into()));
    assert_ne!(l.shown(), crate::tabs::Shown::Stopped);
    // A fact, not something in progress: no "…" (designer review of #1093). The ring stays.
    assert_eq!(l.tooltip(), "Art\nWaiting for Link");
    assert_eq!(l.accessible_name(), "Art, waiting for Link");
    assert!(l.busy(), "the ring went");
    click(&ctx, &mut hub, "Go to Link");
    assert_eq!(
        hub.tabs.active_id(),
        Some(&TabId::panel(LINK, "link")),
        "Go to Link didn't open Link"
    );
    assert!(asked(&rec).is_empty(), "Go to asks the supervisor nothing");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Needs a newer Hideout: the card, Update Hideout focused (Enter presses it, Esc does nothing),
/// Go back only with a version kept; the red mark on the tab.
#[test]
fn needs_hub_shows_its_card_with_update_hideout() {
    let ctx = ctx();
    ctx.enable_accesskit();
    let dir = temp_dir("st-needs-hub");
    let (mut hub, rec) = needs_hub(&dir, Some("0.4.2"));
    let out = frames(&ctx, &mut hub, vec![]);
    let t = drawn(&out);
    for s in [
        "\u{26A0} Needs a newer Hideout",
        "Update Hideout",
        "Go back to 0.4.2",
    ] {
        assert!(t.iter().any(|x| x == s), "{s}: {t:?}");
    }
    assert_eq!(focused_label(&out).as_deref(), Some("Update Hideout"));
    let l = art_tab(&hub);
    assert_eq!(l.shown(), crate::tabs::Shown::Stopped);
    assert!(l.tooltip().contains(words::NEEDS_HUB));
    let _ = frames(&ctx, &mut hub, vec![key(Key::Escape, Modifiers::NONE)]);
    assert!(asked(&rec).is_empty());
    let _ = frames(&ctx, &mut hub, vec![key(Key::Enter, Modifiers::NONE)]);
    // The card names its module, so the hub can open the release that module needs (#991).
    assert_eq!(asked(&rec), [format!("update-hub {SUP}")]);
    click(&ctx, &mut hub, "Go back to 0.4.2");
    assert_eq!(
        asked(&rec),
        [format!("update-hub {SUP}"), format!("go-back {SUP}")]
    );
    let (mut hub, _) = needs_hub(&dir, None);
    let t = drawn(&frames(&ctx, &mut hub, vec![]));
    assert!(!t.iter().any(|s| s.starts_with("Go back")), "{t:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Quarantined: the panel host's quarantine reaches the supervisor once; the card names the
/// page, says why, Start again focused, Go back with a version; Start again releases the panels;
/// a state that leaves quarantined (from the Modules page, say) releases them too.
#[test]
fn quarantined_reaches_the_supervisor_and_shows_its_card() {
    let ctx = ctx();
    ctx.enable_accesskit();
    let dir = temp_dir("st-quarantined");
    let (mut hub, rec) = quarantined(&dir, Some("0.4.2"));
    let out = frames(&ctx, &mut hub, vec![]);
    assert_eq!(asked(&rec), [format!("quarantine {SUP}")], "told once");
    let t = drawn(&out);
    for s in [
        "\u{26A0} Art's page couldn't be shown",
        words::QUARANTINED_LINE,
        "Start again",
        "Go back to 0.4.2",
    ] {
        assert!(t.iter().any(|x| x == s), "{s}: {t:?}");
    }
    // Only this card: not the generic one under it.
    assert!(!t.iter().any(|s| s.contains("isn't running")), "{t:?}");
    assert_eq!(focused_label(&out).as_deref(), Some("Start again"));
    let l = art_tab(&hub);
    assert_eq!(l.shown(), crate::tabs::Shown::Stopped);
    assert!(l.tooltip().contains("Its page couldn't be shown"));
    assert_eq!(l.accessible_name(), "Art, its page couldn't be shown");
    let _ = frames(&ctx, &mut hub, vec![key(Key::Enter, Modifiers::NONE)]);
    assert_eq!(
        asked(&rec),
        [format!("quarantine {SUP}"), format!("start-again {SUP}")]
    );
    let key_ = PanelKey::new(SUP, "art");
    assert_ne!(
        hub.panels.panel(&key_).unwrap().state,
        PanelState::Quarantined
    );

    // Released by the state leaving quarantined: greyed "starting" until it registers.
    let (mut hub, _) = quarantined(&dir, None);
    hub.panels.set_fact(
        &format!("module.{SUP}.state"),
        serde_json::json!("starting"),
    );
    assert_eq!(
        hub.panels.panel(&key_).unwrap().state,
        PanelState::Greyed("starting".into())
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// #1104, in the lab's order (`hideout_lab_checks.py` step 1093.4): the page is open and live; the
/// panel host quarantines it (frames pass before the supervisor's fact comes back); the lab
/// opens the page again (`focus Art`: AccessKit's Focus and Click on its tab). Start again still
/// has the focus, Enter presses it: the supervisor is asked, the panels are released, greyed
/// "starting", and the next `ui.register` makes the page live again.
#[test]
fn quarantine_then_start_again_in_the_labs_order() {
    use egui::accesskit::{Action as AxAction, ActionRequest, Role};
    let ctx = ctx();
    ctx.enable_accesskit();
    let dir = temp_dir("st-quarantine-lab");
    let (mut hub, rec) = art_in(&dir, &[("state", serde_json::json!("running"))]);
    let key_ = PanelKey::new(SUP, "art");
    let _ = frames(&ctx, &mut hub, vec![]);
    // The panel host's quarantine (lab control's `quarantine` queues exactly this).
    hub.panels.quarantine(SUP);
    let _ = frames(&ctx, &mut hub, vec![]);
    assert_eq!(asked(&rec), [format!("quarantine {SUP}")], "told once");
    // The supervisor's answer, a while later.
    for (k, v) in [
        ("state", serde_json::json!("quarantined")),
        ("reason", serde_json::json!(words::QUARANTINED_LINE)),
    ] {
        hub.panels.set_fact(&format!("module.{SUP}.{k}"), v);
    }
    let out = frames(&ctx, &mut hub, vec![]);
    assert_eq!(focused_label(&out).as_deref(), Some("Start again"));
    // The lab's `focus Art`: Focus and Click on Art's tab, which is already open.
    let tab = out
        .platform_output
        .accesskit_update
        .as_ref()
        .and_then(|u| {
            u.nodes.iter().find(|(_, n)| {
                n.role() == Role::Tab && n.label().is_some_and(|l| l.starts_with("Art"))
            })
        })
        .map(|(id, _)| *id)
        .expect("Art's tab in the AccessKit tree");
    let ask = |action| {
        Event::AccessKitActionRequest(ActionRequest {
            action,
            target: tab,
            data: None,
        })
    };
    let _ = ctx.run(
        input(
            1100.0,
            640.0,
            vec![ask(AxAction::Focus), ask(AxAction::Click)],
        ),
        |c| hub.frame(c),
    );
    let out = frames(&ctx, &mut hub, vec![]);
    assert_eq!(
        focused_label(&out).as_deref(),
        Some("Start again"),
        "opening the page again left the focus on its tab"
    );
    // Enter presses Start again.
    let _ = frames(&ctx, &mut hub, vec![key(Key::Enter, Modifiers::NONE)]);
    assert_eq!(
        asked(&rec),
        [format!("quarantine {SUP}"), format!("start-again {SUP}")]
    );
    assert_ne!(
        hub.panels.panel(&key_).unwrap().state,
        PanelState::Quarantined,
        "Start again didn't release the panels"
    );
    // The supervisor starts it: greyed "starting", then its `ui.register` makes the page live.
    hub.panels.set_fact(
        &format!("module.{SUP}.state"),
        serde_json::json!("starting"),
    );
    assert_eq!(
        hub.panels.panel(&key_).unwrap().state,
        PanelState::Greyed("starting".into())
    );
    hub.panels.register(SUP, SUP, None, &params("art", "Art"));
    hub.panels
        .set_fact(&format!("module.{SUP}.state"), serde_json::json!("running"));
    let t = drawn(&frames(&ctx, &mut hub, vec![]));
    assert_eq!(hub.panels.panel(&key_).unwrap().state, PanelState::Live);
    assert!(
        !t.iter()
            .any(|s| s.contains("couldn't be shown") || s.contains("isn't running")),
        "{t:?}"
    );
    assert!(
        t.iter().any(|s| s == "Art page from Art."),
        "the page: {t:?}"
    );
    assert_eq!(asked(&rec).len(), 2, "quarantined again: {:?}", asked(&rec));
    let _ = std::fs::remove_dir_all(&dir);
}

/// Updating: one line, "Updating… Your place on this page is kept.", said once; the ring with
/// "Updating…" on the tab; no doubled ellipsis from the generic line.
#[test]
fn updating_is_one_header_line_and_a_ring() {
    let ctx = ctx();
    let dir = temp_dir("st-updating");
    let (mut hub, _) = updating(&dir);
    let t = drawn(&frames(&ctx, &mut hub, vec![]));
    assert!(
        t.iter()
            .any(|s| s == "Updating\u{2026} Your place on this page is kept."),
        "{t:?}"
    );
    assert!(!t.iter().any(|s| s.contains("\u{2026}\u{2026}")), "{t:?}");
    assert_eq!(art_tab(&hub).state, TabState::Busy(words::UPDATING.into()));
    let _ = std::fs::remove_dir_all(&dir);
}

/// Update ready: the module's reason, Force and Not now reach the supervisor; the page stays
/// live and nothing takes the focus. Update waiting for you: the permission words, Review greyed
/// with "Comes in a later Hideout." (no permission dialog yet), Not now works.
#[test]
fn the_waiting_updates_offer_force_review_and_not_now() {
    let ctx = ctx();
    ctx.enable_accesskit();
    let dir = temp_dir("st-update");
    let (mut hub, rec) = update_busy(&dir);
    let out = frames(&ctx, &mut hub, vec![]);
    let t = drawn(&out);
    for s in [
        "Update ready",
        "Waits until you finish a flash running",
        "Force",
        "Not now",
        "Art page from Art.",
    ] {
        assert!(t.iter().any(|x| x == s), "{s}: {t:?}");
    }
    assert_eq!(focused_label(&out), None, "an update line took the focus");
    assert_eq!(art_tab(&hub).state, TabState::Live);
    click(&ctx, &mut hub, "Force");
    click(&ctx, &mut hub, "Not now");
    assert_eq!(
        asked(&rec),
        [format!("force {SUP}"), format!("not-now {SUP}")]
    );

    let (mut hub, rec) = update_waiting(&dir);
    let out = frames(&ctx, &mut hub, vec![]);
    let t = drawn(&out);
    for s in [
        "Update waiting for you",
        "The new version wants to see every key you press.",
        "Review",
        "Not now",
    ] {
        assert!(t.iter().any(|x| x == s), "{s}: {t:?}");
    }
    let review = out
        .platform_output
        .accesskit_update
        .iter()
        .flat_map(|u| u.nodes.iter())
        .find(|(_, n)| n.label() == Some("Review"))
        .map(|(_, n)| n.clone())
        .expect("Review in the tree");
    assert!(review.is_disabled());
    assert_eq!(review.description(), Some(words::COMES_LATER));
    click(&ctx, &mut hub, "Review");
    assert!(asked(&rec).is_empty(), "a greyed Review asked something");
    click(&ctx, &mut hub, "Not now");
    assert_eq!(asked(&rec), [format!("not-now {SUP}")]);
    let _ = std::fs::remove_dir_all(&dir);
}

/// The panel host keeps a running module's page live for waiting and the waiting updates (the
/// panels came greyed for every state but running before, and a waiting module never registers
/// again to bring them back).
#[test]
fn running_states_keep_the_page_live() {
    let dir = temp_dir("st-live");
    for s in ["waiting", "update-busy", "update-waiting"] {
        let (mut hub, _) = art_in(&dir, &[]);
        hub.panels
            .set_fact(&format!("module.{SUP}.state"), serde_json::json!(s));
        assert_eq!(
            hub.panels.panel(&PanelKey::new(SUP, "art")).unwrap().state,
            PanelState::Live,
            "{s}"
        );
    }
    let (mut hub, _) = art_in(&dir, &[]);
    hub.panels.set_fact(
        &format!("module.{SUP}.state"),
        serde_json::json!("updating"),
    );
    assert_eq!(
        hub.panels.panel(&PanelKey::new(SUP, "art")).unwrap().state,
        PanelState::Greyed("updating".into()),
        "a swap greys the old page"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

// ---- The tray tile ----------------------------------------------------------------------------

fn tile_host(state: &str) -> (hideout_panels::PanelHost, PanelKey) {
    tile_host_as(SUP, state)
}

/// [`tile_host`] under module id `id` (the sheet draws several hosts in one window: each tile
/// needs its own widget ids).
fn tile_host_as(id: &str, state: &str) -> (hideout_panels::PanelHost, PanelKey) {
    let mut host = hideout_panels::PanelHost::default();
    let reg: hideout_contract::panel::Register = serde_json::from_value(serde_json::json!({
        "instance": "i1",
        "panels": [{"id": "tile", "slot": "tile", "title": "Art", "icon": "x",
            "tree": {"type": "column", "id": "tile", "children": [
                {"type": "value", "id": "glance", "props": {"tone": "neutral", "value": "Painting"}}]}}]
    }))
    .unwrap();
    host.register(id, id, None, &reg);
    host.set_manifest(manifest(id, "Art"));
    host.set_manifest(manifest(LINK, "Link"));
    for (k, v) in [
        ("state", serde_json::json!(state)),
        ("waiting", serde_json::json!(LINK)),
    ] {
        host.set_fact(&format!("module.{id}.{k}"), v);
    }
    (host, PanelKey::new(id, "tile"))
}

fn tile_texts(state: &str) -> Vec<String> {
    let ctx = ctx();
    let (mut host, key) = tile_host(state);
    let mut out = None;
    for _ in 0..3 {
        out = Some(ctx.run(input(400.0, 300.0, vec![]), |c| {
            egui::CentralPanel::default().show(c, |ui| {
                hideout_panels::tile_ui(&mut host, ui, &key);
            });
        }));
    }
    drawn(&out.unwrap())
}

/// The tile (plugin-modules plan's table, supervisor plan's words): waiting names the root over
/// the live body; updating says "Updating…"; needs a newer Hideout and quarantined are red with
/// their one button; the waiting updates leave the tile as it is (updates never touch the tray).
#[test]
fn the_tile_says_each_state() {
    let t = tile_texts("waiting");
    assert!(t.iter().any(|s| s == "Waiting for Link"), "{t:?}");
    assert!(t.iter().any(|s| s == "Painting"), "the body went: {t:?}");
    assert!(!t.iter().any(|s| s == "\u{26A0}"), "{t:?}");
    let t = tile_texts("updating");
    assert!(t.iter().any(|s| s == "Updating\u{2026}"), "{t:?}");
    let t = tile_texts("needs-hub");
    assert!(t.iter().any(|s| s == "Needs a newer Hideout"), "{t:?}");
    assert!(t.iter().any(|s| s == "Update Hideout"), "{t:?}");
    let t = tile_texts("quarantined");
    assert!(t.iter().any(|s| s == "Its tile couldn't be shown"), "{t:?}");
    assert!(t.iter().any(|s| s == "Open"), "{t:?}");
    for s in ["update-busy", "update-waiting"] {
        let t = tile_texts(s);
        assert!(t.iter().any(|x| x == "Painting"), "{s}: {t:?}");
        assert!(
            !t.iter().any(|x| x.starts_with("Update") || x == "\u{26A0}"),
            "{s}: {t:?}"
        );
    }
}

/// hideout-panels can't depend on the supervisor: its tile words are kept equal here.
#[test]
fn the_tile_says_the_supervisors_words() {
    assert_eq!(hideout_panels::tray::TILE_UPDATING, words::UPDATING);
    assert_eq!(
        hideout_panels::tray::tile_waiting("Link"),
        words::waiting_for("Link")
    );
}

// ---- The Modules row and the tray -------------------------------------------------------------

fn installed() -> Vec<Installed> {
    [
        ("m.art", "Art"),
        ("m.link", "Link"),
        ("m.kiln", "Kiln"),
        ("m.quill", "Quill"),
        ("m.dock", "Dock"),
        ("m.echo", "Echo"),
        ("m.fern", "Fern"),
    ]
    .iter()
    .map(|(id, name)| Installed {
        adopted: false,
        id: (*id).into(),
        name: (*name).into(),
        version: "1.0.0".into(),
        enabled: true,
    })
    .collect()
}

fn state_facts() -> HashMap<String, serde_json::Value> {
    [
        ("module.m.art.state", serde_json::json!("waiting")),
        ("module.m.art.waiting", serde_json::json!("m.link")),
        ("module.m.link.state", serde_json::json!("crash-stopped")),
        ("module.m.kiln.state", serde_json::json!("needs-hub")),
        ("module.m.quill.state", serde_json::json!("quarantined")),
        ("module.m.dock.state", serde_json::json!("updating")),
        ("module.m.echo.state", serde_json::json!("update-busy")),
        ("module.m.fern.state", serde_json::json!("update-waiting")),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect()
}

fn state_rows() -> Vec<Row> {
    let facts = state_facts();
    let fact = |k: &str| facts.get(k).cloned();
    let all = installed();
    let mut rows: Vec<Row> = all.iter().map(|m| row_in(m, &all, &fact, None)).collect();
    crate::modules_list::order(&mut rows);
    rows
}

/// Column M: each state's words and its one main button; only the needs-you rows are red and
/// first (root cause once: Art waiting is not, Link stopped is).
#[test]
fn the_modules_rows_say_each_state() {
    let rows = state_rows();
    let get = |id: &str| rows.iter().find(|r| r.id == id).unwrap();
    let main = |r: &Row| r.primary.as_ref().map(|p| (p.label.clone(), p.does));
    let a = get("m.art");
    assert_eq!(a.state, "Waiting for Link");
    assert!(!a.stopped);
    assert_eq!(main(a), Some(("Go to Link".into(), Does::GoTo)));
    assert_eq!(get("m.kiln").state, "Needs a newer Hideout");
    assert!(get("m.kiln").stopped);
    let q = get("m.quill");
    assert_eq!(q.state, "Quill's page couldn't be shown");
    assert!(q.stopped);
    assert_eq!(main(q), Some(("Start again".into(), Does::StartAgain)));
    assert_eq!(get("m.dock").state, "Updating\u{2026}");
    assert_eq!(main(get("m.dock")), None);
    assert_eq!(main(get("m.echo")), Some(("Force".into(), Does::Force)));
    assert_eq!(get("m.echo").state, "Update ready");
    assert_eq!(get("m.fern").state, "Update waiting for you");
    assert_eq!(main(get("m.fern")), Some(("Review".into(), Does::Review)));
    // Without the list the root isn't named, and never by its id.
    let facts = state_facts();
    let fact = |k: &str| facts.get(k).cloned();
    let r = crate::modules_list::row(&installed()[0], &fact, None);
    assert_eq!(r.state, "Waiting for another module");
    assert!(r.primary.is_none());
}

/// The Modules row's Review is greyed with "Comes in a later Hideout." as on the page, and a
/// click on it asks nothing (designer review of #1093).
#[test]
fn the_modules_rows_review_is_greyed() {
    let ctx = ctx();
    ctx.enable_accesskit();
    let rows = state_rows();
    let t = theme::tokens(true);
    let mut clicks = Vec::new();
    let mut out = None;
    let mut at = None;
    for k in 0..5 {
        let ev = match (k, at) {
            (2, Some(p)) => vec![Event::PointerMoved(p)],
            (3, Some(p)) => vec![Event::PointerButton {
                pos: p,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: Modifiers::NONE,
            }],
            (4, Some(p)) => vec![Event::PointerButton {
                pos: p,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: Modifiers::NONE,
            }],
            _ => vec![],
        };
        let o = ctx.run(input(1000.0, 700.0, ev), |c| {
            egui::CentralPanel::default().show(c, |ui| {
                clicks.extend(page_ui(ui, &t, &rows, None));
            });
        });
        at = text_rect(&o, "Review").map(|r| r.center());
        out = Some(o);
    }
    let out = out.unwrap();
    let review = out
        .platform_output
        .accesskit_update
        .iter()
        .flat_map(|u| u.nodes.iter())
        .find(|(_, n)| n.label() == Some("Review"))
        .map(|(_, n)| n.clone())
        .expect("Review in the tree");
    assert!(review.is_disabled());
    assert_eq!(review.description(), Some(words::COMES_LATER));
    assert!(clicks.is_empty(), "a greyed Review clicked: {clicks:?}");
}

fn tray_for(states: &[(&str, &str)]) -> Marks {
    let mut m = Marks::default();
    for (id, name) in [("m.art", "Art"), ("m.link", "Link"), ("m.kiln", "Kiln")] {
        m.name(id, name);
    }
    for (id, s) in states {
        m.fact(&format!("module.{id}.state"), &serde_json::json!(s));
    }
    m
}

/// The tray quiet rule: waiting, quarantined, updating and the waiting updates draw nothing;
/// needs a newer Hideout is the red dot with its line; a module waiting on a stopped one names
/// only the root.
#[test]
fn the_tray_names_only_what_needs_the_user() {
    for s in [
        "waiting",
        "quarantined",
        "updating",
        "update-busy",
        "update-waiting",
    ] {
        let mut m = tray_for(&[("m.art", s)]);
        assert_eq!(
            m.take_change().unwrap().0,
            crate::icon::State::Normal,
            "{s} touched the tray"
        );
    }
    let m = tray_for(&[("m.kiln", "needs-hub")]);
    assert_eq!(
        (m.look().0, m.look().1.as_str()),
        (crate::icon::State::NeedsYou, "Kiln needs a newer Hideout")
    );
    let m = tray_for(&[("m.art", "waiting"), ("m.link", "crash-stopped")]);
    let (_, line, tip) = m.look();
    assert!(line.starts_with("Link stopped"), "{line}");
    assert!(
        !line.contains("Art") && !tip.contains("Art"),
        "{line} / {tip}"
    );
}

/// No id from "Never shown", and no module id, in any string these states draw: the pages,
/// the tiles, the rows and the tray lines.
#[test]
fn no_ids_in_the_new_states_strings() {
    let ctx = ctx();
    let dir = temp_dir("st-never");
    let mut all: Vec<String> = Vec::new();
    type Make<'a> = Box<dyn Fn() -> Hub + 'a>;
    let makes: Vec<Make> = vec![
        Box::new(|| waiting(&dir).0),
        Box::new(|| needs_hub(&dir, Some("0.4.2")).0),
        Box::new(|| quarantined(&dir, Some("0.4.2")).0),
        Box::new(|| updating(&dir).0),
        Box::new(|| update_busy(&dir).0),
        Box::new(|| update_waiting(&dir).0),
    ];
    for make in makes {
        let mut hub = make();
        all.extend(drawn(&frames(&ctx, &mut hub, vec![])));
        let l = art_tab(&hub);
        all.extend([l.tooltip(), l.accessible_name()]);
    }
    for s in [
        "waiting",
        "needs-hub",
        "quarantined",
        "updating",
        "update-busy",
        "update-waiting",
    ] {
        all.extend(tile_texts(s));
        let m = tray_for(&[("m.art", s)]);
        let (_, line, tip) = m.look();
        all.extend([line, tip]);
    }
    for r in state_rows() {
        all.extend([r.state.clone(), r.label.clone()]);
        all.extend(r.primary.map(|p| p.label));
    }
    for s in &all {
        for n in words::NEVER_SHOWN {
            assert!(
                !s.to_lowercase().contains(&n.to_lowercase()),
                "{s:?} shows {n:?}"
            );
        }
        assert!(
            !s.contains("mod.") && !s.contains("m.art"),
            "{s:?} shows an id"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

// ---- Review sheets (HIDEOUT_REVIEW_DIR), one per surface -------------------------------------

/// Images stacked top to bottom on the dark background, 12 px apart.
fn stack(parts: Vec<(u32, u32, Vec<u8>)>) -> (u32, u32, Vec<u8>) {
    let gap = 12u32;
    let w = parts.iter().map(|p| p.0).max().unwrap_or(1);
    let h = parts.iter().map(|p| p.1 + gap).sum::<u32>() + gap;
    let mut img = vec![0u8; (w * h * 4) as usize];
    for p in img.chunks_mut(4) {
        p.copy_from_slice(&[0x10, 0x0e, 0x0d, 0xff]);
    }
    let mut y0 = gap;
    for (pw, ph, px) in parts {
        for y in 0..ph {
            let src = &px[(y * pw * 4) as usize..((y + 1) * pw * 4) as usize];
            let at = (((y0 + y) * w) * 4) as usize;
            img[at..at + src.len()].copy_from_slice(src);
        }
        y0 += ph + gap;
    }
    (w, h, img)
}

/// A caption strip (what the next picture shows).
fn caption(text: &str, w: u32) -> Option<(u32, u32, Vec<u8>)> {
    let text = text.to_string();
    let px = rasterise(w, 30, move |c| {
        egui::CentralPanel::default().show(c, |ui| {
            ui.label(egui::RichText::new(&text).strong());
        });
    })?;
    Some((w, 30, px))
}

/// The page sheet: each state's page (the module's tab active), as the window draws it.
#[test]
fn renders_the_state_pages_for_review() {
    let dir = temp_dir("png-st-pages");
    type Make<'a> = Box<dyn Fn() -> Hub + 'a>;
    let shots: Vec<(&str, Make)> = vec![
        ("Waiting (root cause once)", Box::new(|| waiting(&dir).0)),
        (
            "Needs a newer Hideout, a version kept",
            Box::new(|| needs_hub(&dir, Some("0.4.2")).0),
        ),
        (
            "Needs a newer Hideout, none kept",
            Box::new(|| needs_hub(&dir, None).0),
        ),
        (
            "Quarantined, a version kept",
            Box::new(|| quarantined(&dir, Some("0.4.2")).0),
        ),
        ("Updating (hot swap)", Box::new(|| updating(&dir).0)),
        ("Update ready (busy)", Box::new(|| update_busy(&dir).0)),
        (
            "Update waiting for you (Review greyed)",
            Box::new(|| update_waiting(&dir).0),
        ),
    ];
    let mut parts = Vec::new();
    for (label, make) in shots {
        let Some(c) = caption(label, 1100) else {
            eprintln!("no system font; state review skipped");
            return;
        };
        parts.push(c);
        let mut hub = make();
        let px = render(&mut hub, 1100, 330, |_| {}).unwrap();
        parts.push((1100, 330, px));
    }
    let (w, h, img) = stack(parts);
    save_png("built-states-sheet-page.png", w, h, img);
    let _ = std::fs::remove_dir_all(&dir);
}

/// The tab sheet: one window with a tab per state, then each tab's tooltip and accessible name.
#[test]
fn renders_the_state_tabs_for_review() {
    let dir = temp_dir("png-st-tabs");
    let mut hub = hub_in(&dir, None);
    let tabs = [
        ("mod.art", "Art", "waiting"),
        ("mod.kiln", "Kiln", "needs-hub"),
        ("mod.quill", "Quill", "quarantined"),
        ("mod.dock", "Dock", "updating"),
        ("mod.echo", "Echo", "update-busy"),
        ("mod.fern", "Fern", "update-waiting"),
        ("mod.link", "Link", "crash-stopped"),
    ];
    for (id, name, _) in tabs {
        let panel = name.to_lowercase();
        hub.panels.register(id, id, None, &params(&panel, name));
        hub.panels.set_manifest(manifest(id, name));
        hub.open(TabId::panel(id, &panel));
    }
    for (id, _, s) in tabs {
        hub.panels
            .set_fact(&format!("module.{id}.state"), serde_json::json!(s));
    }
    hub.panels
        .set_fact("module.mod.art.waiting", serde_json::json!("mod.link"));
    let first = hub.tabs.position(&TabId::panel("mod.art", "art")).unwrap();
    hub.tabs.activate(first);
    let Some(strip) = render(&mut hub, 1280, 420, |_| {}) else {
        eprintln!("no system font; state review skipped");
        return;
    };
    let lines: Vec<String> = tabs
        .iter()
        .map(|(id, _, s)| {
            let panel = TabId::panel(*id, id.trim_start_matches("mod."));
            let l = hub.look(hub.tabs.position(&panel).unwrap());
            format!(
                "{s}: tooltip \u{201C}{}\u{201D} \u{b7} name \u{201C}{}\u{201D} \u{b7} {:?}",
                l.tooltip().replace('\n', " / "),
                l.accessible_name(),
                l.shown()
            )
        })
        .collect();
    let table = rasterise(1280, 230, move |c| {
        egui::CentralPanel::default().show(c, |ui| {
            for l in &lines {
                ui.label(l);
            }
        });
    })
    .unwrap();
    let (w, h, img) = stack(vec![(1280, 420, strip), (1280, 230, table)]);
    save_png("built-states-sheet-tab.png", w, h, img);
    let _ = std::fs::remove_dir_all(&dir);
}

/// The tile sheet: the tray panel's tile in each state.
#[test]
fn renders_the_state_tiles_for_review() {
    let states = [
        "running",
        "waiting",
        "updating",
        "update-busy",
        "update-waiting",
        "needs-hub",
        "quarantined",
    ];
    let mut hosts: Vec<(String, hideout_panels::PanelHost, PanelKey)> = states
        .iter()
        .map(|s| {
            let (h, k) = tile_host_as(&format!("mod.art-{s}"), s);
            (s.to_string(), h, k)
        })
        .collect();
    let (w, h) = (1040u32, 520u32);
    let Some(px) = rasterise(w, h, |c| {
        egui::CentralPanel::default().show(c, |ui| {
            for row in hosts.chunks_mut(4) {
                ui.horizontal_top(|ui| {
                    for (s, host, key) in row.iter_mut() {
                        ui.vertical(|ui| {
                            ui.label(egui::RichText::new(s.as_str()).small().weak());
                            hideout_panels::tile_ui(host, ui, key);
                        });
                    }
                });
                ui.add_space(10.0);
            }
        });
    }) else {
        eprintln!("no system font; state review skipped");
        return;
    };
    save_png("built-states-sheet-tile.png", w, h, px);
}

/// The Modules sheet: the page's rows for every state, in the page's order.
#[test]
fn renders_the_state_rows_for_review() {
    let rows = state_rows();
    let (w, h) = (1000u32, 620u32);
    let Some(px) = rasterise(w, h, |c| {
        let t = theme::tokens(true);
        egui::CentralPanel::default().show(c, |ui| {
            let _ = page_ui(ui, &t, &rows, None);
        });
    }) else {
        eprintln!("no system font; state review skipped");
        return;
    };
    save_png("built-states-sheet-modules.png", w, h, px);
}

/// The tray sheet: for each state the icon (4x) and the menu's status line and tooltip.
#[test]
fn renders_the_state_tray_for_review() {
    let cases: Vec<(&str, Vec<(&str, &str)>)> = vec![
        (
            "waiting (Art on Link, Link running)",
            vec![("m.art", "waiting")],
        ),
        (
            "waiting on a stopped root",
            vec![("m.art", "waiting"), ("m.link", "crash-stopped")],
        ),
        ("needs-hub", vec![("m.kiln", "needs-hub")]),
        ("quarantined", vec![("m.art", "quarantined")]),
        ("updating", vec![("m.art", "updating")]),
        ("update-busy", vec![("m.art", "update-busy")]),
        ("update-waiting", vec![("m.art", "update-waiting")]),
    ];
    let (s, w, rh) = (48u32, 1000u32, 64u32);
    let mut parts = Vec::new();
    for (label, states) in cases {
        let m = tray_for(&states);
        let (state, line, tip) = m.look();
        let text = format!(
            "{label}  \u{2192}  line \u{201C}{line}\u{201D} \u{b7} tooltip \u{201C}{tip}\u{201D}"
        );
        let Some(mut px) = rasterise(w, rh, move |c| {
            egui::CentralPanel::default().show(c, |ui| {
                ui.horizontal(|ui| {
                    ui.add_space(s as f32 + 16.0);
                    ui.label(&text);
                });
            });
        }) else {
            eprintln!("no system font; state review skipped");
            return;
        };
        let g = crate::icon::rgba(s, state);
        for y in 0..s {
            for x in 0..s {
                let src = &g[((y * s + x) * 4) as usize..][..4];
                let a = src[3] as u32;
                let d = &mut px[(((y + 8) * w + x + 8) * 4) as usize..][..4];
                for k in 0..3 {
                    d[k] = ((src[k] as u32 * a + d[k] as u32 * (255 - a)) / 255) as u8;
                }
            }
        }
        parts.push((w, rh, px));
    }
    let (w, h, img) = stack(parts);
    save_png("built-states-sheet-tray.png", w, h, img);
}

// ---- The sidebar draws the tab's ring (#1094) -------------------------------------------------

/// The open-arc rings in a frame: (in the sidebar, in the tab strip). Both draw the same shape
/// from `strip::ring_points`; the sidebar is the left 200 px.
fn rings(out: &egui::FullOutput) -> (usize, usize) {
    fn walk(s: &epaint::Shape, v: &mut Vec<f32>) {
        match s {
            epaint::Shape::Path(p) if p.points.len() == 25 && !p.closed => v.push(p.points[0].x),
            epaint::Shape::Vec(all) => all.iter().for_each(|s| walk(s, v)),
            _ => {}
        }
    }
    let mut xs = Vec::new();
    for c in &out.shapes {
        walk(&c.shape, &mut xs);
    }
    let side = xs.iter().filter(|x| **x < 200.0).count();
    (side, xs.len() - side)
}

fn labels(out: &egui::FullOutput) -> Vec<String> {
    out.platform_output
        .accesskit_update
        .clone()
        .unwrap()
        .nodes
        .iter()
        .filter_map(|(_, n)| n.label().map(String::from))
        .collect()
}

/// The hubs of the sidebar cases: (label, hub, ring?, the tab's accessible name).
fn sidebar_cases(dir: &std::path::Path) -> Vec<(&'static str, Hub, bool, &'static str)> {
    let with = |state: &str| supervised(dir, &[("state", serde_json::json!(state))]).0;
    let gone = |state: &str| {
        let mut hub = with("running");
        hub.panels.module_gone(SUP, state);
        hub
    };
    vec![
        (
            "Waiting for Link",
            waiting(dir).0,
            true,
            "Art, waiting for Link",
        ),
        ("Updating", updating(dir).0, true, "Art, updating\u{2026}"),
        ("Starting", gone("starting"), true, "Art, starting\u{2026}"),
        (
            "Restarting (2 of 5)",
            restarting(dir, 8, 2, Duration::ZERO).0,
            true,
            "Art, restarting\u{2026} (2 of 5)",
        ),
        (
            "Stopped after 5 crashes (mark wins, no ring)",
            stopped(dir, "start-again", None).0,
            false,
            "Art, stopped after 5 crashes",
        ),
        (
            "Needs a newer Hideout (mark wins, no ring)",
            needs_hub(dir, None).0,
            false,
            "Art, needs a newer Hideout",
        ),
        ("Running (nothing)", with("running"), false, "Art"),
    ]
}

/// A module in each ring state shows the ring in the sidebar and on its tab; the stopped mark
/// wins over it; the sidebar entry has the tab's tooltip and accessible name (waiting has no
/// ellipsis, updating has one).
#[test]
fn the_sidebar_draws_the_tabs_ring_with_its_words() {
    let ctx = ctx();
    ctx.enable_accesskit();
    let dir = temp_dir("st-sidebar-ring");
    for (label, mut hub, ring, name) in sidebar_cases(&dir) {
        // The Modules page is active, so the Art entry is unselected like most.
        hub.open(TabId::page("modules"));
        let out = frames(&ctx, &mut hub, vec![]);
        let tab = art_tab(&hub);
        assert_eq!(tab.busy(), ring, "{label}");
        assert_eq!(rings(&out), (ring as usize, ring as usize), "{label}");
        assert_eq!(
            labels(&out).iter().filter(|n| *n == name).count(),
            2, // the sidebar entry and the tab
            "{label}: {:?}",
            labels(&out)
        );
        assert_eq!(
            tab.accessible_name().to_lowercase(),
            name.to_lowercase(),
            "{label}"
        );
    }
    let (_, hub, _, _) = sidebar_cases(&dir).remove(0);
    assert_eq!(art_tab(&hub).tooltip(), "Art\nWaiting for Link");
    assert!(!art_tab(&hub).accessible_name().contains('\u{2026}'));
    let (_, hub, _, _) = sidebar_cases(&dir).remove(1);
    assert_eq!(art_tab(&hub).tooltip(), "Art\nUpdating\u{2026}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// The sidebar sheet: each state's sidebar entry beside its tab, with the words.
#[test]
fn renders_the_state_sidebar_for_review() {
    let dir = temp_dir("png-st-sidebar");
    let mut parts = Vec::new();
    for (label, mut hub, _, _) in sidebar_cases(&dir) {
        hub.open(TabId::page("modules"));
        let l = art_tab(&hub);
        let text = format!(
            "{label}: tooltip \u{201C}{}\u{201D} \u{b7} name \u{201C}{}\u{201D}",
            l.tooltip().replace('\n', " / "),
            l.accessible_name()
        );
        let Some(c) = caption(&text, 760) else {
            eprintln!("no system font; state review skipped");
            return;
        };
        parts.push(c);
        let px = render(&mut hub, 760, 200, |_| {}).unwrap();
        parts.push((760, 200, px));
    }
    let (w, h, img) = stack(parts);
    save_png("built-states-sheet-sidebar.png", w, h, img);
    let _ = std::fs::remove_dir_all(&dir);
}
