//! Mockups for the knob's on-screen feedback (#719). HOLD: research only, nothing here is product
//! code. Each option is one storyboard sheet of the seven moments the user asked about (click to
//! the next app, turning on an app, the end of the cycle, a hold, a double-click mute, the app's
//! sound ending, turning on master), drawn as rough frames of the bottom of the desktop with
//! Windows' own volume flyout where Windows would show it. Set `FOXGLOVE_UI_PREVIEW_DIR` to keep
//! the PNGs (`knob-mockup-a-flyout-twin.png`, `-b-corner-chip`, `-c-keyboard-first`, and the left
//! screen's target line, `knob-mockup-left-screen.png`).

use super::{keep, preview_ctx, render, PPP};
use egui::{pos2, vec2, Align2, Color32, FontId, Painter, Pos2, Rect, Vec2};
use egui_phosphor::regular as icon;

const FRAME: Vec2 = vec2(300.0, 176.0);
const CAPTION: f32 = 46.0;
const GAP: f32 = 14.0;
const COLS: usize = 4;

const DESKTOP: Color32 = Color32::from_rgb(0x1e, 0x2a, 0x3c);
const TASKBAR: Color32 = Color32::from_rgb(0x20, 0x20, 0x22);
const CARD: Color32 = Color32::from_rgb(0x2c, 0x2c, 0x2e);
const TRACK: Color32 = Color32::from_rgb(0x5a, 0x5a, 0x5e);
const ACCENT: Color32 = Color32::from_rgb(0x4c, 0xc2, 0xff);
const TEXT: Color32 = Color32::from_rgb(0xf2, 0xf2, 0xf2);
const DIM: Color32 = Color32::from_rgb(0xa8, 0xa8, 0xae);
const SHEET: Color32 = Color32::from_rgb(0x14, 0x14, 0x16);

const SPOTIFY: Color32 = Color32::from_rgb(0x1d, 0xb9, 0x54);
const DISCORD: Color32 = Color32::from_rgb(0x58, 0x65, 0xf2);

type Draw = Box<dyn Fn(&Painter, Rect)>;

/// The bottom of a desktop: wallpaper, the taskbar with a few icons and the clock.
fn desktop(p: &Painter, r: Rect) {
    p.rect_filled(r, 6.0, DESKTOP);
    let bar = Rect::from_min_max(pos2(r.left(), r.bottom() - 22.0), r.max);
    p.rect_filled(bar, 0.0, TASKBAR);
    for i in 0..5 {
        let x = r.center().x - 50.0 + i as f32 * 22.0;
        p.rect_filled(
            Rect::from_center_size(pos2(x, bar.center().y), vec2(14.0, 14.0)),
            3.0,
            Color32::from_rgb(0x44, 0x44, 0x4a),
        );
    }
    p.text(
        pos2(bar.right() - 8.0, bar.center().y),
        Align2::RIGHT_CENTER,
        "10:42",
        FontId::proportional(10.0),
        DIM,
    );
}

/// Where Windows 11 shows its volume flyout: centred, just above the taskbar.
fn flyout_spot(r: Rect) -> Rect {
    Rect::from_center_size(
        pos2(r.center().x, r.bottom() - 22.0 - 12.0 - 20.0),
        vec2(196.0, 40.0),
    )
}

fn bar(p: &Painter, r: Rect, level: u8, fill: Color32) {
    p.rect_filled(r, 2.0, TRACK);
    let w = r.width() * f32::from(level.min(100)) / 100.0;
    p.rect_filled(Rect::from_min_size(r.min, vec2(w, r.height())), 2.0, fill);
}

/// Windows' own volume flyout (master), as the volume keys bring it up.
fn win_flyout(p: &Painter, r: Rect, level: u8) {
    let s = flyout_spot(r);
    p.rect_filled(s, 8.0, CARD);
    p.text(
        pos2(s.left() + 16.0, s.center().y),
        Align2::LEFT_CENTER,
        icon::SPEAKER_HIGH,
        FontId::proportional(16.0),
        TEXT,
    );
    bar(
        p,
        Rect::from_min_size(pos2(s.left() + 40.0, s.center().y - 2.0), vec2(116.0, 4.0)),
        level,
        ACCENT,
    );
    p.text(
        pos2(s.right() - 12.0, s.center().y),
        Align2::RIGHT_CENTER,
        level.to_string(),
        FontId::proportional(12.0),
        TEXT,
    );
    p.text(
        pos2(s.center().x, s.bottom() + 4.0),
        Align2::CENTER_TOP,
        "Windows' own flyout",
        FontId::proportional(8.0),
        DIM,
    );
}

