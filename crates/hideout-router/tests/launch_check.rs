// SPDX-License-Identifier: MIT
//! The supervisor's launch check at `initialize` (§7.5, §8.2 v1.3): a good token from the wrong
//! process is refused with error 5 and reported as [`Out::Refused`]; the right one passes.

use std::collections::BTreeMap;
use std::time::Instant;

use hideout_contract::capability::Holder;
use hideout_router::{ModuleEntry, Origin, Out, Router, Settings};
use serde_json::{json, Value};

const ART: &str = "foxglove.example-art";

fn router() -> Router {
    let mut r = Router::new(Settings::default(), Box::new(|_, _| Vec::new()));
    r.add_module(ModuleEntry {
        id: ART.into(),
        name: "Example Art".into(),
        holder: Holder::FirstParty,
        granted: vec![],
        catalog_provide: Some(vec![]),
        channel_risk: BTreeMap::new(),
        activate_on_call: false,
        panels: None,
        settings_dir: String::new(),
        data_dir: String::new(),
        registered_version: None,
    });
    r.set_token(ART, "tok");
    r.set_launch_check(Box::new(|id, pid| {
        if (id == ART || id == "vulpes") && pid == Some(42) {
            Ok(())
        } else {
            Err(format!("{id} from {pid:?}"))
        }
    }));
    r
}

fn initialize(r: &mut Router, pid: u32) -> Vec<Out> {
    let c = r.connect(Origin::Pipe);
    r.set_peer_pid(c, pid);
    let msg = json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {"id": ART, "version": "0.4.0", "instance": "i", "token": "tok",
                   "hub": "1.0", "api": {"foxglove": "1.13"}}});
    r.receive(c, &msg.to_string(), Instant::now())
}

fn answer(out: &[Out]) -> Value {
    out.iter()
        .find_map(|o| match o {
            Out::Send(_, v) => Some(v.clone()),
            _ => None,
        })
        .unwrap()
}

#[test]
fn the_started_process_passes() {
    let mut r = router();
    let out = initialize(&mut r, 42);
    assert!(answer(&out).get("result").is_some(), "{out:?}");
    assert!(!out.iter().any(|o| matches!(o, Out::Refused(_))));
}

#[test]
fn another_process_with_the_token_is_refused() {
    let mut r = router();
    let out = initialize(&mut r, 43);
    assert_eq!(answer(&out)["error"]["code"], 5);
    assert!(out.contains(&Out::Refused(ART.into())));
    assert!(out.iter().any(|o| matches!(o, Out::Close(_))));
}

fn adopted_router(registered: &str) -> Router {
    let mut r = router();
    r.add_module(ModuleEntry {
        id: "vulpes".into(),
        name: "Example AI".into(),
        holder: Holder::ThirdParty(hideout_contract::capability::Source::Sideload),
        granted: vec![],
        catalog_provide: None,
        channel_risk: BTreeMap::new(),
        activate_on_call: false,
        panels: None,
        settings_dir: String::new(),
        data_dir: String::new(),
        registered_version: Some(registered.into()),
    });
    r.set_token("vulpes", "tok");
    r
}

fn initialize_adopted(r: &mut Router, version: Option<&str>) -> Vec<Out> {
    let c = r.connect(Origin::Pipe);
    r.set_peer_pid(c, 42);
    let mut params = json!({"id": "vulpes", "instance": "i", "token": "tok",
                            "hub": "1.0", "api": {"foxglove": "1.13"}});
    if let Some(v) = version {
        params["version"] = json!(v);
    }
    let msg = json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": params});
    r.receive(c, &msg.to_string(), Instant::now())
}

#[test]
fn an_adopted_module_saying_its_registered_version_connects() {
    let mut r = adopted_router("0.4.0");
    let out = initialize_adopted(&mut r, Some("0.4.0"));
    assert!(answer(&out).get("result").is_some(), "{out:?}");
    assert!(!out.iter().any(|o| matches!(o, Out::Refused(_))));
}

#[test]
fn an_adopted_module_with_another_version_is_stopped_as_a_trust_mismatch() {
    for said in [Some("0.5.0"), None] {
        let mut r = adopted_router("0.4.0");
        let out = initialize_adopted(&mut r, said);
        let a = answer(&out);
        assert_eq!(a["error"]["code"], 5, "{said:?}");
        assert!(a["error"].to_string().contains("trust.mismatch"), "{a}");
        assert!(out.contains(&Out::Refused("vulpes".into())));
        assert!(out.iter().any(|o| matches!(o, Out::Close(_))));
    }
}
