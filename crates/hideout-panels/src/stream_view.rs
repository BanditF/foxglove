// SPDX-License-Identifier: MIT
//! Draws stream nodes (§5.4; UX: docs/hideout-streams-ux.md): the frame, and the hub's own
//! state marks around it. The box is laid out before the first frame and never moves when a
//! frame arrives or changes size; a frozen frame is dimmed only with words next to it.
//! Frames are never a live region: the node's accessible description carries its state.

use crate::model::PanelKey;
use crate::streams::{
    accessible_description, look_words, Look, Picture, StreamAct, StreamKey, Streams, TRIMMED_LINE,
};
use egui::{
    Color32, CornerRadius, FontId, Pos2, Rect, RichText, Sense, TextureOptions, Ui, Vec2,
    WidgetInfo, WidgetType,
};
use hideout_contract::panel::Node;
use serde_json::{Map, Value};
use std::time::Instant;

/// Everything a stream node needs from the panel draw.
pub(crate) struct StreamDraw<'a> {
    pub key: &'a PanelKey,
    pub streams: &'a Streams,
    pub acts: &'a mut Vec<StreamAct>,
    /// The module's state is `running`.
    pub running: bool,
    pub now: Instant,
}

impl StreamDraw<'_> {
    fn look(&self, k: &StreamKey) -> Look {
        self.streams.look(k, self.running, self.now)
    }
}

fn num(p: &Map<String, Value>, k: &str) -> Option<f32> {
    p.get(k)
        .and_then(Value::as_f64)
        .filter(|v| *v > 0.0)
        .map(|v| v as f32)
}

/// The colour of a frozen frame's veil: the surface at about 60% over the frame (UX: the
/// stale view's greyed body, the frame at about 40%).
fn veil(ui: &Ui) -> Color32 {
    let bg = ui.visuals().panel_fill;
    Color32::from_rgba_unmultiplied(bg.r(), bg.g(), bg.b(), 153)
}

fn dims(look: Look) -> bool {
    matches!(look, Look::LastSeen(_) | Look::Stopped)
}

/// A small chip with `text` at `at` (top-left of the box).
fn chip(ui: &Ui, at: Pos2, text: &str) -> Rect {
    let font = FontId::proportional(12.0);
    let fg = ui.visuals().text_color();
    let galley = ui.painter().layout_no_wrap(text.to_string(), font, fg);
    let r = Rect::from_min_size(at, galley.size() + Vec2::new(12.0, 6.0));
    ui.painter()
        .rect_filled(r, CornerRadius::same(6), ui.visuals().extreme_bg_color);
    ui.painter().galley(r.min + Vec2::new(6.0, 3.0), galley, fg);
    r
}

