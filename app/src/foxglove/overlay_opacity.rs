//! The layer overlay's two opacities: the HUD (key faces, strips, borders) and the key labels.
//! Both run 0–100%. The slider has a detent at 20%: dragging near it rests there (hold Alt to
//! pass it), the arrow keys step 1%. The key labels follow the HUD until their own slider is
//! touched; "Follow HUD" links them again. The logic is plain functions, tested without a window.

/// Where the slider rests, and how close (in percent) a drag has to come to rest there.
pub const DETENT: u8 = 20;
pub const SNAP: u8 = 3;

/// A dragged value, pulled onto the detent when it's within [`SNAP`] of it, unless `free` (Alt).
pub fn snap(value: u8, free: bool) -> u8 {
    if !free && value.abs_diff(DETENT) <= SNAP {
        DETENT
    } else {
        value
    }
}

/// The key labels' opacity: their own, or the HUD's while they follow it (`None`).
pub fn label(hud: u8, own: Option<u8>) -> u8 {
    own.unwrap_or(hud).min(100)
}

/// The labels' setting after their slider showed `shown` and the user left it at `now`:
/// touching it unlinks them at that value; otherwise it stays as it was.
pub fn touched(own: Option<u8>, shown: u8, now: u8) -> Option<u8> {
    if now != shown {
        Some(now)
    } else {
        own
    }
}

/// How the overlay is drawn for opacities `hud` and `label` (percent): the factor for the whole
/// overlay, then the faces' and the labels' share of it, so the brighter one is drawn at its
/// own value and the other relative to it.
pub fn factors(hud: u8, label: u8) -> (f32, f32, f32) {
    let hud = f32::from(hud.min(100)) / 100.0;
    let label = f32::from(label.min(100)) / 100.0;
    let whole = hud.max(label);
    if whole <= 0.0 {
        return (0.0, 1.0, 1.0);
    }
    (whole, hud / whole, label / whole)
}

/// The two rows of Settings → Layer overlay (a grid): "Opacity" for the HUD and "Key labels",
/// which follows it until moved, with a "Follow HUD" box to link it again.
pub fn rows(ui: &mut egui::Ui, settings: &mut crate::settings::Settings) {
    ui.label("Opacity");
    slider(
        ui,
        &mut settings.opacity,
        "How see-through the layer overlay is",
    );
    ui.end_row();

    ui.label("Key labels");
    ui.horizontal(|ui| {
        let shown = label(settings.opacity, settings.label_opacity);
        let mut now = shown;
        slider(ui, &mut now, "How see-through the key labels are");
        settings.label_opacity = touched(settings.label_opacity, shown, now);
        let mut follow = settings.label_opacity.is_none();
        if ui
            .checkbox(&mut follow, "Follow HUD")
            .on_hover_text("The labels move with Opacity; moving their slider unlinks them")
            .changed()
        {
            settings.label_opacity = linked(follow, now);
        }
    });
    ui.end_row();
}

/// The labels' setting after "Follow HUD" was set to `follow`, their slider showing `shown`.
pub fn linked(follow: bool, shown: u8) -> Option<u8> {
    (!follow).then_some(shown)
}

/// The faint outline that marks a hidden (0%) overlay while it's dragged.
pub fn paint_outline(ui: &egui::Ui, rect: egui::Rect) {
    let colour = egui::Color32::from_white_alpha(70);
    ui.painter().rect_stroke(
        rect.shrink(1.0),
        6.0,
        egui::Stroke::new(1.5, colour),
        egui::StrokeKind::Inside,
    );
}

