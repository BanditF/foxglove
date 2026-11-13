//! A panic boundary around what a page or a tray tile draws (#1143, H2.4).
//!
//! [`page`] and [`tile`] run one draw behind `catch_unwind`. If it panics, the page (or tile) is
//! `Disabled` with the panic's message: it draws a notice instead ("This page stopped working.
//! The keyboard and lights are unaffected.") and is not drawn again, frame after frame, until
//! the user clicks "Try again". Only the draw is guarded: the link, the leases and the services
//! run on their own threads and never see it. The panic hook has already logged the panic with
//! its place (`app_log`); this adds one line saying which page it stopped.
//!
//! Disabled pages are listed in the diagnostics report ([`problems`]) and in the health rows
//! (`ui.pages`, so `foxglove-lab health` shows them).
//!
//! Lab builds only: with the `lab-hooks` cargo feature (never on in a release), the env var
//! `FOXGLOVE_LAB_PANIC=<key>[,<key>]` makes the draw with that key panic, to prove all of this
//! on the real app. Keys are `page:<Label>` (`page:Keyboard`) and `tile:<name>` (`tile:screens`).

use std::collections::BTreeMap;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::Mutex;

use egui::RichText;

use super::health::{HealthRow, Level};
use super::theme;

/// What the page stopped with.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Disabled {
    title: String,
    reason: String,
}

static DISABLED: Mutex<BTreeMap<String, Disabled>> = Mutex::new(BTreeMap::new());

fn disabled() -> std::sync::MutexGuard<'static, BTreeMap<String, Disabled>> {
    DISABLED.lock().unwrap_or_else(|e| e.into_inner())
}

/// Draws a page's body in `ui` behind a panic boundary.
pub fn page(ui: &mut egui::Ui, key: &str, title: &str, body: impl FnOnce(&mut egui::Ui)) {
    run(ui, key, title, false, body);
}

/// Draws a tray panel tile behind a panic boundary; a failed tile shows a short notice and the
/// other tiles go on.
pub fn tile(ui: &mut egui::Ui, key: &str, title: &str, body: impl FnOnce(&mut egui::Ui)) {
    run(ui, key, title, true, body);
}

fn run(ui: &mut egui::Ui, key: &str, title: &str, compact: bool, body: impl FnOnce(&mut egui::Ui)) {
    let known = disabled().get(key).cloned();
    if let Some(d) = known {
        notice(ui, key, &d, compact);
        return;
    }
    // `ui.scope`: whatever the draw leaves half-built (a layout, a width) is the child's, not
    // the caller's. egui's locks don't poison, and the child `Ui`s drop as the panic unwinds.
    let drawn = catch_unwind(AssertUnwindSafe(|| {
        lab_trigger(key);
        ui.scope(body);
    }));
    if let Err(payload) = drawn {
        let d = Disabled {
            title: title.to_string(),
            reason: super::services::panic_message(&*payload),
        };
        eprintln!("Foxglove: {} stopped drawing: {}", d.title, d.reason);
        super::health::register("page_guard", health_rows);
        disabled().insert(key.to_string(), d.clone());
        super::hub_panels::guard_changed(); // the Hub's panels say so (P8)
        ui.ctx().request_repaint();
        notice(ui, key, &d, compact);
    }
}

/// Whether the page or tile with `key` is stopped (it shows the notice, not its content).
pub fn stopped(key: &str) -> bool {
    disabled().contains_key(key)
}

/// "Try again": draws it on the next frame.
fn retry(ctx: &egui::Context, key: &str) {
    disabled().remove(key);
    super::hub_panels::guard_changed();
    eprintln!("Foxglove: trying {key} again");
    ctx.request_repaint();
}

fn notice(ui: &mut egui::Ui, key: &str, d: &Disabled, compact: bool) {
    let p = theme::palette(ui);
    let text = if compact {
        "This part stopped working. The keyboard and lights are unaffected."
    } else {
        "This page stopped working. The keyboard and lights are unaffected."
    };
    ui.push_id(("page-guard", key), |ui| {
        theme::card(ui)
            .stroke(egui::Stroke::new(1.0, p.warn))
            .inner_margin(egui::Margin::symmetric(14, if compact { 8 } else { 12 }))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                let said =
                    ui.add(egui::Label::new(RichText::new(text).color(p.text_strong)).wrap());
                said.widget_info(|| {
                    egui::WidgetInfo::labeled(
                        egui::WidgetType::Label,
                        true,
                        format!("{}: {text}", d.title),
                    )
                });
                ui.add_space(4.0);
                ui.add(egui::Label::new(RichText::new(&d.reason).small().color(p.muted)).wrap());
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if theme::pill(ui, true, "Try again").clicked() {
                        retry(ui.ctx(), key);
                    }
                    // The existing report; it lists this page (see `problems`).
                    if ui
                        .button("Copy details")
                        .on_hover_text("Copies the diagnostics report for a bug report")
                        .clicked()
                    {
                        super::diagnostics::copy(ui.ctx());
                    }
                });
            });
    });
}

/// One line per disabled page or tile, for the diagnostics report.
pub fn problems() -> Vec<String> {
    disabled()
        .values()
        .map(|d| {
            format!(
                "{}: stopped drawing after a panic ({}); the rest of the app runs",
                d.title, d.reason
            )
        })
        .collect()
}