/// An app's icon: its colour and a glyph (the real one is the app's exe icon).
fn app_icon(p: &Painter, c: Pos2, tint: Color32, glyph: &str, size: f32) {
    p.circle_filled(c, size / 2.0, tint);
    p.text(
        c,
        Align2::CENTER_CENTER,
        glyph,
        FontId::proportional(size * 0.6),
        Color32::WHITE,
    );
}

struct Card<'a> {
    glyph: &'a str,
    tint: Color32,
    title: &'a str,
    /// The level, or none (a message only).
    level: Option<u8>,
    muted: bool,
    /// A second line, small: "2 of 3", "back to master".
    note: &'a str,
}

/// Option A's card, in the flyout's own place and size.
fn card(p: &Painter, s: Rect, c: &Card) {
    p.rect_filled(s, 8.0, CARD);
    app_icon(
        p,
        pos2(s.left() + 20.0, s.center().y),
        c.tint,
        c.glyph,
        22.0,
    );
    let title = if c.muted {
        format!("{} {}", c.title, icon::SPEAKER_SLASH)
    } else {
        c.title.to_string()
    };
    p.text(
        pos2(s.left() + 38.0, s.top() + 10.0),
        Align2::LEFT_CENTER,
        title,
        FontId::proportional(11.0),
        TEXT,
    );
    if let Some(level) = c.level {
        bar(
            p,
            Rect::from_min_size(pos2(s.left() + 38.0, s.top() + 25.0), vec2(112.0, 4.0)),
            level,
            if c.muted { TRACK } else { ACCENT },
        );
        p.text(
            pos2(s.right() - 12.0, s.top() + 27.0),
            Align2::RIGHT_CENTER,
            level.to_string(),
            FontId::proportional(12.0),
            if c.muted { DIM } else { TEXT },
        );
    }
    if !c.note.is_empty() {
        p.text(
            pos2(s.right() - 10.0, s.top() + 10.0),
            Align2::RIGHT_CENTER,
            c.note,
            FontId::proportional(8.5),
            DIM,
        );
    }
}

/// A timing tag at a frame's top-left: how long the thing shows.
fn timing(p: &Painter, r: Rect, text: &str) {
    p.text(
        pos2(r.left() + 8.0, r.top() + 8.0),
        Align2::LEFT_TOP,
        text,
        FontId::proportional(9.0),
        Color32::from_rgb(0xff, 0xd2, 0x6a),
    );
}

/// The knob's own turn, drawn as an arrow at the top-right: what the hand does.
fn hand(p: &Painter, r: Rect, text: &str) {
    p.text(
        pos2(r.right() - 8.0, r.top() + 8.0),
        Align2::RIGHT_TOP,
        text,
        FontId::proportional(9.0),
        TEXT,
    );
}

/// The keyboard's left screen (portrait, as the person sees it), at a frame's left: the volume
/// line names the knob's target.
fn left_screen(p: &Painter, r: Rect, name: &str, level: &str, muted: bool) {
    let s = Rect::from_min_size(pos2(r.left() + 8.0, r.top() + 24.0), vec2(40.0, 92.0));
    p.rect_filled(s, 2.0, Color32::BLACK);
    p.rect_filled(s.shrink(3.0), 0.0, Color32::from_rgb(0x0c, 0x0c, 0x0c));
    p.text(
        pos2(s.center().x, s.top() + 10.0),
        Align2::CENTER_CENTER,
        "VOL",
        FontId::monospace(7.0),
        Color32::WHITE,
    );
    p.text(
        pos2(s.center().x, s.top() + 22.0),
        Align2::CENTER_CENTER,
        name,
        FontId::monospace(7.0),
        Color32::WHITE,
    );
    p.text(
        pos2(s.center().x, s.top() + 36.0),
        Align2::CENTER_CENTER,
        if muted { "MUTE" } else { level },
        FontId::monospace(10.0),
        Color32::WHITE,
    );
    p.text(
        pos2(s.center().x, s.bottom() + 3.0),
        Align2::CENTER_TOP,
        "left screen",
        FontId::proportional(7.0),
        DIM,
    );
}

