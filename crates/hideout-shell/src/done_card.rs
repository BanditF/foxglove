// SPDX-License-Identifier: MIT
//! The Done card (first-run plan FR4, `ux-plan-first-run.md` §2 and §5): after the user's Allow
//! starts an app, the Modules page says so, live, in one status line built from the app's own
//! facts, with one button to see it work (the app's declared `identify` action, else "Open
//! <app>"), and up to three later offers.
//!
//! The offers are plain lines with one button each, and **Not now** hides one until the next
//! start. None is ever ticked, preset or run for the user: a button only opens the place where
//! the user decides (Settings for Start with Windows, the sharing dialog, the add step of
//! another found app). Nothing here turns on start-up or changes a setting.
//!
//! Window-free model (words, the status line, the offers) and its drawing. Generic: it names no
//! app and reads only facts any app can publish (`keyboard.*`, `module.<id>.state`,
//! `module.<id>.starts_at_sign_in`). One noun for the user: "app".

use serde_json::Value;

/// Every word of the card.
pub mod words {
    pub const SHOW_FALLBACK_PREFIX: &str = "Open ";
    pub const NOT_NOW: &str = "Not now";
    pub const HIDE: &str = "Hide";
    pub const HIDE_HINT: &str = "Hide this card";
    pub const LATER: &str = "Later, if you want";
    pub const SIGN_IN_LINE: &str = "Start Hideout when you sign in";
    pub const SIGN_IN_BUTTON: &str = "Turn on\u{2026}";
    pub const PRESS_A_LAYER_KEY: &str = "Press a layer key: the layer overlay should appear.";
    pub const NO_KEYBOARD: &str =
        "No keyboard found yet. Plug it in or turn it on, and this updates by itself.";

    pub fn sharing_line(name: &str) -> String {
        format!("Let {name} share your keyboard with other apps")
    }
    pub const SHARING_BUTTON: &str = "Review\u{2026}";

    pub fn more_line(name: &str) -> String {
        format!("Also found: {name}")
    }
    pub const MORE_BUTTON: &str = "Set up";

    pub fn running(name: &str) -> String {
        format!("{name} is running.")
    }
    pub fn starting(name: &str) -> String {
        format!("{name} is starting\u{2026}")
    }
    pub fn off(name: &str) -> String {
        format!("{name} is off.")
    }
    pub fn stopped(name: &str) -> String {
        format!("{name} stopped. Its row below has the button to start it again.")
    }
}

/// One later offer. It identifies a line and what its button opens; it carries no choice.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Offer {
    /// Opens Settings, where Start with Windows and its confirm are.
    StartWithWindows,
    /// Opens the keyboard-sharing dialog.
    Sharing,
    /// Another found app (its id): starts the same Set up step as the Found card.
    SetUp(String),
}

/// What a press on the card asks the page to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Press {
    /// The main button: the app's `identify` action, else open the app.
    Show,
    /// An offer's button (it opens the place to decide; it decides nothing).
    Offer(Offer),
    /// An offer's **Not now**.
    NotNow(Offer),
    /// **Hide** on the card.
    Hide,
}

/// A line of the status text: the bold first sentence and the rest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Status {
    pub head: String,
    pub rest: String,
    /// The line under it ("Press a layer key..."), only while the keyboard is there.
    pub hint: Option<String>,
}

/// One offer line as drawn.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OfferLine {
    pub offer: Offer,
    pub text: String,
    pub button: String,
}

/// The card as drawn.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Card {
    pub status: Status,
    /// The main button's words: the manifest's for `identify`, else "Open <app>".
    pub show: String,
    pub offers: Vec<OfferLine>,
}

/// What the page knows when it builds the card.
pub struct Inputs<'a> {
    /// The app's name, from its manifest.
    pub name: &'a str,
    pub facts: &'a dyn Fn(&str) -> Option<Value>,
    /// The words of the manifest's `identify` action, when it has one.
    pub identify: Option<String>,
    /// The keyboard-sharing grant is not on yet and this app can have it.
    pub sharing_open: bool,
    /// Other found apps not yet in Hideout: (id, name).
    pub more: Vec<(String, String)>,
}

/// The card's state: which app it is about and which offers the user put off.
#[derive(Default)]
pub struct Done {
    module: Option<String>,
    not_now: Vec<Offer>,
}

