// SPDX-License-Identifier: MIT
//! The Modules page, the hub's one page of its own (contract §2.1 job 2): every installed module
//! with its name, version, state in plain words (docs/hideout-supervisor-ux.md, column **M**),
//! one main button for the state and **Open**. In safe mode the banner with **Start normally**
//! and **Find the module that breaks Hideout** comes first (§7.10); during that search the
//! shell's bar takes its place, the rows say on or off for the round and Turn on / Turn off are
//! locked (#1033). With nothing installed it shows the empty state. Generic only: it names
//! no module (§2.2). The rows and their drawing live in `hideout_shell::modules_list`.
#![cfg_attr(not(windows), allow(dead_code))]

/// The page's id and title, kept apart from egui so they test window-free.
pub const ID: &str = "hub.modules";
pub const TITLE: &str = "Modules";

/// Where the page reads facts from (the router's cache); `None` on a hub without a router.
#[cfg(windows)]
pub type Facts = Box<dyn Fn(&str) -> Option<serde_json::Value>>;

/// What the page's buttons ask of the supervisor (Start normally, the ladder, Turn on); `None`
/// on a hub without one. Turn on answers why a module wasn't turned on.
#[cfg(windows)]
pub trait Actions {
    fn start_normally(&self);
    fn start_again(&self, id: &str);
    fn start_with_defaults(&self, id: &str);
    fn go_back(&self, id: &str);
    fn turn_off(&self, id: &str);
    fn turn_on(&self, id: &str) -> Result<(), String>;
    /// A withdrawn module's Check for updates (#1026).
    fn check_for_updates(&self, id: &str);
}

#[cfg(windows)]
impl Actions for hideout_supervisor::driver::Supervisor {
    fn start_normally(&self) {
        hideout_supervisor::driver::Supervisor::start_normally(self)
    }
    fn start_again(&self, id: &str) {
        hideout_supervisor::driver::Supervisor::start_again(self, id)
    }
    fn start_with_defaults(&self, id: &str) {
        hideout_supervisor::driver::Supervisor::start_with_defaults(self, id)
    }
    fn go_back(&self, id: &str) {
        hideout_supervisor::driver::Supervisor::go_back(self, id)
    }
    fn turn_off(&self, id: &str) {
        hideout_supervisor::driver::Supervisor::turn_off(self, id)
    }
    fn turn_on(&self, id: &str) -> Result<(), String> {
        hideout_supervisor::driver::Supervisor::turn_on(self, id)
    }
    fn check_for_updates(&self, id: &str) {
        hideout_supervisor::driver::Supervisor::check_for_updates(self, id)
    }
}

#[cfg(windows)]
pub struct ModulesPage {
    pub installed: Vec<hideout_shell::modules_list::Installed>,
    pub facts: Option<Facts>,
    pub actions: Option<Box<dyn Actions>>,
    /// The hub's closes turned safe mode on at this start (`run\hub-run.json`), so the banner
    /// names that cause; a hand-set flag reads neutral.
    pub safe_by_closes: bool,
    /// Finding the module that breaks Hideout (#1033); `None` on a second hub.
    pub finder: Option<hideout_supervisor::finder::Finder>,
    restarts: hideout_shell::modules_list::Restarts,
}

#[cfg(windows)]
impl ModulesPage {
    /// Whether the hub is in safe mode (the banner shows).
    fn safe_mode(&self) -> bool {
        self.facts
            .as_ref()
            .and_then(|f| f("hub.safe_mode"))
            .and_then(|v| v.as_bool())
            == Some(true)
    }

