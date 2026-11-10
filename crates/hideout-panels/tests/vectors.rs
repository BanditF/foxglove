//! The shared panel vectors (tests/hideout-vectors/panels, transcripts/ui and the
//! re-registration transcript) through the panel host.

use hideout_contract::messages::{UiAssetPut, UiPatch};
use hideout_contract::panel::{parse_message, Register};
use hideout_panels::{PanelHost, PanelKey, PanelState};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/hideout-vectors")
}

fn load(key: &str) -> Value {
    serde_json::from_str(&std::fs::read_to_string(root().join(key)).unwrap()).unwrap()
}

#[test]
fn every_panel_vector_matches_the_index() {
    let index = load("index.json");
    let mut checked = 0;
    for (key, expected) in index["vectors"].as_object().unwrap() {
        if !key.starts_with("panels/") {
            continue;
        }
        checked += 1;
        // `ui.action` needs the module's [[actions]] (spec v1.4), which this host isn't given yet.
        if expected["context"].get("actions").is_some() {
            continue;
        }
        let text = std::fs::read_to_string(root().join(key)).unwrap();
        let msg = match parse_message(&text) {
            Ok(m) => m,
            Err(e) => {
                assert_eq!(expected["result"], "invalid", "{key}: {e}");
                assert!(e.starts_with("ui.limits:"), "{key}: {e}");
                continue;
            }
        };
        let params: Register = serde_json::from_value(msg["params"].clone()).unwrap();
        let mut host = PanelHost::default();
        let rejected = host.register("example.module", "Example", None, &params);
        let registered: Vec<&str> = host
            .panels()
            .iter()
            .filter(|p| p.live())
            .map(|p| p.key.panel.as_str())
            .collect();
        match expected["result"].as_str().unwrap() {
            "valid" => {
                assert!(rejected.is_empty(), "{key}: {rejected:?}");
                assert_eq!(registered.len(), params.panels.len(), "{key}");
            }
            "invalid" => {
                assert!(!rejected.is_empty(), "{key}");
                assert!(rejected.iter().all(|r| r.error == expected["id"]), "{key}");
                assert!(registered.is_empty(), "{key}");
                // The rejected panel still shows, greyed with its error (§5.5).
                assert!(host
                    .panels()
                    .iter()
                    .all(|p| p.state
                        == PanelState::Rejected(expected["id"].as_str().unwrap().into())));
            }
            "partial" => {
                assert_eq!(
                    serde_json::to_value(&rejected).unwrap(),
                    expected["rejected"]
                );
                assert_eq!(json!(registered), expected["registered"], "{key}");
            }
            other => panic!("{key}: result {other}"),
        }
    }
    assert_eq!(checked, 26, "panel vectors in the index");
}

fn steps(key: &str) -> Vec<Value> {
    load(key)["steps"].as_array().unwrap().clone()
}

fn art_panel() -> Register {
    let s = steps("transcripts/lifecycle/re-registration-restart.json");
    let reg = s
        .iter()
        .find(|s| s["msg"]["method"] == "ui.register")
        .unwrap();
    serde_json::from_value(reg["msg"]["params"].clone()).unwrap()
}

