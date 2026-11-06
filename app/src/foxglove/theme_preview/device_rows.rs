//! Thumbnails in place (#804, D2), dark and light: the Lights page's Other devices rows
//! (`ui-804-other-devices-{dark,light}.png`) and the Keyboard page's Batteries list
//! (`ui-804-batteries-{dark,light}.png`). Kept with `FOXGLOVE_UI_PREVIEW_DIR` set.

use super::{keep, shell_preview_in, Samples};
use crate::foxglove::shell::Page;
use crate::foxglove::theme::Choice;

#[test]
fn device_rows_render() {
    let mut images = Vec::new();
    for (choice, suffix) in [(Choice::Dark, "dark"), (Choice::Light, "light")] {
        let lights = shell_preview_in(Page::Lights, &Samples::full(), 1180.0, choice);
        let batteries = shell_preview_in(Page::Keyboard, &Samples::full(), 1300.0, choice);
        keep(&format!("ui-804-other-devices-{suffix}.png"), &lights);
        keep(&format!("ui-804-batteries-{suffix}.png"), &batteries);
        images.push((lights, batteries));
    }
    assert_ne!(images[0].0, images[1].0, "dark and light differ");
    assert_ne!(images[0].1, images[1].1, "dark and light differ");
}
