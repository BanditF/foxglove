// SPDX-License-Identifier: MIT
//! The hub's dialog for a program outside Hideout that asks for more than it may have (S1,
//! `review/design-s1-outside-tools.md` section 5). Drawn by the hub, never by the program. Its
//! name is the program's own word for itself and is shown as quoted plain text (cleaned, cut
//! at 40 characters, regular weight, never accent-colored); Hideout can't confirm it. "Not now"
//! is the default: it has the focus, and Enter, Esc and the window's close button all answer it.
//! "Allow" is armed one second after the dialog is first drawn, so a key already on its way
//! can't allow something. Nothing here allows anything by itself: only Allow does, and only
//! for the items listed.

use hideout_contract::capability::{tool_class, Capability, ToolClass};
use hideout_router::{Router, ToolAsk, ToolId};

use crate::dismiss::dismissed;
use crate::egui;
use crate::keyboard_grant_ui::rgb as c;
use crate::permission_dialog::{ALLOW, NOT_NOW};
use crate::theme::Tokens;

/// Seconds before Allow can be pressed.
pub const ARM_SECS: f64 = 1.0;
/// The longest name drawn, in characters.
pub const NAME_MAX: usize = 40;

pub const TITLE: &str = "Allow a program outside Hideout?";
pub const ASKS: &str = "It asks to:";
pub const FOOTER_KEPT: &str =
    "Hideout remembers your answer for this program file. If the file changes, it asks again.";
pub const FOOTER_RUNNER: &str =
    "This program runs scripts, so Hideout asks again the next time it connects.";
/// The program that opened the connection is what is allowed, not whoever drives it.
pub const SAME_PROGRAM: &str = "Anything that uses this program can use what you allow.";

/// The program's own name as the words show it: cleaned, cut, always in quotes.
pub fn quoted_name(raw: &str) -> String {
    format!("\u{201c}{}\u{201d}", Router::clean_name_to(raw, NAME_MAX))
}

/// The Apps page header's title; shown only when `n` is at least 1.
pub fn header_title(n: usize) -> String {
    format!(
        "A program outside Hideout asks for {n} permission{}",
        if n == 1 { "" } else { "s" }
    )
}

/// The Apps page header's body.
pub fn header_body(raw: &str) -> String {
    format!(
        "It calls itself {}. Nothing is allowed until you review it.",
        quoted_name(raw)
    )
}

/// What the program calls itself, and that Hideout can't confirm it.
pub fn calls_itself(raw: &str) -> String {
    format!(
        "It calls itself {}. Hideout can't confirm that name.",
        quoted_name(raw)
    )
}

/// The program file line. A path is plain text: control characters are dropped.
pub fn file_line(path: &str) -> String {
    let plain: String = path.chars().filter(|c| !c.is_control()).collect();
    if plain.trim().is_empty() {
        "Program file: not known".into()
    } else {
        format!("Program file: {plain}")
    }
}

/// One permission the program asks for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolItem {
    pub raw: String,
    pub does: String,
    /// Drawn in the sensitive style.
    pub red: bool,
}

/// The open dialog for one waiting program.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolDialog {
    pub id: ToolId,
    /// What the program calls itself: untrusted, cleaned only when drawn.
    pub name: String,
    pub items: Vec<ToolItem>,
    pub runs_scripts: bool,
    /// When Allow's wait started (egui time, seconds): the first frame it was drawn.
    opened: Option<f64>,
}

/// What the dialog answered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolAnswer {
    Allow,
    NotNow,
}

impl ToolDialog {
    /// The dialog for a queued ask. An item that can never be allowed to a program outside
    /// Hideout, or that can't be read, is not drawn.
    pub fn from_ask(a: &ToolAsk) -> ToolDialog {
        let items = a
            .items
            .iter()
            .filter(|i| {
                // A channel's class was checked when the ask was queued.
                Capability::parse(&i.raw)
                    .map(|cap| tool_class(&cap, Some("normal")) == ToolClass::Ask)
                    .unwrap_or(false)
            })
            .map(|i| ToolItem {
                raw: i.raw.clone(),
                does: i.does.clone(),
                red: i.red,
            })
            .collect();
        ToolDialog {
            id: a.id.clone(),
            name: a.name.clone(),
            items,
            runs_scripts: a.runs_scripts,
            opened: None,
        }
    }

