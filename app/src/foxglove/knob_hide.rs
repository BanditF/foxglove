//! Apps hidden from the knob (#952, UX plan `docs/knob-exclude-ux.md`). A click on the knob goes
//! to the next app that's playing sound; it skips:
//!
//! - apps that haven't played anything during this run (#854, `Heard`), and apps that only
//!   listen to the speakers (#1060: their meter follows the mix, `Heard`'s mirror test);
//! - apps whose program has no window at all (a background helper holding a stream; hidden and
//!   tool windows count, so a player in the tray stays; when it can't be told, the app stays);
//! - apps on the hidden list: the user's choices (settings section `[knob]`, "Hide from knob" in
//!   the tray panel, "Show again" on the Keyboard page's Knob card) over Foxglove's shipped list
//!   (`resources/knob-hidden.toml`, data, not code). The user's choice wins and is kept, so a
//!   later shipped list never hides an app the user showed again.
//!
//! Apps are matched by their program's file name, never by session id or pid: a hidden app stays
//! hidden after it (or Foxglove) restarts, and after an update moves it to another folder. An
//! app is one row however many processes play its sound (#1061); its program is the windowed
//! one, never a helper's (hiding a WebView2 host would hide every app that uses it).
//! Nothing here names an app.

use serde::{Deserialize, Serialize};
use std::sync::{Mutex, OnceLock};

use egui::RichText;
use foxglove_panel::sessions::AppGroup;

use super::theme;

/// The settings section.
pub const SECTION: &str = "knob";

/// The user's choices (`[knob]`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub apps: Vec<Choice>,
}

/// One program the user hid, or showed again over Foxglove's list.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Choice {
    /// The program's full path when it was chosen (shown in the tooltip; matched by file name).
    pub path: String,
    /// The app's name as the knob showed it.
    pub name: String,
    /// Hidden (`true`), or shown again over Foxglove's list (`false`).
    pub hidden: bool,
    /// When, `YYYY-MM-DD`.
    pub date: String,
}

/// One entry of Foxglove's shipped list.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct SeedApp {
    pub file: String,
    pub name: String,
    /// Where the program usually is (`%ProgramFiles%\Vendor\app.exe`), for its icon when it
    /// neither plays sound nor runs (#981). Only the icon uses it; matching stays by file name.
    #[serde(default)]
    pub path: Option<String>,
}

#[derive(Deserialize)]
struct SeedFile {
    #[serde(default)]
    app: Vec<SeedApp>,
}

/// Parses a shipped list; a broken one hides nothing (and says so).
pub fn parse_seed(text: &str) -> Vec<SeedApp> {
    match toml::from_str::<SeedFile>(text) {
        Ok(f) => f.app,
        Err(e) => {
            eprintln!("Foxglove: knob: the shipped hidden list can't be read: {e}");
            Vec::new()
        }
    }
}

/// Foxglove's shipped list.
pub fn seed() -> &'static [SeedApp] {
    static SEED: OnceLock<Vec<SeedApp>> = OnceLock::new();
    SEED.get_or_init(|| parse_seed(include_str!("../../resources/knob-hidden.toml")))
}

/// A path's file name.
pub fn file_name(path: &str) -> &str {
    path.rsplit(['\\', '/']).next().unwrap_or(path)
}

/// Whether two paths (or file names) name the same program: their file names, in any case.
pub fn same_program(a: &str, b: &str) -> bool {
    let (a, b) = (file_name(a), file_name(b));
    !a.is_empty() && a.eq_ignore_ascii_case(b)
}

/// Why an app is hidden.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Why {
    /// "Hidden by you · <date>".
    User { date: String },
    /// "On Foxglove's list".
    Seed,
}

impl Why {
    pub fn words(&self) -> String {
        match self {
            Why::User { date } if date.is_empty() => "Hidden by you".into(),
            Why::User { date } => format!("Hidden by you · {date}"),
            Why::Seed => "On Foxglove's list".into(),
        }
    }
}

/// One row of the Knob card's list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    /// The program (a full path, or the shipped list's file name).
    pub path: String,
    pub name: String,
    pub why: Why,
}

/// The hidden list the knob filters by: the user's choices over the shipped list.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Hidden {
    pub user: Vec<Choice>,
    pub seed: Vec<SeedApp>,
}

impl Hidden {
    /// The list as saved now (the shipped list alone when the settings can't be read).
    pub fn load() -> Hidden {
        Hidden {
            user: super::store::try_load::<Settings>(SECTION)
                .unwrap_or_default()
                .apps,
            seed: seed().to_vec(),
        }
    }