    /// Carries out a click. Open, Update Hideout and a Turn on that waits for the permission
    /// dialog leave the page (the module's page, the releases page).
    fn on_click(&mut self, click: hideout_shell::modules_list::Click) {
        use hideout_shell::modules_list::{Click, Does};
        let id = match &click {
            Click::Open(id) | Click::Primary(id, _) => Some(id.clone()),
            Click::StartNormally | Click::FindCulprit | Click::CopyDiagnostics => None,
        };
        let a = self.actions.as_deref();
        let open = match click {
            Click::Open(_) => true,
            Click::StartNormally => {
                match a {
                    Some(a) => a.start_normally(),
                    None => log::warn!("Start normally: no supervisor in this hub"),
                }
                false
            }
            Click::FindCulprit => {
                match &self.finder {
                    Some(f) => f.begin(),
                    None => log::warn!("Find the module: no supervisor in this hub"),
                }
                false
            }
            // Copied by the page itself (it has the egui context).
            Click::CopyDiagnostics => false,
            Click::Primary(id, does) => match (does, a) {
                (Does::UpdateHub, _) => {
                    hideout_shell::open_update_page();
                    false
                }
                (_, None) => true,
                (Does::StartAgain | Does::TryAgain, Some(a)) => {
                    a.start_again(&id);
                    false
                }
                // The result shows on the module's page (the status line), so it opens.
                (Does::CheckUpdates, Some(a)) => {
                    a.check_for_updates(&id);
                    true
                }
                (Does::StartDefaults, Some(a)) => {
                    a.start_with_defaults(&id);
                    false
                }
                (Does::GoBack, Some(a)) => {
                    a.go_back(&id);
                    false
                }
                (Does::TurnOff, Some(a)) => {
                    a.turn_off(&id);
                    self.set_enabled(&id, false);
                    false
                }
                (Does::TurnOn, Some(a)) => match a.turn_on(&id) {
                    Ok(()) => {
                        self.set_enabled(&id, true);
                        false
                    }
                    Err(why) => {
                        log::warn!("Modules page: {id} not turned on ({why}); opening its page");
                        true
                    }
                },
            },
        };
        if let (true, Some(id)) = (open, id) {
            hideout_shell::open_module(&id);
        }
    }

    fn set_enabled(&mut self, id: &str, on: bool) {
        if let Some(m) = self.installed.iter_mut().find(|m| m.id == id) {
            m.enabled = on;
        }
    }

    /// The rows for this frame, in the page's order, and whether a countdown is running
    /// (repaint in a second).
    fn rows(&mut self) -> (Vec<hideout_shell::modules_list::Row>, bool) {
        use hideout_shell::modules_list::row;
        let none = |_: &str| None;
        let facts: &dyn Fn(&str) -> Option<serde_json::Value> = match &self.facts {
            Some(f) => f.as_ref(),
            None => &none,
        };
        let now = std::time::Instant::now();
        let mut counting = false;
        let mut rows: Vec<_> = self
            .installed
            .iter()
            .map(|m| {
                let restarting =
                    facts(&format!("module.{}.state", m.id)).is_some_and(|v| v == "restarting");
                let fact = facts(&format!("module.{}.restart", m.id)).filter(|_| restarting);
                let left = self.restarts.left(&m.id, fact.as_ref(), now);
                counting |= left.is_some_and(|(s, _)| s > 0);
                row(m, facts, left)
            })
            .collect();
        // A search's words (#1033): on or off for this round, or not part of it.
        if let Some(s) = self.finder.as_ref().and_then(|f| f.rows()) {
            for r in &mut rows {
                let on = s.in_search.contains(&r.id).then(|| s.on.contains(&r.id));
                hideout_shell::modules_list::search_words(r, on);
            }
        }
        hideout_shell::modules_list::order(&mut rows);
        (rows, counting)
    }
}

/// What is installed: id, name (from its manifest), version and whether it is on.
#[cfg(windows)]
pub fn installed(home: &std::path::Path) -> Vec<hideout_shell::modules_list::Installed> {
    let Ok(reg) = hideout_registry::Registry::open_from_env(home) else {
        return Vec::new();
    };
    let mut out: Vec<_> = reg
        .inventory
        .modules
        .iter()
        .map(|(id, m)| hideout_shell::modules_list::Installed {
            id: id.clone(),
            name: reg.manifest(id).map_or_else(|| id.clone(), |x| x.name),
            version: m.version.clone(),
            enabled: reg.hub.enabled(id),
        })
        .collect();
    out.sort_by_key(|m| m.name.to_lowercase());
    out
}

#[cfg(windows)]
impl hideout_shell::Page for ModulesPage {
    fn id(&self) -> &str {
        ID
    }

    fn title(&self) -> &str {
        TITLE
    }

    /// Its own glyph among the hub pages (#953 F4): the module icon, since it lists the modules.
    fn icon(&self) -> &str {
        "module"
    }

