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
    /// Update ready's Force (§7.6).
    fn force_update(&self, id: &str);
    /// The page Update Hideout opens for `id` (#991): its catalog's release page, else the
    /// releases page. `None`: the releases page.
    fn update_hideout_url(&self, _id: &str) -> Option<String> {
        None
    }
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
    fn force_update(&self, id: &str) {
        hideout_supervisor::driver::Supervisor::force_update(self, id)
    }
    fn update_hideout_url(&self, id: &str) -> Option<String> {
        Some(hideout_supervisor::driver::Supervisor::update_hideout_url(
            self, id,
        ))
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
    /// A newer list from the inventory watch (#1054.1); `None` on a second hub.
    pub fresh: Option<Fresh>,
    restarts: hideout_shell::modules_list::Restarts,
}

/// The Modules page's list as the inventory watch last read it (#1054.1): the watch thread
/// puts a list in when it differs from the last one, and the page takes it at its next frame,
/// so a removed module's row goes and an installed one's comes without a restart.
#[cfg(windows)]
#[derive(Clone, Default)]
pub struct Fresh(std::sync::Arc<std::sync::Mutex<FreshInner>>);

#[cfg(windows)]
#[derive(Default)]
struct FreshInner {
    last: Option<Vec<hideout_shell::modules_list::Installed>>,
    waiting: bool,
}

#[cfg(windows)]
impl Fresh {
    /// A list read just now; answers whether it differs from the last one put (the page then
    /// needs a frame).
    pub fn put(&self, list: Vec<hideout_shell::modules_list::Installed>) -> bool {
        let mut f = self.0.lock().unwrap();
        if f.last.as_ref() == Some(&list) {
            return false;
        }
        f.last = Some(list);
        f.waiting = true;
        true
    }

    /// The list put since the last take, if any.
    pub(crate) fn take(&self) -> Option<Vec<hideout_shell::modules_list::Installed>> {
        let mut f = self.0.lock().unwrap();
        if !std::mem::take(&mut f.waiting) {
            return None;
        }
        f.last.clone()
    }
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