/// Draws a storyboard: frames in a grid, each with its caption below.
fn sheet(name: &str, title: &str, frames: Vec<(&str, Draw)>) -> image::RgbaImage {
    let ctx = preview_ctx();
    let rows = frames.len().div_ceil(COLS);
    let size = vec2(
        COLS as f32 * (FRAME.x + GAP) + GAP,
        40.0 + rows as f32 * (FRAME.y + CAPTION + GAP) + GAP,
    );
    let img = render(&ctx, size, SHEET, |ctx| {
        egui::Area::new(egui::Id::new(("knob-mockup", name)))
            .fixed_pos(Pos2::ZERO)
            .show(ctx, |ui| {
                ui.set_min_size(size);
                let p = ui.painter();
                p.text(
                    pos2(GAP, 20.0),
                    Align2::LEFT_CENTER,
                    title,
                    FontId::proportional(15.0),
                    TEXT,
                );
                for (i, (caption, draw)) in frames.iter().enumerate() {
                    let (col, row) = (i % COLS, i / COLS);
                    let min = pos2(
                        GAP + col as f32 * (FRAME.x + GAP),
                        40.0 + row as f32 * (FRAME.y + CAPTION + GAP),
                    );
                    let r = Rect::from_min_size(min, FRAME);
                    desktop(p, r);
                    draw(p, r);
                    let text = Rect::from_min_size(
                        pos2(r.left(), r.bottom() + 4.0),
                        vec2(FRAME.x, CAPTION),
                    );
                    let galley = p.layout(
                        caption.to_string(),
                        FontId::proportional(10.0),
                        TEXT,
                        FRAME.x,
                    );
                    p.galley(text.min, galley, TEXT);
                }
            });
    });
    keep(name, &img);
    img
}

/// The frame's flyout spot holds something other than the wallpaper.
fn shows_at_spot(img: &image::RgbaImage, frame: usize) {
    let (col, row) = (frame % COLS, frame / COLS);
    let r = Rect::from_min_size(
        pos2(
            GAP + col as f32 * (FRAME.x + GAP),
            40.0 + row as f32 * (FRAME.y + CAPTION + GAP),
        ),
        FRAME,
    );
    let spot = flyout_spot(r);
    let at = |x: f32, y: f32| img.get_pixel((x * PPP) as u32, (y * PPP) as u32).0;
    assert_ne!(
        at(spot.center().x - 60.0, spot.top() + 3.0),
        at(r.left() + 4.0, r.top() + 60.0),
        "frame {frame}: nothing at the flyout's place"
    );
}

