//! Sound on the Scenes page (the plan: review/ux-plan-sound-effect.md, slice 3): the words for
//! every state and the controls on the Effect card (Follows, Smoothing, the level bar, the
//! privacy line).
//!
//! Words and rules are plain functions of a [`SoundView`], so tests and the review images draw
//! them with no audio device. Nothing here reads a level or sends a colour: the level comes from
//! [`super::sound_feed`], the light output is [`super::sound_out`].
//!
//! The plan's never-shown list holds: no "endpoint", "render", "session", "peak", "meter", "dB",
//! "loopback", "fact", "poll", and never a number for the loudness.

use egui::{Align2, RichText, Sense, Stroke};
use foxglove_scene::Rgb;

use super::announce;
use super::scene_looks::Look;
use super::scenes_ui::{strip, Tone};
use super::sound::{self, Smooth};
use super::sound_feed::{Status, GAME_WORDS};
use super::theme;

/// The page's status line while the effect is picked and not running.
pub const NOT_RUNNING: &str = "Not running.";
/// What the level bar says while the scene doesn't run.
pub const BAR_IDLE: &str = "Not running";
/// The bar's ends.
pub const BAR_ENDS: (&str, &str) = ("quiet", "loud");

/// What the page knows of Sound.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SoundView {
    /// What the source last said.
    pub status: Status,
    /// The level now, 0 (quiet) to 1 (loud): the bar's marker. Never written as a number.
    pub level: f32,
    /// The keyboard holds steady (game mode).
    pub game: bool,
    /// The running scene is the Sound look.
    pub live: bool,
    /// Windows' animation effects are off: Sound stays calm and Quick is not offered.
    pub reduced: bool,
}

/// The words after "Running: Sound · ", and their tone.
pub fn running_detail(status: Status, game: bool) -> (&'static str, Tone) {
    match status {
        Status::Playing | Status::Quiet if game => (GAME_WORDS, Tone::Plain),
        Status::Playing => (status.words(), Tone::Ok),
        Status::NoDevice | Status::CantRead => (status.words(), Tone::Warn),
        Status::Quiet | Status::Nothing | Status::Muted => (status.words(), Tone::Plain),
    }
}

/// The page's status line for the Sound look.
pub fn status_line(running: bool, v: &SoundView) -> (String, Tone) {
    if running && v.live {
        let (d, tone) = running_detail(v.status, v.game);
        return (format!("Running: {} · {d}", sound::NAME), tone);
    }
    (NOT_RUNNING.into(), Tone::Muted)
}

/// The word under the level bar: the state, never a number.
pub fn bar_word(running: bool, v: &SoundView) -> &'static str {
    if running && v.live {
        v.status.level_word(v.level)
    } else {
        BAR_IDLE
    }
}

/// Where the bar's marker sits (0-1), when there is a level to show.
pub fn marker(running: bool, v: &SoundView) -> Option<f32> {
    (running && v.live && v.status.follows()).then(|| v.level.clamp(0.0, 1.0))
}

/// The bar's name for a screen reader: its state in words.
pub fn bar_name(running: bool, v: &SoundView) -> String {
    format!("Sound level bar, {}", bar_word(running, v).to_lowercase())
}

/// How often the bar is drawn again while a level moves: at most 10 a second, once a second
/// with Windows' animation effects off. `None`: nothing moves, no repaint is asked for.
pub fn bar_repaint(running: bool, v: &SoundView) -> Option<std::time::Duration> {
    marker(running, v)?;
    Some(std::time::Duration::from_millis(if v.reduced {
        1_000
    } else {
        100
    }))
}

/// "No speakers or headset connected".
pub const SAID_NO_DEVICE: &str = "No speakers or headset connected";
/// "Sound is muted".
pub const SAID_MUTED: &str = "Sound is muted";
/// "Sound level back".
pub const SAID_BACK: &str = "Sound level back";
/// The least time between two status announcements, in seconds (the plan, section 8).
pub const SAY_GAP_S: f64 = 5.0;

