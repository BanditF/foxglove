//! The layer overlay's two opacities: the HUD (key faces, strips, borders) and the key labels.
//! Both run 0–100%. The slider runs the row's width and has a detent at 20%, marked by a tick:
//! dragging within 4% of it snaps there (hold Alt to pass it), the arrow keys step 1%. The key
//! labels follow the HUD until their own slider is touched; "Follow HUD" links them again.
//!
//! While a slider is dragged it holds the overlay on screen as a live preview ([`previewing`]).
//! The hold starts with the press and ends with the drag's end or Settings closing (minimizing
//! closes it too): events, no timer. At 0% the shown overlay is a faint outline ([`outline`]).
//! The logic is plain functions, tested without a window.

use std::cell::Cell;

/// Where the slider rests, and how close (in percent) a drag has to come to rest there.
pub const DETENT: u8 = 20;
pub const SNAP: u8 = 4;

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
        |_| {},
    );
    ui.end_row();

    ui.label("Key labels");
    let shown = label(settings.opacity, settings.label_opacity);
    let mut now = shown;
    let mut follow = settings.label_opacity.is_none();
    let mut follow_changed = false;
    slider(ui, &mut now, "How see-through the key labels are", |ui| {
        follow_changed = ui
            .checkbox(&mut follow, "Follow HUD")
            .on_hover_text("The labels move with Opacity; moving their slider unlinks them")
            .changed();
    });
    settings.label_opacity = touched(settings.label_opacity, shown, now);
    if follow_changed {
        settings.label_opacity = linked(follow, now);
    }
    ui.end_row();
}

/// Whether the overlay at opacities `hud` and `label` can't be seen, so it shows as a faint
/// outline instead: while it's dragged, previewed, or shown when a layer comes on.
pub fn outline(hud: u8, label: u8) -> bool {
    factors(hud, label).0 <= 0.0
}

thread_local! {
    /// The slider holding the overlay on screen as a preview, by its id. Settings and the
    /// overlay are drawn on the UI thread (Settings is an immediate viewport).
    static PREVIEW: Cell<Option<egui::Id>> = const { Cell::new(None) };
}

/// The overlay is held on screen by an opacity slider being dragged.
pub fn previewing() -> bool {
    PREVIEW.get().is_some()
}

/// The slider `id` is (or isn't) being dragged this frame: a press starts the hold, the drag's
/// end releases it. Returns whether the hold changed.
fn hold(id: egui::Id, dragging: bool) -> bool {
    match (dragging, PREVIEW.get()) {
        (true, None) => {
            PREVIEW.set(Some(id));
            true
        }
        (false, Some(h)) if h == id => {
            PREVIEW.set(None);
            true
        }
        _ => false,
    }
}

/// Settings closed (or was minimized, which closes it) or lost the focus: no slider is dragged
/// any more.
pub fn release_preview() {
    PREVIEW.set(None);
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

/// An opacity slider (percent) across the row, with a tick at the detent, a value box at the
/// right, `trailing` right of that, and a hover text. Returns true when the value changed.
pub fn slider(
    ui: &mut egui::Ui,
    value: &mut u8,
    hover: &str,
    trailing: impl FnOnce(&mut egui::Ui),
) -> bool {
    let before = *value;
    // The value box is laid out first (at the right), so it edits a copy.
    let mut typed = *value;
    super::settings_ui::split_row(
        ui,
        |ui| {
            trailing(ui);
            ui.add(egui::DragValue::new(&mut typed).range(0..=100).suffix("%"));
        },
        |ui| {
            ui.spacing_mut().slider_width = ui.available_width().max(MIN_RAIL);
            let response = ui
                .add(
                    egui::Slider::new(value, 0..=100)
                        .step_by(1.0)
                        .show_value(false),
                )
                .on_hover_text(format!(
                    "{hover}. It snaps to {DETENT}% when dragged near it; hold Alt to pass it"
                ));
            #[cfg(test)]
            RAIL.set(Some(response.rect));
            // A press focuses it, so the arrow keys fine-tune from there.
            if response.drag_started() {
                response.request_focus();
            }
            // Pointer-driven changes snap; the arrow keys step through the detent.
            let pointer = response.is_pointer_button_down_on()
                || response.dragged()
                || response.drag_stopped()
                || response.clicked();
            if pointer && *value != before {
                *value = snap(*value, ui.input(|i| i.modifiers.alt));
            }
            let dragging = response.is_pointer_button_down_on() || response.dragged();
            if hold(response.id, dragging) || (dragging && *value != before) {
                // The overlay lives in the main viewport: draw it now, at the new value.
                ui.ctx().request_repaint_of(egui::ViewportId::ROOT);
            }
            paint_detent(ui, &response);
        },
    );
    if typed != before {
        *value = typed;
    }
    *value != before
}

/// The narrowest the rail gets, in points.
const MIN_RAIL: f32 = 120.0;

#[cfg(test)]
thread_local! {
    /// The last slider's rail, for the tests' pointer.
    pub static RAIL: Cell<Option<egui::Rect>> = const { Cell::new(None) };
}

/// A tick across the slider's rail at the detent, above and below it.
fn paint_detent(ui: &egui::Ui, response: &egui::Response) {
    let rect = response.rect;
    let radius = rect.height() / 2.5;
    let radius = match ui.style().visuals.handle_shape {
        egui::style::HandleShape::Circle => radius,
        egui::style::HandleShape::Rect { aspect_ratio } => radius * aspect_ratio,
    };
    let x = detent_x(rect.x_range(), radius);
    let colour = ui.visuals().text_color().gamma_multiply(0.6);
    let half = rect.height() * 0.42;
    ui.painter().line_segment(
        [
            egui::pos2(x, rect.center().y - half),
            egui::pos2(x, rect.center().y + half),
        ],
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
        for v in 16..=24 {
            assert_eq!(snap(v, false), 20, "{v}");
        }
        assert_eq!(snap(15, false), 15);
        assert_eq!(snap(25, false), 25);
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
    fn zero_shows_as_an_outline() {
        assert!(outline(0, 0));
        assert!(!outline(0, 30), "the labels still show");
        assert!(!outline(20, 0));
    }

    #[test]
    fn a_drag_holds_the_preview_until_it_ends() {
        let (a, b) = (egui::Id::new("hud"), egui::Id::new("labels"));
        release_preview();
        assert!(!previewing());
        assert!(hold(a, true), "the press starts it");
        assert!(previewing());
        assert!(!hold(a, true), "held while dragged");
        assert!(!hold(b, false), "another slider, not dragged, leaves it");
        assert!(previewing());
        assert!(hold(a, false), "the drag's end releases it");
        assert!(!previewing());
        // Settings closes (or is minimized) mid-drag: released with no drag end.
        hold(b, true);
        release_preview();
        assert!(!previewing());
    }

    #[test]
    fn the_nub_sits_a_fifth_along_the_rail() {
        let x = detent_x(egui::Rangef::new(100.0, 300.0), 10.0);
        assert!((x - (110.0 + 180.0 * 0.2)).abs() < 1e-4);
    }
}