/// Option A, "Flyout twin": Foxglove's card takes the Windows flyout's place, size and look,
/// with the app's icon, name and level; on master Windows' own flyout shows, with a small tab on
/// top while Foxglove holds the knob.
#[test]
fn knob_mockup_a_flyout_twin() {
    let frames: Vec<(&str, Draw)> = vec![
        (
            "1 Click: the next app. The card shows the app, its level and its place in the cycle, 2 s.",
            Box::new(|p: &Painter, r: Rect| {
                timing(p, r, "shows 2 s");
                hand(p, r, "click");
                card(p, flyout_spot(r), &Card { glyph: icon::MUSIC_NOTE, tint: SPOTIFY, title: "Spotify", level: Some(42), muted: false, note: "1 of 3" });
            }),
        ),
        (
            "2 Turn on an app: the same card, the bar follows each detent (2 %); it hides 1.5 s after the last.",
            Box::new(|p: &Painter, r: Rect| {
                timing(p, r, "1.5 s after the last detent");
                hand(p, r, "turn +3");
                card(p, flyout_spot(r), &Card { glyph: icon::MUSIC_NOTE, tint: SPOTIFY, title: "Spotify", level: Some(48), muted: false, note: "" });
            }),
        ),
        (
            "3 Click past the last app: back to master. The card says Master volume with Windows' level, 2 s.",
            Box::new(|p: &Painter, r: Rect| {
                timing(p, r, "shows 2 s");
                hand(p, r, "click (3 of 3 was last)");
                card(p, flyout_spot(r), &Card { glyph: icon::SPEAKER_HIGH, tint: Color32::from_rgb(0x60, 0x60, 0x66), title: "Master volume", level: Some(64), muted: false, note: "back to master" });
            }),
        ),
        (
            "4 Hold (0.5 s): back to master from any app; the same master card, 2 s.",
            Box::new(|p: &Painter, r: Rect| {
                timing(p, r, "at 0.5 s, shows 2 s");
                hand(p, r, "hold");
                card(p, flyout_spot(r), &Card { glyph: icon::SPEAKER_HIGH, tint: Color32::from_rgb(0x60, 0x60, 0x66), title: "Master volume", level: Some(64), muted: false, note: "held: master" });
            }),
        ),
        (
            "5 Double click on an app: muted. The bar greys out with the mute glyph, 2 s. (On master: Windows' mute and its flyout.)",
            Box::new(|p: &Painter, r: Rect| {
                timing(p, r, "shows 2 s");
                hand(p, r, "double click");
                card(p, flyout_spot(r), &Card { glyph: icon::CHAT_CIRCLE, tint: DISCORD, title: "Discord", level: Some(70), muted: true, note: "muted" });
            }),
        ),
        (
            "6 The target's sound ends: the card says so and that the knob is on master again, 3 s.",
            Box::new(|p: &Painter, r: Rect| {
                timing(p, r, "shows 3 s, no knob touch");
                card(p, flyout_spot(r), &Card { glyph: icon::MUSIC_NOTE, tint: Color32::from_rgb(0x3a, 0x6a, 0x4a), title: "Spotify stopped", level: None, muted: false, note: "knob: master" });
            }),
        ),
        (
            "7 Turn on master: Windows' own flyout. While Foxglove holds the knob a small tab on top says Master; Foxglove draws no second popup.",
            Box::new(|p: &Painter, r: Rect| {
                timing(p, r, "Windows' timing");
                hand(p, r, "turn");
                win_flyout(p, r, 66);
                let s = flyout_spot(r);
                let tab = Rect::from_min_size(pos2(s.left() + 10.0, s.top() - 13.0), vec2(74.0, 13.0));
                p.rect_filled(tab, 4.0, CARD);
                p.text(tab.center(), Align2::CENTER_CENTER, "Knob · Master", FontId::proportional(8.0), TEXT);
            }),
        ),
        (
            "Before turning: the left screen's volume line names the target (Spotify 42, or VOL 64 on master); touching the knob brings the card.",
            Box::new(|p: &Painter, r: Rect| {
                left_screen(p, r, "Spotify", "42", false);
                p.text(pos2(r.left() + 60.0, r.top() + 40.0), Align2::LEFT_TOP, "always on: the left screen\non touch: the card", FontId::proportional(10.0), TEXT);
            }),
        ),
    ];
    let img = sheet(
        "knob-mockup-a-flyout-twin.png",
        "A · Flyout twin: Foxglove's card in the flyout's place",
        frames,
    );
    for f in [0, 1, 2, 3, 4, 5, 6] {
        shows_at_spot(&img, f);
    }
}

/// The corner chip, above the tray clock: icon, name, and the level while it changes.
fn chip(
    p: &Painter,
    r: Rect,
    glyph: &str,
    tint: Color32,
    text: &str,
    level: Option<u8>,
    muted: bool,
) {
    let w = if level.is_some() { 150.0 } else { 96.0 };
    let s = Rect::from_min_size(
        pos2(r.right() - 8.0 - w, r.bottom() - 22.0 - 8.0 - 26.0),
        vec2(w, 26.0),
    );
    p.rect_filled(s, 13.0, CARD);
    app_icon(p, pos2(s.left() + 13.0, s.center().y), tint, glyph, 18.0);
    let label = if muted {
        format!("{text} {}", icon::SPEAKER_SLASH)
    } else {
        text.to_string()
    };
    p.text(
        pos2(s.left() + 27.0, s.center().y),
        Align2::LEFT_CENTER,
        label,
        FontId::proportional(10.0),
        TEXT,
    );
    if let Some(level) = level {
        bar(
            p,
            Rect::from_min_size(pos2(s.left() + 78.0, s.center().y - 2.0), vec2(46.0, 4.0)),
            level,
            if muted { TRACK } else { ACCENT },
        );
        p.text(
            pos2(s.right() - 8.0, s.center().y),
            Align2::RIGHT_CENTER,
            level.to_string(),
            FontId::proportional(10.0),
            TEXT,
        );
    }
}

