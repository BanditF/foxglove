//! `scene.start` and `lights.status` (API 1.18, #1377): the scene and the lights for callers that
//! have no window, such as agents through the hub's alias pipe.
//!
//! What a caller sees (the PR body repeats it):
//! - `scene.start {look, params?}` needs `lighting.write` (the table's permission; the router
//!   checks it before the module is asked). It starts the look as the Scenes page's Start does
//!   (the look becomes the chosen one, a running scene takes it) and answers `{look, scene}`.
//!   A parameter left out takes Foxglove's default for that look.
//! - A look name or a parameter that is unknown, out of range, the wrong type or one the look
//!   does not use is -32602 with the reason; nothing is clamped silently. A look Foxglove must
//!   not start now (Screen glow without the user's yes, Temperature without its sensors,
//!   Foxglove paused) is BUSY (3) with the page's own words. Starts closer than
//!   [`MIN_START_GAP`] are RATE_LIMITED (8). Only an unknown *method* is -32601.
//! - `lights.status {}` (`devices.read`) answers the running look, the keyboard's lights and,
//!   for Temperature, the reading's state and the colour the lights were last told to show.
//! - Same user only: the pipe is the hub's, the caller's grants are the router's. Replies carry
//!   no paths, tokens or secrets: states, names and the reading.
//!
//! The parameters are parsed by [`parse_start`], a pure function (no state, no device), so the
//! tests need no window and no keyboard.
use std::sync::Mutex;
use std::time::{Duration, Instant};

use foxglove_plugin_api::methods::{
    self, LightsStatus, SceneStartLook, SceneStartParams, SceneStarted, TemperatureStatus,
};
use foxglove_plugin_api::rpc::{code, Error};
use serde_json::{json, Value};

use super::scene_looks::{self as looks, Background, Direction, Kind, Look};
use super::temperature::{Colours, RANGE_C};
use super::temperature_feed::Status;

/// The least time between two `scene.start` calls (all callers together): a looping caller
/// cannot churn the keyboard's effect messages.
pub const MIN_START_GAP: Duration = Duration::from_secs(2);
/// The longest `scene.start` params text, bytes. Real calls are under 300.
pub const MAX_PARAMS_BYTES: usize = 1024;
const MAX_NAME: usize = 24;
const MAX_PALETTE: usize = 40;

fn bad(message: impl Into<String>) -> Error {
    Error {
        code: code::INVALID_PARAMS,
        message: message.into(),
        data: None,
    }
}

/// The look a `scene.start` asks for, or the reason it is refused. Pure.
pub fn parse_start(raw: Value) -> Result<Look, Error> {
    parse_start_on(raw, &Look::default())
}

/// A look `scene.start` must never start, whatever the user allowed in the app: the ones that
/// read the screen. Screen glow's "Allow and start" is the user's click in Foxglove; a caller
/// of the API has no such click.
pub fn api_refusal(kind: Kind) -> Option<String> {
    kind.reads_screen()
        .then(|| "Screen glow can only be started by the user in Foxglove".to_string())
}

/// As [`parse_start`], with the settings the user last saved for the same kind as the base: what
/// the caller leaves out stays as the user tuned it (another kind starts from its defaults).
pub fn parse_start_on(raw: Value, base: &Look) -> Result<Look, Error> {
    // Bounded before anything is looked at.
    if raw.to_string().len() > MAX_PARAMS_BYTES {
        return Err(bad(format!(
            "scene.start params are over {MAX_PARAMS_BYTES} bytes"
        )));
    }
    // serde would also read a JSON array as the struct's fields in order.
    if !raw.is_object()
        || raw
            .get("params")
            .is_some_and(|p| !(p.is_object() || p.is_null()))
    {
        return Err(bad("scene.start takes an object {look, params?}"));
    }
    let p: SceneStartParams = serde_json::from_value(raw).map_err(|e| bad(e.to_string()))?;
    if p.look.len() > MAX_NAME {
        return Err(bad("unknown look"));
    }
    let kind: Kind = serde_json::from_value(json!(p.look))
        .map_err(|_| bad(format!("unknown look {:?}; known: {}", p.look, kinds())))?;
    let extra = p.params.unwrap_or_default();
    apply(kind, &extra, base)
}