/// What the card remembers between frames to say a lost or returned level once.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Heard {
    seen: Option<Status>,
    /// The level was lost (no device, muted, unreadable) and has not come back yet.
    lost: bool,
}

impl Heard {
    /// What to say now that the status is `now`: only a change of state, never the level.
    /// "Nothing is playing" and "Quiet" are never said.
    pub fn next(self, now: Status) -> (Heard, Option<&'static str>) {
        let was = |s: Status| self.seen == Some(s);
        let mut h = Heard {
            seen: Some(now),
            ..self
        };
        let said = match now {
            Status::NoDevice if !was(Status::NoDevice) => {
                h.lost = true;
                Some(SAID_NO_DEVICE)
            }
            Status::Muted if !was(Status::Muted) => {
                h.lost = true;
                Some(SAID_MUTED)
            }
            Status::CantRead => {
                h.lost = true;
                None
            }
            Status::Playing if self.lost => {
                h.lost = false;
                Some(SAID_BACK)
            }
            _ => None,
        };
        (h, said)
    }
}

fn region_id() -> egui::Id {
    egui::Id::new("sound-card-announce")
}

/// Says a level lost or back through the live region, at most once per [`SAY_GAP_S`]. A change
/// held back by the limit is said when the time is up. Not running: forgets.
fn announce_status(ui: &egui::Ui, live: bool, status: Status) {
    let ctx = ui.ctx();
    let key = region_id().with("heard");
    if !live {
        ctx.data_mut(|d| d.remove::<(Heard, f64)>(key));
        return;
    }
    let (mem, at): (Heard, f64) = ctx
        .data(|d| d.get_temp::<(Heard, f64)>(key))
        .unwrap_or((Heard::default(), f64::NEG_INFINITY));
    let (next, said) = mem.next(status);
    let now = ctx.input(|i| i.time);
    match said {
        Some(_) if now - at < SAY_GAP_S => {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(SAY_GAP_S - (now - at)));
        }
        Some(words) => {
            announce::say(ctx, region_id(), words);
            ctx.data_mut(|d| d.insert_temp(key, (next, now)));
        }
        None => {
            ctx.data_mut(|d| d.insert_temp(key, (next, at)));
        }
    }
}

/// Follows: above the palette pills. A plain label, no control.
pub fn head(ui: &mut egui::Ui) {
    ui.label(theme::caps_label(ui, "Follows"));
    ui.label(sound::FOLLOWS);
}

/// Smoothing, the level bar and the privacy line: below the palette pills.
pub fn tail(
    ui: &mut egui::Ui,
    look: &Look,
    v: &SoundView,
    running: bool,
    set: &mut impl FnMut(Look, bool),
) {
    let p = theme::palette(ui);
    announce_status(ui, running && v.live, v.status);
    announce::region(ui, region_id());
    ui.label(theme::caps_label(ui, "Smoothing"));
    // With animation effects off Sound stays calm whatever was picked: Calm shows as chosen.
    let shown = if v.reduced {
        Smooth::Calm
    } else {
        look.sound_smoothing
    };
    ui.horizontal_wrapped(|ui| {
        theme::pill_group(ui, "Smoothing", |ui, g| {
            for s in Smooth::ALL {
                let enabled = !(v.reduced && s == Smooth::Quick);
                let pill = g.pill_enabled(ui, enabled, shown == s, s.label());
                let pill = if enabled {
                    pill.on_hover_text(sound::SMOOTHING_HINT)
                } else {
                    ui.ctx().accesskit_node_builder(pill.id, |n| {
                        n.set_description(sound::REDUCED_HINT)
                    });
                    pill.on_disabled_hover_text(sound::REDUCED_HINT)
                };
                if pill.clicked() && look.sound_smoothing != s {
                    set(
                        Look {
                            sound_smoothing: s,
                            ..look.clone()
                        },
                        true,
                    );
                }
            }
        })
    });
    let hint = if v.reduced {
        sound::REDUCED_HINT
    } else {
        sound::SMOOTHING_HINT
    };
    ui.add(egui::Label::new(RichText::new(hint).color(p.muted)).wrap());
    ui.add_space(6.0);
    bar(ui, look, v, running);
    if let Some(after) = bar_repaint(running, v) {
        ui.ctx().request_repaint_after(after);
    }
    ui.add_space(4.0);
    // Always on the card: what is read, and what never is.
    ui.add(egui::Label::new(RichText::new(sound::PRIVACY).color(p.text)).wrap());
}