/// Option B, "Corner chip": a small chip above the tray stays as long as the knob is on an app
/// (no chip: master), and grows a level bar while the level changes.
#[test]
fn knob_mockup_b_corner_chip() {
    let frames: Vec<(&str, Draw)> = vec![
        (
            "1 Click: the chip appears above the clock with the app's icon and name, and stays while the knob is on that app.",
            Box::new(|p: &Painter, r: Rect| {
                timing(p, r, "stays while on an app");
                hand(p, r, "click");
                chip(p, r, icon::MUSIC_NOTE, SPOTIFY, "Spotify", Some(42), false);
            }),
        ),
        (
            "2 Turn on an app: the chip grows its level bar and number; 1.5 s after the last detent it shrinks to the name.",
            Box::new(|p: &Painter, r: Rect| {
                timing(p, r, "bar 1.5 s after the last detent");
                hand(p, r, "turn +3");
                chip(p, r, icon::MUSIC_NOTE, SPOTIFY, "Spotify", Some(48), false);
            }),
        ),
        (
            "3 Click past the last app: the chip says Master for 1.5 s, then goes. No chip means master.",
            Box::new(|p: &Painter, r: Rect| {
                timing(p, r, "1.5 s, then gone");
                hand(p, r, "click (last app)");
                chip(p, r, icon::SPEAKER_HIGH, Color32::from_rgb(0x60, 0x60, 0x66), "Master", None, false);
            }),
        ),
        (
            "4 Hold: the same, Master for 1.5 s, then no chip.",
            Box::new(|p: &Painter, r: Rect| {
                timing(p, r, "at 0.5 s; 1.5 s, then gone");
                hand(p, r, "hold");
                chip(p, r, icon::SPEAKER_HIGH, Color32::from_rgb(0x60, 0x60, 0x66), "Master", None, false);
            }),
        ),
        (
            "5 Double click on an app: the chip shows the mute glyph and a grey bar, and keeps the glyph while muted.",
            Box::new(|p: &Painter, r: Rect| {
                timing(p, r, "glyph while muted");
                hand(p, r, "double click");
                chip(p, r, icon::CHAT_CIRCLE, DISCORD, "Discord", Some(70), true);
            }),
        ),
        (
            "6 The target's sound ends: the chip says it stopped for 3 s, then goes (master).",
            Box::new(|p: &Painter, r: Rect| {
                timing(p, r, "3 s, then gone");
                chip(p, r, icon::MUSIC_NOTE, Color32::from_rgb(0x3a, 0x6a, 0x4a), "Spotify stopped", None, false);
            }),
        ),
        (
            "7 Turn on master: Windows' own flyout only. Nothing from Foxglove: no chip is the master sign.",
            Box::new(|p: &Painter, r: Rect| {
                timing(p, r, "Windows' timing");
                hand(p, r, "turn");
                win_flyout(p, r, 66);
            }),
        ),
        (
            "Before turning: chip there = that app; no chip = master. The left screen line names it too.",
            Box::new(|p: &Painter, r: Rect| {
                left_screen(p, r, "Spotify", "42", false);
                chip(p, r, icon::MUSIC_NOTE, SPOTIFY, "Spotify", None, false);
                p.text(pos2(r.left() + 60.0, r.top() + 40.0), Align2::LEFT_TOP, "always on while on an app:\nthe chip", FontId::proportional(10.0), TEXT);
            }),
        ),
    ];
    sheet(
        "knob-mockup-b-corner-chip.png",
        "B · Corner chip: a small indicator while the knob is on an app",
        frames,
    );
}

