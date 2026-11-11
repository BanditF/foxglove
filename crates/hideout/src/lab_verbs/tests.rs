// SPDX-License-Identifier: MIT
//! #1096, window-free: outside a lab hub every hub verb is refused before anything is staged or
//! queued (the supervisor's own check; hideout-supervisor's `lab_verbs` test drives the states).

use super::*;
use hideout_router::server::Hub;
use hideout_router::{Router, Settings};

#[cfg(windows)]
#[test]
fn every_hub_verb_is_refused_outside_a_lab_hub() {
    // No HIDEOUT_LAB in this test binary: the registry reads lab mode off.
    let home = std::env::temp_dir().join(format!("hideout-lab-verbs-off-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).unwrap();
    let router = Router::new(Settings::default(), Box::new(|_, asked| asked.to_vec()));
    let (events, _rx) = std::sync::mpsc::channel();
    let sup = Supervisor::start(&home, Hub::new(router, events));
    let short = Duration::from_millis(50);
    for (verb, rest) in [
        ("update-ready", "org.example.art a flash"),
        ("update-waiting", "org.example.art devices.read"),
        ("updating", "org.example.art"),
        ("quarantine", "org.example.art"),
        ("module-state", "org.example.art quarantined"),
    ] {
        let e = answer_within(&sup, verb, rest, short).unwrap_err();
        assert!(e.starts_with("lab hub only"), "{verb}: {e}");
    }
    assert!(answer_within(&sup, "updating", " ", short).is_err());
    assert!(answer_within(&sup, "bogus", "org.example.art", short)
        .unwrap_err()
        .contains("unknown hub verb"));
    drop(sup);
    let _ = std::fs::remove_dir_all(&home);
}
