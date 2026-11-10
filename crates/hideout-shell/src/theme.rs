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
    /// The quiet warning (amber): the crash bar's edge and, blended, its fill.
    pub warn: Rgb,
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
    warn: Rgb(0xb0, 0x78, 0x00),
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
    warn: Rgb(0xf5, 0xc4, 0x51),
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

/// The sidebar's red warning mark: danger on the sidebar, `on_accent` on the selected entry's
/// accent fill (where red would not read).
pub fn nav_mark(t: &Tokens, selected: bool) -> Rgb {
    if selected {
        t.on_accent
    } else {
        t.danger
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

/// The fill of a tab: the active one joins the page (`bg`), the others sit on the strip (`chrome`).
pub fn tab_fill(t: &Tokens, active: bool) -> Rgb {
    if active {
        t.bg
    } else {
        t.chrome
    }
}

/// A tab's label: full contrast on the active tab, muted on the others, and muted on a popped-out
/// or turned-off tab even when it is active. Muted still reads at 4.5:1 or more.
pub fn tab_label(t: &Tokens, active: bool, muted: bool) -> Rgb {
    if active && !muted {
        t.text
    } else {
        t.text_muted
    }
}

/// The crash bar's fill: the amber at a fifth over the page, so its text keeps full contrast.
pub fn warn_fill(t: &Tokens) -> Rgb {
    let mix = |w: u8, b: u8| ((w as u16 + 4 * b as u16) / 5) as u8;
    Rgb(
        mix(t.warn.0, t.bg.0),
        mix(t.warn.1, t.bg.1),
        mix(t.warn.2, t.bg.2),
    )
}

/// Every foreground the strip draws on a tab fill, with a name, for the contrast test:
/// label, close ×, dirty dot (label colour), count pill text on its pill, stopped mark.
pub fn tab_pairs(t: &Tokens) -> Vec<(&'static str, Rgb, Rgb)> {
    let mut v = Vec::new();
    for active in [false, true] {
        let fill = tab_fill(t, active);
        for muted in [false, true] {
            v.push(("tab label", tab_label(t, active, muted), fill));
        }
        v.push(("stopped mark", t.danger, fill));
        v.push(("popped mark", t.text_muted, fill));
    }
    v.push(("count pill", t.on_accent, t.accent));
    v.push(("overflow buttons", t.text, t.chrome));
    v.push(("empty page hint", t.text_muted, t.bg));
    v.push(("dialog text", t.text, t.surface));
    v.push(("stopped card", t.text, t.surface));
    v.push(("stopped card title", t.danger, t.surface));
    v.push(("stopped card button", t.on_accent, t.accent));
    v.push(("crash bar", t.text, warn_fill(t)));
    for selected in [false, true] {
        v.push(("sidebar mark", nav_mark(t, selected), nav_fill(t, selected)));
    }
    v
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
    #[test]
    fn every_tab_label_and_mark_reads_on_its_fill() {
        for t in [LIGHT, DARK] {
            for (what, fg, bg) in tab_pairs(&t) {
                let r = contrast(fg, bg);
                assert!(
                    r >= 4.5,
                    "{} {what}: {r:.2}",
                    if t.dark { "dark" } else { "light" }
                );
            }
        }
    }
}
