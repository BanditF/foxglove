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
pub const TITLE: &str = hideout_shell::modules_list::TITLE;

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
    /// A hand-over begins: the row is saved as on without starting it and held, so a hub that
    /// restarts before the app started brings it up. Fakes may do nothing.
    fn begin_hand_over(&self, _id: &str) -> Result<(), String> {
        Ok(())
    }
    /// The hand-over ended; `went` false saves the row back as off.
    fn end_hand_over(&self, _id: &str, _went: bool) {}
    /// Turn on as a handle any thread can call, so a hand-over's worker can start the app itself
    /// when its copy has gone (the page may not be on screen then). `None`: the page starts it.
    fn starter(&self) -> Option<crate::permission_actions::Starter> {
        None
    }
    /// A withdrawn module's Check for updates (#1026).
    fn check_for_updates(&self, id: &str);
    /// Update ready's Force (§7.6).
    fn force_update(&self, id: &str);
    /// Review on "Update waiting for you" after the user's Allow: the swap runs.
    fn review(&self, _id: &str) {}
    /// "Remove from Hideout..." after the user's confirm: the module is stopped if it runs
    /// and the registry forgets it ([`hideout_supervisor::driver::Supervisor::remove`]).
    /// Its own settings and files are never deleted.
    fn remove(&self, _id: &str) -> Result<(), String> {
        Err("no supervisor in this hub".into())
    }
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
    fn begin_hand_over(&self, id: &str) -> Result<(), String> {
        hideout_supervisor::driver::Supervisor::begin_hand_over(self, id)
    }
    fn end_hand_over(&self, id: &str, went: bool) {
        hideout_supervisor::driver::Supervisor::end_hand_over(self, id, went)
    }
    fn starter(&self) -> Option<crate::permission_actions::Starter> {
        let me = self.clone();
        Some(std::sync::Arc::new(move |id: &str| {
            hideout_supervisor::driver::Supervisor::turn_on(&me, id)
        }))
    }
    fn check_for_updates(&self, id: &str) {
        hideout_supervisor::driver::Supervisor::check_for_updates(self, id)
    }
    fn force_update(&self, id: &str) {
        hideout_supervisor::driver::Supervisor::force_update(self, id)
    }
    fn review(&self, id: &str) {
        hideout_supervisor::driver::Supervisor::review(self, id)
    }
    fn remove(&self, id: &str) -> Result<(), String> {
        hideout_supervisor::driver::Supervisor::remove(self, id)
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
    /// The keyboard grant's row, header line and dialog (#1155-B); `None` on a second hub.
    pub grant: Option<crate::keyboard_actions::GrantUi>,
    /// The module permission dialog, header lines and sections (#1178); `None` on a second hub.
    pub perm: Option<crate::permission_actions::PermUi>,
    /// The Found card and Add an app… (slice 5); `None` on a second hub.
    pub discover: Option<crate::discover::DiscoverUi>,
    restarts: hideout_shell::modules_list::Restarts,
    /// The Done card after the user's Allow started an app (first-run FR4).
    done: hideout_shell::done_card::Done,
    /// The Remove confirm: the row menu asks, only the confirm's Remove removes.
    ask: hideout_shell::modules_list::RemoveAsk,
    /// Which rebuild restarts were already told, by module id (the fact's value), so a redraw,
    /// a search or a reopened window doesn't say one twice.
    told: Told,
}

/// The announcements the page hands the shell, and what it already told.
#[derive(Default)]
struct Told {
    /// The first frame only learns what is there; a restart from before isn't news.
    seeded: bool,
    by_id: std::collections::BTreeMap<String, String>,
    said: Vec<String>,
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
    /// Modules whose Remove failed after they stopped (the supervisor says so).
    failed: Vec<String>,
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

    /// A Remove that waited for the module to stop did not work; answers whether the page
    /// needs a frame (always).
    pub fn put_failed(&self, id: &str) -> bool {
        self.0.lock().unwrap().failed.push(id.into());
        true
    }