#[test]
fn patch_event_asset_transcript() {
    let mut host = PanelHost::default();
    let module = "foxglove.example-art";
    assert!(host.register(module, "Art", None, &art_panel()).is_empty());
    let key = PanelKey::new(module, "art");
    let now = Instant::now();
    let mut seq = None;
    for step in steps("transcripts/ui/patch-event-asset.json") {
        let msg = &step["msg"];
        match msg["method"].as_str() {
            Some("ui.patch") => {
                let patch: UiPatch = serde_json::from_value(msg["params"].clone()).unwrap();
                host.patch(module, &patch, now).unwrap();
            }
            Some("ui.event") => {
                // The hub sends it when the user clicks "next".
                host.emit(hideout_panels::model::Interaction {
                    key: key.clone(),
                    node: msg["params"]["node"].as_str().unwrap().into(),
                    kind: msg["params"]["kind"].as_str().unwrap().into(),
                    value: Value::Null,
                    optimistic: false,
                });
                let out = host.take_events();
                assert_eq!(out.len(), 1);
                assert_eq!(out[0].module, module);
                assert_eq!(serde_json::to_value(&out[0].event).unwrap(), msg["params"]);
                seq = Some(out[0].seq);
            }
            Some("ui.asset.put") => {
                let put: UiAssetPut = serde_json::from_value(msg["params"].clone()).unwrap();
                host.assets.put(&put).unwrap();
                assert!(host.assets.get(&put.hash).is_some());
            }
            _ if msg.get("error").is_some() => {
                let reason = msg["error"]["data"]["reason"].as_str().unwrap().to_string();
                host.event_result(seq.take().unwrap(), Err(reason));
                assert_eq!(
                    host.panel(&key).unwrap().notice.as_deref(),
                    Some("No art pack is loaded")
                );
            }
            _ if msg.get("result").is_some() => {
                if let Some(s) = seq.take() {
                    host.event_result(s, Ok(()));
                }
            }
            _ => {}
        }
    }
    let tree = &host.panel(&key).unwrap().tree;
    assert_eq!(tree.children[0].props["text"], "Art (3)");
    assert!(tree.children.iter().all(|c| c.id != "extra"));
}

#[test]
fn patch_rate_transcript_coalesces_without_errors() {
    let mut host = PanelHost::default();
    let module = "foxglove.example-art";
    host.register(module, "Art", None, &art_panel());
    let start = Instant::now();
    let mut applied_in_first_second = 0;
    let mut prev = json!("Art");
    for i in 0..100u32 {
        let now = start + Duration::from_millis(u64::from(i) * 10);
        let patch: UiPatch = serde_json::from_value(json!({"panel": "art", "ops": [
            {"op": "set", "node": "title", "props": {"text": format!("Art {i}")}}]}))
        .unwrap();
        host.patch(module, &patch, now).unwrap();
        // A deadline already past would make the shell repaint at once, every frame (#935).
        if let Some(at) = host.flush(now) {
            assert!(at > now, "flush returned a past deadline");
        }
        let text = host
            .panel(&PanelKey::new(module, "art"))
            .unwrap()
            .tree
            .children[0]
            .props["text"]
            .clone();
        if text != prev {
            applied_in_first_second += 1;
            prev = text;
        }
    }
    assert!(applied_in_first_second <= 21, "{applied_in_first_second}");
    // The held-back patches land once their slot comes: the last one wins.
    let end = start + Duration::from_secs(2);
    assert_eq!(host.flush(end), None);
    let tree = &host.panel(&PanelKey::new(module, "art")).unwrap().tree;
    assert_eq!(tree.children[0].props["text"], "Art 99");
}

#[test]
fn re_registration_transcript_keeps_view_state() {
    let mut host = PanelHost::default();
    let module = "foxglove.example-art";
    let mut first = art_panel();
    first.instance = "inst-1".into();
    host.register(module, "Art", None, &first);
    let key = PanelKey::new(module, "art");
    host.view_mut(&key)
        .scroll
        .insert("root".into(), [0.0, 120.0]);
    host.module_gone(module, "restarting");
    assert_eq!(
        host.panel(&key).unwrap().state,
        PanelState::Greyed("restarting".into())
    );
    assert!(host.register(module, "Art", None, &art_panel()).is_empty());
    let p = host.panel(&key).unwrap();
    assert!(p.live());
    assert_eq!(p.instance, "inst-2");
    assert_eq!(host.view(&key).unwrap().scroll["root"], [0.0, 120.0]);
}

const ART: &str = "foxglove.example-art";

fn register_art(host: &mut PanelHost) -> PanelKey {
    let s = steps("transcripts/ui/visibility.json");
    let reg = s
        .iter()
        .find(|s| s["msg"]["method"] == "ui.register")
        .unwrap();
    let params: Register = serde_json::from_value(reg["msg"]["params"].clone()).unwrap();
    assert!(host.register(ART, "Art", None, &params).is_empty());
    PanelKey::new(ART, "art.page")
}

