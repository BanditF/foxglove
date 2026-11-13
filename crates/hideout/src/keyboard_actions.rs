// SPDX-License-Identifier: MIT
//! What the keyboard grant's dialog and row do (#1155-B): Allow pins and records the program
//! file, Not now remembers the decline, Turn off revokes at once. Each change is written by the
//! registry (`inventory.json` for the grant, `hideout.toml` for the decline) and pushed to the
//! router in the same call, so a turn-off closes the legacy module's areas at once. Nothing here
//! grants anything by itself: only `allow` does, and it is called only from the user's Allow.

use std::path::Path;

use hideout_registry::keyboard::KeyboardState;
use hideout_registry::Registry;
use hideout_router::server::Hub;
use hideout_shell::keyboard_grant::{Answer, Ask, Dialog, HeaderKind, Situation};
use hideout_shell::theme::Tokens;

/// Maps the registry's view to the shell's. `started_without_act`: the module started with no
/// click of the user's (autostart, an upgrade), which shows the header line.
pub fn situation(s: KeyboardState, started_without_act: bool) -> Situation {
    match s {
        KeyboardState::NotApplicable => Situation::Standalone,
        KeyboardState::NotAsked => Situation::NotAsked {
            started_without_act,
        },
        KeyboardState::Declined => Situation::Declined,
        KeyboardState::Granted { at } => Situation::Granted { at },
        KeyboardState::Revoked { .. } => Situation::Revoked,
        KeyboardState::Changed { at, reason } => Situation::Changed {
            at,
            moved: reason == hideout_registry::keyboard::VoidReason::Moved,
        },
    }
}

fn push(reg: &Registry, hub: &Hub) {
    let grants = hideout_supervisor::driver::keyboard_grants(reg)
        .into_iter()
        .collect();
    hub.act(|r| r.set_keyboard_grants(grants));
}

/// The path the dialog shows: the canonical full path of the program Allow would bind to.
pub fn dialog_path(home: &Path, id: &str) -> Result<String, String> {
    let reg = Registry::open_from_env(home).map_err(|e| format!("{e:?}"))?;
    let exe = reg.keyboard_exe(id).map_err(|e| format!("{e:?}"))?;
    hideout_registry::keyboard::canonical_path(Path::new(&exe)).map_err(|e| e.to_string())
}

/// Allow: the program file is pinned (write-locked) and hashed through that one handle, and the
/// grant records that path and hash. `shown_path` is the path the dialog showed the user: Allow
/// is refused unless it is the very file that was pinned (the binding line must be true), and
/// for a network path. Refused when the program isn't where the module is registered.
pub fn allow(home: &Path, hub: &Hub, id: &str, shown_path: &str) -> Result<(), String> {
    let mut reg = Registry::open_from_env(home).map_err(|e| format!("{e:?}"))?;
    let exe = reg.keyboard_exe(id).map_err(|e| format!("{e:?}"))?;
    let pin = hideout_supervisor::pin::Pin::open(Path::new(&exe)).map_err(|e| e.to_string())?;
    if hideout_registry::keyboard::is_unc(&pin.path) {
        return Err("the program is on a network path".into());
    }
    if !hideout_registry::keyboard::same_path(shown_path, &pin.path) {
        return Err(MISMATCH.into());
    }
    reg.allow_keyboard(id, &pin.path, &pin.sha256, &hideout_supervisor::now_utc())
        .map_err(|e| format!("{e:?}"))?;
    push(&reg, hub);
    Ok(())
}

/// `allow`'s refusal when the path the dialog showed isn't the file that was pinned.
pub const MISMATCH: &str = "the program is not the one the dialog showed";

/// Not now, Esc or the close box: remembered in `hideout.toml`, never asked by itself again.
pub fn decline(home: &Path, id: &str) -> Result<(), String> {
    let mut reg = Registry::open_from_env(home).map_err(|e| format!("{e:?}"))?;
    reg.decline_keyboard(id).map_err(|e| format!("{e:?}"))
}

/// Turn off: acts at once and fails safe. The router drops the grant first (in memory, no file
/// involved); then the record is written. A failed write is reported and never undoes the
/// router's change: the areas stay closed.
pub fn turn_off(home: &Path, hub: &Hub, id: &str) -> Result<(), String> {
    hub.act(|r| r.drop_keyboard_grant(id));
    let mut reg = Registry::open_from_env(home).map_err(|e| format!("{e:?}"))?;
    reg.revoke_keyboard(id, &hideout_supervisor::now_utc())
        .map_err(|e| format!("{e:?}"))
}

