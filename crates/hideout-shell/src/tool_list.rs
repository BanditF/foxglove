// SPDX-License-Identifier: MIT
//! "Programs outside Hideout" at the bottom of the Apps page (#1470): the program files the
//! user allowed to do more than read, newest first, each with a Forget button. Drawn by the hub
//! from what it holds and from its own record: every answer has a row, so every answer can be
//! forgotten. A file's name and path are untrusted plain text (the dialog's
//! [`file_line`] rules); nothing a program calls itself is shown here. The list is built from
//! what the caller read; nothing here reads or hashes a file.
//! Forget asks nothing and opens no dialog: the caller takes the answer away, then calls
//! [`ToolList::after_forget`] with whether the record was saved.

use crate::egui;
use crate::keyboard_grant_ui::rgb as c;
use crate::theme::Tokens;
use crate::tool_dialog::{file_line, ToolItem};

pub const HEADING: &str = "Programs outside Hideout";
pub const HELPER: &str = "You allowed these programs to do more than read. Hideout knows each one by its file. A program that runs scripts isn't listed: its answer ends when it closes.";
/// Shown only when the list holds [`FULL`] rows.
pub const FULL_LINE: &str =
    "Hideout keeps 64 programs. Allowing another one forgets the last one in this list.";
pub const MAY: &str = "It may:";
pub const FORGET: &str = "Forget";
pub const FORGET_HOVER: &str = "Hideout forgets what you allowed this program.";
pub const MISSING: &str = "This file isn't here now. If it comes back unchanged, your answer still counts. Press Forget to remove it.";
pub const DATE_UNKNOWN: &str = "Allowed in Hideout, date not known";
/// In place of the count and the date on a row whose Forget was not saved: the program can do
/// none of it in this run.
pub const NOT_NOW: &str = "Not allowed now";
/// In place of "It may:" on a row with nothing Hideout can list (an app that offered it is gone,
/// say): the row is there so the answer can be forgotten.
pub const NO_ITEMS: &str =
    "Hideout can't show what this program was allowed. Press Forget to remove it.";
/// The note when Forget could not be saved a second time for the same row.
pub const STILL_NOT_SAVED: &str =
    "Hideout still couldn't save that. Quit Hideout and open it again, then press Forget.";
/// The name of a file whose path leaves nothing to show.
pub const NO_NAME: &str = "A program";

/// The most programs Hideout keeps.
pub const FULL: usize = 64;
/// The longest file name shown whole, in characters.
pub const NAME_MAX: usize = 60;
const NAME_HEAD: usize = 40;
/// How much of a long name's end is kept: the extension stays visible.
const NAME_TAIL: usize = 19;

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// The file name of a program file: the path's last part, as plain text. The characters a name
/// never shows are dropped; a name over [`NAME_MAX`] characters keeps its first 40 and its last
/// 19 around an ellipsis; a path with nothing left is "A program".
pub fn file_name(path: &str) -> String {
    let plain: String = path
        .chars()
        .filter(|c| !hideout_contract::module::hidden_in_name(*c))
        .collect();
    let last = plain.rsplit(['\\', '/']).next().unwrap_or("").trim();
    if last.is_empty() {
        return NO_NAME.into();
    }
    let n = last.chars().count();
    if n <= NAME_MAX {
        return last.to_string();
    }
    let head: String = last.chars().take(NAME_HEAD).collect();
    let tail: String = last.chars().skip(n - NAME_TAIL).collect();
    format!("{head}\u{2026}{tail}")
}

/// The days in a month of a year (February by the leap rule); 0 for no such month.
pub fn days_in(year: u32, month: usize) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400)) => {
            29
        }
        2 => 28,
        _ => 0,
    }
}

/// "Allowed in Hideout on 17 Nov 2026" for `2026-11-17T09:00:00Z`. A missing time, or one that
/// is no real date (30 Feb, 31 Apr), says the date is not known: never a zero date, never
/// "today". The day is the one written: the caller gives the PC's local day.
pub fn date_line(allowed_at: Option<&str>) -> String {
    let words = |at: &str| -> Option<String> {
        let d = at.get(..10)?;
        if !d.is_ascii() || (at.len() > 10 && !at[10..].starts_with('T')) {
            return None;
        }
        let mut p = d.split('-');
        let (y, m, day) = (p.next()?, p.next()?, p.next()?);
        if y.len() != 4 || m.len() != 2 || day.len() != 2 {
            return None;
        }
        let (y, m, day): (u32, usize, u32) = (y.parse().ok()?, m.parse().ok()?, day.parse().ok()?);
        (y >= 2000 && (1..=days_in(y, m)).contains(&day))
            .then(|| format!("{day} {} {y}", MONTHS[m - 1]))
    };
    match allowed_at.and_then(words) {
        Some(w) => format!("Allowed in Hideout on {w}"),
        None => DATE_UNKNOWN.into(),
    }
}

