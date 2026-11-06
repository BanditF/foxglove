//! The overlay and the wallpaper feed follow the focused keyboard, or a pin (#801, C3;
//! docs/design/device-focus-layouts-thumbnails-lights-declutter.md section 1). They are the two
//! views that can pin, because they run while the window is closed: KeyPeek on the Corne while
//! the K5 Pro is being looked at. Every other view follows the focus.
//!
//! - **The cards:** "Keyboard: Follow focus ▾" in the layer overlay's card (Keyboard page) and
//!   in the Live wallpaper card (App page). One click opens the list, one click picks.
//! - **The overlay:** [`overlay_plan`] says whether the overlay's keymap connection moves to the
//!   keyboard its view names. It reads keymaps over ZMK Studio, so it moves only to a Foxglove
//!   keyboard (the host link), and only once it has a keyboard of its own: a focus on a VIA or
//!   LampArray keyboard leaves it where it is, and the card says so.
//! - **The feed:** `live_feed` and `wallpaper_sync` send the host link's facts and keys only
//!   while their keyboard is the connected host-link keyboard ([`feed_is_host_link`]); for any
//!   other keyboard the wallpaper gets its name and shape.
//! - **Events only:** [`start`] waits on C1's `focus.*` facts and wakes the window (the tray
//!   panel's title, the overlay's check) and the Scenes room. Nothing polls.

use super::keyboard_focus::{self as kf, Keyboard, Source, View};
use super::theme;

/// The list's first choice, and the box's words while nothing is pinned.
pub const FOLLOW: &str = "Follow focus";

/// The row's name.
pub const LABEL: &str = "Keyboard";

/// The box's width in both cards, so the overlay's and the feed's rows match.
pub const COMBO_WIDTH: f32 = 220.0;

/// After a keyboard that went this run.
pub const GONE: &str = "not connected";

/// The overlay row's ⓘ.
pub const OVERLAY_HOVER: &str = "Follow focus shows the keyboard picked at the start of the status strip. Pick a keyboard here to keep the overlay on it, also while this window is closed.";

/// The feed row's ⓘ.
pub const FEED_HOVER: &str = "Follow focus sends the keyboard picked at the start of the status strip. Pick a keyboard here to keep the wallpaper on it, also while this window is closed.";

/// Under the overlay's row when its keyboard is one it can't read.
pub const OVERLAY_CANT: &str =
    "The overlay reads ZMK keymaps, so it keeps showing the keyboard it has.";

/// Under the feed's row when its keyboard isn't a Foxglove keyboard.
pub const FEED_SHAPE_ONLY: &str =
    "Only a Foxglove keyboard sends keys and layers: the wallpaper gets this one's name and shape.";

/// What the row shows.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PinView {
    /// The pinned keyboard's key; `None` follows the focus.
    pub pinned: Option<String>,
    /// Every keyboard known this run: key, name, connected.
    pub keyboards: Vec<(String, String, bool)>,
    /// The keyboard the view shows now, when it is one the view can't fully show (see
    /// [`OVERLAY_CANT`], [`FEED_SHAPE_ONLY`]).
    pub limited: bool,
}

impl PinView {
    /// From C1's model, for `view`.
    pub fn now(view: View) -> PinView {
        let keyboards = kf::keyboards();
        let showing = kf::for_view(view);
        PinView {
            pinned: kf::pins().get(view).map(String::from),
            keyboards: keyboards
                .iter()
                .map(|k| (k.key.clone(), k.name.clone(), k.connected))
                .collect(),
            limited: showing.is_some_and(|k| k.source != Source::HostLink),
        }
    }

    /// The box's words: [`FOLLOW`], or the pinned keyboard's name (and that it went).
    pub fn selected(&self) -> String {
        let Some(pin) = &self.pinned else {
            return FOLLOW.into();
        };
        match self.keyboards.iter().find(|(k, _, _)| k == pin) {
            Some((_, name, true)) => name.clone(),
            Some((_, name, false)) => format!("{name} ({GONE})"),
            // Pinned to a keyboard not seen this run: it follows the focus until it is.
            None => FOLLOW.into(),
        }
    }
}

#[cfg(test)]
thread_local! {
    /// The previews' rows (overlay, feed), with the list of one open.
    pub static PREVIEW: std::cell::RefCell<Option<(PinView, PinView)>> = const { std::cell::RefCell::new(None) };
    pub static PREVIEW_OPEN: std::cell::Cell<Option<View>> = const { std::cell::Cell::new(None) };
}

fn view_now(view: View) -> PinView {
    #[cfg(test)]
    if let Some((overlay, feed)) = PREVIEW.with_borrow(|p| p.clone()) {
        return match view {
            View::Overlay => overlay,
            View::Feed => feed,
        };
    }
    PinView::now(view)
}

