// SPDX-License-Identifier: MIT
//! `example.room-map`: a sample module with one page, a floor plan drawn as a `canvas` whose
//! colours stream over a direct channel (docs/hideout-contract.md §5.4, §8.4).
//!
//! - The page declares the shapes once (`ui.register`); only colours move, as stream frames
//!   (kind 2, one RGBA per shape in declared order), never as patches.
//! - The hub opens the stream (`channel.create`) only while the page is on screen; the module
//!   answers with a one-time pipe and writes [`FPS`] frames a second until the hub closes it
//!   (minimised, another tab, Pause) or says `ui.visibility hidden`. Nothing runs while hidden.
//! - A warm light walks from room to room, so a live map is easy to tell from a frozen one.

use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use foxglove_plugin::channel::{self, ChannelConn};
use foxglove_plugin::hideout::{ChannelCreate, Handler, ModuleRegister, Register, RpcError};
use foxglove_plugin_api::hideout::frame::{encode, Frame};
use serde_json::{json, Value};

pub const ID: &str = "example.room-map";
/// The channel the canvas streams on.
pub const CHANNEL: &str = "map";
/// Frames a second the module sends (the hub draws at most 30).
pub const FPS: u32 = 25;
/// The canvas's own units (§5.2 v1.5): the box's aspect is 400:240.
pub const WIDTH: f32 = 400.0;
pub const HEIGHT: f32 = 240.0;

/// The floor plan: id, label, x, y, w, h in canvas units.
pub const ROOMS: &[(&str, &str, f32, f32, f32, f32)] = &[
    ("living", "Living", 8.0, 8.0, 184.0, 128.0),
    ("kitchen", "Kitchen", 200.0, 8.0, 112.0, 128.0),
    ("bath", "Bath", 320.0, 8.0, 72.0, 64.0),
    ("store", "Store", 320.0, 80.0, 72.0, 56.0),
    ("hall", "Hall", 8.0, 144.0, 104.0, 88.0),
    ("office", "Office", 120.0, 144.0, 136.0, 88.0),
    ("bed", "Bedroom", 264.0, 144.0, 128.0, 88.0),
];

/// The page: one canvas, a rounded key per room, then a text label per room (shape order is
/// the frame's colour order).
pub fn panels(instance: &str) -> Register {
    let mut shapes: Vec<Value> = ROOMS
        .iter()
        .map(|(id, _, x, y, w, h)| {
            json!({"type": "key", "id": id, "props": {"x": x, "y": y, "w": w, "h": h}})
        })
        .collect();
    // A dark chip behind each label, so it reads whatever colour the room is.
    shapes.extend(ROOMS.iter().map(|(id, label, x, y, _, _)| {
        let w = 12.0 + 7.5 * label.len() as f32;
        json!({"type": "key", "id": format!("{id}-chip"),
               "props": {"x": x + 4.0, "y": y + 4.0, "w": w, "h": 20}})
    }));
    shapes.extend(ROOMS.iter().map(|(id, label, x, y, _, _)| {
        json!({"type": "text", "id": format!("{id}-label"),
               "props": {"x": x + 10.0, "y": y + 6.0, "text": label, "size": 13}})
    }));
    serde_json::from_value(json!({
        "instance": instance,
        "panels": [{"id": "map", "slot": "page", "title": "Room map", "icon": "map", "tree":
            {"type": "column", "id": "root", "children": [
                {"type": "label", "id": "hint", "props": {"text": "Where the light is"}},
                {"type": "canvas", "id": "rooms", "props": {
                    "width": WIDTH, "height": HEIGHT, "label": "Room map",
                    "stream": CHANNEL, "stale_after_ms": 500},
                 "children": shapes}]}}],
    }))
    .expect("a valid ui.register")
}

pub fn registration() -> ModuleRegister {
    ModuleRegister {
        channels: vec![CHANNEL.into()],
        ..Default::default()
    }
}

/// The label colour, and the backing chip each label sits on so it reads on any room colour.
pub const LABEL: [u8; 4] = [235, 235, 240, 255];
pub const LABEL_BACKING: [u8; 4] = [22, 24, 32, 255];