/// The note after Forget.
pub fn forgot(name: &str) -> String {
    format!("Hideout forgot {name}. It can't do those things now, and it asks again the next time it starts.")
}

/// The note when Forget took the answer away but the record could not be saved.
pub fn not_saved(name: &str) -> String {
    format!("Hideout forgot {name} for now but couldn't save that. Press Forget again.")
}

/// The note when a listed program connected with a changed file.
pub fn file_changed(name: &str) -> String {
    format!("{name} changed, so Hideout forgot what you allowed it. If it asks again, press Review\u{2026} at the top of this page.")
}

/// The section's accessible name.
pub fn section_said(n: usize) -> String {
    format!("{HEADING}, {n} program{}", if n == 1 { "" } else { "s" })
}

/// One listed program file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolRow {
    /// The path as the record holds it: what Forget names. Never drawn as it is.
    pub path: String,
    /// The file name, cleaned.
    pub name: String,
    /// "Program file: ...", cleaned.
    pub file: String,
    /// What it may do, sensitive items first.
    pub items: Vec<ToolItem>,
    /// "Allowed in Hideout on ..." or that the date is not known.
    pub date: String,
    /// The file was looked for and is not there.
    pub missing: bool,
    /// The record is still in the file but Hideout does not honour it in this run: a Forget
    /// that could not be saved.
    pub unsaved: bool,
}

impl ToolRow {
    /// The row for one answer. `items` are what it lists that a program outside Hideout can be
    /// allowed now; an answer with none still has a row, so it can be forgotten.
    pub fn new(path: &str, items: Vec<ToolItem>, allowed_at: Option<&str>) -> ToolRow {
        let (red, plain): (Vec<ToolItem>, Vec<ToolItem>) = items.into_iter().partition(|i| i.red);
        ToolRow {
            path: path.to_string(),
            name: file_name(path),
            file: file_line(path),
            items: red.into_iter().chain(plain).collect(),
            date: date_line(allowed_at),
            missing: false,
            unsaved: false,
        }
    }

    /// "2 allowed": the lines drawn. Nothing for a row with no lines: never "0 allowed".
    pub fn count(&self) -> Option<String> {
        (!self.items.is_empty()).then(|| format!("{} allowed", self.items.len()))
    }

    /// The row's accessible name.
    pub fn said(&self) -> String {
        let mut s = self.name.clone();
        if self.unsaved {
            s.push_str(", ");
            s.push_str(NOT_NOW);
        } else {
            if let Some(n) = self.count() {
                s.push_str(", ");
                s.push_str(&n);
            }
            s.push_str(", ");
            s.push_str(&self.date);
        }
        if self.missing {
            s.push_str(", file not here now");
        }
        s
    }

    /// The Forget button's accessible name.
    pub fn forget_said(&self) -> String {
        format!("{FORGET} {}", self.name)
    }

    /// Every line the row shows, in order.
    pub fn lines(&self) -> Vec<String> {
        let mut out = vec![self.name.clone(), self.file.clone()];
        if self.unsaved {
            out.push(NOT_NOW.to_string());
        } else {
            out.extend(self.count());
            out.push(self.date.clone());
        }
        if self.items.is_empty() {
            out.push(NO_ITEMS.to_string());
        } else {
            out.push(MAY.to_string());
            out.extend(self.items.iter().map(ToolItem::line));
        }
        if self.missing {
            out.push(MISSING.to_string());
        }
        out
    }
}

/// The section's state: the rows, the note lines and where the focus goes after Forget.
#[derive(Clone, Debug, Default)]
pub struct ToolList {
    /// Newest allowed first.
    rows: Vec<ToolRow>,
    /// The line after a Forget, and whether it asks the user to act (the save failed).
    note: Option<(String, bool)>,
    /// The row whose last Forget could not be saved.
    failed: Option<String>,
    /// The newest "file changed" line, and whether it was on screen yet.
    changed: Option<(String, bool)>,
    /// The last row was just forgotten: the heading and the note stay until the user leaves.
    keep_head: bool,
    /// The row whose Forget button takes the focus at the next draw.
    focus: Option<String>,
    /// The frame the section was last drawn in.
    drawn: Option<u64>,
}