/// Option C, "Keyboard first": the left screen says what the knob drives, always; the desktop
/// gets a small glance card only when the target changes, never per detent.
#[test]
fn knob_mockup_c_keyboard_first() {
    let glance = |p: &Painter, r: Rect, glyph: &str, tint: Color32, text: &str| {
        let s = Rect::from_center_size(
            pos2(r.center().x, flyout_spot(r).center().y),
            vec2(130.0, 28.0),
        );
        p.rect_filled(s, 14.0, CARD);
        app_icon(p, pos2(s.left() + 14.0, s.center().y), tint, glyph, 18.0);
        p.text(
            pos2(s.left() + 28.0, s.center().y),
            Align2::LEFT_CENTER,
            text,
            FontId::proportional(10.0),
            TEXT,
        );
    };
    let frames: Vec<(&str, Draw)> = vec![
        (
            "1 Click: the left screen's volume line becomes Spotify 42; a small glance card names it for 1 s.",
            Box::new(move |p: &Painter, r: Rect| {
                timing(p, r, "glance 1 s");
                hand(p, r, "click");
                left_screen(p, r, "Spotify", "42", false);
                glance(p, r, icon::MUSIC_NOTE, SPOTIFY, "Knob: Spotify");
            }),
        ),
        (
            "2 Turn on an app: only the left screen's number moves. Nothing on the desktop.",
            Box::new(|p: &Painter, r: Rect| {
                timing(p, r, "left screen only");
                hand(p, r, "turn +3");
                left_screen(p, r, "Spotify", "48", false);
            }),
        ),
        (
            "3 Click past the last app: the line says VOL Master 64; the glance card says Knob: Master, 1 s.",
            Box::new(move |p: &Painter, r: Rect| {
                timing(p, r, "glance 1 s");
                hand(p, r, "click (last app)");
                left_screen(p, r, "Master", "64", false);
                glance(p, r, icon::SPEAKER_HIGH, Color32::from_rgb(0x60, 0x60, 0x66), "Knob: Master");
            }),
        ),
        (
            "4 Hold: the same as 3.",
            Box::new(move |p: &Painter, r: Rect| {
                timing(p, r, "glance 1 s");
                hand(p, r, "hold");
                left_screen(p, r, "Master", "64", false);
                glance(p, r, icon::SPEAKER_HIGH, Color32::from_rgb(0x60, 0x60, 0x66), "Knob: Master");
            }),
        ),
        (
            "5 Double click on an app: the line says MUTE. No desktop card.",
            Box::new(|p: &Painter, r: Rect| {
                timing(p, r, "left screen only");
                hand(p, r, "double click");
                left_screen(p, r, "Discord", "70", true);
            }),
        ),
        (
            "6 The target's sound ends: the line goes back to Master; the glance card says Spotify stopped, 2 s.",
            Box::new(move |p: &Painter, r: Rect| {
                timing(p, r, "glance 2 s");
                left_screen(p, r, "Master", "64", false);
                glance(p, r, icon::MUSIC_NOTE, Color32::from_rgb(0x3a, 0x6a, 0x4a), "Spotify stopped");
            }),
        ),
        (
            "7 Turn on master: Windows' own flyout; the left screen's line says Master, so the context is on the keyboard.",
            Box::new(|p: &Painter, r: Rect| {
                timing(p, r, "Windows' timing");
                hand(p, r, "turn");
                left_screen(p, r, "Master", "66", false);
                win_flyout(p, r, 66);
            }),
        ),
        (
            "Before turning: the left screen, always. The desktop shows something only when the target changes.",
            Box::new(|p: &Painter, r: Rect| {
                left_screen(p, r, "Spotify", "42", false);
                p.text(pos2(r.left() + 60.0, r.top() + 40.0), Align2::LEFT_TOP, "always on: the left screen\non a change: a 1 s glance", FontId::proportional(10.0), TEXT);
            }),
        ),
    ];
    sheet(
        "knob-mockup-c-keyboard-first.png",
        "C · Keyboard first: the left screen says it, the desktop only on a change",
        frames,
    );
}