/// The kinds a caller may name, for the error text.
fn kinds() -> String {
    Kind::ALL
        .iter()
        .filter(|k| !k.reads_screen())
        .map(|k| kind_name(*k))
        .collect::<Vec<_>>()
        .join(", ")
}

/// A kind's wire name (its serde name).
pub fn kind_name(k: Kind) -> String {
    serde_json::to_value(k)
        .ok()
        .and_then(|v| v.as_str().map(String::from))
        .unwrap_or_default()
}

fn apply(kind: Kind, p: &SceneStartLook, base: &Look) -> Result<Look, Error> {
    let mut look = if base.kind == kind {
        base.clone()
    } else {
        Look {
            kind,
            ..Look::default()
        }
    };
    let must = |used: bool, name: &str| -> Result<(), Error> {
        if used {
            Ok(())
        } else {
            Err(bad(format!(
                "{name} is not a parameter of {}",
                kind_name(kind)
            )))
        }
    };
    if let Some(name) = &p.palette {
        must(kind.uses_palette(), "palette")?;
        if name.len() > MAX_PALETTE || name == looks::WALLPAPER || !looks::is_foxglove_palette(name)
        {
            return Err(bad("palette is not one of Foxglove's own palettes"));
        }
        look.palette = name.clone();
    }
    if let Some(s) = p.speed {
        must(kind.uses_speed(), "speed")?;
        if !looks::SPEEDS.contains(&s) {
            return Err(bad("speed is 1 to 10"));
        }
        look.speed = s;
    }
    if let Some(d) = &p.direction {
        must(kind.uses_direction(), "direction")?;
        look.direction = match d.as_str() {
            "down" => Direction::Down,
            "up" => Direction::Up,
            "right" => Direction::Right,
            "left" => Direction::Left,
            _ => return Err(bad("direction is down, up, right or left")),
        };
    }
    if let Some(w) = p.width_mm {
        must(kind.uses_width(), "width_mm")?;
        if !looks::WIDTHS.contains(&w) {
            return Err(bad("width_mm is 20 to 400"));
        }
        look.width_mm = w;
    }
    if let Some(c) = &p.colour {
        must(kind.uses_colour(), "colour")?;
        look.colour = looks::hex(c).map_err(|_| bad("colour is #rrggbb"))?;
    }
    if let Some(b) = &p.background {
        must(kind.uses_colour(), "background")?;
        look.background = match b.as_str() {
            "night" => Background::Night,
            "off" => Background::Off,
            _ => return Err(bad("background is night or off")),
        };
    }
    if let Some(s) = &p.source {
        must(kind.reads_temperature(), "source")?;
        let ok = !s.is_empty()
            && s.len() <= MAX_NAME
            && s.bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-');
        if !ok {
            return Err(bad(
                "source is lower case letters, digits, _ or -, up to 24",
            ));
        }
        look.temp_source = s.clone();
    }
    if let Some(c) = &p.colours {
        must(kind.reads_temperature(), "colours")?;
        look.temp_colours = match c.as_str() {
            "palette" => Colours::Palette,
            "cool-to-hot" => Colours::CoolToHot,
            _ => return Err(bad("colours is palette or cool-to-hot")),
        };
    }
    for (name, v, slot) in [
        ("cool_c", p.cool_c, &mut look.cool_c),
        ("hot_c", p.hot_c, &mut look.hot_c),
    ] {
        if let Some(v) = v {
            must(kind.reads_temperature(), name)?;
            if !RANGE_C.contains(&v) || v % super::temperature::STEP_C != 0 {
                return Err(bad(format!("{name} is 20 to 100 in steps of 5")));
            }
            *slot = v;
        }
    }
    // The ends must keep their gap; refuse rather than move one silently.
    if look.kind.reads_temperature() {
        let ends = super::temperature::clamp_ends(look.cool_c, look.hot_c);
        if ends != (look.cool_c, look.hot_c) {
            return Err(bad("hot_c must be at least 10 above cool_c"));
        }
    }
    Ok(look)
}