impl Done {
    /// The user's Allow started `module`: the card shows for it (and every offer again).
    pub fn show_for(&mut self, module: &str) {
        self.module = Some(module.to_string());
        self.not_now.clear();
    }

    pub fn module(&self) -> Option<&str> {
        self.module.as_deref()
    }

    pub fn hide(&mut self) {
        self.module = None;
    }

    /// **Not now** on an offer: hidden until the next start.
    pub fn put_off(&mut self, o: Offer) {
        if !self.not_now.contains(&o) {
            self.not_now.push(o);
        }
    }

    /// The card for this frame, or `None` while it is hidden.
    pub fn card(&self, i: &Inputs) -> Option<Card> {
        let module = self.module.as_deref()?;
        let state = (i.facts)(&format!("module.{module}.state"))
            .and_then(|v| v.as_str().map(str::to_string));
        let status = status_line(i.name, state.as_deref(), i.facts);
        let show = i
            .identify
            .clone()
            .unwrap_or_else(|| format!("{}{}", words::SHOW_FALLBACK_PREFIX, i.name));
        let mut offers = Vec::new();
        // Hideout already starts at sign-in: nothing to offer. Unknown reads as "not yet".
        let starts = (i.facts)(&format!("module.{module}.starts_at_sign_in"))
            .and_then(|v| v.as_bool())
            == Some(true);
        if !starts {
            offers.push(OfferLine {
                offer: Offer::StartWithWindows,
                text: words::SIGN_IN_LINE.into(),
                button: words::SIGN_IN_BUTTON.into(),
            });
        }
        if i.sharing_open {
            offers.push(OfferLine {
                offer: Offer::Sharing,
                text: words::sharing_line(i.name),
                button: words::SHARING_BUTTON.into(),
            });
        }
        for (id, name) in &i.more {
            offers.push(OfferLine {
                offer: Offer::SetUp(id.clone()),
                text: words::more_line(name),
                button: words::MORE_BUTTON.into(),
            });
        }
        offers.retain(|o| !self.not_now.contains(&o.offer));
        Some(Card {
            status,
            show,
            offers,
        })
    }
}

/// The status line from the app's own facts (event driven: the page asks each frame it is
/// drawn, and a fact change wakes it). Nothing is hardcoded about the app.
pub fn status_line(
    name: &str,
    state: Option<&str>,
    facts: &dyn Fn(&str) -> Option<Value>,
) -> Status {
    let plain = |head: String| Status {
        head,
        rest: String::new(),
        hint: None,
    };
    match state {
        Some("starting") | Some("restarting") => return plain(words::starting(name)),
        Some("off") => return plain(words::off(name)),
        Some("crash-stopped") | Some("crash-looped") => return plain(words::stopped(name)),
        _ => {}
    }
    let head = words::running(name);
    match facts("keyboard.connected").and_then(|v| v.as_bool()) {
        Some(true) => {
            let mut parts = Vec::new();
            let kb = facts("keyboard.name").and_then(|v| v.as_str().map(str::to_string));
            parts.push(match kb {
                Some(n) if !n.trim().is_empty() => format!("Keyboard: {}", n.trim()),
                _ => "Keyboard connected".to_string(),
            });
            if let Some(l) =
                facts("keyboard.layer_name").and_then(|v| v.as_str().map(str::to_string))
            {
                let l = l.trim();
                if !l.is_empty() {
                    parts.push(if l.to_lowercase().ends_with("layer") {
                        l.to_string()
                    } else {
                        format!("{l} layer")
                    });
                }
            }
            if let Some(p) = battery(facts) {
                parts.push(format!("battery {p} %"));
            }
            Status {
                head,
                rest: parts.join(" \u{b7} "),
                hint: Some(words::PRESS_A_LAYER_KEY.into()),
            }
        }
        Some(false) => Status {
            head,
            rest: words::NO_KEYBOARD.into(),
            hint: None,
        },
        // This app shares no keyboard facts: the head alone says it runs.
        None => plain(head),
    }
}

/// The lowest battery percent any part reports (a half that is nearly flat matters most).
fn battery(facts: &dyn Fn(&str) -> Option<Value>) -> Option<i64> {
    [
        "keyboard.battery",
        "keyboard.left.battery",
        "keyboard.right.battery",
    ]
    .iter()
    .filter_map(|k| facts(k).and_then(|v| v.as_i64()))
    .filter(|p| (0..=100).contains(p))
    .min()
}

