// SPDX-License-Identifier: MIT
//! Review images of the stream states (H2.9), window-free: the panel host draws each state of a
//! canvas, image and text stream, and the sample room map page, into egui meshes; the test
//! rasterises them itself (no window, no GPU) and writes PNGs to `HIDEOUT_REVIEW_DIR` (default:
//! the temp dir). Also checks that a frame of another size does not move the page.
#![cfg(windows)]

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use egui::{epaint, Pos2, Rect, TextureId, Vec2};
use hideout_contract::frame::{encode, Frame};
use hideout_contract::panel::Register;
use hideout_panels::streams::{StreamAct, StreamCommand, StreamKey};
use hideout_panels::{panel_ui, PanelHost, PanelKey, PanelState};
use serde_json::{json, Value};

const M: &str = "example.review";

fn rasterise(w: u32, h: u32, mut draw: impl FnMut(&egui::Context)) -> Option<Vec<u8>> {
    let fonts = hideout_panels::render::test_fonts()?;
    let ctx = egui::Context::default();
    ctx.set_fonts(fonts);
    ctx.set_visuals(egui::Visuals::dark());
    let mut textures: HashMap<TextureId, epaint::ColorImage> = HashMap::new();
    let mut last = None;
    for _ in 0..3 {
        let raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(
                Pos2::ZERO,
                Vec2::new(w as f32, h as f32),
            )),
            ..Default::default()
        };
        let out = ctx.run(raw, &mut draw);
        for (id, delta) in &out.textures_delta.set {
            let epaint::ImageData::Color(img) = &delta.image;
            match delta.pos {
                None => {
                    textures.insert(*id, (**img).clone());
                }
                Some([x0, y0]) => {
                    if let Some(t) = textures.get_mut(id) {
                        for y in 0..img.size[1] {
                            for x in 0..img.size[0] {
                                t.pixels[(y0 + y) * t.size[0] + x0 + x] =
                                    img.pixels[y * img.size[0] + x];
                            }
                        }
                    }
                }
            }
        }
        last = Some(ctx.tessellate(out.shapes, out.pixels_per_point));
    }
    let bg = egui::Visuals::dark().panel_fill.to_array();
    let mut px = vec![
        [
            bg[0] as f32 / 255.0,
            bg[1] as f32 / 255.0,
            bg[2] as f32 / 255.0,
            1.0
        ];
        (w * h) as usize
    ];
    for prim in last? {
        let epaint::Primitive::Mesh(mesh) = prim.primitive else {
            continue;
        };
        let Some(tex) = textures.get(&mesh.texture_id) else {
            continue;
        };
        let clip = prim.clip_rect;
        for tri in mesh.indices.chunks(3) {
            let v = [
                &mesh.vertices[tri[0] as usize],
                &mesh.vertices[tri[1] as usize],
                &mesh.vertices[tri[2] as usize],
            ];
            let xs = v.iter().map(|v| v.pos.x);
            let ys = v.iter().map(|v| v.pos.y);
            let minx = xs
                .clone()
                .fold(f32::MAX, f32::min)
                .max(clip.left())
                .max(0.0);
            let maxx = xs.fold(f32::MIN, f32::max).min(clip.right()).min(w as f32);
            let miny = ys.clone().fold(f32::MAX, f32::min).max(clip.top()).max(0.0);
            let maxy = ys.fold(f32::MIN, f32::max).min(clip.bottom()).min(h as f32);
            let area = (v[1].pos.x - v[0].pos.x) * (v[2].pos.y - v[0].pos.y)
                - (v[2].pos.x - v[0].pos.x) * (v[1].pos.y - v[0].pos.y);
            if area.abs() < 1e-6 {
                continue;
            }
            let mut y = miny.floor();
            while y < maxy {
                let mut x = minx.floor();
                while x < maxx {
                    let (cx, cy) = (x + 0.5, y + 0.5);
                    let e = |a: &epaint::Vertex, b: &epaint::Vertex| {
                        (b.pos.x - a.pos.x) * (cy - a.pos.y) - (cx - a.pos.x) * (b.pos.y - a.pos.y)
                    };
                    let (w0, w1, w2) = (
                        e(v[1], v[2]) / area,
                        e(v[2], v[0]) / area,
                        e(v[0], v[1]) / area,
                    );
                    if w0 >= 0.0 && w1 >= 0.0 && w2 >= 0.0 {
                        let u = w0 * v[0].uv.x + w1 * v[1].uv.x + w2 * v[2].uv.x;
                        let vv = w0 * v[0].uv.y + w1 * v[1].uv.y + w2 * v[2].uv.y;
                        let tx = ((u * tex.size[0] as f32) as usize).min(tex.size[0] - 1);
                        let ty = ((vv * tex.size[1] as f32) as usize).min(tex.size[1] - 1);
                        let t = tex.pixels[ty * tex.size[0] + tx].to_array();
                        let mut src = [0f32; 4];
                        for k in 0..4 {
                            let vc = w0 * v[0].color[k] as f32
                                + w1 * v[1].color[k] as f32
                                + w2 * v[2].color[k] as f32;
                            src[k] = vc / 255.0 * t[k] as f32 / 255.0;
                        }
                        let d = &mut px[(y as u32 * w + x as u32) as usize];
                        for k in 0..4 {
                            d[k] = src[k] + d[k] * (1.0 - src[3]);
                        }
                    }
                    x += 1.0;
                }
                y += 1.0;
            }
        }
    }
    Some(
        px.iter()
            .flat_map(|p| {
                [
                    (p[0] * 255.0).round().clamp(0.0, 255.0) as u8,
                    (p[1] * 255.0).round().clamp(0.0, 255.0) as u8,
                    (p[2] * 255.0).round().clamp(0.0, 255.0) as u8,
                    255,
                ]
            })
            .collect(),
    )
}

