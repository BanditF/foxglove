// SPDX-License-Identifier: MIT
//! The hub's dialog for a program outside Hideout that asks for more than it may have (S1,
//! `review/design-s1-outside-tools.md` section 5). Drawn by the hub, never by the program. Its
//! name is the program's own word for itself and is shown as quoted plain text (cleaned, cut
//! at 40 characters, regular weight, never accent-colored); Hideout can't confirm it. "Not now"
//! is the default: it has the focus when the dialog opens; Enter answers the focused button, and
//! Esc and the window's close button always answer Not now.
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
/// The program that opened the connection is what is allowed, not whoever starts it.
pub const SAME_PROGRAM: &str = "Other programs that start this one can use what you allow too.";

/// The program's own name as the words show it: cleaned, cut, always in quotes. `None` when it
/// gave no name (empty, spaces or hidden characters only): it is never shown as "An app".
pub fn quoted_name(raw: &str) -> Option<String> {
    Router::clean_name_opt(raw, NAME_MAX).map(|n| format!("\u{201c}{n}\u{201d}"))
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
    match quoted_name(raw) {
        Some(q) => format!("It calls itself {q}. Nothing is allowed until you review it."),
        None => "It gives no name. Nothing is allowed until you review it.".into(),
    }
}

/// What the program calls itself, and that Hideout can't confirm it.
pub fn calls_itself(raw: &str) -> String {
    match quoted_name(raw) {
        Some(q) => format!("It calls itself {q}. Hideout can't confirm that name."),
        None => "It gives no name, so check the program file below.".into(),
    }
}

/// The longest path shown whole, in characters; a longer one keeps its start and its end.
pub const FILE_SHOWN_MAX: usize = 160;
/// How much of a long path's start is kept.
const FILE_HEAD: usize = 40;
/// How much of a long path's end is kept: the file name is always visible.
const FILE_TAIL: usize = 110;