    /// Why the program at `exe` is hidden; `None`: it isn't (or its program is unknown).
    pub fn why(&self, exe: Option<&str>) -> Option<Why> {
        let exe = exe?;
        if let Some(c) = self.user.iter().find(|c| same_program(&c.path, exe)) {
            return c.hidden.then(|| Why::User {
                date: c.date.clone(),
            });
        }
        self.seed
            .iter()
            .any(|s| same_program(&s.file, exe))
            .then_some(Why::Seed)
    }

    /// The hidden programs, the user's first (newest last), then the shipped list's.
    pub fn rows(&self) -> Vec<Row> {
        let mut rows: Vec<Row> = self
            .user
            .iter()
            .filter(|c| c.hidden)
            .map(|c| Row {
                path: c.path.clone(),
                name: c.name.clone(),
                why: Why::User {
                    date: c.date.clone(),
                },
            })
            .collect();
        for s in &self.seed {
            if !self.user.iter().any(|c| same_program(&c.path, &s.file)) {
                rows.push(Row {
                    path: s.file.clone(),
                    name: s.name.clone(),
                    why: Why::Seed,
                });
            }
        }
        rows
    }

    /// Of the heard apps (in the knob's order), the ones a click offers: not windowless, not
    /// hidden (by the row's windowed program).
    pub fn offer(&self, mut heard: Vec<AppGroup>) -> Vec<AppGroup> {
        heard.retain(|s| s.windowed != Some(false) && self.why(s.exe.as_deref()).is_none());
        heard
    }
}

/// The user's choice for `path` now (for an Undo).
pub fn choice_in(apps: &[Choice], path: &str) -> Option<Choice> {
    apps.iter().find(|c| same_program(&c.path, path)).cloned()
}

/// Hides `path` (named `name`) in `apps`, dated `date`.
pub fn hide_in(apps: &mut Vec<Choice>, path: &str, name: &str, date: &str) {
    apps.retain(|c| !same_program(&c.path, path));
    apps.push(Choice {
        path: path.into(),
        name: name.into(),
        hidden: true,
        date: date.into(),
    });
}

/// Shows `path` again in `apps`: the user's entry goes; over the shipped list, an override stays.
pub fn show_in(apps: &mut Vec<Choice>, seed: &[SeedApp], path: &str, name: &str, date: &str) {
    apps.retain(|c| !same_program(&c.path, path));
    if seed.iter().any(|s| same_program(&s.file, path)) {
        apps.push(Choice {
            path: path.into(),
            name: name.into(),
            hidden: false,
            date: date.into(),
        });
    }
}

/// Puts back the choice `before` had for `path` (an Undo).
pub fn restore_in(apps: &mut Vec<Choice>, path: &str, before: Option<Choice>) {
    apps.retain(|c| !same_program(&c.path, path));
    apps.extend(before);
}

/// Saves a change to the user's list and tells the knob. The choice before it (for an Undo).
fn change(path: &str, f: impl FnOnce(&mut Vec<Choice>)) -> Option<Option<Choice>> {
    let mut before = None;
    let r = super::store::update::<Settings>(SECTION, |s| {
        before = Some(choice_in(&s.apps, path));
        f(&mut s.apps);
    });
    match r {
        Ok(_) => {
            super::knob::hidden_changed();
            before
        }
        Err(e) => {
            eprintln!("Foxglove: knob: the hidden list can't be saved: {e}");
            None
        }
    }
}

/// "Hide from knob": the choice before (for Undo), `None` when it couldn't be saved.
pub fn hide(path: &str, name: &str) -> Option<Option<Choice>> {
    let date = super::health_notice::today();
    eprintln!("Foxglove: knob: {name} hidden ({})", file_name(path));
    change(path, |apps| hide_in(apps, path, name, &date))
}

/// "Show again": the choice before (for Undo).
pub fn show_again(path: &str, name: &str) -> Option<Option<Choice>> {
    let date = super::health_notice::today();
    eprintln!("Foxglove: knob: {name} shown again ({})", file_name(path));
    change(path, |apps| show_in(apps, seed(), path, name, &date))
}

/// Undo: `path`'s choice as it was.
pub fn restore(path: &str, before: Option<Choice>) {
    change(path, |apps| restore_in(apps, path, before));
}

// The Knob card on the Keyboard page.