impl ToolList {
    /// Replaces the rows. `oldest_first` is the record's own order; the list shows the newest
    /// first.
    pub fn set_rows(&mut self, oldest_first: Vec<ToolRow>) {
        self.rows = oldest_first;
        self.rows.reverse();
    }

    pub fn rows(&self) -> &[ToolRow] {
        &self.rows
    }

    /// Whether anything is drawn: no rows draw nothing, except right after the last row was
    /// forgotten or while a "file changed" line waits to be read.
    pub fn shown(&self) -> bool {
        !self.rows.is_empty() || self.keep_head || self.changed.is_some()
    }

    /// Marks the rows whose file was looked for and is not there.
    pub fn set_missing(&mut self, missing: &[String]) {
        for r in &mut self.rows {
            r.missing = missing.contains(&r.path);
        }
    }

    /// Whether the "Hideout keeps 64 programs" line shows: only with a full list. Every answer
    /// has a row, so the rows are the answers kept.
    pub fn full(&self) -> bool {
        self.rows.len() >= FULL
    }

    /// The note lines, in the order drawn.
    pub fn notes(&self) -> Vec<String> {
        self.changed
            .iter()
            .map(|(line, _)| line.clone())
            .chain(self.note.iter().map(|(line, _)| line.clone()))
            .collect()
    }

    /// The row whose Forget button is about to take the focus.
    pub fn focus(&self) -> Option<&str> {
        self.focus.as_deref()
    }

    /// A listed program connected with a changed file and its record went. Only the newest
    /// such line is kept.
    pub fn file_changed(&mut self, path: &str) {
        self.changed = Some((file_changed(&file_name(path)), false));
    }

    /// The user pressed Forget on the row for `path`. `saved` is whether the record was
    /// written; `now` are the rows read after it, oldest first. Saved: the row is gone, the
    /// note says so and the focus goes to the row that took its place, else the row above,
    /// else nowhere. Not saved: the row stays (as `now` has it: not allowed now), the note says
    /// what to do, in other words the second time, and the focus stays on the same button.
    pub fn after_forget(&mut self, path: &str, saved: bool, now: Vec<ToolRow>) {
        let at = self.rows.iter().position(|r| r.path == path);
        let name = at.map_or_else(|| file_name(path), |i| self.rows[i].name.clone());
        if !saved {
            let again = self.failed.as_deref() == Some(path);
            let line = if again {
                STILL_NOT_SAVED.to_string()
            } else {
                not_saved(&name)
            };
            self.note = Some((line, true));
            self.failed = Some(path.to_string());
            self.focus = Some(path.to_string());
            if now.iter().any(|r| r.path == path) {
                self.set_rows(now);
            } else if let Some(i) = at {
                self.rows[i].unsaved = true;
            }
            return;
        }
        self.failed = None;
        self.set_rows(now);
        self.note = Some((forgot(&name), false));
        let at = at.unwrap_or(0);
        self.focus = self
            .rows
            .get(at)
            .or(self.rows.last())
            .map(|r| r.path.clone());
        self.keep_head = self.rows.is_empty();
    }

    /// Called when the section is about to be drawn in frame `frame`. A gap since the last
    /// drawn frame means the user was on another page: the notes they saw go.
    pub fn seen(&mut self, frame: u64) {
        if self.drawn.is_some_and(|d| frame > d + 1) {
            self.left();
        }
        self.drawn = Some(frame);
    }

    /// The user left the page: the Forget note goes, and the "file changed" line once it was
    /// on screen.
    pub fn left(&mut self) {
        self.note = None;
        self.keep_head = false;
        self.focus = None;
        if self.changed.as_ref().is_some_and(|(_, seen)| *seen) {
            self.changed = None;
        }
    }
}

/// Text one point under body size.
fn fine(ui: &egui::Ui, text: impl Into<String>) -> egui::RichText {
    let body = ui
        .style()
        .text_styles
        .get(&egui::TextStyle::Body)
        .map_or(14.0, |f| f.size);
    egui::RichText::new(text).size(body - 1.0)
}