/// The health rows (`health.read`, `foxglove-lab health`): a WARN per disabled page or tile.
fn health_rows() -> Vec<HealthRow> {
    disabled()
        .values()
        .map(|d| {
            HealthRow::new(
                "ui.pages",
                d.title.clone(),
                Level::Warn,
                format!(
                    "stopped drawing after a panic ({}); Try again on the page",
                    d.reason
                ),
            )
        })
        .collect()
}

#[cfg(feature = "lab-hooks")]
fn lab_trigger(key: &str) {
    use std::sync::OnceLock;
    static KEYS: OnceLock<Vec<String>> = OnceLock::new();
    let keys = KEYS.get_or_init(|| {
        std::env::var("FOXGLOVE_LAB_PANIC")
            .map(|v| v.split(',').map(|k| k.trim().to_string()).collect())
            .unwrap_or_default()
    });
    if keys.iter().any(|k| k == key) {
        panic!("lab panic requested for {key}");
    }
}

#[cfg(not(feature = "lab-hooks"))]
fn lab_trigger(_key: &str) {}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Pos2, Rect};
    use std::cell::Cell;

    fn frame(ctx: &egui::Context, draw: impl FnOnce(&mut egui::Ui)) {
        let raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(600.0, 400.0))),
            ..Default::default()
        };
        let mut draw = Some(draw);
        let mut out = ctx.run_ui(raw, |ui| (draw.take().unwrap())(ui));
        out.textures_delta.clear();
    }

    fn is_disabled(key: &str) -> bool {
        disabled().contains_key(key)
    }

    #[test]
    fn a_panicking_page_is_disabled_and_the_others_draw() {
        let ctx = egui::Context::default();
        let other = Cell::new(0);
        let boom = Cell::new(0);
        for _ in 0..3 {
            frame(&ctx, |ui| {
                page(ui, "test:boom-a", "Boom A", |_| {
                    boom.set(boom.get() + 1);
                    panic!("draw failed");
                });
                page(ui, "test:fine-a", "Fine A", |ui| {
                    other.set(other.get() + 1);
                    ui.label("fine");
                });
            });
        }
        assert!(is_disabled("test:boom-a"));
        assert!(!is_disabled("test:fine-a"));
        // Drawn once, never again while disabled; the other page drew every frame.
        assert_eq!((boom.get(), other.get()), (1, 3));
        assert!(problems()
            .iter()
            .any(|l| l.starts_with("Boom A") && l.contains("draw failed")));
        assert!(health_rows()
            .iter()
            .any(|r| r.item == "Boom A" && r.level == Level::Warn));
        retry(&ctx, "test:boom-a");
    }

    #[test]
    fn try_again_draws_the_page_once_more() {
        let ctx = egui::Context::default();
        let fixed = Cell::new(false);
        let tries = Cell::new(0);
        let go = |ui: &mut egui::Ui| {
            page(ui, "test:retry", "Retry", |ui| {
                tries.set(tries.get() + 1);
                if !fixed.get() {
                    panic!("not yet");
                }
                ui.label("ok");
            });
        };
        frame(&ctx, go);
        assert!(is_disabled("test:retry"));
        retry(&ctx, "test:retry");
        fixed.set(true);
        frame(&ctx, go);
        frame(&ctx, go);
        assert!(!is_disabled("test:retry"));
        assert_eq!(tries.get(), 3);
    }

    #[test]
    fn the_next_frame_draws_after_a_panic_in_nested_layouts() {
        let ctx = egui::Context::default();
        frame(&ctx, |ui| {
            page(ui, "test:nested", "Nested", |ui| {
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        egui::ScrollArea::vertical().show(ui, |_| panic!("deep"));
                    });
                });
            });
        });
        let drew = Cell::new(false);
        frame(&ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label("after");
                drew.set(true);
            });
            egui::ScrollArea::vertical().show(ui, |ui| ui.label("scroll"));
        });
        assert!(drew.get());
        retry(&ctx, "test:nested");
    }

    #[test]
    fn a_panicking_tile_disables_just_that_tile() {
        let ctx = egui::Context::default();
        let seen = Cell::new(0);
        for _ in 0..2 {
            frame(&ctx, |ui| {
                tile(ui, "test:tile-one", "Tile one", |_| panic!("tile failed"));
                tile(ui, "test:tile-two", "Tile two", |ui| {
                    seen.set(seen.get() + 1);
                    ui.label("two");
                });
            });
        }
        assert!(is_disabled("test:tile-one"));
        assert!(!is_disabled("test:tile-two"));
        assert_eq!(seen.get(), 2);
        retry(&ctx, "test:tile-one");
    }

    #[test]
    fn saving_works_after_a_panic_inside_a_store_update() {
        super::super::store::test_config_dir();
        let ctx = egui::Context::default();
        frame(&ctx, |ui| {
            page(ui, "test:store", "Store", |_| {
                let _ = super::super::store::update::<toml::Table>("page_guard_test", |_| {
                    panic!("closure failed")
                });
            });
        });
        assert!(is_disabled("test:store"));
        let mut t = toml::Table::new();
        t.insert("n".into(), 7.into());
        super::super::store::save("page_guard_test", &t).expect("save after the panic");
        let back = super::super::store::update::<toml::Table>("page_guard_test", |t| {
            t.insert("m".into(), 8.into());
        })
        .expect("update after the panic");
        assert_eq!(back.get("n").and_then(|v| v.as_integer()), Some(7));
        retry(&ctx, "test:store");
    }
}