#[cfg(windows)]
pub use ui::card_ui;

#[cfg(windows)]
mod ui {
    use super::{Card, Press};
    use crate::theme::{Rgb, Tokens};
    use eframe::egui::{self, RichText};

    fn c(x: Rgb) -> egui::Color32 {
        egui::Color32::from_rgb(x.0, x.1, x.2)
    }

    /// Draws the card above the rows; the press, if any. Only the head is a live region (so a
    /// screen reader says "running" once, not every battery change).
    pub fn card_ui(ui: &mut egui::Ui, t: &Tokens, card: &Card) -> Option<Press> {
        let mut press = None;
        egui::Frame::new()
            .fill(c(t.surface))
            .stroke(egui::Stroke::new(1.0_f32, c(t.border)))
            .corner_radius(egui::CornerRadius::same(6))
            .inner_margin(egui::Margin::symmetric(14, 10))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal_wrapped(|ui| {
                    let head = ui.label(RichText::new(&card.status.head).strong().color(c(t.text)));
                    ui.ctx().accesskit_node_builder(head.id, |n| {
                        n.set_live(egui::accesskit::Live::Polite);
                    });
                    if !card.status.rest.is_empty() {
                        ui.label(RichText::new(&card.status.rest).color(c(t.text)));
                    }
                });
                if let Some(h) = &card.status.hint {
                    ui.label(RichText::new(h).color(c(t.text_muted)));
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui
                        .add(
                            egui::Button::new(RichText::new(&card.show).color(c(t.on_accent)))
                                .fill(c(t.accent)),
                        )
                        .clicked()
                    {
                        press = Some(Press::Show);
                    }
                    if ui
                        .button(super::words::HIDE)
                        .on_hover_text(super::words::HIDE_HINT)
                        .clicked()
                    {
                        press = Some(Press::Hide);
                    }
                });
                if !card.offers.is_empty() {
                    ui.add_space(10.0);
                    ui.label(
                        RichText::new(super::words::LATER)
                            .small()
                            .color(c(t.text_muted)),
                    );
                    for o in &card.offers {
                        ui.push_id(("hub.done.offer", format!("{:?}", o.offer)), |ui| {
                            ui.horizontal_wrapped(|ui| {
                                ui.label(RichText::new(&o.text).color(c(t.text)));
                                if ui.button(&o.button).clicked() {
                                    press = Some(Press::Offer(o.offer.clone()));
                                }
                                if ui.button(super::words::NOT_NOW).clicked() {
                                    press = Some(Press::NotNow(o.offer.clone()));
                                }
                            });
                        });
                    }
                }
            });
        ui.add_space(8.0);
        press
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::BTreeMap;

    fn facts(v: &[(&str, Value)]) -> impl Fn(&str) -> Option<Value> {
        let m: BTreeMap<String, Value> =
            v.iter().map(|(k, v)| (k.to_string(), v.clone())).collect();
        move |k| m.get(k).cloned()
    }

    fn inputs<'a>(f: &'a dyn Fn(&str) -> Option<Value>) -> Inputs<'a> {
        Inputs {
            name: "Foxglove",
            facts: f,
            identify: None,
            sharing_open: true,
            more: vec![("vulpes".into(), "Vulpes".into())],
        }
    }

    fn done() -> Done {
        let mut d = Done::default();
        d.show_for("foxglove");
        d
    }

    #[test]
    fn the_status_line_is_built_from_the_apps_facts() {
        let f = facts(&[
            ("module.foxglove.state", json!("running")),
            ("keyboard.connected", json!(true)),
            ("keyboard.name", json!("Eyelash Corne")),
            ("keyboard.layer_name", json!("Base")),
            ("keyboard.left.battery", json!(82)),
            ("keyboard.right.battery", json!(91)),
        ]);
        let c = done().card(&inputs(&f)).unwrap();
        assert_eq!(c.status.head, "Foxglove is running.");
        assert_eq!(
            c.status.rest,
            "Keyboard: Eyelash Corne \u{b7} Base layer \u{b7} battery 82 %"
        );
        assert_eq!(c.status.hint.as_deref(), Some(words::PRESS_A_LAYER_KEY));
    }

    #[test]
    fn the_line_follows_the_facts_with_no_keyboard_then_a_keyboard() {
        let none = facts(&[
            ("module.foxglove.state", json!("running")),
            ("keyboard.connected", json!(false)),
        ]);
        let c = done().card(&inputs(&none)).unwrap();
        assert_eq!(c.status.rest, words::NO_KEYBOARD);
        assert_eq!(c.status.hint, None);
        // An app without keyboard facts: the head alone.
        let bare = facts(&[("module.foxglove.state", json!("running"))]);
        let c = done().card(&inputs(&bare)).unwrap();
        assert_eq!((c.status.rest.as_str(), c.status.hint), ("", None));
        // A state that is not running says so, and shows no keyboard line.
        let down = facts(&[
            ("module.foxglove.state", json!("crash-stopped")),
            ("keyboard.connected", json!(true)),
        ]);
        let c = done().card(&inputs(&down)).unwrap();
        assert!(c.status.head.starts_with("Foxglove stopped."));
        assert_eq!(c.status.rest, "");
    }

    #[test]
    fn the_button_is_the_manifests_words_else_open_the_app() {
        let f = facts(&[]);
        let mut i = inputs(&f);
        assert_eq!(done().card(&i).unwrap().show, "Open Foxglove");
        i.identify = Some("Show the lights".into());
        assert_eq!(done().card(&i).unwrap().show, "Show the lights");
    }

    #[test]
    fn every_offer_is_a_button_and_none_is_preselected_or_acts_by_itself() {
        let f = facts(&[("module.foxglove.state", json!("running"))]);
        let d = done();
        let c = d.card(&inputs(&f)).unwrap();
        let offers: Vec<_> = c.offers.iter().map(|o| o.offer.clone()).collect();
        assert_eq!(
            offers,
            vec![
                Offer::StartWithWindows,
                Offer::Sharing,
                Offer::SetUp("vulpes".into())
            ]
        );
        // Building the card, drawing it again and a Not now on one change nothing else: no
        // press happens by itself, and Not now never turns anything on.
        for o in &c.offers {
            assert!(!o.button.is_empty());
            assert!(!o.text.to_lowercase().contains("module"));
        }
        assert_eq!(d.card(&inputs(&f)).unwrap(), c);
    }

    #[test]
    fn start_with_windows_is_not_offered_when_hideout_already_starts_at_sign_in() {
        let f = facts(&[("module.foxglove.starts_at_sign_in", json!(true))]);
        let c = done().card(&inputs(&f)).unwrap();
        assert!(!c.offers.iter().any(|o| o.offer == Offer::StartWithWindows));
        // Unknown (no fact yet) still offers it, as a button.
        let unknown = facts(&[]);
        let c = done().card(&inputs(&unknown)).unwrap();
        assert!(c.offers.iter().any(|o| o.offer == Offer::StartWithWindows));
    }

    #[test]
    fn not_now_hides_an_offer_until_the_next_start_and_hide_hides_the_card() {
        let f = facts(&[]);
        let mut d = done();
        d.put_off(Offer::Sharing);
        d.put_off(Offer::Sharing);
        let c = d.card(&inputs(&f)).unwrap();
        assert!(!c.offers.iter().any(|o| o.offer == Offer::Sharing));
        assert_eq!(c.offers.len(), 2);
        // A new app that starts shows every offer again.
        d.show_for("other");
        let i = inputs(&f);
        assert_eq!(d.card(&i).unwrap().offers.len(), 3);
        d.hide();
        assert!(d.card(&i).is_none());
    }

    #[test]
    fn the_sharing_and_more_apps_offers_show_only_when_there_is_something_to_offer() {
        let f = facts(&[]);
        let mut i = inputs(&f);
        i.sharing_open = false;
        i.more.clear();
        let c = done().card(&i).unwrap();
        assert_eq!(
            c.offers.iter().map(|o| &o.offer).collect::<Vec<_>>(),
            vec![&Offer::StartWithWindows]
        );
    }

    #[test]
    fn the_words_use_app_and_none_of_the_banned_ones() {
        let f = facts(&[("keyboard.connected", json!(true))]);
        let c = done().card(&inputs(&f)).unwrap();
        let mut all = vec![c.status.head, c.status.rest, c.show];
        all.extend(c.status.hint);
        for o in c.offers {
            all.push(o.text);
            all.push(o.button);
        }
        for w in all {
            let l = w.to_lowercase();
            for banned in ["module", "manifest", "grant", "capability", "hash"] {
                assert!(!l.contains(banned), "{w:?} says {banned}");
            }
        }
    }
}