/// The one module the grant is for (the adopted legacy module).
pub const MODULE: &str = hideout_registry::keyboard::LEGACY_MODULE;

/// What the Modules page keeps for the keyboard grant: where it stands, the open dialog and
/// the header line. Drawn by the hub (never a module panel); every change to the grant goes
/// through `allow`, `decline` and `turn_off` above, and only the user's Allow grants.
pub struct GrantUi {
    home: std::path::PathBuf,
    hub: Hub,
    pub situation: Situation,
    pub dialog: Option<Dialog>,
    /// The dialog was drawn once already (Not now takes the focus on the first frame only).
    drawn: bool,
    /// "Not now" on a header line hides that line (its kind) until the next hub start; a
    /// different line (changed since you allowed it) still shows.
    header_hidden: Option<HeaderKind>,
    /// The button that opened the dialog: the focus goes back to it when the dialog closes.
    opener: Option<hideout_shell::egui::Id>,
    /// The user has answered or acted on the grant in this run: a module that is on and never
    /// asked then no longer counts as "started without the user's act".
    acted: bool,
    /// Why the last action failed, in words, under the row.
    pub note: Option<String>,
    /// The module's state fact at the last read: the supervisor voids a grant when it starts
    /// the module, so a change of state is the event that re-reads the grant.
    seen: Option<serde_json::Value>,
}

impl GrantUi {
    pub fn new(home: &Path, hub: Hub) -> GrantUi {
        let mut g = GrantUi {
            home: home.to_path_buf(),
            hub,
            situation: Situation::Standalone,
            dialog: None,
            drawn: false,
            header_hidden: None,
            opener: None,
            acted: false,
            note: None,
            seen: None,
        };
        g.refresh();
        g
    }

    /// Reads the grant's state from the registry again.
    pub fn refresh(&mut self) {
        self.situation = match Registry::open_from_env(&self.home) {
            Ok(reg) => {
                let without_act = !self.acted && reg.hub.enabled(MODULE);
                situation(reg.keyboard_state(MODULE), without_act)
            }
            // Unreadable: nothing is shown and nothing is granted.
            Err(_) => Situation::Standalone,
        };
    }

    /// Re-reads the grant when the module's state fact changed (no timer).
    pub fn sync(&mut self, state: Option<serde_json::Value>) {
        if self.seen != state {
            self.seen = state;
            self.refresh();
        }
    }

    /// Turn on… / Review…: opens the dialog for the path Allow would bind to now.
    pub fn open_dialog(&mut self) {
        self.note = None;
        match dialog_path(&self.home, MODULE) {
            Ok(path) => {
                let changed = matches!(self.situation, Situation::Changed { .. });
                self.dialog = Some(Dialog::open(&path, changed));
                self.drawn = false;
            }
            Err(e) => {
                log::warn!("keyboard grant: the dialog can't open ({e})");
                self.note = Some("Foxglove's program file couldn't be found.".into());
            }
        }
    }

    /// The user's answer. Not now is the only default; Allow runs only from the user's Allow
    /// and is refused unless the path is the one the dialog showed.
    pub fn answer(&mut self, a: Answer) {
        let Some(d) = self.dialog.take() else { return };
        self.acted = true;
        let r = match a {
            Answer::Allow => allow(&self.home, &self.hub, MODULE, &d.path),
            Answer::NotNow => decline(&self.home, MODULE),
        };
        if let Err(e) = r {
            log::warn!("keyboard grant: {a:?} failed ({e})");
            self.note = Some(
                match a {
                    Answer::Allow if e == MISMATCH => {
                        "Foxglove wasn't allowed: its program file isn't the one shown."
                    }
                    Answer::Allow => "Foxglove couldn't be allowed. Try again from Turn on\u{2026}",
                    Answer::NotNow => "Your answer couldn't be saved.",
                }
                .into(),
            );
        } else {
            self.note = None;
        }
        self.refresh();
    }

    /// Turn off: at once, no confirm.
    pub fn turn_off(&mut self) {
        self.acted = true;
        match turn_off(&self.home, &self.hub, MODULE) {
            Ok(()) => {
                self.note = None;
                self.refresh();
            }
            Err(e) => {
                // The router already closed the areas: the row says Off whatever the file says.
                log::warn!("keyboard grant: turn off not saved ({e})");
                self.note = Some("Turned off, but the change couldn't be saved.".into());
                self.situation = Situation::Revoked;
            }
        }
    }