fn save_png(name: &str, w: u32, h: u32, rgba: Vec<u8>) -> PathBuf {
    let dir = std::env::var_os("HIDEOUT_REVIEW_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("hideout-review"));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    image::RgbaImage::from_raw(w, h, rgba)
        .unwrap()
        .save(&path)
        .unwrap();
    path
}

/// One stream node on a page, visible, its open granted. Returns the host, page and gen.
fn one(node: Value) -> (PanelHost, PanelKey, StreamKey, u64) {
    let id = node["id"].as_str().unwrap().to_string();
    let reg: Register = serde_json::from_value(json!({"instance": "i1", "panels": [
        {"id": "p", "slot": "page", "title": "P", "icon": "x",
         "tree": {"type": "column", "id": "root", "children": [node]}}]}))
    .unwrap();
    let mut h = PanelHost::default();
    assert!(h.register(M, "Review", None, &reg).is_empty());
    let page = PanelKey::new(M, "p");
    h.set_visible(std::slice::from_ref(&page));
    h.take_visibility();
    h.sync_streams(Instant::now() - Duration::from_secs(5));
    let mut gen = 0;
    for c in h.streams.take_commands() {
        if let StreamCommand::Open {
            module, channel, ..
        } = c
        {
            gen = h
                .streams
                .granted(
                    &module,
                    &channel,
                    Ok(json!({"pipe": r"\\.\pipe\hideout-ch-1-0-r", "nonce": "n"})),
                )
                .unwrap()
                .1;
        }
    }
    let key = StreamKey::new(page.clone(), id);
    (h, page, key, gen)
}

fn payload(f: Frame) -> Vec<u8> {
    encode(&f)[4..].to_vec()
}

fn room_canvas() -> Value {
    let shapes: Vec<Value> = (0..6)
        .map(|i| {
            json!({"type": "key", "id": format!("r{i}"), "props":
                {"x": 8 + (i % 3) * 96, "y": 8 + (i / 3) * 76, "w": 88, "h": 68}})
        })
        .collect();
    json!({"type": "canvas", "id": "map", "props": {"width": 296, "height": 160,
           "label": "Room map", "stream": "previews", "stale_after_ms": 500},
           "children": shapes})
}

fn warm(i: usize) -> Vec<[u8; 4]> {
    (0..6)
        .map(|k| {
            if k == i {
                [255, 140, 60, 255]
            } else {
                [46, 54, 70, 255]
            }
        })
        .collect()
}

fn canvas_in(state: &str) -> PanelHost {
    let (mut h, _page, k, g) = one(room_canvas());
    let now = Instant::now();
    let frame = |h: &mut PanelHost, at: Instant| {
        h.streams.frame(
            &k,
            g,
            &payload(Frame::Canvas {
                seq: 1,
                colours: warm(1),
            }),
            at,
        );
        h.streams.present(at);
    };
    match state {
        "connecting" => {}
        "live" | "reduced" => frame(&mut h, now),
        "paused" => {
            frame(&mut h, now);
            h.streams.act(&StreamAct::Pause(k.clone()), now);
        }
        "stale" => frame(&mut h, now - Duration::from_secs(12)),
        "stopped" => {
            frame(&mut h, now - Duration::from_secs(9));
            // The provider ends it four times: three reopens, then Stopped.
            let mut gen = g;
            for _ in 0..4 {
                h.streams.ended(&k, gen, now);
                h.streams.tick(now + Duration::from_secs(5));
                for c in h.streams.take_commands() {
                    if let StreamCommand::Open {
                        module, channel, ..
                    } = c
                    {
                        let ok = json!({"pipe": r"\\.\pipe\hideout-ch-1-0-r", "nonce": "n"});
                        gen = h.streams.granted(&module, &channel, Ok(ok)).unwrap().1;
                    }
                }
            }
        }
        "restarting" => {
            frame(&mut h, now - Duration::from_secs(4));
            h.module_gone(M, "restarting");
        }
        "refused" => {
            let big = vec![0u8; 5 * 1024 * 1024];
            h.streams.frame(&k, g, &big, now);
        }
        _ => unreachable!(),
    }
    h.streams.reduced_motion = state == "reduced";
    h
}

#[test]
fn renders_the_stream_states_for_review() {
    let states = [
        ("connecting", "Connecting (after 1 s)"),
        ("live", "Live"),
        ("reduced", "Live, reduced motion"),
        ("paused", "Paused by you"),
        ("stale", "No new frames"),
        ("stopped", "Stopped updating"),
        ("restarting", "Module restarting"),
        ("refused", "Couldn't be shown"),
    ];
    let mut hosts: Vec<(PanelHost, &str)> =
        states.iter().map(|(s, c)| (canvas_in(s), *c)).collect();
    let k = StreamKey::new(PanelKey::new(M, "p"), "map");
    let now = Instant::now();
    let looks: Vec<_> = hosts
        .iter()
        .map(|(h, _)| {
            h.streams
                .look(&k, h.panel(&PanelKey::new(M, "p")).unwrap().live(), now)
        })
        .collect();
    println!("{looks:?}");
    let page = PanelKey::new(M, "p");
    let (w, h) = (1280u32, 560u32);
    let Some(rgba) = rasterise(w, h, |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("Canvas / image streams: every state (the box keeps its size)");
            egui::Grid::new("states")
                .spacing([16.0, 16.0])
                .show(ui, |ui| {
                    for (i, (host, caption)) in hosts.iter_mut().enumerate() {
                        ui.push_id(i, |ui| {
                            ui.allocate_ui(Vec2::new(296.0, 220.0), |ui| {
                                ui.set_width(296.0);
                                ui.vertical(|ui| {
                                    ui.label(*caption);
                                    panel_ui(host, ui, &page);
                                });
                            });
                        });
                        if i % 4 == 3 {
                            ui.end_row();
                        }
                    }
                });
        });
    }) else {
        eprintln!("no system font; review render skipped");
        return;
    };
    let p = save_png("built-h29-stream-states.png", w, h, rgba);
    println!("{}", p.display());
}