    fn ui(&mut self, ui: &mut hideout_shell::egui::Ui, t: &hideout_shell::theme::Tokens) {
        use hideout_shell::modules_list::{page_ui_with, Banner, Click, Find, Top};
        let (rows, counting) = self.rows();
        let searching = self.finder.as_ref().is_some_and(|f| f.searching());
        let active = self.finder.as_ref().is_some_and(|f| f.active());
        let banner_shown = self.safe_mode() && !searching;
        // The link's words need the supervisor's list: only while the banner shows.
        let parts = self
            .finder
            .as_ref()
            .filter(|_| banner_shown)
            .map(|f| f.banner());
        let line = hideout_supervisor::words::safe_mode_line(self.safe_by_closes);
        let find = match &parts {
            None if self.finder.is_some() => Find::Nothing,
            None => Find::Later,
            Some(b) => b.find.as_deref().map_or(Find::Nothing, Find::Link),
        };
        let note = parts
            .as_ref()
            .filter(|b| b.stopped)
            .map(|_| hideout_supervisor::bisect::words::STOPPED);
        let top = Top {
            // During a search the shell's bar is the banner.
            banner: banner_shown.then_some(Banner { line, note, find }),
            // Locked while it runs; the result's buttons are the way out.
            locked: active,
        };
        if let Some(click) = page_ui_with(ui, t, &rows, top) {
            if click == Click::CopyDiagnostics {
                if let Some(f) = &self.finder {
                    ui.ctx().copy_text(f.diagnostics());
                }
            }
            self.on_click(click);
        }
        // Only while a row counts down and the page is drawn (no frames while hidden).
        if counting {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_secs(1));
        }
    }
}

