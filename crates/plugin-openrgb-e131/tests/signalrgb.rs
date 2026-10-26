//! Foxglove for SignalRGB (#331) end to end: the packets its plugin sends (recorded from
//! integrations/signalrgb/Foxglove.js by record-packets.mjs) go to the receiver running as
//! Foxglove runs it, and drive the keyboard's lease: a low lease on the first colour, the same
//! lease changed on the next, and given back when SignalRGB closes.

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::{free_port, start, Keyboard};
use serde_json::json;

/// The recorded session: each packet as a big-endian u16 length and its bytes.
fn session() -> Vec<Vec<u8>> {
    let mut bytes: &[u8] = include_bytes!("fixtures/signalrgb-session.bin");
    let mut packets = Vec::new();
    while bytes.len() >= 2 {
        let n = usize::from(u16::from_be_bytes([bytes[0], bytes[1]]));
        packets.push(bytes[2..2 + n].to_vec());
        bytes = &bytes[2 + n..];
    }
    packets
}

#[test]
fn signalrgb_packets_take_change_and_give_back_the_lease() {
    let packets = session();
    assert_eq!(packets.len(), 3, "red, green, then the stream's end");
    let port = free_port();
    let keyboard = Arc::new(Keyboard::default());
    let _plugin = start("test.signalrgb-e131", port, keyboard.clone());
    let socket = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let send = |p: &[u8]| socket.send_to(p, ("127.0.0.1", port)).unwrap();
    // The first packet, until the plugin is up and listening (resending SignalRGB's first frame
    // as its keep-alive would: same sequence, so later ones are not stale).
    let until = Instant::now() + Duration::from_secs(30);
    while keyboard.leases().is_empty() && Instant::now() < until {
        send(&packets[0]);
        std::thread::sleep(Duration::from_millis(300));
    }
    let calls = keyboard.wait("a lease", |c| !c.is_empty());
    assert_eq!(
        calls[0],
        (
            "lighting.lease".into(),
            json!({"device": "kb", "color": {"hue": 0, "saturation": 100, "brightness": 100}, "effect": null, "priority": "low"})
        )
    );
    send(&packets[1]);
    let calls = keyboard.wait("the green update", |c| {
        c.iter().any(|(m, _)| m == "lighting.update")
    });
    let update = calls.iter().find(|(m, _)| m == "lighting.update").unwrap();
    assert_eq!(
        update.1,
        json!({"lease": 7, "color": {"hue": 120, "saturation": 100, "brightness": 100}})
    );
    send(&packets[2]);
    let calls = keyboard.wait("the release", |c| {
        c.iter().any(|(m, _)| m == "lighting.release")
    });
    assert_eq!(
        calls
            .iter()
            .find(|(m, _)| m == "lighting.release")
            .unwrap()
            .1,
        json!({"lease": 7})
    );
    assert_eq!(
        calls.iter().filter(|(m, _)| m == "lighting.lease").count(),
        1,
        "one lease for the session: {calls:?}"
    );
}