    pub fn footer(&self) -> &'static str {
        if self.runs_scripts {
            FOOTER_RUNNER
        } else {
            FOOTER_KEPT
        }
    }

    /// Every line the dialog says, in order (the text the person reads).
    pub fn lines(&self) -> Vec<String> {
        let mut out = vec![
            TITLE.to_string(),
            calls_itself(&self.name),
            file_line(&self.id.image_path),
            ASKS.to_string(),
        ];
        out.extend(self.items.iter().map(|i| i.does.clone()));
        out.push(SAME_PROGRAM.to_string());
        out.push(self.footer().to_string());
        out
    }

    /// Whether Allow can be pressed at `now` (egui time). The wait starts at the first call.
    pub fn armed(&mut self, now: f64) -> bool {
        let start = *self.opened.get_or_insert(now);
        now - start >= ARM_SECS
    }

    /// Seconds until Allow is armed, once the wait has started.
    pub fn wait_left(&self, now: f64) -> f64 {
        self.opened
            .map_or(ARM_SECS, |s| (ARM_SECS - (now - s)).max(0.0))
    }

    /// What Allow allows: the items drawn, as the router parses them.
    pub fn allowed(&self) -> Vec<Capability> {
        self.items
            .iter()
            .filter_map(|i| Capability::parse(&i.raw).ok())
            .collect()
    }

    /// The accessible description.
    pub fn said(&self) -> String {
        format!(
            "{} {}",
            calls_itself(&self.name),
            file_line(&self.id.image_path)
        )
    }
}

/// The dialog. `first`: the first frame, when Not now takes the focus.
pub fn tool_dialog_ui(
    ctx: &egui::Context,
    t: &Tokens,
    d: &mut ToolDialog,
    first: bool,
) -> Option<ToolAnswer> {
    tool_dialog_ids(ctx, t, d, first).0
}

/// [`tool_dialog_ui`] and the focusable widgets in Tab order (Allow, then Not now).
pub(crate) fn tool_dialog_ids(
    ctx: &egui::Context,
    t: &Tokens,
    d: &mut ToolDialog,
    first: bool,
) -> (Option<ToolAnswer>, Vec<egui::Id>) {
    let mut answer = None;
    let mut ids: Vec<egui::Id> = Vec::new();
    let now = ctx.input(|i| i.time);
    let armed = d.armed(now);
    if !armed {
        ctx.request_repaint_after(std::time::Duration::from_secs_f64(d.wait_left(now) + 0.02));
    }
    let screen_h = ctx.content_rect().height();
    let modal = egui::Modal::new(egui::Id::new("hideout.tool-dialog")).show(ctx, |ui| {
        ui.set_width(520.0_f32.min(ctx.content_rect().width() - 56.0));
        ui.label(egui::RichText::new(TITLE).heading().color(c(t.text)));
        ui.add_space(6.0);
        ui.label(egui::RichText::new(calls_itself(&d.name)).color(c(t.text)));
        ui.label(egui::RichText::new(file_line(&d.id.image_path)).color(c(t.text_muted)));
        ui.add_space(8.0);
        ui.label(egui::RichText::new(ASKS).color(c(t.text)));
        egui::ScrollArea::vertical()
            .max_height((screen_h * 0.8 - 260.0).max(120.0))
            .auto_shrink([false, true])
            .show(ui, |ui| {
                for i in &d.items {
                    let colour = if i.red { c(t.danger) } else { c(t.text) };
                    ui.label(egui::RichText::new(format!("\u{2022} {}", i.does)).color(colour));
                }
            });
        ui.add_space(8.0);
        ui.label(egui::RichText::new(SAME_PROGRAM).color(c(t.text_muted)));
        ui.label(egui::RichText::new(d.footer()).color(c(t.text_muted)));
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            let allow = ui.add_enabled(
                armed,
                egui::Button::new(egui::RichText::new(ALLOW).color(c(t.accent)))
                    .stroke(egui::Stroke::new(1.0_f32, c(t.accent))),
            );
            if allow.clicked() && armed {
                answer = Some(ToolAnswer::Allow);
            }
            let not_now = ui.add(
                egui::Button::new(egui::RichText::new(NOT_NOW).color(c(t.on_accent)))
                    .fill(c(t.accent)),
            );
            if first {
                not_now.request_focus();
            }
            if not_now.clicked() {
                answer = Some(ToolAnswer::NotNow);
            }
            ids.push(allow.id);
            ids.push(not_now.id);
        });
    });
    // Tab stays inside the dialog.
    let (tab, shift) = ctx.input(|i| (i.key_pressed(egui::Key::Tab), i.modifiers.shift));
    if tab
        && !ctx
            .memory(|m| m.focused())
            .is_some_and(|f| ids.contains(&f))
    {
        let to = if shift { ids.last() } else { ids.first() };
        if let Some(to) = to {
            ctx.memory_mut(|m| m.request_focus(*to));
        }
    }
    let said = d.said();
    ctx.accesskit_node_builder(modal.response.id, |n| {
        n.set_role(egui::accesskit::Role::Dialog);
        n.set_label(TITLE);
        n.set_description(said);
    });
    if modal.should_close() || dismissed(ctx) {
        answer = Some(ToolAnswer::NotNow);
    }
    (answer, ids)
}

#[cfg(test)]
mod tests {
    use super::*;
    use hideout_router::AskItem;

    fn ask(name: &str, items: &[(&str, &str, bool)], runs: bool) -> ToolAsk {
        ToolAsk {
            id: ToolId {
                image_path: r"C:\Tools\t.exe".into(),
                image_sha256: "aa".into(),
            },
            name: name.into(),
            items: items
                .iter()
                .map(|(raw, does, red)| AskItem {
                    raw: (*raw).into(),
                    does: (*does).into(),
                    red: *red,
                })
                .collect(),
            runs_scripts: runs,
        }
    }

