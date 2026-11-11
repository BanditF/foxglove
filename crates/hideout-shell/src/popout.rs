// SPDX-License-Identifier: MIT
//! Pop-out placement (contract §2.5, spec v1.6; `docs/hideout-popouts-ux.md`): a page's places
//! per screen layout in `hideout.toml`'s `[windows]` table, the screen and layout ids, and where a
//! pop-out opens. Pure: rectangles are physical pixels in virtual-screen coordinates, places are
//! device-independent units from their screen's top-left, so the window code and the tests agree.
//!
//! Clean-room: written from the contract and the pop-outs plan.

use crate::placement::{monitor_for, Monitor, Rect};
use crate::tabs::TabId;
use hideout_contract::config::{PageWindow, Place, WindowState, Windows, PLACES_PER_PAGE};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

/// The smallest pop-out, in device-independent units (§2.5).
pub const MIN: (f64, f64) = (360.0, 240.0);
/// A new pop-out is at least this big (the plan: the page area's size, at least 480 × 360).
pub const FIRST_MIN: (f64, f64) = (480.0, 360.0);
/// The offset from Hideout's window, and between cascading pop-outs (device-independent).
pub const OFFSET: f64 = 32.0;
/// The title row's height: it always stays on a screen.
pub const TITLE_ROW: f64 = 36.0;

/// The `[windows]` key of a page: `<module id>/<panel id>` for a module's page; the hub's own
/// pages go under `hideout/` with their id's dots as dashes (`hub.modules` → `hideout/hub-modules`).
pub fn page_key(id: &TabId) -> String {
    match id {
        TabId::Panel { module, panel } => format!("{module}/{panel}"),
        TabId::Page { page } => format!("hideout/{}", page.replace('.', "-")),
    }
}

/// A screen's EDID id: the second `#` part of its display path (`TST0001`), if it has one.
fn edid(path: &str) -> Option<&str> {
    path.split('#').nth(1).filter(|s| !s.is_empty())
}

/// A screen's id in a place: its display path, or the device name when the path is unknown.
pub fn screen_id(m: &Monitor) -> &str {
    if m.path.is_empty() {
        &m.key
    } else {
        &m.path
    }
}

/// The connected screen a saved `screen` names: the same path (any case); else, when exactly one
/// connected screen has the same EDID id, that one (the same monitor on another port).
pub fn find_screen<'a>(saved: &str, monitors: &'a [Monitor]) -> Option<&'a Monitor> {
    if let Some(m) = monitors
        .iter()
        .find(|m| screen_id(m).eq_ignore_ascii_case(saved))
    {
        return Some(m);
    }
    let id = edid(saved)?;
    let mut same = monitors
        .iter()
        .filter(|m| edid(&m.path).is_some_and(|e| e.eq_ignore_ascii_case(id)));
    match (same.next(), same.next()) {
        (Some(m), None) => Some(m),
        _ => None,
    }
}

/// The screen layout's id: the first 16 lowercase hex digits of the SHA-256 of one line per
/// screen, `<path> <x> <y> <width> <height> <dpi>` (path lowercased, physical pixels, effective
/// DPI), sorted, joined by `\n` (§2.5).
pub fn layout_id(monitors: &[Monitor]) -> String {
    let mut lines: Vec<String> = monitors
        .iter()
        .map(|m| {
            format!(
                "{} {} {} {} {} {}",
                screen_id(m).to_lowercase(),
                m.area.x,
                m.area.y,
                m.area.w,
                m.area.h,
                (m.scale * 96.0).round() as u32
            )
        })
        .collect();
    lines.sort();
    let digest = Sha256::digest(lines.join("\n").as_bytes());
    digest.iter().take(8).map(|b| format!("{b:02x}")).collect()
}

/// Reads the `[windows]` table out of `hideout.toml`'s other keys. Each page is checked on its
/// own: a bad one is dropped (it opens at the fallback place) and named in the returned errors,
/// never the rest (`config.windows`, §2.5).
pub fn load(rest: &toml::Table) -> (BTreeMap<String, PageWindow>, Vec<String>) {
    let mut out = BTreeMap::new();
    let mut errors = Vec::new();
    let Some(table) = rest.get("windows") else {
        return (out, errors);
    };
    let Some(table) = table.as_table() else {
        errors.push("config.windows: [windows] is not a table".into());
        return (out, errors);
    };
    for (key, value) in table {
        let page: Result<PageWindow, _> = value.clone().try_into();
        match page {
            Ok(p) => match Windows::validate_page(key, &p) {
                Ok(()) => {
                    out.insert(key.clone(), p);
                }
                Err(e) => errors.push(format!("config.windows: {}", e.message)),
            },
            Err(e) => errors.push(format!("config.windows: windows.{key:?}: {e}")),
        }
    }
    (out, errors)
}