#[test]
fn renders_image_and_text_streams_and_a_new_frame_size_does_not_move_the_page() {
    let now = Instant::now();
    // An image stream, pixel scaling, 2:1 box; a 4:3 frame letterboxes.
    let (mut img, page, ik, ig) = one(json!({"type": "image", "id": "shot", "props":
        {"label": "Screen preview", "aspect": 2.0, "scaling": "pixel", "width": 296,
         "stream": "screen"}}));
    let checker = |w: u16, h: u16| -> Vec<[u8; 4]> {
        (0..h)
            .flat_map(|y| {
                (0..w).map(move |x| {
                    if (x / 4 + y / 4) % 2 == 0 {
                        [240, 240, 240, 255]
                    } else {
                        [30, 30, 30, 255]
                    }
                })
            })
            .collect()
    };
    let frame = |img: &mut PanelHost, w: u16, h: u16, at: Instant| {
        img.streams.frame(
            &ik,
            ig,
            &payload(Frame::Image {
                seq: 0,
                width: w,
                height: h,
                rgba: checker(w, h),
            }),
            at,
        );
        img.streams.present(at);
    };
    frame(&mut img, 64, 32, now);
    // Text: trimmed, mono, appended after the node's own text.
    let (mut txt, _, tk, tg) = one(json!({"type": "text", "id": "log", "props":
        {"mono": true, "stream": "log", "text": "started\n"}}));
    txt.streams = hideout_panels::Streams::with_text_cap(120);
    txt.sync_streams(now);
    for c in txt.streams.take_commands() {
        if let StreamCommand::Open {
            module, channel, ..
        } = c
        {
            let r = txt
                .streams
                .granted(
                    &module,
                    &channel,
                    Ok(json!({"pipe": r"\\.\pipe\hideout-ch-1-0-r", "nonce": "n"})),
                )
                .unwrap();
            for i in 0..12 {
                txt.streams.frame(
                    &tk,
                    r.1,
                    &payload(Frame::Text {
                        seq: 0,
                        append: format!("12:00:{i:02} light moved to room {}\n", i % 6),
                    }),
                    now + Duration::from_millis(20 * i),
                );
            }
        }
    }
    let _ = tg;
    let (w, h) = (900u32, 420u32);
    let mut rects = Vec::new();
    let draw = |img: &mut PanelHost, txt: &mut PanelHost, rects: &mut Vec<Rect>| {
        rasterise(w, h, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.heading("Image stream (pixel, letterboxed) and text stream (trimmed, mono)");
                ui.horizontal_top(|ui| {
                    ui.allocate_ui(Vec2::new(300.0, 360.0), |ui| {
                        ui.set_width(300.0);
                        ui.vertical(|ui| {
                            let r = panel_ui(img, ui, &page);
                            rects.push(r.rects["shot"]);
                        });
                    });
                    ui.allocate_ui(Vec2::new(520.0, 360.0), |ui| {
                        ui.set_width(520.0);
                        panel_ui(txt, ui, &page);
                    });
                });
            });
        })
    };
    let Some(rgba) = draw(&mut img, &mut txt, &mut rects) else {
        eprintln!("no system font; review render skipped");
        return;
    };
    let p = save_png("built-h29-image-text.png", w, h, rgba);
    println!("{}", p.display());
    let before = *rects.last().unwrap();
    frame(&mut img, 40, 40, now + Duration::from_secs(1));
    rects.clear();
    let rgba = draw(&mut img, &mut txt, &mut rects).unwrap();
    save_png("built-h29-image-new-size.png", w, h, rgba);
    assert_eq!(
        *rects.last().unwrap(),
        before,
        "a new frame size moved the page"
    );
    assert_eq!(img.panel(&page).unwrap().state, PanelState::Live);
}