    #[test]
    fn tool_header_words() {
        assert_eq!(
            header_title(1),
            "A program outside Hideout asks for 1 permission"
        );
        assert_eq!(
            header_title(3),
            "A program outside Hideout asks for 3 permissions"
        );
        assert_eq!(
            header_body("Tool"),
            "It calls itself \u{201c}Tool\u{201d}. Nothing is allowed until you review it."
        );
        let h = crate::permission_dialog::header(
            crate::permission_dialog::HeaderKind::Tool,
            "",
            "Tool",
            Some((2, "")),
        );
        assert_eq!(h.title, header_title(2));
        assert_eq!(h.body, header_body("Tool"));
        assert_eq!(h.review, "Review\u{2026}");
        assert_eq!(h.dismiss, "Not now");
    }

    #[test]
    fn tool_name_cleaned_cut_quoted() {
        assert_eq!(quoted_name("Tool"), "\u{201c}Tool\u{201d}");
        // Control characters go; the name is one line.
        assert_eq!(quoted_name("A\u{7}B\nC"), "\u{201c}AB C\u{201d}");
        // Cut at 40 characters with an ellipsis, inside the quotes.
        let long = "x".repeat(80);
        let q = quoted_name(&long);
        assert!(
            q.starts_with('\u{201c}') && q.ends_with("\u{2026}\u{201d}"),
            "{q}"
        );
        assert_eq!(q.chars().count(), NAME_MAX + 2);
        // Markup in a name stays plain text.
        assert!(quoted_name("<b>hi</b>").contains("<b>hi</b>"));
    }

    #[test]
    fn tool_dialog_words_and_default_not_now() {
        let a = ask(
            "Tool",
            &[
                ("lighting.write", "change your keyboard's lighting", false),
                ("behaviors.invoke", "press keys for you", true),
            ],
            false,
        );
        let mut d = ToolDialog::from_ask(&a);
        let lines = d.lines();
        assert_eq!(lines[0], "Allow a program outside Hideout?");
        assert_eq!(
            lines[1],
            "It calls itself \u{201c}Tool\u{201d}. Hideout can't confirm that name."
        );
        assert_eq!(lines[2], r"Program file: C:\Tools\t.exe");
        assert_eq!(lines[3], "It asks to:");
        assert_eq!(
            &lines[4..6],
            ["change your keyboard's lighting", "press keys for you"]
        );
        assert_eq!(lines[6], SAME_PROGRAM);
        assert_eq!(
            lines[7],
            "Hideout remembers your answer for this program file. If the file changes, it asks again."
        );
        assert!(d.items[1].red && !d.items[0].red);
        let runner = ToolDialog::from_ask(&ask("Py", &[("lighting.write", "x", false)], true));
        assert_eq!(
            runner.footer(),
            "This program runs scripts, so Hideout asks again the next time it connects."
        );
        assert_eq!(file_line(""), "Program file: not known");

        // Not now has the focus; Enter answers it; Allow is not armed at first.
        let t = crate::theme::tokens(true);
        let ctx = egui::Context::default();
        let mut got = Vec::new();
        let mut ids = Vec::new();
        let enter = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(800.0, 600.0),
            )),
            ..Default::default()
        };
        for i in 0..3 {
            let mut raw = enter.clone();
            raw.time = Some(0.0);
            if i == 2 {
                raw.events.push(egui::Event::Key {
                    key: egui::Key::Enter,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                });
            }
            let _ = ctx.run(raw, |c| {
                let (a, v) = tool_dialog_ids(c, &t, &mut d, i == 0);
                got.push(a);
                ids = v;
            });
        }
        assert_eq!(
            ctx.memory(|m| m.focused()),
            Some(ids[1]),
            "Not now is focused"
        );
        assert_eq!(got.last().unwrap(), &Some(ToolAnswer::NotNow), "{got:?}");
        assert!(got.iter().all(|a| *a != Some(ToolAnswer::Allow)));
    }

    #[test]
    fn allow_is_armed_after_one_second() {
        let mut d = ToolDialog::from_ask(&ask("Tool", &[("lighting.write", "x", false)], false));
        assert!(!d.armed(10.0));
        assert!(!d.armed(10.9));
        assert!(d.armed(11.0));
        assert_eq!(d.wait_left(10.25), 0.75);
    }

    #[test]
    fn never_items_not_drawn() {
        let d = ToolDialog::from_ask(&ask(
            "Tool",
            &[
                ("lighting.write", "change lighting", false),
                ("key.events", "read keys", true),
                ("net", "use the network", true),
                ("files", "read files", true),
                ("not a permission", "nothing", false),
            ],
            false,
        ));
        let drawn: Vec<&str> = d.items.iter().map(|i| i.raw.as_str()).collect();
        assert_eq!(drawn, ["lighting.write"]);
        assert_eq!(d.allowed().len(), 1);
        assert!(!d.lines().iter().any(|l| l.contains("read keys")));
    }
}