/// Writes `windows` back into `hideout.toml`'s other keys (`[windows]`; none when empty).
pub fn store(rest: &mut toml::Table, windows: &BTreeMap<String, PageWindow>) {
    if windows.is_empty() {
        rest.remove("windows");
        return;
    }
    match toml::Value::try_from(windows) {
        Ok(v) => {
            rest.insert("windows".into(), v);
        }
        Err(e) => log::warn!("[windows] not saved: {e}"),
    }
}

/// Where a pop-out opens: a rectangle and the state to show it in.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placed {
    pub rect: Rect,
    pub state: WindowState,
}

fn px(v: f64, scale: f32) -> i32 {
    (v * scale as f64).round() as i32
}

fn dip(v: i32, scale: f32) -> f64 {
    let s = if scale > 0.0 { scale as f64 } else { 1.0 };
    v as f64 / s
}

/// A place on its screen, in physical pixels, at least [`MIN`], its title row on a screen.
fn on_screen(p: &Place, m: &Monitor, monitors: &[Monitor]) -> Rect {
    let w = p.width.max(MIN.0);
    let h = p.height.max(MIN.1);
    let r = Rect {
        x: m.area.x + px(p.x, m.scale),
        y: m.area.y + px(p.y, m.scale),
        w: px(w, m.scale),
        h: px(h, m.scale),
    };
    let title = Rect {
        h: px(TITLE_ROW, m.scale),
        ..r
    };
    let seen = monitors.iter().any(|o| overlaps(title, o.work));
    if seen {
        r
    } else {
        fit(r, m)
    }
}

fn overlaps(a: Rect, b: Rect) -> bool {
    // The title row is "on a screen" when a usable part of it is (a grab handle).
    let (x0, x1) = (a.x.max(b.x), (a.x + a.w).min(b.x + b.w));
    let (y0, y1) = (a.y.max(b.y), (a.y + a.h).min(b.y + b.h));
    x1 - x0 >= 48 && y1 - y0 >= a.h.min(16)
}

/// Shrinks `r` into the monitor's work area and moves it fully inside.
fn fit(r: Rect, m: &Monitor) -> Rect {
    let w = r.w.min(m.work.w).max(1);
    let h = r.h.min(m.work.h).max(1);
    Rect {
        x: r.x.clamp(m.work.x, m.work.x + m.work.w - w),
        y: r.y.clamp(m.work.y, m.work.y + m.work.h - h),
        w,
        h,
    }
}

/// Where page `page`'s pop-out opens in the layout `layout` (§2.5): the place for this layout;
/// else the newest place on a screen connected now; else Hideout's screen (`anchor`, the main
/// window's rectangle, or the primary screen), offset 32 from Hideout, at the newest place's size
/// or `first` (device-independent), clamped fully on screen. `taken` are the other pop-outs'
/// rectangles: a fallback that would cover one exactly moves on by another 32.
pub fn restore(
    page: Option<&PageWindow>,
    monitors: &[Monitor],
    layout: &str,
    anchor: Option<Rect>,
    first: (f64, f64),
    taken: &[Rect],
) -> Option<Placed> {
    let places: &[Place] = page.map(|p| p.places.as_slice()).unwrap_or(&[]);
    let exact = places
        .iter()
        .find(|p| p.layout == layout)
        .and_then(|p| find_screen(&p.screen, monitors).map(|m| (p, m)));
    let newest = || {
        let mut on: Vec<(&Place, &Monitor)> = places
            .iter()
            .filter_map(|p| find_screen(&p.screen, monitors).map(|m| (p, m)))
            .collect();
        on.sort_by(|a, b| b.0.saved.cmp(&a.0.saved));
        on.into_iter().next()
    };
    if let Some((p, m)) = exact.or_else(newest) {
        return Some(Placed {
            rect: on_screen(p, m, monitors),
            state: p.state,
        });
    }
    let m = anchor
        .and_then(|a| monitor_for(a, monitors))
        .or_else(|| monitors.iter().find(|m| m.primary))
        .or(monitors.first())?;
    let size = places
        .iter()
        .max_by(|a, b| a.saved.cmp(&b.saved))
        .map(|p| (p.width, p.height))
        .unwrap_or((first.0.max(FIRST_MIN.0), first.1.max(FIRST_MIN.1)));
    let step = px(OFFSET, m.scale);
    let (mut x, mut y) = match anchor {
        Some(a) => (a.x + step, a.y + step),
        None => (m.work.x + step, m.work.y + step),
    };
    let w = px(size.0.max(MIN.0), m.scale);
    let h = px(size.1.max(MIN.1), m.scale);
    let mut r = fit(Rect { x, y, w, h }, m);
    for _ in 0..16 {
        if !taken.iter().any(|t| t.x == r.x && t.y == r.y) {
            break;
        }
        x += step;
        y += step;
        r = fit(Rect { x, y, w, h }, m);
    }
    Some(Placed {
        rect: r,
        state: WindowState::Normal,
    })
}