/// The hub's pages, in sidebar order: only Modules. `facts` and `actions` are `None` on a
/// second hub; `safe_by_closes` is [`ModulesPage::safe_by_closes`].
#[cfg(windows)]
pub fn pages(
    installed: Vec<hideout_shell::modules_list::Installed>,
    facts: Option<Facts>,
    actions: Option<Box<dyn Actions>>,
    safe_by_closes: bool,
    finder: Option<hideout_supervisor::finder::Finder>,
) -> Vec<Box<dyn hideout_shell::Page>> {
    vec![Box::new(ModulesPage {
        installed,
        facts,
        actions,
        safe_by_closes,
        finder,
        restarts: Default::default(),
    })]
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use hideout_shell::modules_list::Installed;
    use serde_json::json;

    #[test]
    fn the_hub_starts_with_only_the_modules_page() {
        let pages = pages(Vec::new(), None, None, false, None);
        assert_eq!(pages.len(), 1);
        assert_eq!((pages[0].id(), pages[0].title()), (ID, TITLE));
        assert_eq!(
            hideout_shell::window_title(pages[0].title()),
            "Hideout \u{b7} Modules"
        );
    }

    #[test]
    fn the_page_lists_every_installed_module_from_its_facts() {
        let facts: Facts = Box::new(|k| match k {
            "module.m.a.state" => Some(json!("crash-stopped")),
            "module.m.b.state" => Some(json!("restarting")),
            "module.m.b.restart" => Some(json!({"delay_s": 4, "crash": 2, "of": 5})),
            _ => None,
        });
        let inst = |id: &str, name: &str| Installed {
            id: id.into(),
            name: name.into(),
            version: "1.0.0".into(),
            enabled: true,
        };
        let mut page = ModulesPage {
            installed: vec![inst("m.a", "Art"), inst("m.b", "Beat"), inst("m.c", "Chat")],
            facts: Some(facts),
            actions: None,
            safe_by_closes: false,
            finder: None,
            restarts: Default::default(),
        };
        let (rows, counting) = page.rows();
        let words: Vec<_> = rows.iter().map(|r| r.state.as_str()).collect();
        assert_eq!(
            words,
            [
                "Stopped after 5 crashes",
                "Restarting in 4 s (2 of 5)",
                "On"
            ]
        );
        assert!(counting);
    }

    #[test]
    fn installed_reads_an_empty_folder_as_nothing() {
        let dir = std::env::temp_dir().join(format!("hideout-modpage-{}", std::process::id()));
        assert!(installed(&dir).is_empty());
    }

    /// What the page asked of the supervisor, in order.
    #[derive(Clone, Default)]
    struct Log(std::rc::Rc<std::cell::RefCell<Vec<String>>>);

    impl Actions for Log {
        fn start_normally(&self) {
            self.0.borrow_mut().push("start-normally".into());
        }
        fn start_again(&self, id: &str) {
            self.0.borrow_mut().push(format!("start-again {id}"));
        }
        fn start_with_defaults(&self, id: &str) {
            self.0.borrow_mut().push(format!("defaults {id}"));
        }
        fn go_back(&self, id: &str) {
            self.0.borrow_mut().push(format!("go-back {id}"));
        }
        fn turn_off(&self, id: &str) {
            self.0.borrow_mut().push(format!("turn-off {id}"));
        }
        fn turn_on(&self, id: &str) -> Result<(), String> {
            self.0.borrow_mut().push(format!("turn-on {id}"));
            Ok(())
        }
        fn check_for_updates(&self, id: &str) {
            self.0.borrow_mut().push(format!("check {id}"));
        }
    }

    /// A withdrawn module's row (#1026): "Withdrawn · 0.5.0" with Check for updates, which
    /// reaches the supervisor; turned off, "Off · 0.5.0 was withdrawn", still no Turn on.
    #[test]
    fn a_withdrawn_row_checks_for_updates() {
        use hideout_shell::modules_list::{Click, Does};
        let off = std::rc::Rc::new(std::cell::Cell::new(false));
        let o = off.clone();
        let facts: Facts = Box::new(move |k| match k {
            "module.m.a.state" => Some(json!(if o.get() { "off" } else { "revoked" })),
            "module.m.a.revoked" => Some(json!({"version": "0.5.0", "reason": "x"})),
            _ => None,
        });
        let log = Log::default();
        let mut page = ModulesPage {
            installed: vec![Installed {
                id: "m.a".into(),
                name: "Art".into(),
                version: "0.5.0".into(),
                enabled: true,
            }],
            facts: Some(facts),
            actions: Some(Box::new(log.clone())),
            safe_by_closes: false,
            restarts: Default::default(),
            finder: None,
        };
        let (rows, _) = page.rows();
        assert_eq!(rows[0].state, "Withdrawn \u{b7} 0.5.0");
        assert!(rows[0].stopped);
        let p = rows[0].primary.clone().unwrap();
        assert_eq!(
            (p.label.as_str(), p.does),
            ("Check for updates", Does::CheckUpdates)
        );
        page.on_click(Click::Primary("m.a".into(), Does::CheckUpdates));
        assert_eq!(*log.0.borrow(), ["check m.a"]);
        off.set(true);
        page.installed[0].enabled = false;
        let (rows, _) = page.rows();
        assert_eq!(rows[0].state, "Off \u{b7} 0.5.0 was withdrawn");
        assert!(rows[0].off && !rows[0].stopped);
        assert_eq!(rows[0].primary.as_ref().unwrap().does, Does::CheckUpdates);
    }

    /// Safe mode: the banner shows, every enabled row reads "On · not started in safe mode", and
    /// Start normally goes to the supervisor. Once `hub.safe_mode` is off the banner goes.
    #[test]
    fn start_normally_from_the_banner_reaches_the_supervisor() {
        use hideout_shell::modules_list::{Click, Does};
        let safe = std::rc::Rc::new(std::cell::Cell::new(true));
        let s = safe.clone();
        let facts: Facts = Box::new(move |k| (k == "hub.safe_mode").then(|| json!(s.get())));
        let log = Log::default();
        let inst = |id: &str, name: &str, enabled: bool| Installed {
            id: id.into(),
            name: name.into(),
            version: "1.0.0".into(),
            enabled,
        };
        let mut page = ModulesPage {
            installed: vec![inst("m.a", "Art", true), inst("m.o", "Oak", false)],
            facts: Some(facts),
            actions: Some(Box::new(log.clone())),
            safe_by_closes: true,
            finder: None,
            restarts: Default::default(),
        };
        assert!(page.safe_mode());
        let (rows, _) = page.rows();
        let words: Vec<_> = rows.iter().map(|r| r.state.as_str()).collect();
        assert_eq!(words, ["On \u{b7} not started in safe mode", "Off"]);
        // Turn on in safe mode (#1033): saved on, the row reads as the others, nothing opens.
        page.on_click(Click::Primary("m.o".into(), Does::TurnOn));
        assert!(page.installed[1].enabled, "Turn on didn't stick to the row");
        let (rows, _) = page.rows();
        assert!(
            rows.iter()
                .all(|r| r.state == "On \u{b7} not started in safe mode"),
            "{:?}",
            rows.iter().map(|r| &r.state).collect::<Vec<_>>()
        );
        page.on_click(Click::StartNormally);
        assert_eq!(*log.0.borrow(), ["turn-on m.o", "start-normally"]);
        safe.set(false);
        assert!(!page.safe_mode());
        let (rows, _) = page.rows();
        assert_eq!(rows[0].state, "On");
        assert_eq!(rows[1].state, "On");
    }
}