/// Option A as built (#719): the real `knob_popup::paint` for each moment, on a work area the
/// size of a 1080p display's bottom strip, with Windows' own flyout (drawn here as a stand-in)
/// under the master tab. Kept as `knob-a-real.png`.
#[test]
fn knob_a_real_renders() {
    use crate::foxglove::knob_popup::{spot, Card};
    let cards: Vec<(&str, Card)> = vec![
        (
            "Click: the next app, 1 of 3",
            Card::App {
                name: "Spotify".into(),
                place: Some((1, 3)),
                level: Some(42),
                muted: false,
            },
        ),
        (
            "Turn: the level, up 1.5 s after the last detent",
            Card::App {
                name: "Spotify".into(),
                place: None,
                level: Some(48),
                muted: false,
            },
        ),
        (
            "Past the last app",
            Card::Master {
                level: Some(64),
                note: "back to master",
                muted: false,
            },
        ),
        (
            "Hold",
            Card::Master {
                level: Some(64),
                note: "held: master",
                muted: false,
            },
        ),
        (
            "Double click",
            Card::App {
                name: "Discord".into(),
                place: None,
                level: Some(70),
                muted: true,
            },
        ),
        (
            "The app's sound ended",
            Card::Stopped {
                name: "Spotify".into(),
            },
        ),
        ("On master: the tab over Windows' flyout", Card::MasterTab),
    ];
    let img = cards_sheet(
        "knob-a-real.png",
        "Knob popup A, as built: the overlay's card in Windows' flyout spot",
        &cards,
    );
    // The first card sits in the flyout's spot: its middle isn't the wallpaper.
    let first = Rect::from_min_size(pos2(GAP, 40.0), CARDS_WORK);
    let s = spot(first);
    let at = |x: f32, y: f32| img.get_pixel((x * PPP) as u32, (y * PPP) as u32).0;
    assert_ne!(
        at(s.center().x, s.center().y),
        at(first.left() + 6.0, first.top() + 6.0)
    );
}

/// #854: the click's card with the names the knob shows now: the program's description when
/// the session has no display name, and only sessions that played (the place counts those).
#[test]
fn knob_names_854_renders() {
    use crate::foxglove::knob_popup::Card;
    let app = |name: &str, place: Option<(usize, usize)>, level: u8| Card::App {
        name: name.into(),
        place,
        level: Some(level),
        muted: false,
    };
    let cards: Vec<(&str, Card)> = vec![
        (
            "Click: Firefox's description, not \"firefox\"; 1 of 2 (a silent helper isn't counted)",
            app("Firefox", Some((1, 2)), 42),
        ),
        (
            "Click: the session's own display name wins",
            app("Spotify", Some((2, 2)), 64),
        ),
        (
            "A long description",
            app("Steam Client WebHelper", Some((1, 3)), 30),
        ),
        (
            "No description: the file name, as before",
            app("retro-arcade", Some((3, 3)), 80),
        ),
    ];
    cards_sheet(
        "knob-854-names.png",
        "Knob (#854): friendly names, only apps that played",
        &cards,
    );
}

/// #855: master's cards while Foxglove holds master volume: a turn (the level, no note) and a
/// double click (muted, as an app's card says it). Windows shows no flyout under them.
#[test]
fn knob_master_855_renders() {
    use crate::foxglove::knob_popup::Card;
    let master = |level: u8, muted: bool| Card::Master {
        level: Some(level),
        note: "",
        muted,
    };
    let cards: Vec<(&str, Card)> = vec![
        (
            "Turn on master: Foxglove's card, no Windows flyout",
            master(66, false),
        ),
        ("Double click on master: muted", master(66, true)),
        ("Turn to 0", master(0, false)),
        ("Turn to 100", master(100, false)),
    ];
    cards_sheet(
        "knob-855-master.png",
        "Knob (#855): master volume on Foxglove's card",
        &cards,
    );
}

/// #1064: master's card above Windows' flyout after the flyout or the volume keys moved master
/// on a device Foxglove sets the volume of: the first time in a run with the slider row, after
/// that without, and muted.
#[test]
fn knob_master_1064_renders() {
    use crate::foxglove::knob_popup::Card;
    let outside = |level: u8, muted: bool, first: bool| Card::Outside {
        level: Some(level),
        muted,
        first,
    };
    let cards: Vec<(&str, Card)> = vec![
        (
            "Flyout or keys, first time this run: the slider row",
            outside(40, false, true),
        ),
        ("After that: the card alone", outside(38, false, false)),
        ("Mute key", outside(38, true, false)),
        ("Mute key, first time this run", outside(38, true, true)),
    ];
    cards_sheet(
        "knob-1064-outside.png",
        "Knob (#1064): master's card above Windows' flyout",
        &cards,
    );
}