    /// The Turn on of the module itself: asks when the module is on and the grant is new or
    /// void (only on the user's act; never from a background event).
    pub fn after_turn_on(&mut self) {
        // The user acted: no "started without your act" line behind the dialog.
        self.acted = true;
        self.refresh();
        let ask = matches!(
            self.situation,
            Situation::NotAsked { .. } | Situation::Changed { .. }
        );
        if ask && Registry::open_from_env(&self.home).is_ok_and(|r| r.keyboard_needs_ok(MODULE)) {
            self.open_dialog();
        }
    }

    /// The header line, unless Not now hid it.
    pub fn header(&self) -> Option<hideout_shell::keyboard_grant::Header> {
        hideout_shell::keyboard_grant::header(&self.situation)
            .filter(|h| self.header_hidden != Some(h.kind))
    }

    /// Draws the header line, then the "Foxglove · Permissions" label and row, at the top of
    /// the Modules page.
    pub fn top(&mut self, ui: &mut hideout_shell::egui::Ui, t: &Tokens) {
        use hideout_shell::egui::RichText;
        use hideout_shell::keyboard_grant_ui as kui;
        if let Some(h) = self.header() {
            match kui::header_ui(ui, t, &h) {
                Some((true, id)) => {
                    self.opener = Some(id);
                    self.open_dialog();
                }
                Some((false, _)) => self.header_hidden = Some(h.kind),
                None => {}
            }
            ui.add_space(8.0);
        }
        if let Some(r) = hideout_shell::keyboard_grant::row(&self.situation) {
            ui.label(
                RichText::new(hideout_shell::keyboard_grant::SECTION_LABEL)
                    .small()
                    .color(kui::rgb(t.text_muted)),
            );
            match kui::row_ui(ui, t, &r) {
                Some((Ask::TurnOff, _)) => self.turn_off(),
                Some((Ask::OpenDialog, id)) => {
                    self.opener = Some(id);
                    self.open_dialog();
                }
                None => {}
            }
            if let Some(n) = &self.note {
                let l = ui.label(RichText::new(n.as_str()).color(kui::rgb(t.text_muted)));
                ui.ctx().accesskit_node_builder(l.id, |nb| {
                    nb.set_live(hideout_shell::egui::accesskit::Live::Polite);
                });
            }
            ui.add_space(16.0);
        }
    }

