// SPDX-License-Identifier: MIT
//! Theme tokens (contract §2.1 job 2): one set for light and one for dark, picked by Windows' app
//! mode unless the user chose one. Panels draw with these names, never with raw colours, so a
//! module's panel matches the hub. The accent is Hideout's orange.

/// An sRGB colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rgb(pub u8, pub u8, pub u8);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tokens {
    pub dark: bool,
    /// The window and page background.
    pub bg: Rgb,
    /// The sidebar and the status strip.
    pub chrome: Rgb,
    /// Cards, inputs, the palette.
    pub surface: Rgb,
    pub border: Rgb,
    pub text: Rgb,
    pub text_muted: Rgb,
    pub accent: Rgb,
    /// Text on an accent fill.
    pub on_accent: Rgb,
    pub danger: Rgb,
}

pub const LIGHT: Tokens = Tokens {
    dark: false,
    bg: Rgb(0xfa, 0xf8, 0xf6),
    chrome: Rgb(0xf0, 0xec, 0xe8),
    surface: Rgb(0xff, 0xff, 0xff),
    border: Rgb(0xd8, 0xd2, 0xcc),
    text: Rgb(0x1f, 0x1b, 0x18),
    text_muted: Rgb(0x5c, 0x55, 0x4f),
    accent: Rgb(0xb8, 0x4a, 0x00),
    on_accent: Rgb(0xff, 0xff, 0xff),
    danger: Rgb(0xb0, 0x1e, 0x1e),
};

pub const DARK: Tokens = Tokens {
    dark: true,
    bg: Rgb(0x17, 0x15, 0x14),
    chrome: Rgb(0x20, 0x1d, 0x1b),
    surface: Rgb(0x2a, 0x26, 0x23),
    border: Rgb(0x3d, 0x37, 0x33),
    text: Rgb(0xf2, 0xee, 0xea),
    text_muted: Rgb(0xa8, 0xa0, 0x99),
    accent: Rgb(0xff, 0x8a, 0x2a),
    on_accent: Rgb(0x1a, 0x0e, 0x04),
    danger: Rgb(0xff, 0x6b, 0x6b),
};

pub fn tokens(dark: bool) -> Tokens {
    if dark {
        DARK
    } else {
        LIGHT
    }
}

/// WCAG relative luminance.
pub fn luminance(c: Rgb) -> f64 {
    let ch = |v: u8| {
        let s = v as f64 / 255.0;
        if s <= 0.04045 {
            s / 12.92
        } else {
            ((s + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * ch(c.0) + 0.7152 * ch(c.1) + 0.0722 * ch(c.2)
}

/// WCAG contrast ratio, 1 to 21.
pub fn contrast(a: Rgb, b: Rgb) -> f64 {
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// The fill behind a sidebar item (the selection fill is the accent).
pub fn nav_fill(t: &Tokens, selected: bool) -> Rgb {
    if selected {
        t.accent
    } else {
        t.chrome
    }
}

/// The label colour of a sidebar item: on the accent fill when selected, else plain text.
pub fn nav_label(t: &Tokens, selected: bool) -> Rgb {
    if selected {
        t.on_accent
    } else {
        t.text
    }
}

/// The label colour of a palette row. A selected row sits on the accent fill, so its label is
/// `on_accent` whether or not the entry can run; an unavailable row that is not selected is muted.
pub fn palette_label(t: &Tokens, selected: bool, unavailable: bool) -> Rgb {
    if selected {
        t.on_accent
    } else if unavailable {
        t.text_muted
    } else {
        t.text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selected_nav_label_contrasts_with_its_fill() {
        for t in [LIGHT, DARK] {
            assert!(
                contrast(nav_label(&t, true), nav_fill(&t, true)) >= 4.5,
                "{t:?} selected"
            );
            assert!(
                contrast(nav_label(&t, false), nav_fill(&t, false)) >= 4.5,
                "{t:?} unselected"
            );
        }
    }

    #[test]
    fn every_palette_label_reads_on_its_fill() {
        for t in [LIGHT, DARK] {
            for selected in [false, true] {
                for unavailable in [false, true] {
                    // The row's fill: the accent when selected, else the modal's surface.
                    let fill = if selected { t.accent } else { t.surface };
                    let label = palette_label(&t, selected, unavailable);
                    assert!(
                        contrast(label, fill) >= 4.5,
                        "{t:?} selected={selected} unavailable={unavailable}: {:.2}",
                        contrast(label, fill)
                    );
                }
            }
        }
    }

    #[test]
    fn text_meets_wcag_aa_on_every_background() {
        for t in [LIGHT, DARK] {
            for bg in [t.bg, t.chrome, t.surface] {
                assert!(contrast(t.text, bg) >= 7.0, "{t:?} text");
                assert!(contrast(t.text_muted, bg) >= 4.5, "{t:?} muted");
                assert!(contrast(t.accent, bg) >= 3.0, "{t:?} accent");
            }
            assert!(contrast(t.on_accent, t.accent) >= 4.5, "{t:?} on accent");
            assert!(contrast(t.danger, t.bg) >= 4.5, "{t:?} danger");
        }
    }

    #[test]
    fn the_pick_follows_the_mode() {
        assert!(tokens(true).dark);
        assert!(!tokens(false).dark);
    }
}
