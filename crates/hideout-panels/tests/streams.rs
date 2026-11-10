// SPDX-License-Identifier: MIT
//! Live streams in panels (§5.4, H2.9), window-free: frames only while visible, the 30 fps
//! cap (frames over it skipped, not queued), stop on hidden and minimised, the reopen schedule,
//! the text trim and restore, and the words (nothing from the UX plan's "never shown" list,
//! nothing announced per frame). The real pipes are in `stream_io` below.

use std::time::{Duration, Instant};

use hideout_contract::frame::{encode, Frame};
use hideout_contract::messages::{PatchOp, PatchOpKind, UiPatch};
use hideout_contract::panel::Register;
use hideout_panels::streams::{
    accessible_description, look_words, Look, Phase, StreamAct, StreamCommand, StreamKey, Streams,
    TRIMMED_LINE,
};
use hideout_panels::{PanelHost, PanelKey};
use serde_json::json;

const M: &str = "example.room";

fn page() -> Register {
    serde_json::from_value(json!({"instance": "i1", "panels": [
        {"id": "room", "slot": "page", "title": "Room", "icon": "x", "tree":
            {"type": "column", "id": "root", "children": [
                {"type": "canvas", "id": "map", "props": {"width": 120, "height": 60,
                  "label": "Room map", "stream": "previews", "stale_after_ms": 500},
                 "children": [
                    {"type": "rect", "id": "a", "props": {"x": 0, "y": 0, "w": 40, "h": 60}},
                    {"type": "rect", "id": "b", "props": {"x": 40, "y": 0, "w": 40, "h": 60}},
                    {"type": "rect", "id": "c", "props": {"x": 80, "y": 0, "w": 40, "h": 60}}]},
                {"type": "image", "id": "shot", "props": {"label": "Screen preview",
                  "aspect": 2.0, "scaling": "pixel", "stream": "screen"}},
                {"type": "text", "id": "log", "props": {"mono": true, "stream": "log"}}]}},
        {"id": "other", "slot": "page", "title": "Other", "icon": "x",
         "tree": {"type": "label", "id": "l", "props": {"text": "-"}}}]}))
    .unwrap()
}

fn host() -> (PanelHost, PanelKey) {
    let mut h = PanelHost::default();
    assert!(h.register(M, "Room", None, &page()).is_empty());
    (h, PanelKey::new(M, "room"))
}

fn key(k: &PanelKey, node: &str) -> StreamKey {
    StreamKey::new(k.clone(), node)
}

fn canvas(seq: u32) -> Vec<u8> {
    let w = encode(&Frame::Canvas {
        seq,
        colours: vec![[seq as u8, 0, 0, 255]; 3],
    });
    w[4..].to_vec()
}

fn text(s: &str) -> Vec<u8> {
    let w = encode(&Frame::Text {
        seq: 0,
        append: s.into(),
    });
    w[4..].to_vec()
}

/// Opens everything that is due and answers each open with a pipe; returns (key, gen) opened.
fn grant_all(h: &mut PanelHost) -> Vec<(StreamKey, u64)> {
    let mut opened = Vec::new();
    for c in h.streams.take_commands() {
        if let StreamCommand::Open {
            key,
            module,
            channel,
            gen,
        } = c
        {
            let r = h.streams.granted(
                &module,
                &channel,
                Ok(json!({"pipe": r"\\.\pipe\hideout-ch-1-0-x", "nonce": "n"})),
            );
            assert_eq!(r.map(|r| (r.0, r.1)), Some((key.clone(), gen)));
            opened.push((key, gen));
        }
    }
    opened
}

/// The router answers each open, in order (as it does).
fn answered(h: &mut PanelHost, cmds: Vec<StreamCommand>) -> Vec<StreamCommand> {
    for c in &cmds {
        if let StreamCommand::Open {
            module, channel, ..
        } = c
        {
            h.streams.granted(
                module,
                channel,
                Ok(json!({"pipe": r"\\.\pipe\hideout-ch-1-0-x", "nonce": "n"})),
            );
        }
    }
    cmds
}

fn gen_of(opened: &[(StreamKey, u64)], k: &StreamKey) -> u64 {
    opened.iter().find(|(o, _)| o == k).unwrap().1
}

