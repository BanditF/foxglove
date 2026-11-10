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
    });
    r.set_token(ART, "tok");
    r.set_launch_check(Box::new(|id, pid| {
        if id == ART && pid == Some(42) {
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
