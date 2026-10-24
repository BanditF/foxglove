//! The plugin end to end: the plugin host's runner starts it as Foxglove does, a sender's E1.31 frame on
//! localhost takes a low-priority lease on a mock keyboard, new colours change it in place, and
//! the lease ends when the sender ends its stream or goes silent.

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::{free_port, start, Keyboard};
use serde_json::json;

static PORT: std::sync::OnceLock<u16> = std::sync::OnceLock::new();

fn port() -> u16 {
    *PORT.get_or_init(free_port)
}

fn send(seq: u8, r: u8, g: u8, b: u8, stop: bool) {
    let mut cmd = std::process::Command::new(env!("CARGO_BIN_EXE_openrgb-e131"));
    cmd.args(["--send-test", &r.to_string(), &g.to_string(), &b.to_string(), "--seq", &seq.to_string()]);
    cmd.args(["--port", &port().to_string()]);
    if stop {
        cmd.arg("--stop");
    }
    assert!(cmd.status().unwrap().success());
}

#[test]
fn a_stream_holds_a_low_lease_and_its_end_gives_the_lights_back() {
    let keyboard = Arc::new(Keyboard::default());
    let plugin = start("test.openrgb-e131", port(), keyboard.clone());
    // Waits are for outcomes, with generous limits; nothing depends on how fast the machine is.
    // A red frame, sent until it lands (the plugin may still be starting and binding the port).
    let until = Instant::now() + Duration::from_secs(30);
    let mut seq = 0u8;
    while keyboard.leases().is_empty() && Instant::now() < until {
        seq += 1;
        send(seq, 255, 0, 0, false);
        std::thread::sleep(Duration::from_millis(300));
    }
    let calls = keyboard.wait("a lease", |c| !c.is_empty());
    assert_eq!(calls[0], ("lighting.lease".into(), json!({"device": "kb", "color": {"hue": 0, "saturation": 100, "brightness": 100}, "effect": null, "priority": "low"})));
    // A new colour changes the same lease; the same colour again sends nothing.
    seq += 1;
    send(seq, 0, 255, 0, false);
    let calls = keyboard.wait("an update", |c| c.iter().any(|(m, _)| m == "lighting.update"));
    let update = calls.iter().find(|(m, _)| m == "lighting.update").unwrap();
    assert_eq!(update.1, json!({"lease": 7, "color": {"hue": 120, "saturation": 100, "brightness": 100}}));
    seq += 1;
    send(seq, 0, 255, 0, false);
    // The sender ends its stream: the lease is given back, once, and no new one was taken.
    send(seq + 1, 0, 0, 0, true);
    let calls = keyboard.wait("the release", |c| c.iter().any(|(m, _)| m == "lighting.release"));
    assert_eq!(calls.iter().filter(|(m, _)| m == "lighting.lease").count(), 1, "{calls:?}");
    assert_eq!(calls.iter().filter(|(m, _)| m == "lighting.update").count(), 1, "the repeat sent nothing: {calls:?}");
    assert_eq!(calls.iter().find(|(m, _)| m == "lighting.release").unwrap().1, json!({"lease": 7}));
    // A new stream that goes silent: released after E1.31's loss timeout.
    send(seq + 2, 0, 0, 255, false);
    keyboard.wait("a second lease", |c| c.iter().filter(|(m, _)| m == "lighting.lease").count() == 2);
    keyboard.wait("the release after silence", |c| c.iter().filter(|(m, _)| m == "lighting.release").count() == 2);
    drop(plugin);
}