#[test]
fn renders_the_sample_room_map_page_for_review() {
    let reg = hideout_room_map::panels("i1");
    let mut h = PanelHost::default();
    assert!(h
        .register(hideout_room_map::ID, "Room map", None, &reg)
        .is_empty());
    let page = PanelKey::new(hideout_room_map::ID, "map");
    h.set_visible(std::slice::from_ref(&page));
    h.take_visibility();
    let now = Instant::now();
    h.sync_streams(now);
    let k = StreamKey::new(page.clone(), "rooms");
    for c in h.streams.take_commands() {
        if let StreamCommand::Open {
            module, channel, ..
        } = c
        {
            let ok = json!({"pipe": r"\\.\pipe\hideout-ch-1-0-r", "nonce": "n"});
            let g = h.streams.granted(&module, &channel, Ok(ok)).unwrap().1;
            // A second of the sample's frames at 25 a second, drawn under the 30 cap.
            for i in 0..25u32 {
                let t = now + Duration::from_millis(40 * u64::from(i));
                let colours = hideout_room_map::colours(2.6 + i as f32 / 25.0);
                h.streams
                    .frame(&k, g, &payload(Frame::Canvas { seq: i, colours }), t);
                h.streams.present(t);
            }
        }
    }
    assert_eq!(h.streams.get(&k).unwrap().drawn, 25);
    let (w, hh) = (720u32, 420u32);
    let Some(rgba) = rasterise(w, hh, |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("Room map (example.room-map), live");
            panel_ui(&mut h, ui, &page);
        });
    }) else {
        eprintln!("no system font; review render skipped");
        return;
    };
    let p = save_png("built-h29-sample-page.png", w, hh, rgba);
    println!("{}", p.display());
}