/// How far (device-independent) right of the window's left edge the pointer sits when a torn-off
/// page opens under it: on the title row's page title, not on its buttons.
pub const GRAB_X: f64 = 120.0;

/// A rectangle's size in device-independent units, on the screen holding its centre.
pub fn dip_size(r: Rect, monitors: &[Monitor]) -> (f64, f64) {
    let scale = monitor_for(r, monitors).map_or(1.0, |m| m.scale);
    (dip(r.w, scale), dip(r.h, scale))
}

/// Where a page dragged off the strip opens (#1054.4; the plan: "Letting go outside Hideout
/// opens the window there, its title row under the pointer"): `size` (device-independent, at
/// least [`MIN`]) on the screen under `pointer`, the pointer [`GRAB_X`] in (at most half the
/// width) and halfway down the title row, which sits `client_top` units below the window's top
/// (Windows' caption). Pulled fully into that screen's work area when it would hang off it.
pub fn at_pointer(
    size: (f64, f64),
    pointer: (i32, i32),
    client_top: f64,
    monitors: &[Monitor],
) -> Option<Rect> {
    let dot = Rect {
        x: pointer.0,
        y: pointer.1,
        w: 1,
        h: 1,
    };
    let m = monitor_for(dot, monitors)
        .or_else(|| monitors.iter().find(|m| m.primary))
        .or(monitors.first())?;
    let w = px(size.0.max(MIN.0), m.scale);
    let h = px(size.1.max(MIN.1), m.scale);
    let grab_x = px(GRAB_X, m.scale).min(w / 2);
    let grab_y = px(client_top.max(0.0) + TITLE_ROW / 2.0, m.scale);
    Some(fit(
        Rect {
            x: pointer.0 - grab_x,
            y: pointer.1 - grab_y,
            w,
            h,
        },
        m,
    ))
}

/// Records where the window is for the layout `layout` (§2.5): `normal` is its restored
/// rectangle (the snapped one when snapped), on the screen holding its centre. One place per
/// layout; a fifth layout drops the place saved longest ago. `now` is `YYYY-MM-DDTHH:MM:SSZ`.
pub fn remember(
    page: &mut PageWindow,
    monitors: &[Monitor],
    layout: &str,
    normal: Rect,
    state: WindowState,
    now: &str,
) {
    let Some(m) = monitor_for(normal, monitors) else {
        return;
    };
    let place = Place {
        layout: layout.to_string(),
        screen: screen_id(m).to_string(),
        x: dip(normal.x - m.area.x, m.scale),
        y: dip(normal.y - m.area.y, m.scale),
        width: dip(normal.w, m.scale).max(1.0),
        height: dip(normal.h, m.scale).max(1.0),
        state,
        saved: now.to_string(),
    };
    match page.places.iter_mut().find(|p| p.layout == layout) {
        Some(p) => *p = place,
        None => page.places.push(place),
    }
    while page.places.len() > PLACES_PER_PAGE {
        let oldest = page
            .places
            .iter()
            .enumerate()
            .min_by(|a, b| a.1.saved.cmp(&b.1.saved))
            .map(|(i, _)| i)
            .unwrap_or(0);
        page.places.remove(oldest);
    }
}