/// The hub's marks over a canvas or image box: dimming, the state chip, the Live dot (reduced
/// motion only), Pause/Play, Try again, Open log.
fn overlay(ui: &mut Ui, rect: Rect, label: &str, k: &StreamKey, d: &mut StreamDraw) -> Look {
    let look = d.look(k);
    if dims(look) {
        ui.painter().rect_filled(rect, 0.0, veil(ui));
    }
    let pad = Vec2::splat(8.0);
    if let Some(words) = look_words(look) {
        if look == Look::Refused {
            let c = rect.center();
            ui.painter().text(
                c - Vec2::new(0.0, 12.0),
                egui::Align2::CENTER_CENTER,
                format!("\u{26A0} {words}"),
                FontId::proportional(14.0),
                ui.visuals().warn_fg_color,
            );
            let b = Rect::from_center_size(c + Vec2::new(0.0, 16.0), Vec2::new(88.0, 24.0));
            if ui.put(b, egui::Button::new("Open log")).clicked() {
                d.acts.push(StreamAct::OpenLog(k.clone()));
            }
        } else if look == Look::Connecting {
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                words,
                FontId::proportional(13.0),
                ui.visuals().weak_text_color(),
            );
        } else {
            chip(ui, rect.min + pad, &words);
        }
    }
    if look == Look::Live && d.streams.reduced_motion {
        // At 2 frames a second motion no longer says "live": a dot and the word.
        let r = chip(
            ui,
            Pos2::new(rect.right() - 56.0, rect.top() + 8.0),
            "  Live",
        );
        ui.painter().circle_filled(
            r.left_center() + Vec2::new(8.0, 0.0),
            3.5,
            Color32::from_rgb(0xFF, 0x7A, 0x7A),
        );
    }
    let corner = |w: f32| {
        Rect::from_min_size(
            rect.right_bottom() - Vec2::new(w, 28.0) - pad,
            Vec2::new(w, 28.0),
        )
    };
    match look {
        Look::Stopped => {
            if ui
                .put(corner(88.0), egui::Button::new("Try again"))
                .clicked()
            {
                d.acts.push(StreamAct::TryAgain(k.clone()));
            }
        }
        Look::Paused => {
            let b = ui
                .put(corner(56.0), egui::Button::new("Play"))
                .on_hover_text(format!("Play {label}"));
            if b.clicked() {
                d.acts.push(StreamAct::Play(k.clone()));
            }
        }
        Look::Live if ui.rect_contains_pointer(rect) => {
            let b = ui
                .put(corner(64.0), egui::Button::new("Pause"))
                .on_hover_text(format!("Pause {label}"));
            if b.clicked() {
                d.acts.push(StreamAct::Pause(k.clone()));
            }
        }
        _ => {}
    }
    look
}

/// The canvas's scale: uniform to the node's width, the height capped at 60% of the visible
/// height, never below 50% (the page scrolls sideways instead).
pub(crate) fn canvas_scale(ui: &Ui, w: f32, h: f32) -> f32 {
    let avail = ui.available_width().max(1.0);
    let max_h = ui.ctx().content_rect().height() * 0.6;
    (avail / w).min(max_h / h).max(0.5)
}

/// The colour of shape `i` from the stream's last frame, if any.
pub(crate) fn shape_colour(streams: &Streams, k: &StreamKey, i: usize) -> Option<Color32> {
    match streams.get(k)?.picture.as_ref()? {
        Picture::Canvas(c) => c
            .get(i)
            .map(|[r, g, b, a]| Color32::from_rgba_unmultiplied(*r, *g, *b, *a)),
        Picture::Image { .. } => None,
    }
}

/// Marks over a streamed canvas the shapes are already drawn in, and its accessible name.
pub(crate) fn canvas_marks(
    ui: &mut Ui,
    rect: Rect,
    n: &Node,
    p: &Map<String, Value>,
    resp: &egui::Response,
    d: &mut StreamDraw,
) {
    let label = p.get("label").and_then(Value::as_str).unwrap_or("");
    let k = StreamKey::new(d.key.clone(), &n.id);
    let look = overlay(ui, rect, label, &k, d);
    let desc = accessible_description(label, look);
    resp.widget_info(|| WidgetInfo::labeled(WidgetType::Other, true, &desc));
}