/// After Show again: the line, and what Undo puts back.
struct Undo {
    line: String,
    path: String,
    before: Option<Choice>,
}

static UNDO: Mutex<Option<Undo>> = Mutex::new(None);

/// The card's words (UX plan, Wording).
pub const TITLE: &str = "Knob";
pub const INTRO: [&str; 2] = [
    "A click on the knob goes to the next app that's playing sound.",
    "It skips apps that haven't played anything, apps that only listen to the speakers, and apps on this list.",
];
pub const CAPTION: &str = "HIDDEN FROM THE KNOB";
pub const EMPTY: &str =
    "No apps hidden. Use Hide from knob in the tray panel while the knob is on one.";

/// "<app> is back in the knob's cycle."
pub fn back_line(name: &str) -> String {
    format!("{name} is back in the knob's cycle.")
}

/// The tooltip of a row: the name and the file it's matched by.
pub fn row_hover(row: &Row) -> String {
    format!(
        "{}\n{} · matched by this file",
        row.name,
        file_name(&row.path)
    )
}

/// Where a row's icon is read from: its full path, else (a shipped list's bare file name) the
/// path of the same program among the apps with sound right now, else among `known` (running
/// programs, then the shipped list's path hints: [`known_paths`]).
pub fn icon_path(row: &Row, apps: &[(String, String)], known: &[String]) -> String {
    if file_name(&row.path) != row.path {
        return row.path.clone();
    }
    apps.iter()
        .map(|(_, path)| path)
        .chain(known)
        .find(|path| same_program(path, &row.path))
        .map_or_else(|| row.path.clone(), Clone::clone)
}

/// `text` with each `%NAME%` replaced by `var(NAME)`; `None` when one isn't set or a `%` is
/// left open.
pub fn expand_vars(text: &str, var: impl Fn(&str) -> Option<String>) -> Option<String> {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find('%') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let end = after.find('%')?;
        out.push_str(&var(&after[..end])?);
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    Some(out)
}

/// Running programs' paths by lower-case file name (`None`: not running), looked up once per
/// visit to the page: the Knob card's first frame fills it, [`page_left`] empties it.
#[derive(Default)]
struct Running {
    paths: std::collections::HashMap<String, Option<String>>,
    /// The last [`known_paths`] answer and the bare names it was for, so a frame with the same
    /// rows reads no process list, setting or path hint again.
    known: Option<(Vec<String>, Vec<String>)>,
}
static RUNNING: Mutex<Option<Running>> = Mutex::new(None);

/// Full paths for the shipped list's bare file names that no app with sound gives (#981): the
/// running programs with those names (one process snapshot on the card's first frame, kept until
/// the page changes; no timer), then the shipped list's path hints.
fn known_paths(rows: &[Row], apps: &[(String, String)]) -> Vec<String> {
    let bare: Vec<&str> = rows
        .iter()
        .map(|r| r.path.as_str())
        .filter(|p| file_name(p) == *p && !apps.iter().any(|(_, a)| same_program(a, p)))
        .collect();
    if bare.is_empty() {
        return Vec::new();
    }
    let mut guard = RUNNING.lock().unwrap_or_else(|e| e.into_inner());
    let running = guard.get_or_insert_with(Default::default);
    if let Some((for_names, out)) = &running.known {
        if for_names
            .iter()
            .map(String::as_str)
            .eq(bare.iter().copied())
        {
            return out.clone();
        }
    }
    let kept = &mut running.paths;
    let missing: Vec<&str> = bare
        .iter()
        .copied()
        .filter(|n| !kept.contains_key(&n.to_lowercase()))
        .collect();
    if !missing.is_empty() {
        let found = super::app_icon::running_paths(&missing);
        for n in missing {
            let path = found.iter().find(|p| same_program(p, n)).cloned();
            kept.insert(n.to_lowercase(), path);
        }
    }
    let mut out: Vec<String> = bare
        .iter()
        .filter_map(|n| kept.get(&n.to_lowercase()).cloned().flatten())
        .collect();
    out.extend(
        seed()
            .iter()
            .filter(|s| bare.iter().any(|n| same_program(n, &s.file)))
            .filter_map(|s| s.path.as_deref())
            .filter_map(|p| expand_vars(p, |v| std::env::var(v).ok())),
    );
    running.known = Some((bare.iter().map(|n| n.to_string()).collect(), out.clone()));
    out
}

/// What a click on the card asks for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CardIntent {
    ShowAgain(Row),
    Undo,
    Add { name: String, path: String },
}

