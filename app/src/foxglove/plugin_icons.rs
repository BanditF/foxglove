//! Plugin icons for the Plugins tab (docs/plugin-icons.md): the first-party sets are built in,
//! a third-party plugin may ship `icon.svg`, `icon-light.svg` and `glyph.svg` at the top of its
//! folder, and anything else gets the `_default` set.

use std::borrow::Cow;
use std::path::Path;

use egui::ImageSource;

macro_rules! set {
    ($id:literal) => {
        (
            $id,
            include_bytes!(concat!("../../resources/plugins/", $id, "/icon.svg")).as_slice(),
            include_bytes!(concat!("../../resources/plugins/", $id, "/icon-light.svg")).as_slice(),
            include_bytes!(concat!("../../resources/plugins/", $id, "/glyph.svg")).as_slice(),
        )
    };
}

/// (id, tile on dark, tile on light, glyph).
type Set = (&'static str, &'static [u8], &'static [u8], &'static [u8]);

const BUILT_IN: &[Set] = &[
    set!("org.foxglove.focus-timer"),
    set!("org.foxglove.openrgb"),
    set!("org.foxglove.signalrgb"),
    set!("org.foxglove.icue"),
    set!("org.foxglove.lightsync"),
    set!("org.foxglove.mic-mute"),
    set!("_default"),
];

const EMPTY: &[u8] = include_bytes!("../../resources/plugins/empty.svg");

#[derive(Clone, Copy)]
enum Kind {
    Tile { dark: bool },
    Glyph,
}

fn built_in(id: &str, kind: Kind) -> Option<ImageSource<'static>> {
    let (id, dark_tile, light_tile, glyph) = BUILT_IN.iter().find(|s| s.0 == id)?;
    let (name, bytes) = match kind {
        Kind::Tile { dark: true } => ("icon", *dark_tile),
        Kind::Tile { dark: false } => ("icon-light", *light_tile),
        Kind::Glyph => ("glyph", *glyph),
    };
    Some(ImageSource::Bytes { uri: Cow::Owned(format!("bytes://plugins/{id}/{name}.svg")), bytes: bytes.into() })
}

/// A file the plugin ships, loaded through egui's file loader.
fn shipped(dir: &Path, names: &[&str]) -> Option<ImageSource<'static>> {
    let file = names.iter().map(|n| dir.join(n)).find(|p| p.is_file())?;
    Some(ImageSource::Uri(Cow::Owned(format!("file://{}", file.to_string_lossy().replace('\\', "/")))))
}

fn pick(id: &str, dir: Option<&Path>, kind: Kind) -> ImageSource<'static> {
    let files: &[&str] = match kind {
        Kind::Tile { dark: true } => &["icon.svg"],
        Kind::Tile { dark: false } => &["icon-light.svg", "icon.svg"],
        Kind::Glyph => &["glyph.svg"],
    };
    built_in(id, kind)
        .or_else(|| dir.and_then(|d| shipped(d, files)))
        .or_else(|| built_in("_default", kind))
        .expect("the default set is built in")
}

/// The colour tile for catalog cards and the install dialog, for the current theme.
pub fn tile(ui: &egui::Ui, id: &str, dir: Option<&Path>, size: f32) -> egui::Image<'static> {
    egui::Image::new(pick(id, dir, Kind::Tile { dark: ui.visuals().dark_mode })).fit_to_exact_size(egui::vec2(size, size))
}

/// The monochrome glyph, tinted with the text colour, for installed-plugin rows.
pub fn glyph(ui: &egui::Ui, id: &str, dir: Option<&Path>, size: f32) -> egui::Image<'static> {
    egui::Image::new(pick(id, dir, Kind::Glyph))
        .tint(ui.visuals().text_color())
        .fit_to_exact_size(egui::vec2(size, size))
}

/// The empty state's illustration, tinted like weak text.
pub fn empty(ui: &egui::Ui, width: f32) -> egui::Image<'static> {
    egui::Image::new(ImageSource::Bytes { uri: Cow::Borrowed("bytes://plugins/empty.svg"), bytes: EMPTY.into() })
        .tint(ui.visuals().weak_text_color())
        .fit_to_exact_size(egui::vec2(width, width * 160.0 / 240.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uri(s: ImageSource<'static>) -> String {
        match s {
            ImageSource::Bytes { uri, .. } | ImageSource::Uri(uri) => uri.into_owned(),
            ImageSource::Texture(_) => String::new(),
        }
    }

    #[test]
    fn built_in_then_shipped_then_default() {
        assert_eq!(uri(pick("org.foxglove.openrgb", None, Kind::Glyph)), "bytes://plugins/org.foxglove.openrgb/glyph.svg");
        assert_eq!(uri(pick("org.foxglove.icue", None, Kind::Tile { dark: false })), "bytes://plugins/org.foxglove.icue/icon-light.svg");
        assert_eq!(uri(pick("com.example.x", None, Kind::Glyph)), "bytes://plugins/_default/glyph.svg");
        let dir = std::env::temp_dir().join(format!("foxglove-icons-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("icon.svg"), "<svg/>").unwrap();
        // No icon-light.svg: the light theme falls back to icon.svg; no glyph: the default one.
        assert!(uri(pick("com.example.x", Some(&dir), Kind::Tile { dark: false })).ends_with("/icon.svg"));
        assert_eq!(uri(pick("com.example.x", Some(&dir), Kind::Glyph)), "bytes://plugins/_default/glyph.svg");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn built_in_svgs_render() {
        let options = resvg::usvg::Options::default();
        let all = BUILT_IN.iter().flat_map(|s| [(s.0, s.1), (s.0, s.2), (s.0, s.3)]).chain([("empty", EMPTY)]);
        for (id, bytes) in all {
            let image = egui_extras::image::load_svg_bytes(bytes, &options).unwrap_or_else(|e| panic!("{id}: {e}"));
            assert!(image.pixels.iter().any(|p| p.a() > 0), "{id} draws nothing");
        }
    }

    #[test]
    fn every_catalog_entry_has_a_built_in_set() {
        for entry in super::super::plugin_install::catalog() {
            assert!(BUILT_IN.iter().any(|s| s.0 == entry.id), "{} has no icons", entry.id);
        }
    }
}