    pub(crate) fn take_failed(&self) -> Vec<String> {
        std::mem::take(&mut self.0.lock().unwrap().failed)
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
            Click::StartNormally
            | Click::FindCulprit
            | Click::CopyDiagnostics
            | Click::AskRemove(_)
            | Click::Remove(_)
            | Click::CancelRemove
            | Click::DismissRemove(_) => None,
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
            Click::AskRemove(id) => {
                self.ask.ask(&id);
                false
            }
            Click::CancelRemove => {
                self.ask.cancel();
                false
            }
            Click::DismissRemove(id) => {
                self.ask.clear(&id);
                false
            }
            Click::Remove(id) => {
                self.remove_confirmed(&id);
                false
            }
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
                // The permission dialog (§6.4) opens on the user's click; without a hub that
                // holds the router there is none, and the module's page opens as before.
                (Does::Review, _) => match self.perm.as_mut() {
                    Some(p) => {
                        p.open(&id, crate::permission_actions::Opener::Review);
                        false
                    }
                    None => true,
                },
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
                        // The legacy module's keyboard grant asks on the user's own Turn on.
                        if id == crate::keyboard_actions::MODULE {
                            if let Some(g) = self.grant.as_mut() {
                                g.after_turn_on();
                            }
                        }
                        false
                    }
                    // Waiting for the user's OK: Turn on… opens the dialog (Allow then turns it on).
                    Err(_) if self.perm.as_ref().is_some_and(|p| p.needs_ok(&id)) => {
                        if let Some(p) = self.perm.as_mut() {
                            p.open(&id, crate::permission_actions::Opener::TurnOn);
                        }
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

    /// The confirm's Remove: only for the module that was asked about, and only with a
    /// supervisor to stop it. The row goes at once; the inventory watch's list agrees.
    fn remove_confirmed(&mut self, id: &str) {
        if !self.ask.confirm(id) {
            log::warn!("Modules page: Remove {id} without its confirm; ignored");
            return;
        }
        let Some(a) = self.actions.as_deref() else {
            log::warn!("Modules page: Remove {id}: no supervisor in this hub");
            return;
        };
        match a.remove(id) {
            Ok(()) => {
                // The row stays as "Removing…" until the inventory list no longer has it (a
                // module that runs is only stopping): the page never says it is gone first.
                self.ask
                    .set(id, hideout_shell::modules_list::Removal::Removing);
                if let Some(g) = self.grant.as_mut() {
                    g.refresh();
                }
                if let Some(p) = self.perm.as_mut() {
                    p.refresh();
                }
            }
            Err(why) => {
                log::warn!("Modules page: {id} not removed ({why})");
                self.ask
                    .set(id, hideout_shell::modules_list::Removal::Failed);
            }
        }
    }

    /// What follows the user's Allow in the permission dialog: the module turns on (Turn on…,
    /// a header's Review…) or its update swaps (Review…); Change… only records.
    fn after_allow(&mut self, done: crate::permission_actions::Done) {
        use crate::permission_actions::Then;
        if let Some(g) = self.grant.as_mut() {
            g.refresh();
        }
        if self.actions.is_none() {
            return;
        }
        let starter = self.actions.as_deref().and_then(|a| a.starter());
        match done.then {
            // FR5: the dialog said a standalone copy would be closed. It is asked to quit on a
            // worker; the app starts when that finishes well (`settle_hand_over`). If the copy
            // does not go (or anything is in doubt) the app is not started, a note with Start
            // again shows, and the Startup entry is never touched.
            Then::TurnOn if done.handover && self.begin_hand_over(&done.id, starter.clone()) => {}
            // A standalone copy opened after the dialog, which did not tell the user it would be
            // closed: nothing is started beside it and nothing is closed unasked. The note with
            // Start again shows, and Start again is the user's go-ahead to close it.
            // The look is on a worker like the hand-over (`settle_hand_over`).
            Then::TurnOn
                if !done.handover
                    && self
                        .perm
                        .as_mut()
                        .is_some_and(|p| p.start_late_check(&done.id, starter.clone())) => {}
            Then::TurnOn => self.turn_on_now(&done.id),
            Then::Review => {
                if let Some(a) = self.actions.as_deref() {
                    a.review(&done.id);
                }
            }
            Then::Nothing => {}
        }
        if let Some(p) = self.perm.as_mut() {
            p.refresh();
        }
    }

    /// The row is saved as on and held before the copy is asked to quit: its quit cannot be taken
    /// back, so a hub that exits before the app started still starts it at the next start. True:
    /// the worker runs and `settle_hand_over` ends the hold. False: nothing runs (it could not be
    /// saved, or there is no standalone support) and the plain start follows, which shows why.
    fn begin_hand_over(
        &mut self,
        id: &str,
        starter: Option<crate::permission_actions::Starter>,
    ) -> bool {
        if let Some(Err(why)) = self.actions.as_deref().map(|a| a.begin_hand_over(id)) {
            log::warn!("Modules page: {id} not saved as on before the hand-over ({why})");
            return false;
        }
        let running = self
            .perm
            .as_mut()
            .is_some_and(|p| p.start_hand_over(id, starter));
        if !running {
            if let Some(a) = self.actions.as_deref() {
                a.end_hand_over(id, true);
            }
        }
        running
    }

    /// Starts the app now and shows the row and the Done card as on.
    fn turn_on_now(&mut self, id: &str) {
        let Some(a) = self.actions.as_deref() else {
            return;
        };
        let result = a.turn_on(id);
        self.after_turn_on(id, result);
    }

    /// The row, the Done card and any failure line for a start that has been tried.
    fn after_turn_on(&mut self, id: &str, result: Result<(), String>) {
        match result {
            Ok(()) => {
                self.ask.clear_turn_on_failed(id);
                self.set_enabled(id, true);
                // The user's Allow started it: the Done card says so, live.
                self.done.show_for(id);
                if id == crate::keyboard_actions::MODULE {
                    if let Some(g) = self.grant.as_mut() {
                        g.after_turn_on();
                    }
                }
            }
            Err(why) => {
                log::warn!("Modules page: {id} not turned on ({why})");
                let name = self
                    .installed
                    .iter()
                    .find(|m| m.id == id)
                    .map_or(id, |m| m.name.as_str());
                let line = hideout_shell::modules_list::turn_on_failed(name, &why);
                self.ask.set_turn_on_failed(id, line);
            }
        }
        if let Some(p) = self.perm.as_mut() {
            p.refresh();
        }
    }

    /// A finished hand-over: the app starts once its copy has gone (looked at once per frame;
    /// `wait` is for tests).
    fn settle_hand_over(&mut self, wait: bool) {
        // Every finished app, not only the first: two apps allowed together both start. A worker
        // that could start its app has done so already (whether or not this page was on screen);
        // only the row and the card are left to do here.
        while let Some((id, h)) = self.perm.as_mut().and_then(|p| p.finished_hand_over(wait)) {
            // A late check holds nothing (its end is a no-op); a hand-over that failed is saved
            // back as off before the note, so nothing starts beside the copy that stayed.
            if let Some(a) = self.actions.as_deref() {
                a.end_hand_over(&id, h.went);
            }
            if h.went {
                match h.started {
                    Some(r) => self.after_turn_on(&id, r),
                    None => self.turn_on_now(&id),
                }
            }
        }
    }

    /// Start again after a hand-over that did not happen: the same step, once more.
    fn start_again_pending(&mut self) {
        let Some(id) = self.perm.as_mut().and_then(|p| p.take_retry()) else {
            return;
        };
        self.after_allow(crate::permission_actions::Done {
            id,
            code: String::new(),
            chosen: Vec::new(),
            declined: Vec::new(),
            then: crate::permission_actions::Then::TurnOn,
            handover: true,
        });
    }

    /// An app was just added (Found card or Browse): the list takes it now and the normal
    /// first-run Allow dialog opens for it. Nothing is allowed or started until the user's Allow.
    fn after_add(&mut self, id: &str) {
        if let Some(home) = self.discover.as_ref().map(|d| d.home.clone()) {
            match read_installed(&home) {
                Ok(list) => self.installed = list,
                Err(why) => {
                    log::warn!("Modules page: the list after adding {id}: {why}");
                    if let Some(d) = self.discover.as_mut() {
                        d.state.note = Some(crate::discover::Note::error(
                            crate::discover::words::LIST_NOT_READ,
                        ));
                    }
                }
            }
        }
        if let Some(p) = self.perm.as_mut() {
            p.refresh();
            p.open(id, crate::permission_actions::Opener::TurnOn);
        }
    }

    /// The Done card for this frame: the status from the app's facts, the button from its
    /// manifest and the offers still worth making. `None` while it is hidden or its app is gone.
    fn done_card(&self) -> Option<hideout_shell::done_card::Card> {
        use hideout_shell::done_card::Inputs;
        let id = self.done.module()?;
        let name = self.installed.iter().find(|m| m.id == id)?.name.clone();
        let none = |_: &str| None;
        let facts: &dyn Fn(&str) -> Option<serde_json::Value> = match &self.facts {
            Some(f) => f.as_ref(),
            None => &none,
        };
        let more = self
            .discover
            .as_ref()
            .map(|d| {
                d.state
                    .shown()
                    .into_iter()
                    .filter(|c| c.id != id && !self.installed.iter().any(|m| m.id == c.id))
                    .map(|c| (c.id.clone(), c.name.clone()))
                    .collect()
            })
            .unwrap_or_default();
        self.done.card(&Inputs {
            name: &name,
            facts,
            identify: hideout_shell::quick_actions::identify_words(id),
            hint: hideout_shell::quick_actions::identify_hint(id),
            has_page: hideout_shell::quick_actions::has_page(id),
            sharing_open: id == crate::keyboard_actions::MODULE
                && self.grant.as_ref().is_some_and(|g| g.can_offer()),
            more,
        })
    }

    /// A press on the Done card. A button only goes where the user decides (Settings, the
    /// sharing dialog, the add step); nothing is turned on from here.
    fn on_done(&mut self, press: hideout_shell::done_card::Press, ids: &[String]) {
        use hideout_shell::done_card::{Offer, Press};
        let Some(id) = self.done.module().map(String::from) else {
            return;
        };
        match press {
            Press::Show => match hideout_shell::quick_actions::identify_words(&id) {
                Some(_) => hideout_shell::quick_actions::invoke(
                    &id,
                    hideout_shell::quick_actions::IDENTIFY,
                ),
                None => hideout_shell::open_module(&id),
            },
            Press::Offer(Offer::StartWithWindows) => {
                hideout_shell::open_module(hideout_shell::settings_view::ID)
            }
            Press::Offer(Offer::Sharing) => {
                if let Some(g) = self.grant.as_mut() {
                    g.open_dialog();
                }
            }
            Press::Offer(Offer::SetUp(app)) => {
                let added = self.discover.as_mut().and_then(|d| d.set_up(&app, ids));
                if let Some(added) = added {
                    self.after_add(&added);
                }
            }
            Press::NotNow(o) => self.done.put_off(o),
            Press::Hide => self.done.hide(),
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
            let listed = &self.installed;
            self.ask
                .retain_listed(|id| listed.iter().any(|m| m.id == id));
            if let Some(g) = self.grant.as_mut() {
                g.refresh();
            }
            if let Some(p) = self.perm.as_mut() {
                p.refresh();
            }
        }
        // A Remove that had to wait for the module to stop and then did not work.
        for id in self
            .fresh
            .as_ref()
            .map(Fresh::take_failed)
            .unwrap_or_default()
        {
            self.ask
                .set(&id, hideout_shell::modules_list::Removal::Failed);
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
        // A rebuild restart is told once, when its line first appears (absent to set).
        for r in &rows {
            let at = r
                .rebuilt_said
                .as_ref()
                .and_then(|_| facts(&format!("module.{}.rebuilt", r.id)))
                .and_then(|v| v.as_str().map(str::to_string));
            match (at, r.rebuilt_said.as_ref()) {
                (Some(at), Some(said)) => {
                    let new = self.told.by_id.get(&r.id) != Some(&at);
                    if new && self.told.seeded {
                        self.told.said.push(said.clone());
                    }
                    self.told.by_id.insert(r.id.clone(), at);
                }
                _ => {
                    self.told.by_id.remove(&r.id);
                }
            }
        }
        self.told.seeded = true;
        // A search's words (#1033): on or off for this round, or not part of it.
        if let Some(s) = self.finder.as_ref().and_then(|f| f.rows()) {
            for r in &mut rows {
                let on = s.in_search.contains(&r.id).then(|| s.on.contains(&r.id));
                hideout_shell::modules_list::search_words(r, on);
            }
        }
        for r in &mut rows {
            r.removal = self.ask.state(&r.id);
            r.turn_on_note = self.ask.turn_on_line(&r.id).map(String::from);
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
        .map(|(id, m)| {
            let dev_folder = reg.dev_folder_of(id);
            hideout_shell::modules_list::Installed {
                id: id.clone(),
                // A manifest for a newer hub is named from its head (#1096).
                name: reg.module_name(id),
                version: m.version.clone(),
                enabled: reg.hub.enabled(id),
                adopted: m.source == hideout_registry::inventory::ADOPTED,
                dev: dev_folder.is_some(),
                dev_folder,
            }
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

    fn take_said(&mut self) -> Vec<String> {
        std::mem::take(&mut self.told.said)
    }

    /// A finished hand-over settles on every hub frame, not only while this page is drawn: the
    /// hold ends and a failed one is saved back as off even if the user left the page or closed
    /// the window to the tray meanwhile.
    fn background(&mut self) {
        self.settle_hand_over(false);
    }

    fn ui(&mut self, ui: &mut hideout_shell::egui::Ui, t: &hideout_shell::theme::Tokens) {
        use hideout_shell::modules_list::{page_ui_with_head, Banner, Click, Find, Top};
        let (rows, counting) = self.rows();
        // Review... / Set up... pressed on Settings > Developer: open that app's dialog (the
        // user's own click made the request; Allow is still theirs).
        let asked = crate::review_ask::take();
        if let (Some(p), Some((id, set_up))) = (self.perm.as_mut(), asked.last()) {
            let how = if *set_up {
                crate::permission_actions::Opener::Header
            } else {
                p.review_opener(id)
            };
            p.open(id, how);
        }
        let state = self
            .facts
            .as_ref()
            .and_then(|f| f(&format!("module.{}.state", crate::keyboard_actions::MODULE)));
        if let Some(g) = self.grant.as_mut() {
            g.sync(state);
        }
        // A module's start voids a grant whose program changed: its state fact is the event
        // that re-reads the permissions (no timer).
        if let Some(p) = self.perm.as_mut() {
            let states: Vec<Option<serde_json::Value>> = self
                .installed
                .iter()
                .map(|m| {
                    self.facts
                        .as_ref()
                        .and_then(|f| f(&format!("module.{}.state", m.id)))
                })
                .collect();
            p.sync(states);
        }
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
            remove: self.ask.pending(),
            // Without a supervisor (a second hub) there is nothing to stop it with.
            can_remove: self.actions.is_some(),
        };
        let mut grant = self.grant.take();
        let mut perm = self.perm.take();
        let mut discover = self.discover.take();
        let ids: Vec<String> = self.installed.iter().map(|m| m.id.clone()).collect();
        let mut added: Option<String> = None;
        let card = self.done_card();
        let mut pressed = None;
        // The Done card lists the other found apps itself: the Found card waits.
        if let Some(d) = discover.as_mut() {
            // The Found card steps aside only while this card still offers to set an app up.
            d.quiet = card.as_ref().is_some_and(|c| {
                c.offers
                    .iter()
                    .any(|o| matches!(o.offer, hideout_shell::done_card::Offer::SetUp(_)))
            });
        }
        let clicked = page_ui_with_head(ui, t, &rows, top, &mut |ui| {
            if let Some(c) = &card {
                pressed = hideout_shell::done_card::card_ui(ui, t, c);
            }
            if let Some(g) = grant.as_mut() {
                g.top(ui, t);
            }
            if let Some(p) = perm.as_mut() {
                p.top(ui, t);
            }
            if let Some(d) = discover.as_mut() {
                added = added.take().or(d.ui(ui, t, &ids));
            }
        });
        self.grant = grant;
        self.perm = perm;
        self.discover = discover;
        self.start_again_pending();
        self.settle_hand_over(false);
        if let Some(id) = added {
            self.after_add(&id);
        }
        if let Some(p) = pressed {
            self.on_done(p, &ids);
        }
        if let Some(click) = clicked {
            if click == Click::CopyDiagnostics {
                if let Some(f) = &self.finder {
                    ui.ctx().copy_text(f.diagnostics());
                }
            }
            self.on_click(click);
        }
        // The keyboard grant's dialog, over the page.
        if let Some(g) = self.grant.as_mut() {
            g.dialog(ui.ctx(), t);
        }
        // The module permission dialog: Allow records the grant, then the module turns on.
        let done = self
            .perm
            .as_mut()
            .map(|p| p.dialog(ui.ctx(), t))
            .unwrap_or_default();
        for done in done {
            self.after_allow(done);
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
#[allow(clippy::too_many_arguments)]
pub fn pages(
    installed: Vec<hideout_shell::modules_list::Installed>,
    facts: Option<Facts>,
    actions: Option<Box<dyn Actions>>,
    safe_by_closes: bool,
    finder: Option<hideout_supervisor::finder::Finder>,
    fresh: Option<Fresh>,
    grant: Option<crate::keyboard_actions::GrantUi>,
    perm: Option<crate::permission_actions::PermUi>,
    discover: Option<crate::discover::DiscoverUi>,
) -> Vec<Box<dyn hideout_shell::Page>> {
    vec![Box::new(ModulesPage {
        installed,
        facts,
        actions,
        safe_by_closes,
        finder,
        fresh,
        grant,
        perm,
        discover,
        restarts: Default::default(),
        done: Default::default(),
        ask: Default::default(),
        told: Default::default(),
    })]
}

#[cfg(all(test, windows))]
mod first_run;

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use hideout_shell::modules_list::Installed;
    use serde_json::json;

    #[test]
    fn the_hub_starts_with_only_the_modules_page() {
        let pages = pages(Vec::new(), None, None, false, None, None, None, None, None);
        assert_eq!(pages.len(), 1);
        assert_eq!((pages[0].id(), pages[0].title()), (ID, TITLE));
        assert_eq!(
            hideout_shell::window_title(pages[0].title()),
            "Hideout \u{b7} Apps"
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
            adopted: false,
            dev: false,
            dev_folder: None,
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
            grant: None,
            perm: None,
            discover: None,
            restarts: Default::default(),
            done: Default::default(),
            ask: Default::default(),
            told: Default::default(),
        };
        let (rows, counting) = page.rows();
        let words: Vec<_> = rows.iter().map(|r| r.state.as_str()).collect();
        assert_eq!(
            words,
            [
                "Stopped after 5 crashes",
                "Restarting in 4 s, try 2 of 5",
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
            adopted: false,
            dev: false,
            dev_folder: None,
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
            discover: None,
            restarts: Default::default(),
            done: Default::default(),
            ask: Default::default(),
            told: Default::default(),
            grant: None,
            perm: None,
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
        fn begin_hand_over(&self, id: &str) -> Result<(), String> {
            self.0.borrow_mut().push(format!("begin {id}"));
            Ok(())
        }
        fn end_hand_over(&self, id: &str, went: bool) {
            self.0.borrow_mut().push(format!("end {id} {went}"));
        }
        fn check_for_updates(&self, id: &str) {
            self.0.borrow_mut().push(format!("check {id}"));
        }
        fn force_update(&self, id: &str) {
            self.0.borrow_mut().push(format!("force {id}"));
        }
        fn remove(&self, id: &str) -> Result<(), String> {
            self.0.borrow_mut().push(format!("remove {id}"));
            // "m.bad" is a module the supervisor can't remove.
            if id == "m.bad" {
                return Err("still running".into());
            }
            Ok(())
        }
        fn update_hideout_url(&self, id: &str) -> Option<String> {
            match id {
                "m.tag" => Some(TAG.into()),
                "m.off" => Some("https://example.invalid/hideout-v0.4.0".into()),
                _ => None,
            }
        }
    }

    /// Slice 2: what follows an Allow. Turn on goes to the supervisor and the row reads on;
    /// Change only records; a supervisor that refuses leaves the row off.
    #[test]
    fn after_allow_turns_on_only_when_the_allow_says_so() {
        use crate::permission_actions::{Done, Then};
        let done = |then| Done {
            id: "m.a".into(),
            code: String::new(),
            chosen: Vec::new(),
            declined: Vec::new(),
            then,
            handover: false,
        };
        let inst = Installed {
            adopted: true,
            id: "m.a".into(),
            name: "Art".into(),
            version: "1.0.0".into(),
            enabled: false,
            dev: false,
            dev_folder: None,
        };
        let log = Log::default();
        let mut page = ModulesPage {
            installed: vec![inst],
            facts: None,
            actions: Some(Box::new(log.clone())),
            safe_by_closes: false,
            restarts: Default::default(),
            done: Default::default(),
            grant: None,
            perm: None,
            finder: None,
            fresh: None,
            ask: Default::default(),
            told: Default::default(),
            discover: None,
        };
        page.after_allow(done(Then::Nothing));
        assert!(log.0.borrow().is_empty());
        assert!(!page.installed[0].enabled);
        page.after_allow(done(Then::TurnOn));
        assert_eq!(*log.0.borrow(), ["turn-on m.a"]);
        assert!(page.installed[0].enabled);
    }

    /// A promised hand-over saves the row as on before anything else, so a hub that exits while
    /// the copy leaves still starts the app at its next start.
    #[test]
    fn a_hand_over_saves_the_row_as_on_first() {
        use crate::permission_actions::{Done, Then};
        let inst = Installed {
            adopted: true,
            id: "m.a".into(),
            name: "Art".into(),
            version: "1.0.0".into(),
            enabled: false,
            dev: false,
            dev_folder: None,
        };
        let log = Log::default();
        let mut page = ModulesPage {
            installed: vec![inst],
            facts: None,
            actions: Some(Box::new(log.clone())),
            safe_by_closes: false,
            restarts: Default::default(),
            done: Default::default(),
            grant: None,
            perm: None,
            finder: None,
            fresh: None,
            ask: Default::default(),
            told: Default::default(),
            discover: None,
        };
        page.after_allow(Done {
            id: "m.a".into(),
            code: String::new(),
            chosen: Vec::new(),
            declined: Vec::new(),
            then: Then::TurnOn,
            handover: true,
        });
        assert_eq!(
            *log.0.borrow(),
            ["begin m.a", "end m.a true", "turn-on m.a"]
        );
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
            adopted: false,
            dev: false,
            dev_folder: None,
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
            discover: None,
            restarts: Default::default(),
            done: Default::default(),
            ask: Default::default(),
            told: Default::default(),
            grant: None,
            perm: None,
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
            adopted: false,
            dev: false,
            dev_folder: None,
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
            discover: None,
            restarts: Default::default(),
            done: Default::default(),
            ask: Default::default(),
            told: Default::default(),
            grant: None,
            perm: None,
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
                adopted: false,
                dev: false,
                dev_folder: None,
                id: "m.a".into(),
                name: "Art".into(),
                version: "0.5.0".into(),
                enabled: true,
            }],
            facts: Some(facts),
            actions: Some(Box::new(log.clone())),
            safe_by_closes: false,
            discover: None,
            restarts: Default::default(),
            done: Default::default(),
            ask: Default::default(),
            told: Default::default(),
            grant: None,
            perm: None,
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
            adopted: false,
            dev: false,
            dev_folder: None,
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
            grant: None,
            perm: None,
            discover: None,
            restarts: Default::default(),
            done: Default::default(),
            ask: Default::default(),
            told: Default::default(),
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

    /// Remove from Hideout: the menu only asks, a Remove without its confirm does nothing, the
    /// confirm removes once (the supervisor stops and the registry forgets), Cancel asks
    /// for nothing, and the row goes.
    #[test]
    fn remove_needs_the_confirm_and_reaches_the_supervisor_once() {
        use hideout_shell::modules_list::Click;
        let inst = |id: &str| Installed {
            adopted: true,
            id: id.into(),
            name: id.into(),
            version: "1.0.0".into(),
            enabled: true,
            dev: false,
            dev_folder: None,
        };
        let log = Log::default();
        let mut page = ModulesPage {
            installed: vec![inst("m.a"), inst("m.b")],
            facts: None,
            actions: Some(Box::new(log.clone())),
            safe_by_closes: false,
            restarts: Default::default(),
            done: Default::default(),
            ask: Default::default(),
            told: Default::default(),
            grant: None,
            perm: None,
            finder: None,
            fresh: None,
            discover: None,
        };
        let ids =
            |p: &mut ModulesPage| -> Vec<String> { p.rows().0.into_iter().map(|r| r.id).collect() };
        // Not asked: ignored.
        page.on_click(Click::Remove("m.a".into()));
        assert!(log.0.borrow().is_empty());
        // Asking removes nothing.
        page.on_click(Click::AskRemove("m.a".into()));
        assert!(log.0.borrow().is_empty());
        assert_eq!(page.ask.pending(), Some("m.a"));
        // The confirm for another module is not this one's.
        page.on_click(Click::Remove("m.b".into()));
        assert!(log.0.borrow().is_empty());
        // Cancel: nothing, and a later Remove is unasked again.
        page.on_click(Click::CancelRemove);
        page.on_click(Click::Remove("m.a".into()));
        assert!(log.0.borrow().is_empty());
        assert_eq!(ids(&mut page), ["m.a", "m.b"]);
        // Ask, confirm: once.
        page.on_click(Click::AskRemove("m.a".into()));
        page.on_click(Click::Remove("m.a".into()));
        page.on_click(Click::Remove("m.a".into()));
        assert_eq!(*log.0.borrow(), ["remove m.a"]);
        assert_eq!(page.ask.pending(), None);
        // The row does not vanish on Ok: it reads Removing until the inventory's list drops it.
        use hideout_shell::modules_list::Removal;
        assert_eq!(ids(&mut page), ["m.a", "m.b"]);
        let removal = |p: &mut ModulesPage, id: &str| {
            p.rows().0.into_iter().find(|r| r.id == id).unwrap().removal
        };
        assert_eq!(removal(&mut page, "m.a"), Some(Removal::Removing));
        assert_eq!(removal(&mut page, "m.b"), None);
        // The list without it: the row and its state go.
        page.installed.retain(|m| m.id != "m.a");
        page.ask
            .retain_listed(|id| page.installed.iter().any(|m| m.id == id));
        assert_eq!(ids(&mut page), ["m.b"]);
        assert_eq!(page.ask.state("m.a"), None);
    }

    /// A Remove that fails keeps the row with an error (Try again asks again, Close puts it
    /// away); one that fails after the module stopped (the supervisor says so) shows the same.
    #[test]
    fn a_failed_remove_keeps_the_row_with_an_error() {
        use hideout_shell::modules_list::{Click, Removal};
        let inst = |id: &str| Installed {
            adopted: false,
            id: id.into(),
            name: id.into(),
            version: "1.0.0".into(),
            enabled: true,
            dev: false,
            dev_folder: None,
        };
        let fresh = Fresh::default();
        let mut page = ModulesPage {
            installed: vec![inst("m.bad"), inst("m.late")],
            facts: None,
            actions: Some(Box::new(Log::default())),
            safe_by_closes: false,
            restarts: Default::default(),
            done: Default::default(),
            ask: Default::default(),
            told: Default::default(),
            grant: None,
            perm: None,
            finder: None,
            fresh: Some(fresh.clone()),
            discover: None,
        };
        let state = |p: &mut ModulesPage, id: &str| {
            p.rows()
                .0
                .into_iter()
                .find(|r| r.id == id)
                .map(|r| r.removal)
        };
        page.on_click(Click::AskRemove("m.bad".into()));
        page.on_click(Click::Remove("m.bad".into()));
        assert_eq!(state(&mut page, "m.bad"), Some(Some(Removal::Failed)));
        // Try again asks (the error goes, the confirm shows); Close puts the error away.
        page.on_click(Click::AskRemove("m.bad".into()));
        assert_eq!(state(&mut page, "m.bad"), Some(None));
        assert_eq!(page.ask.pending(), Some("m.bad"));
        page.on_click(Click::CancelRemove);
        page.on_click(Click::AskRemove("m.bad".into()));
        page.on_click(Click::Remove("m.bad".into()));
        page.on_click(Click::DismissRemove("m.bad".into()));
        assert_eq!(state(&mut page, "m.bad"), Some(None));
        // Failing after it stopped: the supervisor's word reaches the row.
        assert!(fresh.put_failed("m.late"));
        assert_eq!(state(&mut page, "m.late"), Some(Some(Removal::Failed)));
    }

    /// Without a supervisor (a second hub) the confirm removes nothing and the row stays.
    #[test]
    fn without_a_supervisor_remove_changes_nothing() {
        use hideout_shell::modules_list::Click;
        let mut page = ModulesPage {
            installed: vec![Installed {
                adopted: false,
                id: "m.a".into(),
                name: "Art".into(),
                version: "1.0.0".into(),
                enabled: true,
                dev: false,
                dev_folder: None,
            }],
            facts: None,
            actions: None,
            safe_by_closes: false,
            restarts: Default::default(),
            done: Default::default(),
            ask: Default::default(),
            told: Default::default(),
            grant: None,
            perm: None,
            finder: None,
            fresh: None,
            discover: None,
        };
        page.on_click(Click::AskRemove("m.a".into()));
        page.on_click(Click::Remove("m.a".into()));
        assert_eq!(page.installed.len(), 1);
    }

    /// A rebuild restart is handed to the shell once: not on the first frame (nothing new),
    /// once when the fact appears, not again on a redraw, and again after the app left Running.
    #[test]
    fn a_rebuild_restart_is_told_once_per_appearance() {
        use hideout_shell::Page;
        let facts: std::sync::Arc<
            std::sync::Mutex<std::collections::HashMap<String, serde_json::Value>>,
        > = Default::default();
        let f = facts.clone();
        let mut page = ModulesPage {
            installed: vec![hideout_shell::modules_list::Installed {
                id: "m.a".into(),
                name: "Art".into(),
                version: "1".into(),
                enabled: true,
                adopted: false,
                dev: true,
                dev_folder: None,
            }],
            facts: Some(Box::new(move |k: &str| f.lock().unwrap().get(k).cloned())),
            actions: None,
            safe_by_closes: false,
            restarts: Default::default(),
            done: Default::default(),
            ask: Default::default(),
            told: Default::default(),
            grant: None,
            perm: None,
            finder: None,
            fresh: None,
            discover: None,
        };
        let set = |state: &str, at: Option<&str>| {
            let mut m = facts.lock().unwrap();
            m.insert("module.m.a.state".into(), state.into());
            match at {
                Some(a) => m.insert("module.m.a.rebuilt".into(), a.into()),
                None => m.remove("module.m.a.rebuilt"),
            };
        };
        set("running", None);
        page.rows();
        assert!(page.take_said().is_empty());
        set("running", Some("20:41"));
        page.rows();
        assert_eq!(page.take_said(), ["Art rebuilt and restarted"]);
        page.rows();
        page.rows();
        assert!(page.take_said().is_empty(), "a redraw says nothing");
        set("restarting", None);
        page.rows();
        set("running", Some("20:41"));
        page.rows();
        assert_eq!(page.take_said(), ["Art rebuilt and restarted"]);
    }
}