#[test]
fn badge_transcript_is_stored_per_panel() {
    use hideout_contract::messages::{Badge, UiBadge};
    let mut host = PanelHost::default();
    let key = register_art(&mut host);
    let mut seen = Vec::new();
    for s in steps("transcripts/ui/badge.json") {
        if s["msg"]["method"] != "ui.badge" {
            continue;
        }
        let b: UiBadge = serde_json::from_value(s["msg"]["params"].clone()).unwrap();
        host.set_badge(ART, &b);
        let p = host.panel(&key).unwrap();
        seen.push((p.badge, p.badge_count));
    }
    // attention, count 120 (the hub shows 99+), dirty, none; the unregistered panel is ignored.
    assert_eq!(
        seen,
        [
            (Badge::Attention, None),
            (Badge::Count, Some(120)),
            (Badge::Dirty, None),
            (Badge::None, None),
            (Badge::None, None),
        ]
    );
    // A new instance resets the badge; the same instance keeps it.
    host.set_badge(
        ART,
        &UiBadge {
            panel: "art.page".into(),
            badge: Badge::Attention,
            count: None,
        },
    );
    let reg = steps("transcripts/ui/visibility.json")
        .into_iter()
        .find(|s| s["msg"]["method"] == "ui.register")
        .unwrap();
    let mut params: Register = serde_json::from_value(reg["msg"]["params"].clone()).unwrap();
    host.register(ART, "Art", None, &params);
    assert_eq!(host.panel(&key).unwrap().badge, Badge::Attention);
    params.instance = "i-2".into();
    host.register(ART, "Art", None, &params);
    assert_eq!(host.panel(&key).unwrap().badge, Badge::None);
}

#[test]
fn visibility_transcript_is_emitted_on_change_only() {
    use hideout_contract::messages::Visibility;
    let mut host = PanelHost::default();
    let key = register_art(&mut host);
    let mut got = Vec::new();
    let mut step = |host: &mut PanelHost, shown: Option<&PanelKey>| {
        host.set_shown(shown);
        host.set_shown(shown); // unchanged: nothing more
        got.extend(host.take_visibility());
    };
    step(&mut host, Some(&key)); // shown right after ui.register
    step(&mut host, None); // the user switches to another tab
    step(&mut host, Some(&key)); // and back
    host.close_panel(&key); // the user closes the tab
    step(&mut host, None);
    // The tile (spec v1.4) follows the tray panel, not a tab.
    for open in [true, true, false, false] {
        host.set_tray_open(open);
        got.extend(host.take_visibility());
    }
    let wire: Vec<(String, Visibility)> = got.into_iter().map(|(m, v)| (m, v.state)).collect();
    let expected: Vec<Visibility> = steps("transcripts/ui/visibility.json")
        .iter()
        .filter(|s| s["msg"]["method"] == "ui.visibility")
        .map(|s| serde_json::from_value(s["msg"]["params"]["state"].clone()).unwrap())
        .collect();
    assert_eq!(
        wire,
        expected
            .into_iter()
            .map(|v| (ART.to_string(), v))
            .collect::<Vec<_>>()
    );
}

#[test]
fn banners_use_plain_words_and_keep_ids_in_details() {
    let mut host = PanelHost::default();
    let key = register_art(&mut host);
    host.module_gone(ART, "crash-looped");
    let p = host.panel(&key).unwrap();
    let line = p.state.banner("Art").unwrap();
    assert_eq!(line, "Art: Stopped after 5 crashes.");
    assert!(!line.contains("crash-looped"));
    assert_eq!(p.state.details().unwrap(), "State: crash-looped");
    let rejected = PanelState::Rejected("ui.limits".into());
    let line = rejected.banner("Art").unwrap();
    assert_eq!(line, "Art's page couldn't be shown.");
    assert_eq!(rejected.details().unwrap(), "Error: ui.limits");
}