/// The start rate limit: at most one start per [`MIN_START_GAP`].
#[derive(Debug, Default)]
pub struct Limiter {
    last: Option<Instant>,
}

impl Limiter {
    /// Takes a start at `now`, or says how long to wait (a refused take moves nothing).
    pub fn take(&mut self, now: Instant) -> Result<(), Duration> {
        if let Some(l) = self.last {
            let since = now.saturating_duration_since(l);
            if since < MIN_START_GAP && now >= l {
                return Err(MIN_START_GAP - since);
            }
        }
        self.last = Some(now);
        Ok(())
    }
}

static LIMIT: Mutex<Limiter> = Mutex::new(Limiter { last: None });

/// `scene.start`: parse, refuse what must not start, rate limit, start.
pub fn start(raw: Value) -> Result<Value, Error> {
    let look = parse_start_on(raw, &looks::load_chosen().look)?;
    if let Some(why) = api_refusal(look.kind) {
        return Err(Error {
            code: code::BUSY,
            message: why,
            data: None,
        });
    }
    if super::pause::paused() {
        return Err(Error {
            code: code::BUSY,
            message: super::pause::REFUSED.into(),
            data: None,
        });
    }
    if let Some(why) = super::scenes_ui::refusal(&look) {
        return Err(Error {
            code: code::BUSY,
            message: why,
            data: None,
        });
    }
    // Taken only for a start that will run, so a refused one costs the caller nothing.
    let waited = LIMIT
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take(Instant::now());
    if let Err(wait) = waited {
        return Err(Error {
            code: code::RATE_LIMITED,
            message: format!(
                "scene.start is limited to one start per {} s; try again in {} s",
                MIN_START_GAP.as_secs(),
                wait.as_secs() + 1
            ),
            data: None,
        });
    }
    let name = kind_name(look.kind);
    eprintln!("Foxglove: scene.start {name} (API)");
    super::scenes_ui::start_look(look);
    Ok(json!(SceneStarted {
        look: name,
        scene: super::scene_run::running().map(|(n, _)| n),
    }))
}

/// The wire word for a reading's state.
pub fn reading_state(s: Status) -> &'static str {
    match s {
        Status::HubLost => "no-hub",
        Status::NotAllowed => "not-allowed",
        Status::AppOff => "app-off",
        Status::AppStopped => "app-stopped",
        Status::Reading => "waiting",
        Status::Working { .. } => "ok",
        Status::Missing => "missing",
        Status::NoReport => "no-report",
    }
}

/// `#RRGGBB` for a colour.
fn hex_of(c: [u8; 3]) -> String {
    format!("#{:02X}{:02X}{:02X}", c[0], c[1], c[2])
}

/// The Temperature part of `lights.status`, from what the feed and the output last said. Pure.
pub fn temperature_part(
    source: &str,
    feed: Status,
    last: Option<super::temperature_out::LastTarget>,
) -> TemperatureStatus {
    TemperatureStatus {
        source: source.to_string(),
        reading_state: reading_state(feed).to_string(),
        // Only a working reading is a number: never 0 for a missing one.
        temp_c: feed.reading(),
        target: last.and_then(|l| l.colour).map(hex_of),
        fallback: last.is_none_or(|l| l.colour.is_none()),
    }
}