#[test]
fn streams_open_only_while_the_panel_is_visible() {
    let (mut h, room) = host();
    let t0 = Instant::now();
    h.sync_streams(t0);
    assert!(h.streams.take_commands().is_empty(), "nothing opens unseen");

    h.set_visible(std::slice::from_ref(&room));
    let vis = h.take_visibility();
    assert_eq!(vis.len(), 2, "room visible, other hidden");
    h.sync_streams(t0);
    let opened = grant_all(&mut h);
    let nodes: Vec<&str> = opened.iter().map(|(k, _)| k.node.as_str()).collect();
    assert_eq!(nodes, ["log", "map", "shot"]);

    // Another tab, then minimised: every stream closes, nothing reopens.
    for shown in [vec![PanelKey::new(M, "other")], vec![]] {
        h.set_visible(&shown);
        h.sync_streams(t0);
        let cmds = h.streams.take_commands();
        if shown.is_empty() {
            assert!(cmds.is_empty(), "already closed");
        } else {
            assert_eq!(cmds.len(), 3);
            assert!(cmds
                .iter()
                .all(|c| matches!(c, StreamCommand::Close { .. })));
        }
    }
    // A frame from a closed channel is ignored.
    let m = key(&room, "map");
    assert!(h
        .streams
        .frame(&m, gen_of(&opened, &m), &canvas(1), t0)
        .is_none());
    assert!(!h.streams.present(t0));
    assert_eq!(h.streams.get(&m).unwrap().phase, Phase::Closed);
}

#[test]
fn frames_over_30_a_second_are_skipped_not_queued() {
    let (mut h, room) = host();
    let t0 = Instant::now();
    h.set_visible(std::slice::from_ref(&room));
    h.sync_streams(t0);
    let opened = grant_all(&mut h);
    let m = key(&room, "map");
    let g = gen_of(&opened, &m);
    // 60 frames over one second (the provider's cap), the hub repainting after each.
    let mut drawn = 0;
    for i in 0..60u32 {
        let t = t0 + Duration::from_micros(16_667 * u64::from(i));
        assert!(h.streams.frame(&m, g, &canvas(i), t).is_none());
        if h.streams.present(t) {
            drawn += 1;
        }
    }
    let s = h.streams.get(&m).unwrap();
    assert!((29..=31).contains(&drawn), "drawn {drawn}");
    assert_eq!(s.drawn, drawn);
    // Every frame not drawn was replaced by a newer one: no backlog is kept.
    // At most one waits for the next slot.
    assert!(s.skipped + s.drawn >= 59, "{} + {}", s.skipped, s.drawn);
    // The next repaint draws the newest frame, not an old one.
    h.streams.present(t0 + Duration::from_secs(2));
    assert_eq!(s_colour(&h, &m), [59, 0, 0, 255], "latest frame wins");
    // A burst of frames between two repaints draws once, with the newest colours.
    let t = t0 + Duration::from_secs(3);
    for i in 100..110 {
        h.streams.frame(&m, g, &canvas(i), t);
    }
    assert!(h.streams.present(t));
    assert_eq!(s_colour(&h, &m), [109, 0, 0, 255]);

    // Reduced motion: 2 a second.
    h.streams.reduced_motion = true;
    let mut drawn = 0;
    let t4 = t0 + Duration::from_secs(4);
    for i in 0..30u32 {
        let t = t4 + Duration::from_millis(33 * u64::from(i));
        h.streams.frame(&m, g, &canvas(i), t);
        if h.streams.present(t) {
            drawn += 1;
        }
    }
    assert!(drawn <= 2, "reduced motion drew {drawn} in a second");
    assert_eq!(h.streams.look(&m, true, t4), Look::Live);
}

fn s_colour(h: &PanelHost, k: &StreamKey) -> [u8; 4] {
    match h.streams.get(k).unwrap().picture.as_ref().unwrap() {
        hideout_panels::streams::Picture::Canvas(c) => c[0],
        other => panic!("{other:?}"),
    }
}

