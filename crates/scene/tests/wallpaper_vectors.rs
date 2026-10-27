//! The shared test vectors (#493, #494): the same file is checked by bandit-wallpapers'
//! `tools/foxglove_sync_test.js` (its copy is `tools/scene_vectors.json`), so the Rust clock and
//! scanline and the wallpaper's JavaScript port give the same phase and band for the same inputs.

use foxglove_scene::effects::{band_centre, scanline_level};
use foxglove_scene::space::{along, extent};
use foxglove_scene::{Clock, Point, WallpaperScene};
use serde_json::Value;

const VECTORS: &str = include_str!("wallpaper_vectors.json");

fn f(v: &Value) -> f32 {
    v.as_f64().expect("a number") as f32
}

#[test]
fn rust_matches_the_shared_wallpaper_vectors() {
    let doc: Value = serde_json::from_str(VECTORS).unwrap();
    assert_eq!(doc["format"], "foxglove-scene-vectors");
    let cases = doc["cases"].as_array().unwrap();
    assert!(cases.len() >= 6);
    for case in cases {
        let name = case["name"].as_str().unwrap();
        let payload = case["payload"].as_str().unwrap();
        let s = WallpaperScene::decode(payload).unwrap_or_else(|| panic!("{name}: payload"));
        assert_eq!(s.encode(), payload, "{name}: encode(decode(payload))");
        let now = case["now_unix_ms"].as_i64().unwrap();
        let t = Clock::aligned(s.epoch_unix_ms).at(now);
        assert_eq!(t.ms(), case["scene_ms"].as_i64().unwrap(), "{name}");
        let phase = t.phase(s.period_ms);
        assert!(
            (phase - f(&case["phase"])).abs() < 1e-4,
            "{name}: phase {phase}"
        );
        let span = extent(&s.room, s.angle_deg);
        assert!(
            (span - f(&case["span_mm"])).abs() < 1e-3,
            "{name}: span {span}"
        );
        let centre = band_centre(t, s.period_ms, span, s.width_mm);
        assert!(
            (centre - f(&case["centre_mm"])).abs() < 1e-3,
            "{name}: centre {centre}"
        );
        for p in case["points"].as_array().unwrap() {
            let at = Point::new(f(&p["at"][0]), f(&p["at"][1]));
            let here = along(at, &s.room, s.angle_deg) * span;
            assert!(
                (here - f(&p["here_mm"])).abs() < 1e-3,
                "{name}: here {here} at {at:?}"
            );
            let level = scanline_level(at, t, &s.room, s.angle_deg, s.width_mm, s.period_ms);
            assert!(
                (level - f(&p["level"])).abs() < 1e-4,
                "{name}: level {level} at {at:?}"
            );
        }
    }
}