/// WCAG relative luminance of an sRGB colour.
pub fn luminance(c: [u8; 4]) -> f32 {
    let lin = |v: u8| {
        let v = f32::from(v) / 255.0;
        if v <= 0.040_45 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * lin(c[0]) + 0.7152 * lin(c[1]) + 0.0722 * lin(c[2])
}

/// WCAG contrast ratio of two colours.
pub fn contrast(a: [u8; 4], b: [u8; 4]) -> f32 {
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// The colours at `t` seconds, in shape order: each room dim blue-grey, the lit room warm
/// orange, fading in and out as the light walks on (2 s per room); then the labels' backing
/// chips (dark) and the labels (light), which stay the same.
pub fn colours(t: f32) -> Vec<[u8; 4]> {
    let n = ROOMS.len() as f32;
    let pos = (t / 2.0) % n;
    let mut out: Vec<[u8; 4]> = (0..ROOMS.len())
        .map(|i| {
            let d = (pos - i as f32).abs().min(n - (pos - i as f32).abs());
            let glow = (1.0 - d).clamp(0.0, 1.0);
            let mix = |a: f32, b: f32| (a + (b - a) * glow).round() as u8;
            [mix(46.0, 255.0), mix(54.0, 140.0), mix(70.0, 60.0), 255]
        })
        .collect();
    out.extend(std::iter::repeat_n(LABEL_BACKING, ROOMS.len()));
    out.extend(std::iter::repeat_n(LABEL, ROOMS.len()));
    out
}

/// Writes frames to `conn` at [`FPS`] until a write fails (the hub closed the channel) or
/// `stop` is set. Returns the frames written.
pub fn stream(conn: &mut impl Write, stop: &AtomicBool) -> u32 {
    let start = Instant::now();
    let every = Duration::from_secs(1) / FPS;
    let mut seq = 0u32;
    while !stop.load(Ordering::SeqCst) {
        let frame = Frame::Canvas {
            seq,
            colours: colours(start.elapsed().as_secs_f32()),
        };
        if conn.write_all(&encode(&frame)).is_err() {
            break;
        }
        seq = seq.wrapping_add(1);
        // The frame clock: the next frame is due one interval after this one was.
        let due = start + every * seq;
        std::thread::sleep(due.saturating_duration_since(Instant::now()));
    }
    seq
}

/// The module: answers `channel.create` with a one-time pipe and streams on its own thread.
#[derive(Default)]
pub struct RoomMap {
    /// Set when the page is hidden or closed: the current stream ends.
    stop: Arc<AtomicBool>,
    pub exit: bool,
}

impl Handler for RoomMap {
    fn channel_create(&mut self, c: ChannelCreate) -> Result<Value, RpcError> {
        if c.channel != CHANNEL {
            return Err(RpcError::new(1, "no such channel"));
        }
        // A new stream gets a fresh stop flag; the old one (if any) was already told.
        self.stop = Arc::new(AtomicBool::new(false));
        let stop = self.stop.clone();
        // For the consumer the hub names only, in or out of an AppContainer (#1092).
        let pipe = channel::serve_create(ID, &c, move |mut conn: ChannelConn| {
            stream(&mut conn, &stop);
        })
        .map_err(|e| RpcError::new(6, e.to_string()))?;
        Ok(json!({ "pipe": pipe }))
    }

    fn notification(&mut self, method: &str, p: Value) {
        if method == "ui.visibility" && p["state"] != "visible" {
            self.stop.store(true, Ordering::SeqCst);
        }
    }

    fn exit(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        self.exit = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use foxglove_plugin_api::hideout::frame::decode_wire;
    use foxglove_plugin_api::hideout::panel::{validate, Panel, LIMITS};

    #[test]
    fn the_page_is_valid_and_one_colour_per_shape() {
        let reg = panels("i1");
        let page: Panel = serde_json::from_value(reg.panels[0].clone()).unwrap();
        let canvas = &page.tree.children[1];
        assert_eq!(canvas.stream_name(), Some(CHANNEL));
        assert_eq!(canvas.children.len(), colours(0.0).len());
        validate(&page, &LIMITS).expect("a valid page");
    }

    #[test]
    fn every_label_reads_on_its_room_at_every_moment() {
        let n = ROOMS.len();
        for step in 0..(n * 2 * 25) {
            let c = colours(step as f32 / 25.0);
            for i in 0..n {
                // The label sits on its chip, never on the room itself.
                let r = contrast(c[2 * n + i], c[n + i]);
                assert!(r >= 4.5, "label {i} at {step}: {r:.2}:1");
            }
        }
    }

    #[test]
    fn the_manifest_declares_the_page_and_offers_the_channel() {
        use foxglove_plugin_api::hideout::module::{Context, Module};
        let m = Module::parse(include_str!("../module.toml"), &Context::default()).unwrap();
        assert_eq!(m.id, ID);
        let declared: Vec<&str> = m.panels.iter().map(|p| p.id.as_str()).collect();
        let registered: Vec<String> = panels("i1")
            .panels
            .iter()
            .map(|p| p["id"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(declared, registered);
        let text = include_str!("../module.toml");
        assert!(text.contains(&format!("\"channel:{CHANNEL}\"")));
    }

    #[test]
    fn streams_between_20_and_30_frames_a_second_until_stopped() {
        struct Sink(Vec<Vec<u8>>);
        impl Write for Sink {
            fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
                self.0.push(b.to_vec());
                Ok(b.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let stop = Arc::new(AtomicBool::new(false));
        let s2 = stop.clone();
        let t = std::thread::spawn(move || {
            let mut sink = Sink(Vec::new());
            let n = stream(&mut sink, &s2);
            (n, sink.0)
        });
        std::thread::sleep(Duration::from_secs(1));
        stop.store(true, Ordering::SeqCst);
        let (n, frames) = t.join().unwrap();
        assert!((20..=31).contains(&n), "{n} frames in a second");
        for f in frames {
            assert!(matches!(decode_wire(&f), Ok(Frame::Canvas { .. })));
        }
    }
}