/// Draws the section under the app rows. Returns the path of the row whose Forget was pressed.
/// Nothing is drawn for an empty list ([`ToolList::shown`]).
pub fn tool_list_ui(ui: &mut egui::Ui, t: &Tokens, list: &mut ToolList) -> Option<String> {
    use egui::RichText;
    list.seen(ui.ctx().cumulative_frame_nr());
    if !list.shown() {
        return None;
    }
    let mut pressed = None;
    let focus = list.focus.take();
    // A key held down repeats, and the focus moves to the next row's Forget: only a fresh press
    // counts, so a held key forgets one program.
    let held = ui.input(|i| {
        i.events.iter().any(|e| {
            matches!(
                e,
                egui::Event::Key {
                    key: egui::Key::Enter | egui::Key::Space,
                    pressed: true,
                    repeat: true,
                    ..
                }
            )
        })
    });
    let said = section_said(list.rows.len());
    let section = ui.push_id("hub.tools", |ui| {
        ui.vertical(|ui| {
            ui.add_space(20.0);
            ui.label(RichText::new(HEADING).size(17.0).color(c(t.text)));
            ui.add_space(4.0);
            ui.label(fine(ui, HELPER).color(c(t.text_muted)));
            if list.full() {
                ui.label(fine(ui, FULL_LINE).color(c(t.text_muted)));
            }
            // Polite live regions: a screen reader hears them without the focus moving.
            let live = |ui: &mut egui::Ui, line: &str, colour: crate::theme::Rgb| {
                ui.add_space(4.0);
                let note = ui.label(RichText::new(line).color(c(colour)));
                let said = line.to_string();
                ui.ctx().accesskit_node_builder(note.id, |n| {
                    n.set_live(egui::accesskit::Live::Polite);
                    n.set_label(said);
                });
                ui.is_rect_visible(note.rect)
            };
            if let Some((line, seen)) = list.changed.as_mut() {
                // Read only once it was on screen: drawn below the window's edge is not seen.
                *seen |= live(ui, line, t.text_muted);
            }
            if let Some((line, act)) = &list.note {
                // The one note that asks the user to act stands out.
                live(ui, line, if *act { t.attention } else { t.text_muted });
            }
            ui.add_space(8.0);
            for r in &list.rows {
                let row = ui.push_id(("hub.tools.row", &r.path), |ui| {
                    egui::Frame::new()
                        .fill(c(t.surface))
                        .stroke(egui::Stroke::new(1.0_f32, c(t.border)))
                        .corner_radius(egui::CornerRadius::same(6))
                        .inner_margin(egui::Margin::symmetric(16, 10))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.horizontal(|ui| {
                                // Room for the button, so a long path wraps beside it.
                                let text_w = (ui.available_width() - 96.0).max(0.0);
                                ui.vertical(|ui| {
                                    ui.set_max_width(text_w);
                                    ui.label(RichText::new(&r.name).size(15.0).color(c(t.text)));
                                    ui.label(RichText::new(&r.file).color(c(t.text_muted)));
                                    if r.unsaved {
                                        ui.label(RichText::new(NOT_NOW).color(c(t.text_muted)));
                                    } else {
                                        ui.horizontal_wrapped(|ui| {
                                            if let Some(n) = r.count() {
                                                ui.label(RichText::new(n).strong().color(c(t.ok)));
                                            }
                                            ui.label(RichText::new(&r.date).color(c(t.text_muted)));
                                        });
                                    }
                                    if r.items.is_empty() {
                                        ui.label(RichText::new(NO_ITEMS).color(c(t.text_muted)));
                                    } else {
                                        ui.label(RichText::new(MAY).color(c(t.text)));
                                    }
                                    for i in &r.items {
                                        let colour = if i.red { t.danger } else { t.text };
                                        ui.label(RichText::new(i.line()).color(c(colour)));
                                    }
                                    if r.missing {
                                        ui.label(fine(ui, MISSING).color(c(t.attention)));
                                    }
                                });
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        let b = ui.button(FORGET);
                                        let (said, desc) = (r.forget_said(), r.file.clone());
                                        ui.ctx().accesskit_node_builder(b.id, |n| {
                                            n.set_label(said);
                                            n.set_description(desc);
                                        });
                                        if focus.as_deref() == Some(r.path.as_str()) {
                                            b.request_focus();
                                            b.scroll_to_me(None);
                                        }
                                        if b.clicked() && !held {
                                            pressed = Some(r.path.clone());
                                        }
                                        b.on_hover_text(FORGET_HOVER);
                                    },
                                );
                            });
                        })
                        .response
                });
                let said = r.said();
                ui.ctx().accesskit_node_builder(row.inner.id, |n| {
                    n.set_role(egui::accesskit::Role::Group);
                    n.set_label(said);
                });
                ui.add_space(8.0);
            }
        })
        .response
    });
    ui.ctx().accesskit_node_builder(section.inner.id, |n| {
        n.set_role(egui::accesskit::Role::Group);
        n.set_label(said);
    });
    pressed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(does: &str, red: bool) -> ToolItem {
        ToolItem {
            raw: does.replace(' ', "."),
            does: does.into(),
            red,
        }
    }

    fn row(path: &str) -> ToolRow {
        ToolRow::new(
            path,
            vec![item("change your keyboard's lighting", false)],
            Some("2026-11-17T09:00:00Z"),
        )
    }

    fn list(paths: &[&str]) -> ToolList {
        let mut l = ToolList::default();
        l.set_rows(paths.iter().map(|p| row(p)).collect());
        l
    }

    /// Draws the section once; returns the accessibility nodes' labels and descriptions and
    /// whether anything took space.
    fn draw(ctx: &egui::Context, l: &mut ToolList) -> (Vec<(String, String)>, f32) {
        let (said, used, _) = draw_with(ctx, l, vec![]);
        (said, used)
    }

    /// [`draw`] with input events; also the row whose Forget was pressed.
    fn draw_with(
        ctx: &egui::Context,
        l: &mut ToolList,
        events: Vec<egui::Event>,
    ) -> (Vec<(String, String)>, f32, Option<String>) {
        ctx.enable_accesskit();
        let t = crate::theme::tokens(true);
        let mut used = 0.0;
        let mut pressed = None;
        let out = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(900.0, 700.0),
                )),
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let before = ui.cursor().top();
                    pressed = tool_list_ui(ui, &t, l);
                    used = ui.cursor().top() - before;
                });
            },
        );
        let nodes = out.platform_output.accesskit_update.unwrap().nodes;
        let said = nodes
            .iter()
            .map(|(_, n)| {
                (
                    // A plain line of text is the node's value; a named control has a label.
                    n.label().or(n.value()).unwrap_or_default().to_string(),
                    n.description().unwrap_or_default().to_string(),
                )
            })
            .collect();
        (said, used, pressed)
    }

    fn enter(repeat: bool) -> Vec<egui::Event> {
        vec![egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: None,
            pressed: true,
            repeat,
            modifiers: egui::Modifiers::NONE,
        }]
    }

    /// Forget asks nothing and the focus moves to the next row's Forget: a held key must not
    /// walk down the list.
    #[test]
    fn a_held_key_forgets_one_program() {
        let mut l = list(&[r"C:\a.exe", r"C:\b.exe", r"C:\c.exe"]);
        let ctx = egui::Context::default();
        l.focus = Some(r"C:\c.exe".into());
        draw(&ctx, &mut l);
        let mut forgot = Vec::new();
        for frame in 0..6 {
            // One fresh press, then the key repeats while it is held.
            let (_, _, pressed) = draw_with(&ctx, &mut l, enter(frame > 0));
            if let Some(path) = pressed {
                let left: Vec<ToolRow> = l
                    .rows()
                    .iter()
                    .rev()
                    .filter(|r| r.path != path)
                    .cloned()
                    .collect();
                l.after_forget(&path, true, left);
                forgot.push(path);
            }
        }
        assert_eq!(forgot, [r"C:\c.exe"]);
        assert_eq!(l.rows().len(), 2);
        // The key is let go. The focus is on the next row's button, and a fresh press there
        // counts.
        let mut up = enter(false);
        if let egui::Event::Key { pressed, .. } = &mut up[0] {
            *pressed = false;
        }
        assert_eq!(draw_with(&ctx, &mut l, up).2, None);
        let (_, _, pressed) = draw_with(&ctx, &mut l, enter(false));
        assert_eq!(pressed.as_deref(), Some(r"C:\b.exe"));
    }

    #[test]
    fn tool_row_words_and_order() {
        let r = ToolRow::new(
            r"C:\Tools\lights.exe",
            vec![
                item("change your keyboard's lighting", false),
                item("run your keyboard's actions", true),
                item("make pictures", false),
            ],
            Some("2026-11-17T09:00:00Z"),
        );
        assert_eq!(
            r.lines(),
            [
                "lights.exe",
                r"Program file: C:\Tools\lights.exe",
                "3 allowed",
                "Allowed in Hideout on 17 Nov 2026",
                "It may:",
                // The sensitive item first and marked in words, the others in their own order.
                "\u{2022} Sensitive: run your keyboard's actions",
                "\u{2022} change your keyboard's lighting",
                "\u{2022} make pictures",
            ]
        );
        // An answer with nothing Hideout can list still has a row, so it can be forgotten:
        // no count (never "0 allowed") and a line that says what to do.
        let bare = ToolRow::new(r"C:\Tools\x.exe", vec![], None);
        assert_eq!(
            bare.lines(),
            [
                "x.exe",
                r"Program file: C:\Tools\x.exe",
                "Allowed in Hideout, date not known",
                "Hideout can't show what this program was allowed. Press Forget to remove it.",
            ]
        );
        assert_eq!(bare.said(), "x.exe, Allowed in Hideout, date not known");
        // The list shows the newest allowed first: the record's order, turned round.
        let l = list(&[r"C:\old.exe", r"C:\mid.exe", r"C:\new.exe"]);
        let names: Vec<&str> = l.rows().iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["new.exe", "mid.exe", "old.exe"]);
        // One noun for apps, and never the developer words.
        for line in [
            HEADING,
            HELPER,
            FULL_LINE,
            MISSING,
            FORGET_HOVER,
            NOT_NOW,
            NO_ITEMS,
            STILL_NOT_SAVED,
            &forgot("a.exe"),
            &not_saved("a.exe"),
            &file_changed("a.exe"),
        ] {
            let low = line.to_lowercase();
            assert!(!low.contains("module") && !low.contains("plugin"), "{line}");
        }
    }

    #[test]
    fn unknown_date_says_not_known_never_zero() {
        assert_eq!(date_line(None), "Allowed in Hideout, date not known");
        for odd in [
            "",
            "0000-00-00T00:00:00Z",
            "1970-01-01T00:00:00Z",
            "2026-13-01T00:00:00Z",
            "2026-11-00T00:00:00Z",
            "2026-11-31T00:00:00Z",
            "2026-02-30T00:00:00Z",
            "2027-02-29T00:00:00Z",
            "2100-02-29",
            "today",
            "2026-11-17 and more words",
            "\u{202e}2026-11-17",
        ] {
            assert_eq!(date_line(Some(odd)), DATE_UNKNOWN, "{odd:?}");
        }
        assert_eq!(
            date_line(Some("2026-11-07T23:59:59Z")),
            "Allowed in Hideout on 7 Nov 2026"
        );
        assert_eq!(
            date_line(Some("2028-02-29")),
            "Allowed in Hideout on 29 Feb 2028"
        );
        let r = ToolRow::new(
            r"C:\Tools\old.exe",
            vec![item("make pictures", false)],
            None,
        );
        assert_eq!(r.date, "Allowed in Hideout, date not known");
        assert_eq!(
            r.said(),
            "old.exe, 1 allowed, Allowed in Hideout, date not known"
        );
    }

    #[test]
    fn path_and_file_name_drop_direction_marks() {
        // A right-to-left override would show "exe.txt" for a file that ends in ".exe".
        let r = row("C:\\Tools\\safe\u{202e}txt.exe\u{200b}\u{7}");
        assert_eq!(r.name, "safetxt.exe");
        assert_eq!(r.file, r"Program file: C:\Tools\safetxt.exe");
        // What Forget names is the record's own path, untouched.
        assert_eq!(r.path, "C:\\Tools\\safe\u{202e}txt.exe\u{200b}\u{7}");
        assert_eq!(file_name("C:/Tools/a b.exe"), "a b.exe");
        assert_eq!(file_name("lights.exe"), "lights.exe");
        for nothing in ["", r"C:\Tools\", "\u{202e}\u{200b}", r"C:\Tools\   "] {
            assert_eq!(file_name(nothing), "A program", "{nothing:?}");
        }
        // Text that looks like markup or a link stays the text it is.
        assert_eq!(
            file_name(r"C:\T\[Hideout](x) **Allow**.exe"),
            "[Hideout](x) **Allow**.exe"
        );
    }

    #[test]
    fn long_file_name_keeps_extension() {
        let long = format!(r"C:\Tools\{}.exe", "n".repeat(100));
        let name = file_name(&long);
        assert_eq!(name.chars().count(), 60);
        assert!(name.starts_with(&"n".repeat(40)), "{name}");
        assert!(name.ends_with("nnn.exe"), "{name}");
        assert_eq!(name.chars().nth(40), Some('\u{2026}'));
        // Exactly at the limit: whole.
        let at = format!("{}.exe", "n".repeat(56));
        assert_eq!(file_name(&at), at);
        // A long path keeps its start and its end, as in the dialog.
        let deep = format!(r"C:\{}\tool.exe", "d".repeat(300));
        let r = row(&deep);
        assert!(r.file.ends_with(r"\tool.exe") && r.file.contains('\u{2026}'));
        assert_eq!(r.name, "tool.exe");
    }

    #[test]
    fn missing_file_line() {
        let mut r = row(r"C:\Tools\gone.exe");
        assert!(!r.lines().contains(&MISSING.to_string()));
        r.missing = true;
        assert_eq!(
            r.lines().last().map(String::as_str),
            Some("This file isn't here now. If it comes back unchanged, your answer still counts. Press Forget to remove it.")
        );
        assert_eq!(
            r.said(),
            "gone.exe, 1 allowed, Allowed in Hideout on 17 Nov 2026, file not here now"
        );
    }

    #[test]
    fn empty_list_draws_nothing() {
        let mut l = ToolList::default();
        assert!(!l.shown());
        let ctx = egui::Context::default();
        let (said, used) = draw(&ctx, &mut l);
        assert_eq!(used, 0.0);
        assert!(!said.iter().any(|(label, _)| label.contains(HEADING)));
        // Right after the last row was forgotten the heading and the note stay.
        let mut l = list(&[r"C:\only.exe"]);
        l.after_forget(r"C:\only.exe", true, vec![]);
        assert!(l.shown() && l.rows().is_empty());
        assert_eq!(l.focus(), None, "no row is left to take the focus");
        let (said, used) = draw(&ctx, &mut l);
        assert!(used > 0.0);
        let labels: Vec<&str> = said.iter().map(|(label, _)| label.as_str()).collect();
        assert!(labels.contains(&"Programs outside Hideout, 0 programs"));
        assert!(labels.contains(
            &"Hideout forgot only.exe. It can't do those things now, and it asks again the next time it starts."
        ));
        // The user leaves the page: after that nothing is drawn.
        l.left();
        assert!(!l.shown());
        assert_eq!(draw(&ctx, &mut l).1, 0.0);
    }

    #[test]
    fn the_notes_go_when_the_user_was_on_another_page() {
        let mut l = list(&[r"C:\a.exe", r"C:\b.exe"]);
        l.seen(10);
        l.after_forget(r"C:\a.exe", true, vec![row(r"C:\b.exe")]);
        // Drawn frame after frame: the note stays.
        l.seen(11);
        l.seen(12);
        assert_eq!(l.notes().len(), 1);
        // Frames went by without the section: the user was elsewhere.
        l.seen(20);
        assert!(l.notes().is_empty());
        // A "file changed" line that came while the user was elsewhere waits to be read.
        l.file_changed("C:\\Tools\\li\u{202e}ghts.exe");
        l.seen(40);
        assert_eq!(
            l.notes(),
            ["lights.exe changed, so Hideout forgot what you allowed it. If it asks again, press Review\u{2026} at the top of this page."]
        );
        let ctx = egui::Context::default();
        draw(&ctx, &mut l);
        l.left();
        assert!(l.notes().is_empty());
        // Only the newest is kept.
        l.file_changed(r"C:\one.exe");
        l.file_changed(r"C:\two.exe");
        assert_eq!(l.notes().len(), 1);
        assert!(l.notes()[0].starts_with("two.exe changed"));
    }

    #[test]
    fn forget_write_failure_keeps_row_and_says_so() {
        let mut l = list(&[r"C:\a.exe", r"C:\b.exe"]);
        l.after_forget(r"C:\b.exe", false, vec![]);
        assert_eq!(l.rows().len(), 2, "the row stays");
        assert_eq!(
            l.notes(),
            ["Hideout forgot b.exe for now but couldn't save that. Press Forget again."]
        );
        assert_eq!(
            l.focus(),
            Some(r"C:\b.exe"),
            "the same button keeps the focus"
        );
        // The program can do none of it in this run, and the row says so.
        let b = l.rows()[0].clone();
        assert!(b.unsaved && !l.rows()[1].unsaved);
        assert_eq!(b.said(), "b.exe, Not allowed now");
        assert_eq!(
            b.lines(),
            [
                "b.exe",
                r"Program file: C:\b.exe",
                "Not allowed now",
                "It may:",
                "\u{2022} change your keyboard's lighting",
            ]
        );
        let ctx = egui::Context::default();
        let (said, _) = draw(&ctx, &mut l);
        assert!(said.iter().any(|(l, _)| l == "Not allowed now"), "{said:?}");
        assert!(!said.iter().any(|(l, _)| l == "1 allowed, b.exe"));
        // A second failure says something new, with a new next step.
        l.after_forget(r"C:\b.exe", false, vec![]);
        assert_eq!(
            l.notes(),
            ["Hideout still couldn't save that. Quit Hideout and open it again, then press Forget."]
        );
        assert!(l.note.as_ref().is_some_and(|(_, act)| *act));
        assert_eq!(l.rows().len(), 2);
        // Pressed again and saved: the note is replaced.
        l.after_forget(r"C:\b.exe", true, vec![row(r"C:\a.exe")]);
        assert_eq!(l.rows().len(), 1);
        assert_eq!(
            l.notes(),
            ["Hideout forgot b.exe. It can't do those things now, and it asks again the next time it starts."]
        );
        assert!(l.note.as_ref().is_some_and(|(_, act)| !*act));
        // A later failure on that row starts with the first words again.
        l.after_forget(r"C:\a.exe", false, vec![]);
        assert!(l.notes()[0].starts_with("Hideout forgot a.exe for now"));
    }

    /// The "file changed" line is read only once it was on screen.
    #[test]
    fn a_note_below_the_edge_is_not_seen_yet() {
        let paths: Vec<String> = (0..3).map(|i| format!(r"C:\T\{i}.exe")).collect();
        let mut l = ToolList::default();
        l.set_rows(paths.iter().map(|p| row(p)).collect());
        l.file_changed(r"C:\Tools\lights.exe");
        let ctx = egui::Context::default();
        let t = crate::theme::tokens(true);
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(900.0, 700.0));
        let frame = |l: &mut ToolList, push: f32| {
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(screen),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        ui.add_space(push);
                        let _ = tool_list_ui(ui, &t, l);
                    });
                },
            );
        };
        // Drawn far below the window's bottom edge: leaving the page keeps it.
        frame(&mut l, 2000.0);
        l.left();
        assert_eq!(l.notes().len(), 1);
        // Drawn on screen: now it was read.
        frame(&mut l, 0.0);
        l.left();
        assert!(l.notes().is_empty());
    }

    #[test]
    fn focus_goes_to_next_row_then_previous() {
        // Shown newest first: c, b, a.
        let mut l = list(&[r"C:\a.exe", r"C:\b.exe", r"C:\c.exe"]);
        l.after_forget(r"C:\b.exe", true, vec![row(r"C:\a.exe"), row(r"C:\c.exe")]);
        assert_eq!(l.focus(), Some(r"C:\a.exe"), "the row that took its place");
        l.after_forget(r"C:\a.exe", true, vec![row(r"C:\c.exe")]);
        assert_eq!(
            l.focus(),
            Some(r"C:\c.exe"),
            "it was the last row: the one above"
        );
        // The button really takes the focus when drawn, once.
        let ctx = egui::Context::default();
        draw(&ctx, &mut l);
        assert!(ctx.memory(|m| m.focused()).is_some());
        assert_eq!(l.focus(), None);
        l.after_forget(r"C:\c.exe", true, vec![]);
        assert_eq!(l.focus(), None, "no row left: no focus is asked for");
    }

    #[test]
    fn the_64_line_shows_only_with_a_full_list() {
        let paths: Vec<String> = (0..FULL).map(|i| format!(r"C:\T\{i}.exe")).collect();
        let mut l = ToolList::default();
        l.set_rows(paths.iter().take(FULL - 1).map(|p| row(p)).collect());
        assert!(!l.full());
        let ctx = egui::Context::default();
        let (said, _) = draw(&ctx, &mut l);
        assert!(!said.iter().any(|(label, _)| label == FULL_LINE));
        l.set_rows(paths.iter().map(|p| row(p)).collect());
        assert!(l.full());
        let (said, _) = draw(&ctx, &mut l);
        assert!(said.iter().any(|(label, _)| {
            label
            == "Hideout keeps 64 programs. Allowing another one forgets the last one in this list."
        }));
    }

    #[test]
    fn accessible_names() {
        let mut l = ToolList::default();
        let mut gone = row(r"C:\Tools\gone.exe");
        gone.missing = true;
        l.set_rows(vec![
            gone,
            ToolRow::new(
                r"C:\Tools\lights.exe",
                vec![
                    item("change your keyboard's lighting", false),
                    item("run your keyboard's actions", true),
                ],
                Some("2026-11-17T09:00:00Z"),
            ),
        ]);
        let ctx = egui::Context::default();
        let (said, _) = draw(&ctx, &mut l);
        let has = |label: &str| said.iter().any(|(l, _)| l == label);
        assert!(has("Programs outside Hideout, 2 programs"), "{said:?}");
        assert!(has(
            "lights.exe, 2 allowed, Allowed in Hideout on 17 Nov 2026"
        ));
        assert!(has(
            "gone.exe, 1 allowed, Allowed in Hideout on 17 Nov 2026, file not here now"
        ));
        // The same words on screen and for a screen reader.
        assert!(has("\u{2022} Sensitive: run your keyboard's actions"));
        assert!(has("\u{2022} change your keyboard's lighting"));
        // The button says which program, and its description is the path line.
        assert!(said.contains(&(
            "Forget lights.exe".to_string(),
            r"Program file: C:\Tools\lights.exe".to_string()
        )));
        assert!(has("Forget gone.exe"));
        assert_eq!(section_said(1), "Programs outside Hideout, 1 program");
    }
}