    /// The page Update Hideout opens for `id` (#991, §7.8 v1.11): the release its catalog names
    /// when that page is allowed, else the releases page.
    fn update_page(&self, id: &str) -> String {
        let url = self
            .actions
            .as_deref()
            .and_then(|a| a.update_hideout_url(id));
        hideout_shell::update_page(url.as_deref().unwrap_or("")).to_string()
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
                    hideout_shell::open_update_page_at(&self.update_page(&id));
                    false
                }
                // Go to <module>: the root's page (root cause once, §7.8).
                (Does::GoTo, _) => {
                    let root = self
                        .facts
                        .as_ref()
                        .and_then(|f| f(&format!("module.{id}.waiting")))
                        .and_then(|v| v.as_str().map(String::from));
                    if let Some(r) = root {
                        hideout_shell::open_module(&r);
                    }
                    false
                }
                // The page's card has the new permission and Not now.
                (Does::Review, _) => true,
                (Does::Force, Some(a)) => {
                    a.force_update(&id);
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
        use hideout_shell::modules_list::row_in;
        if let Some(list) = self.fresh.as_ref().and_then(Fresh::take) {
            self.installed = list;
        }
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
                row_in(m, &self.installed, facts, left)
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

/// What is installed: id, name (from its manifest), version and whether it is on. A registry
/// that can't be read is logged with its reason and reads as nothing (the page's first list).
#[cfg(windows)]
pub fn installed(home: &std::path::Path) -> Vec<hideout_shell::modules_list::Installed> {
    match read_installed(home) {
        Ok(list) => {
            // Which folder and how many, so a page without rows says why in the log (#1076).
            log::info!(
                "Modules page: {} installed module(s) in {}",
                list.len(),
                home.display()
            );
            list
        }
        Err(why) => {
            log::warn!("Modules page: the installed modules couldn't be read ({why}); no rows");
            Vec::new()
        }
    }
}

/// [`installed`], or why the registry couldn't be read (#1076): the inventory watch keeps the
/// page's last list then, so a failed read never empties the page.
#[cfg(windows)]
pub fn read_installed(
    home: &std::path::Path,
) -> Result<Vec<hideout_shell::modules_list::Installed>, String> {
    let reg = hideout_registry::Registry::open_from_env(home).map_err(|e| e.to_string())?;
    Ok(list(&reg))
}

/// The page's list from a registry already read.
#[cfg(windows)]
pub fn list(reg: &hideout_registry::Registry) -> Vec<hideout_shell::modules_list::Installed> {
    let mut out: Vec<_> = reg
        .inventory
        .modules
        .iter()
        .map(|(id, m)| hideout_shell::modules_list::Installed {
            id: id.clone(),
            // A manifest for a newer hub is named from its head (#1096).
            name: reg.module_name(id),
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
    fresh: Option<Fresh>,
) -> Vec<Box<dyn hideout_shell::Page>> {
    vec![Box::new(ModulesPage {
        installed,
        facts,
        actions,
        safe_by_closes,
        finder,
        fresh,
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
        let pages = pages(Vec::new(), None, None, false, None, None);
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
            fresh: None,
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

    /// #1054.1: a list from the inventory watch replaces the page's at its next frame (a
    /// removed row goes, an installed one comes); the same list again asks for nothing.
    #[test]
    fn the_page_takes_the_watchs_fresh_list() {
        let inst = |id: &str, name: &str| Installed {
            id: id.into(),
            name: name.into(),
            version: "1.0.0".into(),
            enabled: true,
        };
        let fresh = Fresh::default();
        let mut page = ModulesPage {
            installed: vec![inst("m.a", "Art"), inst("m.b", "Beat")],
            facts: None,
            actions: None,
            safe_by_closes: false,
            finder: None,
            fresh: Some(fresh.clone()),
            restarts: Default::default(),
        };
        let ids = |page: &mut ModulesPage| -> Vec<String> {
            page.rows().0.into_iter().map(|r| r.id).collect()
        };
        assert_eq!(ids(&mut page), ["m.a", "m.b"]);
        assert!(fresh.put(vec![inst("m.b", "Beat"), inst("m.c", "Chat")]));
        assert_eq!(ids(&mut page), ["m.b", "m.c"]);
        // Taken once: a Turn off on the page sticks until the watch reads something new.
        page.set_enabled("m.c", false);
        assert!(!page
            .rows()
            .0
            .iter()
            .any(|r| r.id == "m.c" && r.state == "On"));
        assert!(!fresh.put(vec![inst("m.b", "Beat"), inst("m.c", "Chat")]));
        assert!(!page
            .rows()
            .0
            .iter()
            .any(|r| r.id == "m.c" && r.state == "On"));
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
        fn force_update(&self, id: &str) {
            self.0.borrow_mut().push(format!("force {id}"));
        }
        fn update_hideout_url(&self, id: &str) -> Option<String> {
            match id {
                "m.tag" => Some(TAG.into()),
                "m.off" => Some("https://example.invalid/hideout-v0.4.0".into()),
                _ => None,
            }
        }
    }

    const TAG: &str = "https://github.com/BanditF/foxglove/releases/tag/hideout-v0.4.0";

    /// Needs a newer Hideout rows (#991): Update Hideout opens the release the catalog names
    /// when it is on the allow-list, else the releases page; no supervisor, the releases page.
    /// (The click itself opens a browser, so the page it would open is checked instead.)
    #[test]
    fn update_hideout_on_a_row_opens_the_release_it_needs() {
        use hideout_shell::modules_list::Does;
        let facts: Facts = Box::new(|k| match k {
            "module.m.tag.state" | "module.m.off.state" | "module.m.none.state" => {
                Some(json!("needs-hub"))
            }
            _ => None,
        });
        let inst = |id: &str| Installed {
            id: id.into(),
            name: id.into(),
            version: "1.0.0".into(),
            enabled: true,
        };
        let log = Log::default();
        let mut page = ModulesPage {
            installed: vec![inst("m.tag"), inst("m.off"), inst("m.none")],
            facts: Some(facts),
            actions: Some(Box::new(log.clone())),
            safe_by_closes: false,
            restarts: Default::default(),
            finder: None,
            fresh: None,
        };
        let (rows, _) = page.rows();
        for r in &rows {
            let p = r.primary.as_ref().unwrap();
            assert_eq!(
                (p.label.as_str(), p.does),
                ("Update Hideout", Does::UpdateHub)
            );
        }
        let releases = hideout_contract::release::RELEASES_URL;
        assert_eq!(page.update_page("m.tag"), TAG);
        assert_eq!(page.update_page("m.off"), releases);
        assert_eq!(page.update_page("m.none"), releases);
        page.actions = None;
        assert_eq!(page.update_page("m.tag"), releases);
        assert!(log.0.borrow().is_empty());
    }

    /// The states #1086 publishes (#983): Waiting names the root by its name with Go to it;
    /// Update ready's Force reaches the supervisor; Update waiting for you's Review opens the
    /// page; a quarantined row is red with Start again.
    #[test]
    fn the_waiting_update_and_quarantine_rows() {
        use hideout_shell::modules_list::{Click, Does};
        let facts: Facts = Box::new(|k| match k {
            "module.m.a.state" => Some(json!("waiting")),
            "module.m.a.waiting" => Some(json!("m.l")),
            "module.m.l.state" => Some(json!("crash-stopped")),
            "module.m.b.state" => Some(json!("update-busy")),
            "module.m.c.state" => Some(json!("update-waiting")),
            "module.m.q.state" => Some(json!("quarantined")),
            _ => None,
        });
        let inst = |id: &str, name: &str| Installed {
            id: id.into(),
            name: name.into(),
            version: "1.0.0".into(),
            enabled: true,
        };
        let log = Log::default();
        let mut page = ModulesPage {
            installed: vec![
                inst("m.a", "Art"),
                inst("m.l", "Link"),
                inst("m.b", "Beat"),
                inst("m.c", "Chat"),
                inst("m.q", "Quill"),
            ],
            facts: Some(facts),
            actions: Some(Box::new(log.clone())),
            safe_by_closes: false,
            restarts: Default::default(),
            finder: None,
            fresh: None,
        };
        let (rows, _) = page.rows();
        let get = |id: &str| rows.iter().find(|r| r.id == id).unwrap().clone();
        let a = get("m.a");
        assert_eq!(a.state, "Waiting for Link");
        assert!(!a.stopped, "only the root is red");
        let p = a.primary.unwrap();
        assert_eq!((p.label.as_str(), p.does), ("Go to Link", Does::GoTo));
        let b = get("m.b");
        assert_eq!(b.state, "Update ready");
        assert_eq!(b.primary.unwrap().does, Does::Force);
        let c = get("m.c");
        assert_eq!(c.state, "Update waiting for you");
        assert_eq!(c.primary.unwrap().label, "Review");
        let q = get("m.q");
        assert_eq!(q.state, "Quill's page couldn't be shown");
        assert_eq!(q.label, "Quill's page couldn't be shown");
        assert!(q.stopped);
        assert_eq!(q.primary.unwrap().does, Does::StartAgain);
        page.on_click(Click::Primary("m.b".into(), Does::Force));
        page.on_click(Click::Primary("m.q".into(), Does::StartAgain));
        assert_eq!(*log.0.borrow(), ["force m.b", "start-again m.q"]);
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
            fresh: None,
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
            fresh: None,
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