/// Draws the card's body: the intro, the list (or its empty line), Add an app…, and the line
/// after Show again. `apps`: the apps with sound right now (name, program path); `known`: more
/// program paths for the rows' icons ([`icon_path`]).
pub fn card_body(
    ui: &mut egui::Ui,
    rows: &[Row],
    undo_line: Option<&str>,
    apps: &[(String, String)],
    known: &[String],
) -> Option<CardIntent> {
    let p = theme::palette(ui);
    let mut out = None;
    for line in INTRO {
        ui.label(RichText::new(line).color(p.muted));
    }
    ui.add_space(8.0);
    ui.label(theme::caps_label(ui, CAPTION));
    ui.add_space(2.0);
    if let Some(line) = undo_line {
        ui.horizontal(|ui| {
            ui.label(RichText::new(line).color(p.text));
            // The tray panel's Undo is a full button too (#981: one size).
            if ui.button("Undo").clicked() {
                out = Some(CardIntent::Undo);
            }
        });
    }
    if rows.is_empty() {
        ui.label(RichText::new(EMPTY).color(p.muted));
    }
    for row in rows {
        ui.horizontal(|ui| {
            super::app_icon::show(ui, Some(&icon_path(row, apps, known)));
            ui.vertical(|ui| {
                ui.label(RichText::new(&row.name).color(p.text_strong))
                    .on_hover_text(row_hover(row));
                ui.label(RichText::new(row.why.words()).small().color(p.muted));
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Show again").clicked() {
                    out = Some(CardIntent::ShowAgain(row.clone()));
                }
            });
        });
    }
    ui.add_space(4.0);
    let free: Vec<&(String, String)> = apps
        .iter()
        .filter(|(_, path)| !rows.iter().any(|r| same_program(&r.path, path)))
        .collect();
    // A row, so the menu button keeps its own width (no full-width stretch).
    ui.horizontal(|ui| {
        ui.add_enabled_ui(!free.is_empty(), |ui| {
            ui.menu_button("Add an app…", |ui| {
                for (name, path) in &free {
                    if ui
                        .button(name.as_str())
                        .on_hover_text(file_name(path))
                        .clicked()
                    {
                        out = Some(CardIntent::Add {
                            name: name.clone(),
                            path: path.clone(),
                        });
                        ui.close();
                    }
                }
            })
            .response
            .on_disabled_hover_text("No other app has sound right now.");
        });
    });
    out
}

/// The Knob card on the Keyboard page: reads the list, draws it, carries out a click.
pub fn card(ui: &mut egui::Ui) {
    let rows = Hidden::load().rows();
    let undo_line = UNDO
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .map(|u| u.line.clone());
    let apps = super::knob::apps();
    let known = known_paths(&rows, &apps);
    let intent = super::settings_ui::card(ui, TITLE, |ui| {
        card_body(ui, &rows, undo_line.as_deref(), &apps, &known)
    });
    let mut undo = UNDO.lock().unwrap_or_else(|e| e.into_inner());
    match intent {
        Some(CardIntent::ShowAgain(row)) => {
            if let Some(before) = show_again(&row.path, &row.name) {
                *undo = Some(Undo {
                    line: back_line(&row.name),
                    path: row.path,
                    before,
                });
            }
        }
        Some(CardIntent::Undo) => {
            if let Some(u) = undo.take() {
                restore(&u.path, u.before);
            }
        }
        Some(CardIntent::Add { name, path }) => {
            *undo = None;
            hide(&path, &name);
        }
        None => {}
    }
}