#[test]
fn over_the_rate_or_size_cap_is_refused_and_not_reopened() {
    let (mut h, room) = host();
    let t0 = Instant::now();
    h.set_visible(std::slice::from_ref(&room));
    h.sync_streams(t0);
    let opened = grant_all(&mut h);
    let m = key(&room, "map");
    let g = gen_of(&opened, &m);
    let mut closed = None;
    for i in 0..61u32 {
        closed =
            closed.or(h
                .streams
                .frame(&m, g, &canvas(i), t0 + Duration::from_millis(i.into())));
    }
    assert!(
        matches!(closed, Some(StreamCommand::Close { .. })),
        "61 a second"
    );
    assert_eq!(h.streams.look(&m, true, t0), Look::Refused);
    h.streams.ended(&m, g, t0);
    h.streams.tick(t0 + Duration::from_secs(10));
    assert!(h.streams.take_commands().is_empty(), "never reopened");

    let s = key(&room, "shot");
    let big = vec![0u8; 4 * 1024 * 1024 + 13];
    assert!(h.streams.frame(&s, gen_of(&opened, &s), &big, t0).is_some());
    assert_eq!(h.streams.look(&s, true, t0), Look::Refused);
}

#[test]
fn reopen_after_1_2_4_s_then_stopped_and_try_again_once() {
    let (mut h, room) = host();
    let t0 = Instant::now();
    h.set_visible(std::slice::from_ref(&room));
    h.sync_streams(t0);
    let mut g = gen_of(&grant_all(&mut h), &key(&room, "map"));
    let m = key(&room, "map");
    h.streams.frame(&m, g, &canvas(1), t0);
    h.streams.present(t0);
    let mut t = t0;
    for wait in [1u64, 2, 4] {
        h.streams.ended(&m, g, t);
        assert_eq!(h.streams.get(&m).unwrap().phase, Phase::Reopening);
        h.streams.tick(t + Duration::from_millis(wait * 1000 - 1));
        assert!(h.streams.take_commands().is_empty(), "not before {wait} s");
        t += Duration::from_secs(wait);
        h.streams.tick(t);
        let taken = h.streams.take_commands();
        let cmds = answered(&mut h, taken);
        assert_eq!(cmds.len(), 1, "reopened after {wait} s");
        let StreamCommand::Open { gen, .. } = &cmds[0] else {
            panic!("{cmds:?}")
        };
        g = *gen;
    }
    h.streams.ended(&m, g, t);
    assert_eq!(h.streams.look(&m, true, t), Look::Stopped);
    h.streams.tick(t + Duration::from_secs(60));
    assert!(h.streams.take_commands().is_empty(), "no fourth reopen");

    // Try again: one reopen; if that ends too, Stopped again.
    h.streams.act(&StreamAct::TryAgain(m.clone()), t);
    let taken = h.streams.take_commands();
    let cmds = answered(&mut h, taken);
    let [StreamCommand::Open { gen, .. }] = cmds.as_slice() else {
        panic!("{cmds:?}")
    };
    h.streams.ended(&m, *gen, t);
    assert_eq!(h.streams.look(&m, true, t), Look::Stopped);

    // Visible again starts the count afresh.
    h.set_visible(&[]);
    h.sync_streams(t);
    h.set_visible(std::slice::from_ref(&room));
    h.sync_streams(t);
    let g2 = gen_of(&grant_all(&mut h), &m);
    h.streams.ended(&m, g2, t);
    assert_eq!(h.streams.get(&m).unwrap().phase, Phase::Reopening);
}

#[test]
fn a_patch_removing_the_stream_closes_cleanly() {
    let (mut h, room) = host();
    let t0 = Instant::now();
    h.set_visible(std::slice::from_ref(&room));
    h.sync_streams(t0);
    grant_all(&mut h);
    let p = UiPatch {
        panel: "room".into(),
        ops: vec![PatchOp {
            op: PatchOpKind::Set,
            node: "map".into(),
            props: json!({"stream": null}).as_object().cloned(),
            index: None,
            tree: None,
        }],
    };
    h.patch(M, &p, t0).unwrap();
    h.sync_streams(t0);
    let cmds = h.streams.take_commands();
    assert!(matches!(cmds.as_slice(), [StreamCommand::Close { key, .. }] if key.node == "map"));
    assert!(h.streams.get(&key(&room, "map")).is_none(), "no state kept");
}