    /// Draws the dialog while it is open. The answer is applied and the focus goes back to the
    /// button that opened it.
    pub fn dialog(&mut self, ctx: &hideout_shell::egui::Context, t: &Tokens) {
        use hideout_shell::keyboard_grant_ui as kui;
        if let Some(d) = &self.dialog {
            let first = !self.drawn;
            self.drawn = true;
            if let Some(a) = kui::dialog_ui(ctx, t, d, first) {
                self.answer(a);
                if let Some(id) = self.opener.take() {
                    ctx.memory_mut(|m| m.request_focus(id));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hideout_router::{Router, Settings};
    use std::sync::mpsc;

    const NOW: &str = "2026-11-12T10:00:00Z";

    fn setup(tag: &str) -> (std::path::PathBuf, Hub) {
        let base = std::env::temp_dir().join(format!("hideout-kact-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let install = base.join("install");
        std::fs::create_dir_all(&install).unwrap();
        let exe = install.join("foxglove.exe");
        std::fs::write(&exe, b"fake").unwrap();
        let folder = base.join("manifest");
        std::fs::create_dir_all(&folder).unwrap();
        let run = exe.display().to_string().replace('\\', "/");
        std::fs::write(
            folder.join("module.toml"),
            format!(
                "id = \"foxglove\"\nname = \"Foxglove\"\nversion = \"0.1.0\"\nkind = \"adopted\"\n\
                 family = \"foxglove\"\nhub = \"1.0\"\nrun = ['{run}']\napi = {{ foxglove = \"1.0\" }}\n"
            ),
        )
        .unwrap();
        let home = base.join("home");
        let mut reg = Registry::open(&home, false).unwrap();
        reg.register_adopted(&folder, NOW).unwrap();
        reg.set_enabled("foxglove", true).unwrap();
        let router = Router::new(Settings::default(), Box::new(|_, asked| asked.to_vec()));
        let (events, _rx) = mpsc::channel();
        (home, Hub::new(router, events))
    }

    fn state(home: &Path) -> KeyboardState {
        Registry::open_from_env(home)
            .unwrap()
            .keyboard_state("foxglove")
    }

    #[test]
    fn allow_records_the_pinned_file_and_turn_off_revokes() {
        let (home, hub) = setup("allow");
        assert_eq!(state(&home), KeyboardState::NotAsked);
        let path = dialog_path(&home, "foxglove").unwrap();
        // Allow for a path other than the one the dialog showed is refused.
        assert!(allow(&home, &hub, "foxglove", r"C:\Elsewhere\foxglove.exe").is_err());
        assert_eq!(state(&home), KeyboardState::NotAsked);
        allow(&home, &hub, "foxglove", &path).unwrap();
        assert!(matches!(state(&home), KeyboardState::Granted { .. }));
        let g = Registry::open_from_env(&home).unwrap().inventory.modules["foxglove"]
            .keyboard
            .clone()
            .unwrap();
        assert!(g.allows(
            &path,
            &hideout_supervisor::sha256_file(Path::new(&path)).unwrap()
        ));
        turn_off(&home, &hub, "foxglove").unwrap();
        assert!(matches!(state(&home), KeyboardState::Revoked { .. }));
        // Turn on again is a new Allow; Turn off never reset anything else.
        allow(&home, &hub, "foxglove", &path).unwrap();
        assert!(matches!(state(&home), KeyboardState::Granted { .. }));
        let _ = std::fs::remove_dir_all(home.parent().unwrap());
    }

    #[test]
    fn not_now_is_remembered_and_grants_nothing() {
        let (home, _hub) = setup("decline");
        decline(&home, "foxglove").unwrap();
        assert_eq!(state(&home), KeyboardState::Declined);
        let reg = Registry::open_from_env(&home).unwrap();
        assert!(!reg.keyboard_needs_ok("foxglove"));
        assert!(reg.inventory.modules["foxglove"].keyboard.is_none());
        let _ = std::fs::remove_dir_all(home.parent().unwrap());
    }

    #[test]
    fn the_registry_states_map_to_the_shells_words() {
        use hideout_registry::keyboard::VoidReason;
        assert_eq!(
            situation(KeyboardState::NotApplicable, true),
            Situation::Standalone
        );
        assert_eq!(
            situation(KeyboardState::NotAsked, true),
            Situation::NotAsked {
                started_without_act: true
            }
        );
        assert_eq!(
            situation(
                KeyboardState::Changed {
                    at: NOW.into(),
                    reason: VoidReason::Moved
                },
                false
            ),
            Situation::Changed {
                at: NOW.into(),
                moved: true
            }
        );
    }
    fn ui_for(tag: &str) -> (GrantUi, std::path::PathBuf) {
        let (home, hub) = setup(tag);
        (GrantUi::new(&home, hub), home)
    }

    fn row_button(g: &GrantUi) -> &'static str {
        hideout_shell::keyboard_grant::row(&g.situation)
            .unwrap()
            .button
    }

    #[test]
    fn the_dialog_opens_on_not_now_and_shows_the_path_allow_binds() {
        let (mut g, home) = ui_for("dlg");
        assert_eq!(row_button(&g), "Turn on\u{2026}");
        g.open_dialog();
        let d = g.dialog.clone().unwrap();
        assert_eq!(d.focus, Answer::NotNow);
        assert_eq!(d.path, dialog_path(&home, "foxglove").unwrap());
        // Enter on the opening focus is Not now, and nothing is granted by opening or by Enter.
        assert_eq!(
            d.clone().key(hideout_shell::keyboard_grant::Key::Enter),
            Some(Answer::NotNow)
        );
        assert_eq!(state(&home), KeyboardState::NotAsked);
        let _ = std::fs::remove_dir_all(home.parent().unwrap());
    }

    #[test]
    fn esc_and_close_never_grant_and_not_now_is_remembered() {
        use hideout_shell::keyboard_grant::Key;
        let (mut g, home) = ui_for("esc");
        g.open_dialog();
        let mut d = g.dialog.clone().unwrap();
        // Esc from any focus, and the close box, answer Not now.
        d.key(Key::Tab);
        assert_eq!(d.focus, Answer::Allow);
        let a = d.key(Key::Escape).unwrap();
        assert_eq!((a, d.close()), (Answer::NotNow, Answer::NotNow));
        g.answer(a);
        assert!(g.dialog.is_none());
        assert_eq!(state(&home), KeyboardState::Declined);
        assert_eq!(g.situation, Situation::Declined);
        assert!(
            Registry::open_from_env(&home).unwrap().inventory.modules["foxglove"]
                .keyboard
                .is_none()
        );
        assert_eq!(row_button(&g), "Turn on\u{2026}");
        let _ = std::fs::remove_dir_all(home.parent().unwrap());
    }

    #[test]
    fn allow_from_the_dialog_grants_and_a_changed_file_offers_allow_again() {
        let (mut g, home) = ui_for("changed");
        // Not now on the "needs your OK" line hides only that line.
        assert_eq!(g.header().unwrap().kind, HeaderKind::Ask);
        g.header_hidden = Some(HeaderKind::Ask);
        assert!(g.header().is_none());
        g.open_dialog();
        g.answer(Answer::Allow);
        assert!(matches!(g.situation, Situation::Granted { .. }));
        assert_eq!(row_button(&g), "Turn off");
        // The program file changes under the grant: void, the supervisor's check.
        let mut reg = Registry::open_from_env(&home).unwrap();
        reg.void_keyboard(
            "foxglove",
            hideout_registry::keyboard::VoidReason::Changed,
            NOW,
        )
        .unwrap();
        g.refresh();
        assert!(matches!(
            g.situation,
            Situation::Changed { moved: false, .. }
        ));
        assert_eq!(
            g.header().unwrap().title,
            "Foxglove changed since you allowed it"
        );
        assert_eq!(row_button(&g), "Review\u{2026}");
        // The earlier Not now doesn't hide this one; Not now on it hides it, the row stays.
        assert_eq!(g.header().unwrap().kind, HeaderKind::Changed);
        g.header_hidden = Some(HeaderKind::Changed);
        assert!(g.header().is_none());
        assert_eq!(row_button(&g), "Review\u{2026}");
        // Review opens the same dialog with the reason on top; Allow records it again.
        g.open_dialog();
        let d = g.dialog.clone().unwrap();
        assert!(d.reason.is_some());
        assert_eq!(d.focus, Answer::NotNow);
        g.answer(Answer::Allow);
        assert!(matches!(g.situation, Situation::Granted { .. }));
        let _ = std::fs::remove_dir_all(home.parent().unwrap());
    }

    #[test]
    fn allow_is_refused_when_the_shown_path_is_not_the_pinned_file() {
        let (mut g, home) = ui_for("mismatch");
        g.open_dialog();
        g.dialog.as_mut().unwrap().path = r"C:\Elsewhere\foxglove.exe".into();
        g.answer(Answer::Allow);
        assert_eq!(state(&home), KeyboardState::NotAsked);
        assert!(g.note.as_deref().unwrap().contains("isn't the one shown"));
        let _ = std::fs::remove_dir_all(home.parent().unwrap());
    }

    #[test]
    fn turn_on_of_the_module_asks_only_on_the_users_act_and_never_grants() {
        let (mut g, home) = ui_for("turnon");
        // Started without a click (module on, never asked): header line, no dialog.
        assert_eq!(
            g.situation,
            Situation::NotAsked {
                started_without_act: true
            }
        );
        assert!(g.header().is_some() && g.dialog.is_none());
        g.after_turn_on();
        assert!(g.dialog.is_some());
        // The user acted: no header line behind the dialog.
        assert!(g.header().is_none());
        assert_eq!(state(&home), KeyboardState::NotAsked);
        let _ = std::fs::remove_dir_all(home.parent().unwrap());
    }

    #[test]
    fn turn_off_fails_safe_when_the_record_cannot_be_written() {
        let (mut g, home) = ui_for("failsafe");
        g.open_dialog();
        g.answer(Answer::Allow);
        assert!(matches!(g.situation, Situation::Granted { .. }));
        // The home folder goes away: the write fails, the row still says Off.
        std::fs::remove_dir_all(&home).unwrap();
        std::fs::write(&home, b"not a folder").unwrap();
        g.turn_off();
        assert_eq!(g.situation, Situation::Revoked);
        assert!(g.note.is_some());
        assert_eq!(row_button(&g), "Turn on\u{2026}");
        let _ = std::fs::remove_file(&home);
        let _ = std::fs::remove_dir_all(home.parent().unwrap());
    }

    #[test]
    fn turn_off_revokes_at_once_with_no_dialog() {
        let (mut g, home) = ui_for("off");
        g.open_dialog();
        g.answer(Answer::Allow);
        g.turn_off();
        assert!(g.dialog.is_none());
        assert!(matches!(g.situation, Situation::Revoked));
        let _ = std::fs::remove_dir_all(home.parent().unwrap());
    }
}