/// The program file line. A path is plain text: the characters a name never shows (controls,
/// direction marks, zero-width characters) are dropped, and a path over
/// [`FILE_SHOWN_MAX`] characters shows its first 40, an ellipsis and its last 110.
pub fn file_line(path: &str) -> String {
    let plain: String = path
        .chars()
        .filter(|c| !hideout_contract::module::hidden_in_name(*c))
        .collect();
    if plain.trim().is_empty() {
        return "Program file: not known".into();
    }
    let n = plain.chars().count();
    if n <= FILE_SHOWN_MAX {
        return format!("Program file: {plain}");
    }
    let head: String = plain.chars().take(FILE_HEAD).collect();
    let tail: String = plain.chars().skip(n - FILE_TAIL).collect();
    format!("Program file: {head}\u{2026}{tail}")
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

/// Where Tab sends the focus, when it must be moved. `ids` is Allow then Not now. Before Allow is
/// armed only Not now can hold the focus; after, focus that is outside the dialog is brought
/// back to its first (last with Shift) button.
pub(crate) fn tab_target(
    ids: &[egui::Id],
    armed: bool,
    shift: bool,
    focused: Option<egui::Id>,
) -> Option<egui::Id> {
    let not_now = *ids.last()?;
    if !armed {
        return Some(not_now);
    }
    if focused.is_some_and(|f| ids.contains(&f)) {
        return None;
    }
    if shift {
        ids.last().copied()
    } else {
        ids.first().copied()
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
    // Tab stays inside the dialog, and never on Allow before it is armed.
    let (tab, shift) = ctx.input(|i| (i.key_pressed(egui::Key::Tab), i.modifiers.shift));
    if tab {
        let focused = ctx.memory(|m| m.focused());
        if let Some(to) = tab_target(&ids, armed, shift, focused) {
            ctx.memory_mut(|m| m.request_focus(to));
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
            conn: None,
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
        assert_eq!(quoted_name("Tool").unwrap(), "\u{201c}Tool\u{201d}");
        // Control characters go; the name is one line.
        assert_eq!(quoted_name("A\u{7}B\nC").unwrap(), "\u{201c}AB C\u{201d}");
        // Cut at 40 characters with an ellipsis, inside the quotes.
        let long = "x".repeat(80);
        let q = quoted_name(&long).unwrap();
        assert!(
            q.starts_with('\u{201c}') && q.ends_with("\u{2026}\u{201d}"),
            "{q}"
        );
        assert_eq!(q.chars().count(), NAME_MAX + 2);
        // Markup in a name stays plain text.
        assert!(quoted_name("<b>hi</b>").unwrap().contains("<b>hi</b>"));
    }

    #[test]
    fn a_blank_name_is_never_an_app() {
        for blank in ["", "   ", "\u{200b}\u{202e} \u{feff}", "\t\n"] {
            assert_eq!(quoted_name(blank), None, "{blank:?}");
            assert_eq!(
                header_body(blank),
                "It gives no name. Nothing is allowed until you review it."
            );
            assert_eq!(
                calls_itself(blank),
                "It gives no name, so check the program file below."
            );
            assert!(!header_body(blank).contains("An app"));
        }
    }

    #[test]
    fn the_file_line_is_plain_and_keeps_the_file_name() {
        // A direction override can't turn gpj.exe into exe.jpg.
        assert_eq!(
            file_line("C:\\T\\\u{202e}gpj.exe\u{200b}"),
            "Program file: C:\\T\\gpj.exe"
        );
        assert_eq!(file_line("\u{202e}\u{200b}"), "Program file: not known");
        // A path over 160 characters keeps its first 40 and its last 110.
        let long = format!("C:\\{}\\tool.exe", "d\\".repeat(200));
        let line = file_line(&long);
        let shown = line.strip_prefix("Program file: ").unwrap();
        assert_eq!(shown.chars().count(), 40 + 1 + 110);
        assert!(shown.ends_with("tool.exe"), "{shown}");
        assert!(shown.starts_with("C:\\d\\d\\"), "{shown}");
        assert_eq!(shown.chars().nth(40), Some('\u{2026}'));
        // Exactly 160 characters is shown whole.
        let p = "p".repeat(160);
        assert_eq!(file_line(&p), format!("Program file: {p}"));
        // The accessible text carries the same clean path.
        let mut a = ask("Tool", &[("lighting.write", "x", false)], false);
        a.id.image_path = "C:\\\u{202e}x.exe".into();
        assert!(!ToolDialog::from_ask(&a).said().contains('\u{202e}'));
    }

    #[test]
    fn tab_before_arm_keeps_the_focus_on_not_now() {
        let allow = egui::Id::new("allow");
        let not_now = egui::Id::new("not_now");
        let ids = [allow, not_now];
        // Not armed: Tab, from anywhere, lands on Not now.
        for focused in [None, Some(allow), Some(not_now), Some(egui::Id::new("x"))] {
            for shift in [false, true] {
                assert_eq!(tab_target(&ids, false, shift, focused), Some(not_now));
            }
        }
        // Armed: focus inside the dialog moves on its own; outside comes back in.
        assert_eq!(tab_target(&ids, true, false, Some(allow)), None);
        assert_eq!(tab_target(&ids, true, false, None), Some(allow));
        assert_eq!(tab_target(&ids, true, true, None), Some(not_now));
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
        assert_eq!(
            SAME_PROGRAM,
            "Other programs that start this one can use what you allow too."
        );

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

    /// A click on Allow before it is armed is ignored; the same click after one second allows.
    #[test]
    fn a_click_on_allow_before_one_second_does_nothing() {
        let t = crate::theme::tokens(true);
        let ctx = egui::Context::default();
        let mut d = ToolDialog::from_ask(&ask("Tool", &[("lighting.write", "x", false)], false));
        let screen = Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(800.0, 600.0),
        ));
        let frame = |ctx: &egui::Context,
                     d: &mut ToolDialog,
                     time: f64,
                     first: bool,
                     events: Vec<egui::Event>|
         -> (Option<ToolAnswer>, Vec<egui::Id>) {
            let raw = egui::RawInput {
                screen_rect: screen,
                time: Some(time),
                events,
                ..Default::default()
            };
            let mut ans = (None, Vec::new());
            let _ = ctx.run(raw, |c| {
                ans = tool_dialog_ids(c, &t, d, first);
            });
            ans
        };
        let (_, ids) = frame(&ctx, &mut d, 0.0, true, vec![]);
        for k in 1..4 {
            frame(&ctx, &mut d, 0.05 * k as f64, false, vec![]);
        }
        let at = ctx
            .read_response(ids[0])
            .expect("Allow was drawn")
            .rect
            .center();
        let click = |down: bool| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed: down,
            modifiers: egui::Modifiers::NONE,
        };
        let e1 = frame(
            &ctx,
            &mut d,
            0.1,
            false,
            vec![egui::Event::PointerMoved(at)],
        )
        .0;
        let e2 = frame(&ctx, &mut d, 0.2, false, vec![click(true)]).0;
        let e3 = frame(&ctx, &mut d, 0.3, false, vec![click(false)]).0;
        let early = [e1, e2, e3];
        assert!(
            early.iter().all(|a| *a != Some(ToolAnswer::Allow)),
            "{early:?}"
        );
        // Armed: the same click allows.
        frame(
            &ctx,
            &mut d,
            2.0,
            false,
            vec![egui::Event::PointerMoved(at)],
        );
        frame(&ctx, &mut d, 2.1, false, vec![click(true)]);
        let late = frame(&ctx, &mut d, 2.2, false, vec![click(false)]).0;
        assert_eq!(late, Some(ToolAnswer::Allow));
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