#[test]
fn stale_dims_with_last_seen_and_back_on_screen_stays_undimmed_for_2_s() {
    let (mut h, room) = host();
    let t0 = Instant::now();
    h.set_visible(std::slice::from_ref(&room));
    h.sync_streams(t0);
    let m = key(&room, "map");
    let g = gen_of(&grant_all(&mut h), &m);
    assert_eq!(
        h.streams.look(&m, true, t0),
        Look::Empty,
        "no flash before 1 s"
    );
    assert_eq!(
        h.streams.look(&m, true, t0 + Duration::from_millis(1100)),
        Look::Connecting
    );
    h.streams.frame(&m, g, &canvas(1), t0);
    h.streams.present(t0);
    assert_eq!(
        h.streams.look(&m, true, t0 + Duration::from_millis(400)),
        Look::Live
    );
    let late = h.streams.look(&m, true, t0 + Duration::from_millis(3000));
    assert_eq!(late, Look::LastSeen(Duration::from_secs(3)));
    assert_eq!(look_words(late).unwrap(), "Last seen 3 s ago");
    // Module not running: Last seen, whatever the gap.
    assert!(matches!(
        h.streams.look(&m, false, t0 + Duration::from_millis(10)),
        Look::LastSeen(_)
    ));
    // Hidden and back: the old frame, undimmed for up to 2 s.
    let t1 = t0 + Duration::from_millis(200);
    h.set_visible(&[]);
    h.sync_streams(t1);
    h.set_visible(std::slice::from_ref(&room));
    let t2 = t0 + Duration::from_millis(300);
    h.sync_streams(t2);
    assert_eq!(
        h.streams.look(&m, true, t2 + Duration::from_millis(1500)),
        Look::Live
    );
    assert!(matches!(
        h.streams.look(&m, true, t2 + Duration::from_millis(2100)),
        Look::LastSeen(_)
    ));
    // Pause: closed as if hidden, full colour with "Paused".
    h.streams.take_commands();
    h.streams.act(&StreamAct::Pause(m.clone()), t2);
    assert!(matches!(
        h.streams.take_commands().as_slice(),
        [StreamCommand::Close { .. }]
    ));
    assert_eq!(h.streams.look(&m, true, t2), Look::Paused);
    h.streams.act(&StreamAct::Play(m.clone()), t2);
    assert!(matches!(
        h.streams.take_commands().as_slice(),
        [StreamCommand::Open { .. }]
    ));
}

#[test]
fn streamed_text_trims_oldest_whole_lines_past_the_cap() {
    let (mut h, room) = host();
    h.streams = Streams::with_text_cap(32);
    let t0 = Instant::now();
    h.set_visible(std::slice::from_ref(&room));
    h.sync_streams(t0);
    let l = key(&room, "log");
    let g = gen_of(&grant_all(&mut h), &l);
    for i in 0..10 {
        h.streams.frame(&l, g, &text(&format!("line {i}\n")), t0);
    }
    let s = h.streams.get(&l).unwrap();
    assert!(s.trimmed);
    assert!(s.text.len() <= 32);
    assert_eq!(s.text, "line 6\nline 7\nline 8\nline 9\n");
    // A single line over the cap keeps its newest bytes, cut at a character boundary.
    h.streams.frame(&l, g, &text(&"\u{e9}".repeat(40)), t0);
    let s = h.streams.get(&l).unwrap();
    assert!(s.text.len() <= 32 && s.text.chars().all(|c| c == '\u{e9}'));
    assert_eq!(TRIMMED_LINE, "Earlier text was trimmed.");
    // Text never goes stale: silence is normal.
    assert_eq!(
        h.streams.look(&l, true, t0 + Duration::from_secs(600)),
        Look::Live
    );
}

#[test]
fn the_full_1_mib_cap_holds() {
    let mut s = Streams::default();
    let (mut h, room) = host();
    std::mem::swap(&mut h.streams, &mut s);
    let t0 = Instant::now();
    h.set_visible(std::slice::from_ref(&room));
    h.sync_streams(t0);
    let l = key(&room, "log");
    let g = gen_of(&grant_all(&mut h), &l);
    let line = format!("{}\n", "x".repeat(1023));
    for i in 0..1100u64 {
        // 50 a second, under the rate cap.
        h.streams
            .frame(&l, g, &text(&line), t0 + Duration::from_millis(20 * i));
    }
    let st = h.streams.get(&l).unwrap();
    assert_eq!(
        st.text.len(),
        1024 * 1024,
        "1,048,576 bytes kept, whole lines"
    );
    assert!(st.trimmed);
}

