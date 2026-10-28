//! The art flow's renders (#515): the Screens page's three tiers, the full-keyboard choice and
//! the sending strip's states, drawn from the real egui code with sample keyboards. With
//! `FOXGLOVE_UI_PREVIEW_DIR` set they are kept as `ux-art-built-*.png` for review.

use super::*;
use crate::foxglove::art_service::{Job, JobKind, Outcome, Retry, Snapshot};
use crate::foxglove::art_ui;
use crate::foxglove::shell::Page;
use std::time::Instant;

fn page(art: (Snapshot, crate::foxglove::art_library::Library), height: f32) -> image::RgbaImage {
    let mut samples = Samples::full();
    samples.art = Some(art);
    shell_preview_with(Page::Screens, &samples, height)
}

fn job(kind: JobKind, name: &str) -> Job {
    Job {
        kind,
        name: name.into(),
        quiet: false,
        again: Some(Retry::Status),
    }
}

fn count(img: &image::RgbaImage, c: Color32) -> usize {
    img.pixels()
        .filter(|p| p.0[..3] == [c.r(), c.g(), c.b()])
        .count()
}

/// The page with a keyboard, without one, sending, full with the replace choice, and an error.
#[test]
fn art_flow_pages_render() {
    use crate::foxglove::theme::DARK;
    let sample = art_ui::tests::sample;
    let connected = page(sample(), 1900.0);
    keep("ux-art-built-1-screens.png", &connected);
    // The pictures: the built-ins' and the library's, in the screen's lit tone.
    let lit = Color32::from_rgb(0xe8, 0xea, 0xe9);
    assert!(count(&connected, lit) > 5000, "art pictures on the page");

    let none = shell_preview_with(Page::Screens, &Samples::none(), 900.0);
    keep("ux-art-built-2-no-keyboard.png", &none);

    // Keeping constellations: the send running, its badge and the strip.
    let (mut s, l) = sample();
    s.busy = true;
    s.push_progress = Some((2048, 3599));
    s.job = Some(Job {
        again: Some(Retry::Keep {
            file: "art/constellations.bin".into(),
            slot: 5,
        }),
        ..job(JobKind::Keep, "constellations")
    });
    let sending = page((s, l), 1900.0);
    keep("ux-art-built-3-sending.png", &sending);
    assert!(count(&sending, DARK.accent) > count(&connected, DARK.accent));

    // Full: all eight places kept, and Keep on keyboard asks what to replace.
    let (mut s, l) = sample();
    let mut st = s.status.unwrap();
    st.flash_used = 0xff;
    s.status = Some(st);
    for (n, name) in [(5, "vitals"), (6, "lock"), (7, "route")] {
        s.slots.push(foxglove_link::protocol::SlotInfo {
            slot: n,
            used: true,
            writes: 1,
            len: 4000,
            name: name.into(),
        });
    }
    art_ui::preview_replace(Some(("hearth", Some(2))));
    let full = page((s, l), 1900.0);
    art_ui::preview_replace(None);
    keep("ux-art-built-4-full-replace.png", &full);

    // An error in words: the right half asleep, with Try again.
    let (mut s, l) = sample();
    s.outcome = Some(Outcome {
        job: job(JobKind::Show, "moon-garden"),
        result: Err("the right half is not connected".into()),
        at: Instant::now(),
    });
    keep("ux-art-built-5-error.png", &page((s, l), 1000.0));
}