/// The box and its list; a pick is `Some(new pin)`: `Some(None)` follows the focus again.
fn pick(ui: &mut egui::Ui, view: View, v: &PinView) -> Option<Option<String>> {
    let mut out = None;
    let salt = ("foxglove-focus-pin", view);
    #[cfg(test)]
    if PREVIEW_OPEN.get() == Some(view) {
        // ComboBox keeps its list's state under its persistent id's "popup".
        let button = ui.make_persistent_id(egui::IdSalt::new(salt));
        egui::Popup::open_id(ui.ctx(), button.with("popup"));
    }
    egui::ComboBox::from_id_salt(salt)
        .width(COMBO_WIDTH)
        .selected_text(v.selected())
        .show_ui(ui, |ui| {
            if ui
                .selectable_label(v.pinned.is_none(), FOLLOW)
                .on_hover_text("The keyboard picked at the start of the status strip")
                .clicked()
                && v.pinned.is_some()
            {
                out = Some(None);
            }
            for (key, name, connected) in &v.keyboards {
                let on = v.pinned.as_deref() == Some(key.as_str());
                let text = if *connected {
                    egui::RichText::new(name)
                } else {
                    egui::RichText::new(format!("{name} ({GONE})")).weak()
                };
                if ui.selectable_label(on, text).clicked() && !on {
                    out = Some(Some(key.clone()));
                }
            }
        });
    out
}

/// Saves a pick and says so in the log.
fn apply(view: View, pin: Option<String>) {
    match kf::pin(view, pin.as_deref()) {
        Ok(()) => eprintln!(
            "Foxglove: {} keyboard: {}",
            match view {
                View::Overlay => "overlay",
                View::Feed => "wallpaper feed",
            },
            pin.as_deref().unwrap_or("follow focus")
        ),
        Err(e) => eprintln!("Foxglove: can't pin the keyboard: {e}"),
    }
}

/// The layer overlay's row, in its card's two-column grid: the name, then the box and its ⓘ.
pub fn overlay_row(ui: &mut egui::Ui) {
    let v = view_now(View::Overlay);
    ui.label(LABEL);
    let mut picked = None;
    ui.vertical(|ui| {
        ui.horizontal(|ui| {
            picked = pick(ui, View::Overlay, &v);
            theme::info_dot(ui, OVERLAY_HOVER);
        });
        if v.limited {
            ui.label(egui::RichText::new(OVERLAY_CANT).color(theme::palette(ui).muted));
        }
    });
    ui.end_row();
    if let Some(pin) = picked {
        apply(View::Overlay, pin);
    }
}

/// The Live wallpaper card's row.
pub fn feed_row(ui: &mut egui::Ui) {
    let v = view_now(View::Feed);
    let mut picked = None;
    ui.horizontal(|ui| {
        ui.label(LABEL);
        picked = pick(ui, View::Feed, &v);
        theme::info_dot(ui, FEED_HOVER);
    });
    if v.limited {
        ui.label(egui::RichText::new(FEED_SHAPE_ONLY).color(theme::palette(ui).muted));
    }
    if let Some(pin) = picked {
        apply(View::Feed, pin);
    }
}

/// What the overlay does with its keymap connection when its view's keyboard changes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OverlayPlan {
    /// It stays on the keyboard it has.
    Keep,
    /// It connects to this keyboard's keymap instead.
    Switch { vid: u16, pid: u16 },
}

/// The overlay's move for `view` (its pin, else the focus) while it shows the keyboard
/// `showing` (`vid`, `pid`): it moves only to a connected Foxglove keyboard it doesn't show
/// yet, and only once it shows one (it never starts a connection on its own).
pub fn overlay_plan(view: Option<&Keyboard>, showing: Option<(u16, u16)>) -> OverlayPlan {
    let (Some(k), Some(showing)) = (view, showing) else {
        return OverlayPlan::Keep;
    };
    if k.source != Source::HostLink || !k.connected || (k.vid, k.pid) == showing {
        return OverlayPlan::Keep;
    }
    OverlayPlan::Switch {
        vid: k.vid,
        pid: k.pid,
    }
}

/// The feed's keyboard is the connected host-link keyboard, whose facts (`keyboard.*`) and
/// keys the feed sends; also true before any keyboard was found (the facts as they are).
pub fn feed_is_host_link() -> bool {
    host_link(kf::for_view(View::Feed).as_ref())
}

/// `k` is the connected host-link keyboard (or there is none yet).
pub fn host_link(k: Option<&Keyboard>) -> bool {
    k.is_none_or(|k| k.source == Source::HostLink && k.connected)
}