/// The level bar: the look's colours from quiet to loud, a marker at the level while one is
/// followed, and the state's word.
fn bar(ui: &mut egui::Ui, look: &Look, v: &SoundView, running: bool) {
    let p = theme::palette(ui);
    let c = sound::colours(&look.palette);
    let ramp: Vec<Rgb> = (0..=8)
        .map(|i| sound::colour_at(i as f32 / 8.0, c))
        .collect();
    let font = theme::body(12.0);
    let (top, height, bottom) = (10.0, 12.0, 18.0);
    let (rect, resp) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), top + height + bottom),
        Sense::hover(),
    );
    resp.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Label, true, bar_name(running, v))
    });
    let painter = ui.painter();
    let bar_rect = egui::Rect::from_min_size(
        egui::pos2(rect.left(), rect.top() + top),
        egui::vec2(rect.width(), height),
    );
    strip(painter, bar_rect, &ramp);
    let at = marker(running, v);
    // The bar reads by shape too: full colour up to the level, faded past it, and all faded
    // while no level is followed.
    let from = at.map_or(bar_rect.left(), |at| rect.left() + at * rect.width());
    painter.rect_filled(
        egui::Rect::from_min_max(egui::pos2(from, bar_rect.top()), bar_rect.max),
        0.0,
        p.surface.gamma_multiply(0.75),
    );
    painter.rect_stroke(
        bar_rect,
        2.0,
        Stroke::new(1.0, p.muted),
        egui::StrokeKind::Outside,
    );
    let label_y = bar_rect.bottom() + 3.0;
    painter.text(
        egui::pos2(rect.left(), label_y),
        Align2::LEFT_TOP,
        BAR_ENDS.0,
        font.clone(),
        p.muted,
    );
    painter.text(
        egui::pos2(rect.right(), label_y),
        Align2::RIGHT_TOP,
        BAR_ENDS.1,
        font.clone(),
        p.muted,
    );
    if let Some(at) = at {
        let x = rect.left() + at * rect.width();
        painter.line_segment(
            [
                egui::pos2(x, bar_rect.top()),
                egui::pos2(x, bar_rect.bottom()),
            ],
            Stroke::new(2.0, p.text),
        );
        let tip = egui::pos2(x, bar_rect.top() - 1.0);
        painter.add(egui::Shape::convex_polygon(
            vec![
                tip,
                egui::pos2(x - 4.0, tip.y - 6.0),
                egui::pos2(x + 4.0, tip.y - 6.0),
            ],
            p.text,
            Stroke::NONE,
        ));
    }
    painter.text(
        egui::pos2(rect.center().x, label_y),
        Align2::CENTER_TOP,
        bar_word(running, v),
        font,
        if at.is_some() { p.text } else { p.muted },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn live(status: Status) -> SoundView {
        SoundView {
            status,
            level: 0.5,
            live: true,
            ..SoundView::default()
        }
    }

    const EVERY: [Status; 6] = [
        Status::Playing,
        Status::Quiet,
        Status::Nothing,
        Status::Muted,
        Status::NoDevice,
        Status::CantRead,
    ];

    /// Words the plan says never to show.
    const NEVER: [&str; 11] = [
        "endpoint", "render", "session", "peak", "meter", "dB", "loopback", "fact", "poll", "%",
        "unknown",
    ];

    #[test]
    fn the_status_lines_are_the_plans_exactly() {
        let line = |s| status_line(true, &live(s));
        assert_eq!(
            line(Status::Playing),
            ("Running: Sound · Playing".into(), Tone::Ok)
        );
        assert_eq!(
            line(Status::Quiet),
            ("Running: Sound · Quiet".into(), Tone::Plain)
        );
        assert_eq!(
            line(Status::Nothing),
            ("Running: Sound · Nothing is playing".into(), Tone::Plain)
        );
        assert_eq!(
            line(Status::Muted),
            ("Running: Sound · Your sound is muted".into(), Tone::Plain)
        );
        assert_eq!(
            line(Status::NoDevice),
            (
                "Running: Sound · No speakers or headset are connected right now.".into(),
                Tone::Warn
            )
        );
        assert_eq!(
            line(Status::CantRead),
            (
                "Running: Sound · Foxglove can't read the sound level right now. It tries again when your sound device changes, or press Stop, then Start."
                    .into(),
                Tone::Warn
            )
        );
        let game = |s| {
            status_line(
                true,
                &SoundView {
                    game: true,
                    ..live(s)
                },
            )
        };
        for s in [Status::Playing, Status::Quiet] {
            assert_eq!(
                game(s),
                (
                    "Running: Sound · Keyboard lights hold steady while you play".into(),
                    Tone::Plain
                )
            );
        }
        // What stops the level is said in game mode too.
        assert_eq!(game(Status::Muted), line(Status::Muted));
        assert_eq!(game(Status::NoDevice), line(Status::NoDevice));
        // Not running, or another look runs.
        let idle = ("Not running.".to_string(), Tone::Muted);
        assert_eq!(status_line(false, &live(Status::Playing)), idle);
        assert_eq!(status_line(true, &SoundView::default()), idle);
    }

    #[test]
    fn no_line_uses_a_word_the_plan_forbids_or_a_number_for_the_level() {
        let mut texts: Vec<String> = vec![
            NOT_RUNNING.into(),
            BAR_IDLE.into(),
            BAR_ENDS.0.into(),
            BAR_ENDS.1.into(),
            SAID_NO_DEVICE.into(),
            SAID_MUTED.into(),
            SAID_BACK.into(),
            sound::NAME.into(),
            sound::DESCRIPTION.into(),
            sound::FOLLOWS.into(),
            sound::PRIVACY.into(),
            sound::SMOOTHING_HINT.into(),
            sound::REDUCED_HINT.into(),
            GAME_WORDS.into(),
        ];
        for s in Smooth::ALL {
            texts.push(s.label().into());
        }
        for s in EVERY {
            for game in [false, true] {
                for level in [0.0, 0.3, 0.9] {
                    let v = SoundView {
                        game,
                        level,
                        ..live(s)
                    };
                    texts.push(status_line(true, &v).0);
                    texts.push(bar_word(true, &v).into());
                    texts.push(bar_name(true, &v));
                }
            }
        }
        for t in &texts {
            // Whole words ("speakers" is not "peak").
            let lower = t.to_lowercase();
            let words: Vec<&str> = lower
                .split(|c: char| !c.is_alphanumeric())
                .filter(|w| !w.is_empty())
                .collect();
            for bad in NEVER {
                let bad = bad.to_lowercase();
                assert!(
                    !words.iter().any(|w| *w == bad || *w == format!("{bad}s")),
                    "{bad:?} in {t:?}"
                );
            }
            assert!(!t.chars().any(|c| c.is_ascii_digit()), "a number in {t:?}");
        }
    }

    #[test]
    fn the_privacy_line_names_foxglove_and_says_what_is_never_used() {
        assert!(sound::PRIVACY.contains("Foxglove only reads how loud"));
        assert!(sound::PRIVACY.contains("records"));
        assert!(sound::PRIVACY.contains("never uses the microphone"));
    }

    #[test]
    fn the_bar_shows_a_marker_only_while_a_level_is_followed() {
        for s in EVERY {
            let v = live(s);
            assert_eq!(marker(true, &v).is_some(), s.follows(), "{s:?}");
            assert_eq!(bar_repaint(true, &v).is_some(), s.follows(), "{s:?}");
            assert_eq!(marker(false, &v), None);
            assert_eq!(bar_word(false, &v), "Not running");
        }
        assert_eq!(bar_word(true, &live(Status::Playing)), "Playing");
        assert_eq!(
            bar_word(
                true,
                &SoundView {
                    level: 0.9,
                    ..live(Status::Playing)
                }
            ),
            "Playing"
        );
        // Quiet and Nothing playing share the card's word.
        assert_eq!(bar_word(true, &live(Status::Quiet)), "Quiet");
        assert_eq!(bar_word(true, &live(Status::Nothing)), "Quiet");
        assert_eq!(bar_word(true, &live(Status::Muted)), "Muted");
        assert_eq!(
            bar_name(true, &live(Status::Playing)),
            "Sound level bar, playing"
        );
        // At most 10 a second; once a second with animation effects off.
        let ms = |reduced| {
            bar_repaint(
                true,
                &SoundView {
                    reduced,
                    ..live(Status::Playing)
                },
            )
            .map(|d| d.as_millis())
        };
        assert_eq!(ms(false), Some(100));
        assert_eq!(ms(true), Some(1_000));
    }

    #[test]
    fn a_lost_level_and_its_return_are_said_once_and_the_level_never() {
        let mut h = Heard::default();
        let mut said = Vec::new();
        for s in [
            Status::Nothing,
            Status::Playing,
            Status::Quiet,
            Status::Nothing,
            Status::Playing,
            Status::Muted,
            Status::Muted,
            Status::Quiet,
            Status::Playing,
            Status::NoDevice,
            Status::NoDevice,
            Status::Playing,
            Status::Playing,
        ] {
            let (n, w) = h.next(s);
            h = n;
            said.extend(w);
        }
        assert_eq!(said, [SAID_MUTED, SAID_BACK, SAID_NO_DEVICE, SAID_BACK]);
        // A start with sound already playing has nothing to say.
        assert_eq!(Heard::default().next(Status::Playing).1, None);
        assert_eq!(Heard::default().next(Status::Nothing).1, None);
    }

    fn nodes_of(v: SoundView, look: &Look) -> Vec<egui::accesskit::Node> {
        super::super::store::test_config_dir();
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let mut fonts = egui::FontDefinitions::default();
        theme::add_fonts(&mut fonts);
        ctx.set_fonts(fonts);
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
            head(ui);
            tail(ui, look, &v, true, &mut |_, _| {});
        });
        out.textures_delta.clear();
        out.platform_output
            .accesskit_update
            .expect("a tree")
            .nodes
            .into_iter()
            .map(|(_, n)| n)
            .collect()
    }

    #[test]
    fn the_card_draws_follows_smoothing_the_bar_and_the_privacy_line() {
        let look = Look {
            kind: super::super::scene_looks::Kind::Sound,
            ..Look::default()
        };
        let nodes = nodes_of(live(Status::Playing), &look);
        let labels: Vec<&str> = nodes
            .iter()
            .flat_map(|n| [n.label(), n.value()])
            .flatten()
            .collect();
        for want in [
            sound::FOLLOWS,
            sound::PRIVACY,
            "Calm",
            "Quick",
            "Sound level bar, playing",
        ] {
            assert!(labels.contains(&want), "{want:?} missing: {labels:?}");
        }
        assert!(nodes.iter().all(|n| !n.is_disabled()));
        // Animation effects off: Quick is there, disabled, and says why.
        let nodes = nodes_of(
            SoundView {
                reduced: true,
                ..live(Status::Playing)
            },
            &Look {
                sound_smoothing: Smooth::Quick,
                ..look
            },
        );
        let off: Vec<&egui::accesskit::Node> = nodes.iter().filter(|n| n.is_disabled()).collect();
        assert_eq!(off.len(), 1);
        assert_eq!(off[0].label(), Some("Quick"));
        assert_eq!(off[0].description(), Some(sound::REDUCED_HINT));
    }
}