#[test]
fn the_words_never_name_the_plumbing_and_frames_never_change_the_name() {
    let looks = [
        Look::Empty,
        Look::Connecting,
        Look::Live,
        Look::Paused,
        Look::LastSeen(Duration::from_secs(12)),
        Look::LastSeen(Duration::from_secs(130)),
        Look::Stopped,
        Look::Refused,
    ];
    let never = [
        "channel",
        "pipe",
        "nonce",
        "frame",
        "seq",
        "fps",
        "ui.limits",
        "error",
        "byte",
    ];
    for l in looks {
        let words = format!(
            "{} {}",
            look_words(l).unwrap_or_default(),
            accessible_description("Room map", l)
        )
        .to_lowercase();
        for n in never {
            assert!(!words.contains(n), "{l:?}: {words}");
        }
    }
    assert_eq!(
        accessible_description("Room map", Look::Live),
        "Room map, live preview"
    );
    assert_eq!(
        look_words(Look::LastSeen(Duration::from_secs(130))).unwrap(),
        "Last seen 2 min ago"
    );
    // A live stream's description is the same across frames: nothing to announce.
    let (mut h, room) = host();
    let t0 = Instant::now();
    h.set_visible(std::slice::from_ref(&room));
    h.sync_streams(t0);
    let m = key(&room, "map");
    let g = gen_of(&grant_all(&mut h), &m);
    let mut seen = std::collections::BTreeSet::new();
    for i in 0..20u32 {
        let t = t0 + Duration::from_millis(40 * u64::from(i));
        h.streams.frame(&m, g, &canvas(i), t);
        h.streams.present(t);
        seen.insert(accessible_description(
            "Room map",
            h.streams.look(&m, true, t),
        ));
    }
    assert_eq!(seen.len(), 1, "{seen:?}");
}

#[test]
fn text_is_announced_once_after_a_run_ends_at_most_every_10_s() {
    let (mut h, room) = host();
    let t0 = Instant::now();
    h.set_visible(std::slice::from_ref(&room));
    h.sync_streams(t0);
    let l = key(&room, "log");
    let g = gen_of(&grant_all(&mut h), &l);
    let said = |h: &PanelHost, t: Instant| {
        h.streams
            .get(&l)
            .unwrap()
            .announcing_until
            .is_some_and(|u| t < u)
    };
    // A run of text: nothing while it arrives.
    for i in 0..10u64 {
        let t = t0 + Duration::from_millis(100 * i);
        h.streams.frame(&l, g, &text("x\n"), t);
        h.streams.present(t);
        assert!(!said(&h, t), "announced while text arrives");
    }
    let quiet = t0 + Duration::from_millis(900 + 1500);
    // The hub is woken for it (the other nodes' marks may come first).
    assert!(h.streams.next_wake().is_some_and(|t| t <= quiet));
    h.streams.present(quiet - Duration::from_millis(1));
    assert!(
        !said(&h, quiet - Duration::from_millis(1)),
        "not before 1.5 s quiet"
    );
    h.streams.present(quiet);
    assert!(said(&h, quiet), "one announcement after 1.5 s quiet");
    // More text 3 s later: held until 10 s after the first.
    let t = quiet + Duration::from_secs(3);
    h.streams.frame(&l, g, &text("y\n"), t);
    h.streams.present(t + Duration::from_secs(2));
    assert!(!said(&h, t + Duration::from_secs(2)));
    h.streams.present(quiet + Duration::from_secs(10));
    assert!(said(&h, quiet + Duration::from_secs(10)));
    // Not while focus is inside the node.
    let t = quiet + Duration::from_secs(30);
    h.streams.frame(&l, g, &text("z\n"), t);
    let later = t + Duration::from_secs(2);
    h.streams.act(&StreamAct::FocusInside(l.clone()), later);
    h.streams.present(later);
    assert!(!said(&h, later + Duration::from_secs(3)));
}