/// The work area each card of [`cards_sheet`] is drawn on (a 1080p display's bottom strip).
const CARDS_WORK: egui::Vec2 = vec2(480.0, 150.0);

/// The real `knob_popup::paint` for each card, two to a row, each captioned; kept as `file`.
fn cards_sheet(
    file: &str,
    title: &str,
    cards: &[(&str, crate::foxglove::knob_popup::Card)],
) -> image::RgbaImage {
    use crate::foxglove::knob_popup::{paint, spot, Card};
    let work = CARDS_WORK;
    let cols = 2;
    let rows = cards.len().div_ceil(cols);
    let size = vec2(
        cols as f32 * (work.x + GAP) + GAP,
        40.0 + rows as f32 * (work.y + 24.0 + GAP),
    );
    let ctx = preview_ctx();
    let img = render(&ctx, size, SHEET, |ctx| {
        egui::Area::new(egui::Id::new(("knob-cards", file)))
            .fixed_pos(Pos2::ZERO)
            .show(ctx, |ui| {
                ui.set_min_size(size);
                let p = ui.painter();
                p.text(
                    pos2(GAP, 20.0),
                    Align2::LEFT_CENTER,
                    title,
                    FontId::proportional(15.0),
                    TEXT,
                );
                for (i, (caption, card)) in cards.iter().enumerate() {
                    let min = pos2(
                        GAP + (i % cols) as f32 * (work.x + GAP),
                        40.0 + (i / cols) as f32 * (work.y + 24.0 + GAP),
                    );
                    let w = Rect::from_min_size(min, work);
                    p.rect_filled(w, 4.0, DESKTOP);
                    // The taskbar just below the work area's bottom edge.
                    p.rect_filled(
                        Rect::from_min_size(pos2(w.left(), w.bottom()), vec2(w.width(), 6.0)),
                        0.0,
                        TASKBAR,
                    );
                    if matches!(card, Card::MasterTab | Card::Outside { .. }) {
                        // On a handled device (#1064) Windows' slider rests near the top.
                        let shown: u8 = if matches!(card, Card::Outside { .. }) {
                            98
                        } else {
                            66
                        };
                        // Windows' own flyout, as Windows draws it (a stand-in).
                        let s = spot(w);
                        p.rect_filled(s, 8.0, CARD);
                        p.text(
                            pos2(s.left() + 18.0, s.center().y),
                            Align2::LEFT_CENTER,
                            icon::SPEAKER_HIGH,
                            FontId::proportional(16.0),
                            TEXT,
                        );
                        bar(
                            p,
                            Rect::from_min_size(
                                pos2(s.left() + 44.0, s.center().y - 2.0),
                                vec2(110.0, 4.0),
                            ),
                            shown,
                            ACCENT,
                        );
                        p.text(
                            pos2(s.right() - 12.0, s.center().y),
                            Align2::RIGHT_CENTER,
                            shown.to_string(),
                            FontId::proportional(13.0),
                            TEXT,
                        );
                    }
                    paint(p, w, card);
                    p.text(
                        pos2(w.left(), w.bottom() + 10.0),
                        Align2::LEFT_TOP,
                        *caption,
                        FontId::proportional(11.0),
                        TEXT,
                    );
                }
            });
    });
    keep(file, &img);
    img
}

/// The left screen's volume line with the knob's target (cheap: drawn, not the panel's renderer).
#[test]
fn knob_mockup_left_screen() {
    let states: [(&str, &str, &str, bool); 4] = [
        ("master", "Master", "64", false),
        ("an app", "Spotify", "42", false),
        ("muted", "Discord", "70", true),
        ("a long name", "Firefox", "30", false),
    ];
    let frames: Vec<(&str, Draw)> = states
        .iter()
        .map(|&(caption, name, level, muted)| {
            let draw: Draw = Box::new(move |p: &Painter, r: Rect| {
                p.rect_filled(r, 6.0, SHEET);
                left_screen(p, r.translate(vec2(110.0, 10.0)), name, level, muted);
            });
            (caption, draw)
        })
        .collect();
    sheet(
        "knob-mockup-left-screen.png",
        "Left screen: the volume line names the knob's target",
        frames,
    );
}