/// Follows C1's change events for the rest of the run: each one wakes the window (the tray
/// panel's title, the overlay's check of its keyboard) and marks the Scenes room changed.
pub fn start() {
    let changes = kf::subscribe();
    super::services::spawn_named("focus-follow", move || {
        for _ in changes {
            super::scene_room::changed();
            super::tray_panel::wake();
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kb(key: &str, source: Source, vid: u16, pid: u16, connected: bool) -> Keyboard {
        Keyboard {
            key: key.into(),
            name: key.into(),
            source,
            vid,
            pid,
            device: None,
            connected,
        }
    }

    /// #801's acceptance: a pinned overlay keeps its keyboard while the focus changes, and an
    /// unpinned one follows it.
    #[test]
    fn a_pinned_overlay_keeps_its_keyboard_while_the_focus_changes() {
        use crate::foxglove::keyboard_focus::{Model, Saved, Seen};
        let seen = |key: &str, vid, pid| Seen {
            key: key.into(),
            name: key.into(),
            vid,
            pid,
            device: None,
        };
        let mut m = Model::new(&Saved::default());
        m.report(Source::HostLink, &[seen("corne", 0x1d50, 0x615e)]);
        m.report(Source::Via, &[seen("k5", 0x3434, 0x0250)]);
        assert_eq!(m.focus().unwrap().key, "corne");

        // Unpinned, the overlay goes where the focus goes.
        m.pick("k5").unwrap();
        assert_eq!(m.for_view(View::Overlay).unwrap().key, "k5");

        // Pinned to the Corne, it stays there through every focus change.
        m.pin(View::Overlay, Some("corne")).unwrap();
        for focus in ["corne", "k5", "corne", "k5"] {
            m.pick(focus).unwrap();
            assert_eq!(m.for_view(View::Overlay).unwrap().key, "corne", "{focus}");
            assert_eq!(
                overlay_plan(m.for_view(View::Overlay), Some((0x1d50, 0x615e))),
                OverlayPlan::Keep,
                "{focus}: the overlay's connection stays"
            );
        }
        // The feed has its own pin: it still follows the focus.
        assert_eq!(m.for_view(View::Feed).unwrap().key, "k5");

        // Back to following: the focus again.
        m.pin(View::Overlay, None).unwrap();
        assert_eq!(m.for_view(View::Overlay).unwrap().key, "k5");
    }

    #[test]
    fn the_overlay_moves_only_to_a_connected_foxglove_keyboard_it_doesnt_show() {
        let corne = kb("corne", Source::HostLink, 0x1d50, 0x615e, true);
        let other = kb("plank", Source::HostLink, 0x1d50, 0x6160, true);
        let k5 = kb("k5", Source::Via, 0x3434, 0x0250, true);
        let shows_corne = Some((0x1d50, 0x615e));
        assert_eq!(overlay_plan(Some(&corne), shows_corne), OverlayPlan::Keep);
        assert_eq!(
            overlay_plan(Some(&other), shows_corne),
            OverlayPlan::Switch {
                vid: 0x1d50,
                pid: 0x6160
            }
        );
        assert_eq!(
            overlay_plan(Some(&k5), shows_corne),
            OverlayPlan::Keep,
            "VIA: no ZMK keymap to read"
        );
        let gone = Keyboard {
            connected: false,
            ..other.clone()
        };
        assert_eq!(overlay_plan(Some(&gone), shows_corne), OverlayPlan::Keep);
        assert_eq!(
            overlay_plan(Some(&other), None),
            OverlayPlan::Keep,
            "it never starts a connection on its own"
        );
        assert_eq!(overlay_plan(None, shows_corne), OverlayPlan::Keep);
    }

    #[test]
    fn the_feed_sends_the_host_links_facts_only_for_the_host_link_keyboard() {
        assert!(host_link(None), "no keyboard yet: the facts as they are");
        assert!(host_link(Some(&kb("c", Source::HostLink, 1, 2, true))));
        assert!(!host_link(Some(&kb("c", Source::HostLink, 1, 2, false))));
        assert!(!host_link(Some(&kb("k5", Source::Via, 1, 2, true))));
        assert!(!host_link(Some(&kb("l", Source::LampArray, 1, 2, true))));
    }

    #[test]
    fn the_box_names_the_pin_or_follow_focus() {
        let keyboards = vec![
            ("corne".into(), "Eyelash Corne".into(), true),
            ("k5".into(), "K5 Pro".into(), false),
        ];
        let v = |pinned: Option<&str>| PinView {
            pinned: pinned.map(String::from),
            keyboards: keyboards.clone(),
            limited: false,
        };
        assert_eq!(v(None).selected(), "Follow focus");
        assert_eq!(v(Some("corne")).selected(), "Eyelash Corne");
        assert_eq!(v(Some("k5")).selected(), "K5 Pro (not connected)");
        assert_eq!(v(Some("unseen")).selected(), "Follow focus");
    }
}