/// `lights.status`: from `scene.status`'s words and the Temperature look's own state.
pub fn lights_status(scene: &methods::SceneStatus) -> LightsStatus {
    let running = scene.scene.is_some();
    let temp = running && super::scenes_ui::temperature_live();
    let chosen = looks::load_chosen().look;
    LightsStatus {
        state: scene.state.clone(),
        scene: scene.scene.clone(),
        look: running.then(|| kind_name(if temp { Kind::Temperature } else { chosen.kind })),
        keyboard: scene.keyboard.clone(),
        temperature: temp.then(|| {
            temperature_part(
                &chosen.temp_source,
                super::temperature_feed::latest(),
                super::temperature_out::last_target(),
            )
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::foxglove::temperature_out::LastTarget;

    fn code_of(r: Result<Look, Error>) -> i32 {
        r.unwrap_err().code
    }

    #[test]
    fn a_plain_look_name_starts_that_look_with_the_defaults() {
        let l = parse_start(json!({"look": "temperature"})).unwrap();
        assert_eq!(l.kind, Kind::Temperature);
        assert_eq!((l.cool_c, l.hot_c), (40, 80));
        let l = parse_start(json!({"look": "two-tone", "params": {}})).unwrap();
        assert_eq!(l.kind, Kind::TwoTone);
    }

    #[test]
    fn temperature_params_are_taken_exactly() {
        let l = parse_start(json!({"look": "temperature", "params": {
            "source": "gpu", "colours": "cool-to-hot", "cool_c": 30, "hot_c": 90,
            "palette": "Steel"}}))
        .unwrap();
        assert_eq!((l.cool_c, l.hot_c), (30, 90));
        assert_eq!(l.temp_colours, Colours::CoolToHot);
        assert_eq!(l.palette, "Steel");
    }

    #[test]
    fn scanline_takes_its_own_params() {
        let l = parse_start(json!({"look": "scanline", "params": {
            "speed": 7, "direction": "left", "width_mm": 120, "colour": "#FF8800",
            "background": "off"}}))
        .unwrap();
        assert_eq!((l.speed, l.width_mm, l.colour), (7, 120, [255, 136, 0]));
        assert_eq!(l.direction, Direction::Left);
        assert_eq!(l.background, Background::Off);
    }

    #[test]
    fn unknown_names_keys_and_types_are_refused_with_invalid_params() {
        for p in [
            json!({}),
            json!({"look": 5}),
            json!({"look": "nope"}),
            json!({"look": ""}),
            json!({"look": "x".repeat(500)}),
            json!({"look": "scanline", "extra": 1}),
            json!({"look": "scanline", "params": {"speed": "fast"}}),
            json!({"look": "scanline", "params": {"nope": 1}}),
            json!({"look": "scanline", "params": 5}),
            json!(["scanline"]),
            json!(null),
        ] {
            assert_eq!(code_of(parse_start(p.clone())), code::INVALID_PARAMS, "{p}");
        }
    }

    #[test]
    fn values_out_of_range_are_refused_never_clamped() {
        for p in [
            json!({"speed": 0}),
            json!({"speed": 11}),
            json!({"speed": 300}),
            json!({"width_mm": 19}),
            json!({"width_mm": 401}),
            json!({"colour": "red"}),
            json!({"colour": "#12345"}),
            json!({"direction": "sideways"}),
            json!({"background": "pink"}),
            json!({"palette": "Wallpaper"}),
            json!({"palette": "No such palette"}),
        ] {
            let r = parse_start(json!({"look": "scanline", "params": p}));
            assert_eq!(code_of(r), code::INVALID_PARAMS, "{p}");
        }
        for p in [
            json!({"cool_c": 19}),
            json!({"hot_c": 101}),
            json!({"cool_c": 90, "hot_c": 95}),
            json!({"cool_c": 80, "hot_c": 40}),
            json!({"source": "GPU"}),
            json!({"source": "gpu/../x"}),
            json!({"source": ""}),
            json!({"source": "a".repeat(25)}),
            json!({"colours": "rainbow"}),
        ] {
            let r = parse_start(json!({"look": "temperature", "params": p}));
            assert_eq!(code_of(r), code::INVALID_PARAMS, "{p}");
        }
    }

    #[test]
    fn a_param_the_look_does_not_use_is_refused() {
        for (look, p) in [
            ("scanline", json!({"cool_c": 40})),
            ("scanline", json!({"source": "gpu"})),
            ("temperature", json!({"speed": 3})),
            ("temperature", json!({"colour": "#ffffff"})),
            ("drift", json!({"direction": "up"})),
            ("breathe", json!({"palette": "Steel"})),
        ] {
            let r = parse_start(json!({"look": look, "params": p}));
            assert_eq!(code_of(r), code::INVALID_PARAMS, "{look} {p}");
        }
    }

    #[test]
    fn oversize_params_are_refused_before_parsing() {
        let r = parse_start(json!({"look": "scanline", "params": {"palette": "x".repeat(2000)}}));
        let e = r.unwrap_err();
        assert_eq!(e.code, code::INVALID_PARAMS);
        assert!(e.message.contains("bytes"), "{}", e.message);
    }

    #[test]
    fn screen_glow_is_always_refused_even_when_the_user_allowed_it_in_the_app() {
        let l = parse_start(json!({"look": "screen-glow"})).unwrap();
        assert_eq!(l.kind, Kind::ScreenGlow);
        // The page's own gate lets it through once the user said yes ...
        assert!(crate::foxglove::scenes_ui::start_allowed(l.kind, true));
        // ... the API's never does, and takes no such flag.
        assert!(api_refusal(l.kind).is_some());
        assert!(Kind::ALL
            .iter()
            .all(|k| api_refusal(*k).is_some() == k.reads_screen()));
        assert!(!kinds().contains("screen-glow"));
    }

    #[test]
    fn what_the_caller_leaves_out_stays_as_the_user_saved_it() {
        let saved = Look {
            kind: Kind::Scanline,
            speed: 7,
            ..Look::default()
        };
        let l = parse_start_on(
            json!({"look": "scanline", "params": {"width_mm": 60}}),
            &saved,
        )
        .unwrap();
        assert_eq!((l.speed, l.width_mm), (7, 60));
        let other = parse_start_on(json!({"look": "sweep"}), &saved).unwrap();
        assert_eq!(other.speed, Look::default().speed);
    }

    #[test]
    fn the_ends_step_by_five_like_the_page_and_a_plus_in_a_colour_is_refused() {
        for p in [json!({"cool_c": 33}), json!({"hot_c": 87})] {
            let r = parse_start(json!({"look": "temperature", "params": p}));
            assert_eq!(r.unwrap_err().code, code::INVALID_PARAMS);
        }
        let r = parse_start(json!({"look": "scanline", "params": {"colour": "#+1+2+3"}}));
        assert!(r.is_err());
    }

    #[test]
    fn starts_are_limited_to_one_per_gap_and_a_refused_start_is_free() {
        let mut lim = Limiter::default();
        let t = Instant::now();
        assert!(lim.take(t).is_ok());
        let wait = lim.take(t + Duration::from_millis(500)).unwrap_err();
        assert!(wait <= Duration::from_millis(1500));
        // A refused take did not move the window.
        assert!(lim.take(t + MIN_START_GAP).is_ok());
        assert!(lim
            .take(t + MIN_START_GAP + Duration::from_millis(1))
            .is_err());
    }

    #[test]
    fn lights_status_never_reports_a_missing_reading_as_zero() {
        let t = temperature_part("gpu", Status::Missing, None);
        assert_eq!(t.reading_state, "missing");
        assert_eq!(t.temp_c, None);
        assert!(t.fallback);
        assert_eq!(t.target, None);
        let t = temperature_part(
            "gpu",
            Status::Working {
                temp_c: 63.0,
                game: false,
            },
            Some(LastTarget {
                reading: Some(63.0),
                colour: Some([0xe8, 0x2d, 0x1f]),
            }),
        );
        assert_eq!(t.reading_state, "ok");
        assert_eq!(t.temp_c, Some(63.0));
        assert_eq!(t.target.as_deref(), Some("#E82D1F"));
        assert!(!t.fallback);
        // After the fallback took over, the stale colour is not reported.
        let t = temperature_part(
            "gpu",
            Status::Missing,
            Some(LastTarget {
                reading: None,
                colour: None,
            }),
        );
        assert!(t.fallback && t.target.is_none());
    }

    #[test]
    fn every_reading_state_has_its_own_word() {
        let all = [
            Status::HubLost,
            Status::NotAllowed,
            Status::AppOff,
            Status::AppStopped,
            Status::Reading,
            Status::Working {
                temp_c: 1.0,
                game: false,
            },
            Status::Missing,
            Status::NoReport,
        ];
        let mut words: Vec<_> = all.iter().map(|s| reading_state(*s)).collect();
        words.sort();
        words.dedup();
        assert_eq!(words.len(), all.len());
    }
}