/// Every strip state the page and the tray share, one under the other.
#[test]
fn art_strip_states_render() {
    let base = || Snapshot {
        connected: true,
        status: art_ui::tests::sample().0.status,
        ..Default::default()
    };
    let at = Instant::now();
    let done = |kind, result: Result<(), String>| {
        let mut s = base();
        s.outcome = Some(Outcome {
            job: job(kind, "Uplink"),
            result,
            at,
        });
        s
    };
    let mut sending = base();
    sending.busy = true;
    sending.job = Some(job(JobKind::Keep, "Uplink"));
    sending.push_progress = Some((600, 1000));
    let mut saving = sending.clone();
    saving.push_progress = None;
    let mut kept = done(JobKind::Keep, Ok(()));
    kept.status.as_mut().unwrap().right_state = 0;
    kept.status.as_mut().unwrap().right_set = 19;
    let states = [
        sending,
        saving,
        kept,
        done(JobKind::Show, Err("the right half is not connected".into())),
        done(
            JobKind::Show,
            Err("push failed: left: failed, error: Bluetooth write failed".into()),
        ),
        done(JobKind::Show, Ok(())),
        done(
            JobKind::Keep,
            Err("save failed on the right half: right: save failed".into()),
        ),
    ];
    let strips: Vec<_> = states
        .iter()
        .map(|s| crate::foxglove::art_words::strip(s).expect("a strip"))
        .collect();
    let ctx = preview_ctx();
    let img = render_ui(
        &ctx,
        egui::vec2(720.0, 560.0),
        Color32::from_rgb(0x09, 0x0a, 0x0c),
        |ui| {
            egui::Frame::new()
                .inner_margin(egui::Margin::same(16))
                .show(ui, |ui| {
                    ui.label(crate::foxglove::theme::heading(
                        ui,
                        "Sending art: one strip, the same words on the page and in the tray",
                    ));
                    for s in &strips {
                        ui.add_space(6.0);
                        art_ui::strip_ui(ui, s);
                    }
                });
        },
    );
    keep("ux-art-built-6-strip-states.png", &img);
}

/// The tray's Screens part (#519), idle and sending, and the left screen's Change… list (#521).
#[test]
fn art_tray_and_left_picker_render() {
    use crate::foxglove::tray_panel;
    let tray = |v: &tray_panel::View| {
        let ctx = preview_ctx();
        render(
            &ctx,
            egui::vec2(tray_panel::WIDTH + 40.0, 760.0),
            Color32::from_rgb(0x09, 0x0a, 0x0c),
            |ctx| {
                egui::Area::new(egui::Id::new("tray-panel-preview"))
                    .fixed_pos(egui::pos2(20.0, 20.0))
                    .show(ctx, |ui| {
                        tray_panel::panel(ui, v, &mut Default::default(), &mut Vec::new());
                    });
            },
        )
    };
    let idle = tray_panel::tests::sample();
    keep("ux-art-built-7-tray.png", &tray(&idle));
    let mut s = Snapshot {
        busy: true,
        push_progress: Some((2100, 3500)),
        job: Some(job(JobKind::Show, "Uplink")),
        ..Default::default()
    };
    s.connected = true;
    let sending = tray_panel::View {
        art_strip: crate::foxglove::art_words::strip(&s),
        art_ready: false,
        ..idle
    };
    let img = tray(&sending);
    keep("ux-art-built-8-tray-sending.png", &img);
    assert!(
        count(&img, crate::foxglove::theme::DARK.accent) > 200,
        "the strip's bar"
    );

    let ctx = preview_ctx();
    let img = render_ui(
        &ctx,
        egui::vec2(420.0, 340.0),
        Color32::from_rgb(0x09, 0x0a, 0x0c),
        |ui| {
            egui::Frame::new()
                .inner_margin(egui::Margin::same(16))
                .show(ui, |ui| {
                    ui.label(crate::foxglove::theme::heading(ui, "Left screen: Change…"));
                    ui.add_space(6.0);
                    crate::foxglove::screens_ui::left_choices(
                        ui,
                        crate::foxglove::screens_ui::LeftChoice::Info,
                    );
                });
        },
    );
    keep("ux-art-built-9-left-picker.png", &img);
}

/// After a firmware update: the keyboard card says what happened to the art (#521).
#[test]
fn art_after_update_renders() {
    let (mut s, l) = art_ui::tests::sample();
    s.notice = Some(
        "The keyboard's firmware changed. Your 5 kept pieces are still on the keyboard. \
         New with the keyboard: Bandit skyline. No longer with the keyboard: Bandit boot."
            .into(),
    );
    keep("ux-art-built-10-after-update.png", &page((s, l), 1300.0));
}