/// A streamed image: a box from `aspect` (or the first frame's, once), frames fitted inside
/// (contain), `pixel` scaling in whole steps without blur, `smooth` filtered.
pub(crate) fn image(ui: &mut Ui, n: &Node, p: &Map<String, Value>, d: &mut StreamDraw) {
    let k = StreamKey::new(d.key.clone(), &n.id);
    let s = d.streams.get(&k);
    let label = p.get("label").and_then(Value::as_str).unwrap_or("");
    let aspect = num(p, "aspect")
        .or_else(|| s.and_then(|s| s.first_aspect))
        .unwrap_or(16.0 / 9.0);
    let w = num(p, "width")
        .unwrap_or(f32::INFINITY)
        .min(ui.available_width());
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, w / aspect), Sense::hover());
    ui.painter()
        .rect_filled(rect, 0.0, ui.visuals().extreme_bg_color);
    let pixel = p.get("scaling").and_then(Value::as_str) == Some("pixel");
    if let Some((
        seq,
        Picture::Image {
            width,
            height,
            rgba,
        },
    )) = s.and_then(|s| s.picture.as_ref().map(|p| (s.picture_seq, p)))
    {
        let id = egui::Id::new(("hideout.stream", &k.panel.module, &k.panel.panel, &k.node));
        let cached: Option<(u64, egui::TextureHandle)> = ui.data(|m| m.get_temp(id));
        let tex = match cached {
            Some((s, t)) if s == seq => t,
            _ => {
                let img = egui::ColorImage::from_rgba_unmultiplied(
                    [*width as usize, *height as usize],
                    rgba.as_flattened(),
                );
                let opts = if pixel {
                    TextureOptions::NEAREST
                } else {
                    TextureOptions::LINEAR
                };
                let t = ui
                    .ctx()
                    .load_texture(format!("stream-{}", k.node), img, opts);
                ui.data_mut(|m| m.insert_temp(id, (seq, t.clone())));
                t
            }
        };
        let (fw, fh) = (f32::from(*width).max(1.0), f32::from(*height).max(1.0));
        let mut scale = (rect.width() / fw).min(rect.height() / fh);
        if pixel && scale >= 1.0 {
            scale = scale.floor();
        }
        let r = Rect::from_center_size(rect.center(), Vec2::new(fw * scale, fh * scale));
        ui.painter().image(
            tex.id(),
            r,
            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
            Color32::WHITE,
        );
    }
    let look = overlay(ui, rect, label, &k, d);
    let desc = accessible_description(label, look);
    resp.widget_info(|| WidgetInfo::labeled(WidgetType::Image, true, &desc));
}

/// A streamed `text` or `markdown` node: the node's text plus what the stream appended, follow
/// at the bottom, Copy, and "Earlier text was trimmed." after a trim. Never announced per frame.
pub(crate) fn text(ui: &mut Ui, n: &Node, p: &Map<String, Value>, d: &mut StreamDraw) {
    let k = StreamKey::new(d.key.clone(), &n.id);
    let s = d.streams.get(&k);
    let mut all = p
        .get("text")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    if let Some(s) = s {
        all.push_str(&s.text);
    }
    let mono = p.get("mono").and_then(Value::as_bool) == Some(true);
    let look = d.look(&k);
    let muted = matches!(look, Look::LastSeen(_) | Look::Stopped);
    let trimmed = s.is_some_and(|s| s.trimmed);
    let frame = egui::Frame::group(ui.style()).inner_margin(8.0);
    frame.show(ui, |ui| {
        ui.horizontal(|ui| {
            if let Some(words) = look_words(look) {
                ui.label(RichText::new(words).small().weak());
            }
            if look == Look::Stopped && ui.small_button("Try again").clicked() {
                d.acts.push(StreamAct::TryAgain(k.clone()));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let copied_id = egui::Id::new(("hideout.stream.copied", &k.node));
                let until: Option<f64> = ui.data(|m| m.get_temp(copied_id));
                let t = ui.input(|i| i.time);
                let word = if until.is_some_and(|u| t < u) {
                    "Copied"
                } else {
                    "Copy"
                };
                if ui.small_button(word).clicked() {
                    ui.ctx().copy_text(all.clone());
                    ui.data_mut(|m| m.insert_temp(copied_id, t + 2.0));
                }
            });
        });
        egui::ScrollArea::vertical()
            .id_salt((
                "hideout.stream.text",
                &k.panel.module,
                &k.panel.panel,
                &k.node,
            ))
            .max_height(240.0)
            .stick_to_bottom(true)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                if trimmed {
                    ui.label(RichText::new(TRIMMED_LINE).italics().weak());
                }
                let mut t = RichText::new(&all);
                if mono {
                    t = t.monospace();
                }
                if muted {
                    t = t.color(ui.visuals().weak_text_color());
                }
                ui.add(egui::Label::new(t).wrap());
            });
    });
}