/// `YYYY-MM-DDTHH:MM:SSZ` for a time in seconds since 1970 (UTC; civil-from-days).
pub fn utc(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

/// Now, as [`utc`].
pub fn now_utc() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    utc(secs)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PATH_A: &str =
        r"\\?\DISPLAY#TST0001#5&1a2b3c&0&UID1#{e6f07b5f-ee97-4a90-b076-33f57bf4eaa7}";
    const PATH_B: &str =
        r"\\?\DISPLAY#TST0002#5&1a2b3c&0&UID2#{e6f07b5f-ee97-4a90-b076-33f57bf4eaa7}";

    fn mon(path: &str, x: i32, w: i32, h: i32, scale: f32, primary: bool) -> Monitor {
        Monitor {
            key: format!(r"\\.\DISPLAY{}", if primary { 1 } else { 2 }),
            path: path.into(),
            area: Rect { x, y: 0, w, h },
            work: Rect {
                x,
                y: 0,
                w,
                h: h - (40.0 * scale) as i32,
            },
            scale,
            primary,
        }
    }

    fn two() -> Vec<Monitor> {
        vec![
            mon(PATH_A, 0, 2560, 1440, 1.0, true),
            mon(PATH_B, 2560, 3840, 2160, 1.5, false),
        ]
    }

    #[test]
    fn keys_follow_the_contract_form() {
        let ok = |k: &str| Windows::validate_page(k, &PageWindow::default()).is_ok();
        let chat = page_key(&TabId::panel("example.chat", "chat"));
        assert_eq!(chat, "example.chat/chat");
        assert!(ok(&chat));
        let hub = page_key(&TabId::page("hub.modules"));
        assert_eq!(hub, "hideout/hub-modules");
        assert!(ok(&hub));
    }

    #[test]
    fn the_layout_id_is_sixteen_hex_digits_and_order_free() {
        let id = layout_id(&two());
        assert_eq!(id.len(), 16);
        assert!(id
            .bytes()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
        let mut rev = two();
        rev.reverse();
        assert_eq!(layout_id(&rev), id);
        let mut moved = two();
        moved[1].area.x = -3840;
        assert_ne!(layout_id(&moved), id);
        let mut rescaled = two();
        rescaled[0].scale = 1.25;
        assert_ne!(layout_id(&rescaled), id);
        // The path is compared without case.
        let mut upper = two();
        upper[0].path = PATH_A.to_uppercase();
        assert_eq!(layout_id(&upper), id);
    }

    #[test]
    fn the_layout_id_matches_the_contracts_recipe() {
        // One screen: the line is `<path> <x> <y> <w> <h> <dpi>`, hashed with SHA-256.
        let one = vec![mon(PATH_A, 0, 1920, 1080, 1.0, true)];
        let line = format!("{} 0 0 1920 1080 96", PATH_A.to_lowercase());
        let want: String = Sha256::digest(line.as_bytes())
            .iter()
            .take(8)
            .map(|b| format!("{b:02x}"))
            .collect();
        assert_eq!(layout_id(&one), want);
    }

    #[test]
    fn a_screen_is_known_by_path_then_by_its_only_edid_match() {
        let ms = two();
        assert_eq!(
            find_screen(&PATH_A.to_lowercase(), &ms).unwrap().path,
            PATH_A
        );
        // The same monitor on another port: another UID, the same EDID id.
        let other_port = PATH_B.replace("UID2", "UID9");
        assert_eq!(find_screen(&other_port, &ms).unwrap().path, PATH_B);
        // Two connected screens with that EDID id: neither counts.
        let mut twins = two();
        twins[0].path = PATH_B.replace("UID2", "UID3");
        assert!(find_screen(&other_port, &twins).is_none());
        assert!(find_screen("", &ms).is_none());
    }

    fn page_with(places: Vec<Place>) -> PageWindow {
        PageWindow {
            popped_out: true,
            keep_on_top: false,
            places,
        }
    }

    #[test]
    fn a_place_round_trips_per_layout_in_device_independent_units() {
        let ms = two();
        let layout = layout_id(&ms);
        let mut page = PageWindow::default();
        // On B (150 %): 300 × 200 px from its corner is 200 × 133.3 units.
        let r = Rect {
            x: 2560 + 300,
            y: 200,
            w: 960,
            h: 720,
        };
        remember(
            &mut page,
            &ms,
            &layout,
            r,
            WindowState::Normal,
            "2026-11-10T10:00:00Z",
        );
        let p = &page.places[0];
        assert_eq!(
            (p.screen.as_str(), p.x, p.width, p.height),
            (PATH_B, 200.0, 640.0, 480.0)
        );
        Windows::validate_page("example.chat/chat", &page).unwrap();
        let back = restore(Some(&page), &ms, &layout, None, (0.0, 0.0), &[]).unwrap();
        assert_eq!(back.rect, r);
        assert_eq!(back.state, WindowState::Normal);
        // The same page in the toml form and back.
        let mut rest = toml::Table::new();
        let mut map = BTreeMap::new();
        map.insert("example.chat/chat".to_string(), page.clone());
        store(&mut rest, &map);
        let text = toml::to_string(&rest).unwrap();
        assert!(text.contains("[windows.\"example.chat/chat\"]"), "{text}");
        let (read, errs) = load(&toml::from_str(&text).unwrap());
        assert!(errs.is_empty(), "{errs:?}");
        assert_eq!(read["example.chat/chat"], page);
    }

    #[test]
    fn a_fifth_layout_drops_the_oldest_place() {
        let ms = two();
        let mut page = PageWindow::default();
        let r = Rect {
            x: 10,
            y: 10,
            w: 500,
            h: 400,
        };
        for (i, l) in [
            "0000000000000001",
            "0000000000000002",
            "0000000000000003",
            "0000000000000004",
        ]
        .iter()
        .enumerate()
        {
            remember(
                &mut page,
                &ms,
                l,
                r,
                WindowState::Normal,
                &utc(1_000 + i as u64),
            );
        }
        // Saving layout 1 again refreshes it; then a fifth drops layout 2, the oldest now.
        remember(
            &mut page,
            &ms,
            "0000000000000001",
            r,
            WindowState::Normal,
            &utc(2_000),
        );
        assert_eq!(page.places.len(), 4);
        remember(
            &mut page,
            &ms,
            "0000000000000005",
            r,
            WindowState::Normal,
            &utc(3_000),
        );
        let layouts: Vec<&str> = page.places.iter().map(|p| p.layout.as_str()).collect();
        assert_eq!(layouts.len(), 4);
        assert!(!layouts.contains(&"0000000000000002"), "{layouts:?}");
        assert!(layouts.contains(&"0000000000000001"));
        Windows::validate_page("example.chat/chat", &page).unwrap();
    }

    #[test]
    fn another_layout_uses_the_newest_place_on_a_connected_screen() {
        let ms = two();
        let page = page_with(vec![
            Place {
                layout: "00000000000000aa".into(),
                screen: PATH_A.into(),
                x: 100.0,
                y: 100.0,
                width: 700.0,
                height: 500.0,
                state: WindowState::Maximized,
                saved: "2026-11-01T00:00:00Z".into(),
            },
            Place {
                layout: "00000000000000bb".into(),
                screen: r"\\?\DISPLAY#GONE001#x#{y}".into(),
                x: 0.0,
                y: 0.0,
                width: 900.0,
                height: 600.0,
                state: WindowState::Normal,
                saved: "2026-11-09T00:00:00Z".into(),
            },
        ]);
        let got = restore(Some(&page), &ms, &layout_id(&ms), None, (0.0, 0.0), &[]).unwrap();
        assert_eq!(
            got.rect,
            Rect {
                x: 100,
                y: 100,
                w: 700,
                h: 500
            }
        );
        assert_eq!(got.state, WindowState::Maximized);
    }

    #[test]
    fn a_gone_screen_falls_back_to_hideouts_screen_fully_on_screen() {
        let ms = two();
        let page = page_with(vec![Place {
            layout: "00000000000000bb".into(),
            screen: r"\\?\DISPLAY#GONE001#x#{y}".into(),
            x: 0.0,
            y: 0.0,
            width: 900.0,
            height: 600.0,
            state: WindowState::Normal,
            saved: "2026-11-09T00:00:00Z".into(),
        }]);
        // Hideout is on B, near its bottom-right corner: offset 32 units (48 px), the same size
        // in units (1350 × 900 px), pulled inside B's work area.
        let hub = Rect {
            x: 2560 + 3000,
            y: 1500,
            w: 800,
            h: 600,
        };
        let got = restore(
            Some(&page),
            &ms,
            &layout_id(&ms),
            Some(hub),
            (0.0, 0.0),
            &[],
        )
        .unwrap();
        let b = &ms[1];
        assert_eq!((got.rect.w, got.rect.h), (1350, 900));
        assert!(got.rect.x >= b.work.x && got.rect.x + got.rect.w <= b.work.x + b.work.w);
        assert!(got.rect.y >= b.work.y && got.rect.y + got.rect.h <= b.work.y + b.work.h);
        assert_eq!(got.state, WindowState::Normal);
    }

    #[test]
    fn a_first_pop_out_opens_next_to_hideout_and_cascades() {
        let ms = two();
        let hub = Rect {
            x: 200,
            y: 100,
            w: 1200,
            h: 800,
        };
        let first = restore(None, &ms, &layout_id(&ms), Some(hub), (300.0, 900.0), &[]).unwrap();
        assert_eq!(
            first.rect,
            Rect {
                x: 232,
                y: 132,
                w: 480,
                h: 900
            }
        );
        let second = restore(
            None,
            &ms,
            &layout_id(&ms),
            Some(hub),
            (300.0, 900.0),
            &[first.rect],
        )
        .unwrap();
        assert_eq!((second.rect.x, second.rect.y), (264, 164));
    }

    #[test]
    fn a_saved_place_off_every_screen_is_pulled_back() {
        let ms = two();
        let page = page_with(vec![Place {
            layout: layout_id(&ms),
            screen: PATH_A.into(),
            x: -5000.0,
            y: 3000.0,
            width: 200.0,
            height: 100.0,
            state: WindowState::Normal,
            saved: "2026-11-09T00:00:00Z".into(),
        }]);
        let got = restore(Some(&page), &ms, &layout_id(&ms), None, (0.0, 0.0), &[]).unwrap();
        // At least 360 × 240, inside A's work area.
        assert_eq!((got.rect.x, got.rect.w, got.rect.h), (0, 360, 240));
        assert!(got.rect.y + got.rect.h <= ms[0].work.h);
    }

    #[test]
    fn a_torn_off_page_opens_with_its_title_row_under_the_pointer() {
        let ms = two();
        // On A (100 %): 640 × 480, the pointer 120 in and halfway down the title row below a
        // 31-unit caption.
        let r = at_pointer((640.0, 480.0), (1000, 400), 31.0, &ms).unwrap();
        assert_eq!(
            r,
            Rect {
                x: 880,
                y: 351,
                w: 640,
                h: 480
            }
        );
        // On B (150 %): the same units, in B's pixels.
        let r = at_pointer((640.0, 480.0), (4000, 600), 31.0, &ms).unwrap();
        assert_eq!((r.x, r.y, r.w, r.h), (4000 - 180, 600 - 74, 960, 720));
        // Small: at least the minimum, and the pointer at most half the width in.
        let r = at_pointer((100.0, 100.0), (1000, 400), 0.0, &ms).unwrap();
        assert_eq!((r.x, r.w, r.h), (1000 - 120, 360, 240));
        // Near a screen's corner: pulled fully onto that screen's work area.
        let r = at_pointer((640.0, 480.0), (2550, 1390), 31.0, &ms).unwrap();
        assert_eq!((r.x + r.w, r.y + r.h), (2560, ms[0].work.h));
        assert!(at_pointer((640.0, 480.0), (0, 0), 31.0, &[]).is_none());
        // A size read back from a window on B is in units again.
        assert_eq!(
            dip_size(
                Rect {
                    x: 3000,
                    y: 100,
                    w: 960,
                    h: 720
                },
                &ms
            ),
            (640.0, 480.0)
        );
    }

    #[test]
    fn a_bad_entry_is_dropped_and_the_rest_kept() {
        let text = r#"
theme = "dark"
[windows."example.chat/chat"]
popped_out = true
keep_on_top = true
[windows."not a key"]
popped_out = true
[windows."example.art/art"]
popped_out = true
colour = "red"
"#;
        let table: toml::Table = toml::from_str(text).unwrap();
        let (w, errs) = load(&table);
        assert_eq!(w.len(), 1);
        assert!(w["example.chat/chat"].keep_on_top);
        assert_eq!(errs.len(), 2, "{errs:?}");
        assert!(errs.iter().all(|e| e.starts_with("config.windows")));
    }

    #[test]
    fn utc_times_are_in_the_contract_form() {
        assert_eq!(utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(utc(1_794_225_600), "2026-11-09T12:00:00Z");
        assert_eq!(utc(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(now_utc().len(), 20);
    }
}