/// The page changed: the line after Show again goes (UX plan: "until the page changes").
pub fn page_left() {
    *UNDO.lock().unwrap_or_else(|e| e.into_inner()) = None;
    // The next visit looks for running programs again (#981).
    *RUNNING.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(id: &str, exe: Option<&str>, windowed: Option<bool>) -> AppGroup {
        AppGroup {
            key: id.into(),
            name: id.into(),
            exe: exe.map(Into::into),
            windowed,
            ..AppGroup::default()
        }
    }

    fn ids(list: &[AppGroup]) -> Vec<&str> {
        list.iter().map(|s| s.key.as_str()).collect()
    }

    fn seed_of(files: &[&str]) -> Vec<SeedApp> {
        files
            .iter()
            .map(|f| SeedApp {
                file: f.to_string(),
                name: f.to_string(),
                path: None,
            })
            .collect()
    }

    /// A captured session list (the fixture): what Windows' Volume Mixer had while a game, music,
    /// a chat app's helper process, a vendor helper and the system sounds were active.
    fn fixture() -> Vec<AppGroup> {
        vec![
            session(
                "music",
                Some(r"C:\Users\u\AppData\Roaming\Player\Player.exe"),
                Some(true),
            ),
            session("game", Some(r"D:\Games\Game\game.exe"), Some(true)),
            // A chat app's sound from a helper process; the program has a window.
            session(
                "chat",
                Some(r"C:\Users\u\AppData\Local\Chat\app-1.0.9\Chat.exe"),
                Some(true),
            ),
            // A background helper with no window at all.
            session(
                "helper",
                Some(r"C:\Program Files\Vendor\agent.exe"),
                Some(false),
            ),
            // A vendor control centre with a window, on the shipped list.
            session(
                "vendor",
                Some(r"C:\Program Files\Vendor\CenterMgr.exe"),
                Some(true),
            ),
            // The system sounds: no program, can't tell.
            session("system", None, None),
        ]
    }

    #[test]
    fn the_filter_over_a_captured_session_list_gives_the_cycle() {
        let mut h = Hidden {
            user: Vec::new(),
            seed: seed_of(&["centermgr.exe"]),
        };
        // Windowless and shipped-list apps are skipped; can't-tell stays.
        assert_eq!(
            ids(&h.offer(fixture())),
            ["music", "game", "chat", "system"]
        );
        // The user hides the game (matched by file, in any folder or case).
        hide_in(&mut h.user, r"d:\GAMES\game\GAME.EXE", "Game", "2026-11-09");
        assert_eq!(ids(&h.offer(fixture())), ["music", "chat", "system"]);
        // ...and shows the shipped one again: an override, kept over the shipped list.
        show_in(
            &mut h.user,
            &h.seed.clone(),
            r"C:\Program Files\Vendor\CenterMgr.exe",
            "Center",
            "2026-11-09",
        );
        assert_eq!(
            ids(&h.offer(fixture())),
            ["music", "chat", "vendor", "system"]
        );
    }

    #[test]
    fn a_hide_survives_a_new_session_id_and_a_new_folder() {
        let mut user = Vec::new();
        hide_in(
            &mut user,
            r"C:\Users\u\AppData\Local\Chat\app-1.0.9\Chat.exe",
            "Chat",
            "2026-11-09",
        );
        let h = Hidden {
            user,
            seed: Vec::new(),
        };
        // The app restarted after an update: a new session id, a new version folder.
        let later = vec![session(
            "chat-2",
            Some(r"C:\Users\u\AppData\Local\Chat\app-1.1.0\Chat.exe"),
            Some(true),
        )];
        assert!(h.offer(later).is_empty());
        assert_eq!(
            h.why(Some("chat.exe")),
            Some(Why::User {
                date: "2026-11-09".into()
            })
        );
    }

    #[test]
    fn undo_puts_the_choice_back() {
        let seed = seed_of(&["CenterMgr.exe"]);
        let mut user = Vec::new();
        // Hide, then Undo: nothing kept.
        let before = choice_in(&user, r"D:\Games\Game\game.exe");
        hide_in(&mut user, r"D:\Games\Game\game.exe", "Game", "d");
        restore_in(&mut user, r"D:\Games\Game\game.exe", before);
        assert!(user.is_empty());
        // Show a shipped one again, then Undo: hidden by the shipped list again.
        let before = choice_in(&user, "CenterMgr.exe");
        show_in(&mut user, &seed, "CenterMgr.exe", "Center", "d");
        let h = Hidden {
            user: user.clone(),
            seed: seed.clone(),
        };
        assert_eq!(h.why(Some(r"C:\x\centermgr.exe")), None);
        restore_in(&mut user, "CenterMgr.exe", before);
        let h = Hidden { user, seed };
        assert_eq!(h.why(Some(r"C:\x\centermgr.exe")), Some(Why::Seed));
        // Show one the user hid again: it simply leaves the list.
        let mut user = Vec::new();
        hide_in(&mut user, r"D:\g.exe", "G", "d");
        show_in(&mut user, &[], r"D:\g.exe", "G", "d");
        assert!(user.is_empty());
    }

    #[test]
    fn the_cards_rows_and_words() {
        let mut user = Vec::new();
        hide_in(&mut user, r"C:\Apps\Chat\Chat.exe", "Chat", "2026-11-09");
        let h = Hidden {
            user,
            seed: seed_of(&["CenterMgr.exe"]),
        };
        let rows = h.rows();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].why.words(), "Hidden by you · 2026-11-09");
        assert_eq!(rows[1].why.words(), "On Foxglove's list");
        assert_eq!(row_hover(&rows[0]), "Chat\nChat.exe · matched by this file");
        assert_eq!(back_line("Chat"), "Chat is back in the knob's cycle.");
        // Icons: a full path is used; a bare shipped name borrows a live app's path.
        let live = vec![("Center".to_string(), r"C:\Vendor\CenterMgr.exe".to_string())];
        assert_eq!(icon_path(&rows[0], &live, &[]), r"C:\Apps\Chat\Chat.exe");
        assert_eq!(icon_path(&rows[1], &live, &[]), r"C:\Vendor\CenterMgr.exe");
        assert_eq!(icon_path(&rows[1], &[], &[]), "CenterMgr.exe");
        // No sound now: a running program's path (or a path hint) gives the icon; sound wins.
        let known = vec![
            r"C:\Other\Chat.exe".to_string(),
            r"C:\Running\centermgr.exe".to_string(),
            r"C:\Hint\CenterMgr.exe".to_string(),
        ];
        assert_eq!(
            icon_path(&rows[1], &[], &known),
            r"C:\Running\centermgr.exe"
        );
        assert_eq!(
            icon_path(&rows[1], &live, &known),
            r"C:\Vendor\CenterMgr.exe"
        );
        assert_eq!(icon_path(&rows[0], &[], &known), r"C:\Apps\Chat\Chat.exe");
        // UX plan: "Hide", never "exclude" or "block"; no "session", "mirror", "loopback" or
        // "helper" in the UI (#1060/#1061 addendum).
        assert_eq!(
            INTRO[1],
            "It skips apps that haven't played anything, apps that only listen to the speakers, \
             and apps on this list."
        );
        for s in INTRO.iter().chain([&CAPTION, &EMPTY]) {
            let l = s.to_lowercase();
            for word in [
                "exclude", "block", "session", "mirror", "loopback", "helper",
            ] {
                assert!(!l.contains(word), "{s}");
            }
        }
    }

    #[test]
    fn the_shipped_list_is_data_and_reads() {
        assert!(!seed().is_empty());
        assert!(seed()
            .iter()
            .all(|s| s.file.ends_with(".exe") && !s.name.is_empty()));
        assert!(parse_seed("not = [toml").is_empty());
        let hinted = parse_seed(
            "[[app]]\nfile = \"a.exe\"\nname = \"A\"\npath = '%ProgramFiles%\\A\\a.exe'\n",
        );
        assert_eq!(hinted[0].path.as_deref(), Some(r"%ProgramFiles%\A\a.exe"));
    }

    #[test]
    fn path_hints_expand_their_variables() {
        let var = |v: &str| (v == "ProgramFiles").then(|| r"C:\Program Files".to_string());
        assert_eq!(
            expand_vars(r"%ProgramFiles%\A\a.exe", var).as_deref(),
            Some(r"C:\Program Files\A\a.exe")
        );
        assert_eq!(
            expand_vars(r"C:\A\a.exe", var).as_deref(),
            Some(r"C:\A\a.exe")
        );
        assert_eq!(expand_vars(r"%Unset%\a.exe", var), None);
        assert_eq!(expand_vars(r"%ProgramFiles\a.exe", var), None);
    }

    #[test]
    fn a_running_program_is_found_by_its_file_name() {
        // This test binary runs, so its own file name finds its own path (one snapshot).
        let me = std::env::current_exe().unwrap();
        let name = me.file_name().unwrap().to_string_lossy().to_string();
        let found = super::super::app_icon::running_paths(&[name.as_str()]);
        if cfg!(windows) {
            assert!(found.iter().any(|p| same_program(p, &name)), "{found:?}");
        }
        assert!(super::super::app_icon::running_paths(&["no-such-program-981.exe"]).is_empty());
    }

    #[test]
    fn the_users_list_round_trips_through_the_store() {
        super::super::store::test_config_dir();
        let section = "knob-test-952";
        let mut s = Settings::default();
        hide_in(&mut s.apps, r"D:\g.exe", "G", "2026-11-09");
        super::super::store::save(section, &s).unwrap();
        let back: Settings = super::super::store::load(section).unwrap().unwrap();
        assert_eq!(back, s);
    }
}