/// An opacity slider (percent) with a tick at the detent, a value box beside it and a hover
/// text. Returns true when the value changed.
pub fn slider(ui: &mut egui::Ui, value: &mut u8, hover: &str) -> bool {
    let before = *value;
    ui.horizontal(|ui| {
        let response = ui
            .add(
                egui::Slider::new(value, 0..=100)
                    .step_by(1.0)
                    .show_value(false),
            )
            .on_hover_text(format!(
                "{hover}. It rests at {DETENT}% when dragged near it; hold Alt to pass it"
            ));
        // A press focuses it, so the arrow keys fine-tune from there.
        if response.drag_started() {
            response.request_focus();
        }
        let dragging = response.dragged() || response.drag_stopped() || response.clicked();
        if dragging && *value != before {
            *value = snap(*value, ui.input(|i| i.modifiers.alt));
        }
        paint_detent(ui, &response);
        ui.add(egui::DragValue::new(value).range(0..=100).suffix("%"));
    });
    *value != before
}

/// A small nub under the slider's rail at the detent.
fn paint_detent(ui: &egui::Ui, response: &egui::Response) {
    let rect = response.rect;
    let radius = rect.height() / 2.5;
    let radius = match ui.style().visuals.handle_shape {
        egui::style::HandleShape::Circle => radius,
        egui::style::HandleShape::Rect { aspect_ratio } => radius * aspect_ratio,
    };
    let x = detent_x(rect.x_range(), radius);
    let colour = ui.visuals().weak_text_color();
    let y = rect.center().y + rect.height() * 0.22;
    ui.painter().line_segment(
        [egui::pos2(x, y), egui::pos2(x, rect.bottom())],
        egui::Stroke::new(2.0, colour),
    );
}

/// Where the detent sits on a slider spanning `x` with a handle of `radius` (as egui lays it out).
fn detent_x(x: egui::Rangef, radius: f32) -> f32 {
    let range = x.shrink(radius);
    range.min + range.span() * f32::from(DETENT) / 100.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_drag_near_twenty_rests_on_it() {
        for v in 17..=23 {
            assert_eq!(snap(v, false), 20, "{v}");
        }
        assert_eq!(snap(16, false), 16);
        assert_eq!(snap(24, false), 24);
        assert_eq!(snap(0, false), 0, "zero is reachable");
        assert_eq!(snap(100, false), 100);
    }

    #[test]
    fn alt_passes_the_detent() {
        assert_eq!(snap(19, true), 19);
        assert_eq!(snap(21, true), 21);
    }

    #[test]
    fn labels_follow_the_hud_until_touched() {
        assert_eq!(label(60, None), 60);
        assert_eq!(label(60, Some(90)), 90);
        // Their slider showed the HUD's 60; left there: still following.
        assert_eq!(touched(None, 60, 60), None);
        // Moved: unlinked at the new value.
        assert_eq!(touched(None, 60, 75), Some(75));
        // Already their own: kept, or moved.
        assert_eq!(touched(Some(40), 40, 40), Some(40));
        assert_eq!(touched(Some(40), 40, 0), Some(0));
    }

    #[test]
    fn follow_hud_links_and_unlinks() {
        assert_eq!(linked(true, 75), None, "linked: follows again");
        assert_eq!(linked(false, 60), Some(60), "unlinked where it showed");
    }

    #[test]
    fn the_brighter_part_sets_the_whole() {
        assert_eq!(factors(100, 100), (1.0, 1.0, 1.0));
        assert_eq!(factors(50, 50), (0.5, 1.0, 1.0));
        // Faint faces, clear labels.
        let (whole, face, text) = factors(20, 80);
        assert!((whole - 0.8).abs() < 1e-6 && (face - 0.25).abs() < 1e-6 && text == 1.0);
        // Clear faces, faint labels.
        let (whole, face, text) = factors(80, 20);
        assert!((whole - 0.8).abs() < 1e-6 && face == 1.0 && (text - 0.25).abs() < 1e-6);
        // Both off: hidden, without dividing by zero.
        assert_eq!(factors(0, 0), (0.0, 1.0, 1.0));
        assert_eq!(factors(0, 50).1, 0.0, "faces off, labels on");
    }

    #[test]
    fn the_nub_sits_a_fifth_along_the_rail() {
        let x = detent_x(egui::Rangef::new(100.0, 300.0), 10.0);
        assert!((x - (110.0 + 180.0 * 0.2)).abs() < 1e-4);
    }
}
