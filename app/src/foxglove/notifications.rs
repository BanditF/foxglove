//! Windows notifications on the keyboard (#507): the app's name on the left screen for a few
//! seconds, and an optional short light cue.
//!
//! - **Opt-in, private by default.** Off until the user turns it on. Only the app's name shows;
//!   an app's title and text are read and shown only for apps the user ticked, one by one.
//!   Nothing about a notification is kept: the names of apps seen this run stay in memory for
//!   the Screens page's list, and only the ticked names are saved (`[notifications]`).
//! - **Quiet.** Nothing shows while the PC is locked or the display is off (`away`), while
//!   Foxglove or the left screen is paused, or while Windows holds banners back: Do not disturb
//!   / Focus assist, a full-screen game, a presentation, quiet time (`notify_windows::quiet`,
//!   read when a notification arrives). What arrives meanwhile never shows later.
//! - **Light cue.** A Normal-priority lease ([`NOTIFY_CONN`]) for [`CUE`]: over a scene (which
//!   is handed back and drawn again after, #544), under a High lease such as mic muted.
//! - **Events.** Windows' arrival event with package identity, else a write in the
//!   notification database's folder (`notify_windows`). A wake is read once after [`SETTLE`]; the
//!   cue ends after [`CUE`]. Each wait starts on an event and ends on its deadline. If the folder
//!   keeps changing with nothing new ([`Core::read`] counts), the watch stops until the setting is
//!   turned on again.

use std::collections::{BTreeSet, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use foxglove_panel::render::Notice;
use serde::{Deserialize, Serialize};

/// The lighting-lease connection the cue holds the keyboard under (no plugin has it).
pub const NOTIFY_CONN: u64 = u64::MAX - 3;

const SECTION: &str = "notifications";

/// From a wake to the read: Windows writes a toast in a few steps.
pub const SETTLE: Duration = Duration::from_millis(250);

/// How long the light cue holds the keyboard.
pub const CUE: Duration = Duration::from_millis(1500);

/// Reads in a row that found nothing new, each soon after the last, before the folder watch is
/// taken as never settling and stopped.
const RESTLESS_READS: u32 = 20;
const RESTLESS_GAP: Duration = Duration::from_secs(2);

/// Apps listed on the Screens page (names only, this run).
const MAX_RECENT: usize = 12;

/// The store's `[notifications]`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct NotifySettings {
    /// Show notifications at all (off until the user turns it on).
    pub enabled: bool,
    /// A short light cue on the keyboard with each one.
    pub light_cue: bool,
    /// How long one shows on the left screen, 2-15 seconds.
    pub seconds: u8,
    /// Apps whose title and text may show, by the name Windows gives them.
    pub text_apps: BTreeSet<String>,
}

impl Default for NotifySettings {
    fn default() -> Self {
        NotifySettings {
            enabled: false,
            light_cue: false,
            seconds: 5,
            text_apps: BTreeSet::new(),
        }
    }
}

impl NotifySettings {
    pub fn load() -> Self {
        Self::try_load().unwrap_or_default()
    }

    /// The settings as saved now, or `None` while the store can't be read.
    pub fn try_load() -> Option<Self> {
        super::store::try_load::<Self>(SECTION)
    }

    pub fn save(&self) -> std::io::Result<()> {
        super::store::save(SECTION, self)
    }

    pub fn shown_for(&self) -> Duration {
        Duration::from_secs(u64::from(self.seconds.clamp(2, 15)))
    }
}

/// Whether Windows lets Foxglove read notifications.
#[derive(Clone, Debug, PartialEq)]
pub enum Access {
    Allowed,
    Denied,
    /// The user hasn't been asked.
    Unasked,
    Unavailable(String),
}

/// Why Windows is holding banners back.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Quiet {
    DoNotDisturb,
    FullScreen,
    Presentation,
    /// Windows' quiet time: the first hours after a new user signs in.
    NewUser,
    NotPresent,
}

/// A notification as read; `text` is empty unless its app's text was allowed.
#[derive(Clone, Debug, PartialEq)]
pub struct Toast {
    pub id: u32,
    pub app: String,
    pub text: Vec<String>,
}

/// What the service is doing, for the Screens page.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum State {
    #[default]
    Off,
    /// Windows doesn't let Foxglove read notifications.
    NoAccess,
    /// Listening; `by_event` with Windows' own event (package identity).
    Listening { by_event: bool },
    /// Something else is in the way.
    Unavailable(String),
}

static STATE: Mutex<State> = Mutex::new(State::Off);
static RECENT: Mutex<Vec<String>> = Mutex::new(Vec::new());

pub fn state() -> State {
    STATE.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

fn set_state(s: State) {
    let mut cur = STATE.lock().unwrap_or_else(|e| e.into_inner());
    if *cur != s {
        eprintln!("Foxglove notifications: {s:?}");
        *cur = s;
    }
}

/// The apps that sent notifications this run, newest first (names only).
pub fn recent_apps() -> Vec<String> {
    RECENT.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

fn note_recent(recent: &mut Vec<String>, apps: impl IntoIterator<Item = String>) {
    for app in apps {
        recent.retain(|a| *a != app);
        recent.insert(0, app);
    }
    recent.truncate(MAX_RECENT);
}

/// Windows' side, faked in tests.
pub trait Source {
    /// The notifications in the notification centre; an app's text only when `text_for` says.
    fn toasts(&mut self, text_for: &dyn Fn(&str) -> bool) -> Result<Vec<Toast>, String>;
    fn quiet(&mut self) -> Option<Quiet>;
}

struct Windows;

impl Source for Windows {
    fn toasts(&mut self, text_for: &dyn Fn(&str) -> bool) -> Result<Vec<Toast>, String> {
        super::notify_windows::toasts(text_for)
    }
    fn quiet(&mut self) -> Option<Quiet> {
        super::notify_windows::quiet()
    }
}

/// What a read shows.
#[derive(Debug, PartialEq)]
pub struct Shown {
    pub notice: Notice,
    pub cue: bool,
    /// The apps that sent the new notifications (shown or not), for the Screens page.
    pub apps: Vec<String>,
}

/// The notice for new notifications: the newest one's app, with its title and text only when
/// that app is ticked.
pub fn notice_for(new: &[Toast], s: &NotifySettings) -> Option<Notice> {
    let newest = new.iter().max_by_key(|t| t.id)?;
    let clean = |t: &str| t.split_whitespace().collect::<Vec<_>>().join(" ");
    let allowed = s.text_apps.contains(&newest.app);
    let mut text = newest
        .text
        .iter()
        .map(|t| clean(t))
        .filter(|t| allowed && !t.is_empty());
    let title = text.next();
    let body = text.collect::<Vec<_>>().join(" ");
    Some(Notice {
        app: clean(&newest.app),
        title,
        body: (!body.is_empty()).then_some(body),
        more: new.len() - 1,
    })
}

/// What happened on a read.
#[derive(Debug, PartialEq)]
pub enum Read {
    /// Nothing to show (primed, nothing new, quiet, off or away).
    Nothing,
    Show(Shown),
    /// The wake keeps coming with nothing new: stop watching.
    Restless,
}

/// The service's core: reads after a wake, and what they show.
#[derive(Default)]
pub struct Core {
    /// The ids at the last read; `None` until a read primes it (what was there doesn't show).
    seen: Option<HashSet<u32>>,
    last_read: Option<Instant>,
    idle_reads: u32,
}

impl Core {
    /// Forget what was seen: the next read only primes. Away or off: what arrives meanwhile
    /// never shows later.
    pub fn forget(&mut self) {
        self.seen = None;
        self.idle_reads = 0;
    }

    /// Reads after a wake. `paused`: what's new is taken as seen and nothing shows, so it
    /// doesn't show after the resume either, and the first arrival after it does.
    pub fn read(
        &mut self,
        src: &mut dyn Source,
        s: &NotifySettings,
        paused: bool,
        now: Instant,
    ) -> Read {
        if !s.enabled {
            self.forget();
            return Read::Nothing;
        }
        let text_for = |app: &str| !paused && s.text_apps.contains(app);
        let toasts = match src.toasts(&text_for) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("Foxglove notifications: reading them: {e}");
                return Read::Nothing;
            }
        };
        let ids: HashSet<u32> = toasts.iter().map(|t| t.id).collect();
        let new: Vec<Toast> = match &self.seen {
            Some(seen) => toasts
                .into_iter()
                .filter(|t| !seen.contains(&t.id))
                .collect(),
            None => Vec::new(),
        };
        self.seen = Some(ids);
        let soon = self
            .last_read
            .is_some_and(|t| now.duration_since(t) < RESTLESS_GAP);
        self.last_read = Some(now);
        if new.is_empty() {
            self.idle_reads = if soon { self.idle_reads + 1 } else { 0 };
            return if self.idle_reads >= RESTLESS_READS {
                Read::Restless
            } else {
                Read::Nothing
            };
        }
        self.idle_reads = 0;
        let apps: Vec<String> = new.iter().map(|t| t.app.clone()).collect();
        if paused || src.quiet().is_some() {
            note_recent(&mut RECENT.lock().unwrap_or_else(|e| e.into_inner()), apps);
            return Read::Nothing;
        }
        match notice_for(&new, s) {
            Some(notice) => Read::Show(Shown {
                notice,
                cue: s.light_cue,
                apps,
            }),
            None => Read::Nothing,
        }
    }
}

/// The light cue's clock: a shown notice starts it (or extends one showing); it ends when its
/// time is up or when it may not show any more (off, away, paused).
#[derive(Debug, Default)]
pub struct Cue {
    until: Option<Instant>,
}

impl Cue {
    /// A notice with a cue at `now`: a showing cue runs on, else `take` takes the lights.
    pub fn shown(&mut self, now: Instant, take: impl FnOnce() -> bool) {
        if self.until.is_some() || take() {
            self.until = Some(now + CUE);
        }
    }

    /// Whether the cue ends now; the caller then gives the lights back.
    pub fn ends(&mut self, now: Instant, may_show: bool) -> bool {
        match self.until {
            Some(t) if t <= now || !may_show => {
                self.until = None;
                true
            }
            _ => false,
        }
    }

    pub fn deadline(&self) -> Option<Instant> {
        self.until
    }
}

/// Foxglove or the left screen paused: nothing shows, and no cue.
fn paused() -> bool {
    super::pause::paused() || super::screens::paused()
}

enum Event {
    Wake,
    Settings,
    Away,
    /// Paused or resumed.
    Hold,
    /// Check access again (the Screens page's Try again).
    Retry,
}

static EVENTS: Mutex<Option<mpsc::Sender<Event>>> = Mutex::new(None);
static QUITTING: AtomicBool = AtomicBool::new(false);

fn send(e: Event) {
    if let Some(tx) = EVENTS.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        let _ = tx.send(e);
    }
}

/// Foxglove or the left screen paused or resumed.
pub fn pause_changed() {
    send(Event::Hold);
}

/// Checks Windows' access again, after the user changed it.
pub fn retry() {
    send(Event::Retry);
}

pub fn start() {
    if super::repaint_trace::disabled("notifications") {
        return;
    }
    let _ = super::services::spawn_supervised("notifications", run);
}

/// Takes the keyboard's lights for the cue; false when there's no keyboard, it's paused or
/// quitting, or the keyboard refused (whatever was taken is given back).
fn cue_start() -> bool {
    use foxglove_devices::Device as _;
    if super::pause::paused() || QUITTING.load(Ordering::SeqCst) {
        return false;
    }
    let Some(kb) = super::devices::device() else {
        return false;
    };
    let Some(light) = kb.lighting() else {
        return false;
    };
    let lease = super::lighting_leases::Lease {
        id: 0,
        conn: NOTIFY_CONN,
        device: kb.info().id.to_string(),
        priority: super::lighting_leases::Priority::Normal,
        // A soft warm white, solid.
        hue: 35,
        sat: 20,
        brightness: 70,
        effect: "0".into(),
    };
    match super::lighting_leases::with(|a| a.add(lease, light)) {
        Ok(_) => true,
        Err(e) => {
            eprintln!("Foxglove notifications: the light cue: {e}");
            // The arbiter keeps a lease whose first show failed: end it now.
            cue_end();
            false
        }
    }
}

/// Ends the cue: what was under it shows again.
pub fn cue_end() {
    super::api::end_leases(|l| l.conn == NOTIFY_CONN);
}

/// Quitting: no new cue starts, and one that's showing gives the lights back.
pub fn quit() {
    QUITTING.store(true, Ordering::SeqCst);
    if super::lighting_leases::holders()
        .iter()
        .any(|(_, conn, _)| *conn == NOTIFY_CONN)
    {
        cue_end();
    }
}

/// The service's state between events.
struct Service {
    settings: NotifySettings,
    core: Core,
    arrivals: Option<super::notify_windows::Arrivals>,
    /// Access and the watch are tried once, then again after a change to these settings or
    /// the page's Try again.
    tried: bool,
    read_at: Option<Instant>,
    cue: Cue,
    wake: Arc<dyn Fn() + Send + Sync>,
}

impl Service {
    /// Off: drops the watch and what was seen. On and not yet tried: checks access and watches.
    fn settle(&mut self) {
        if !self.settings.enabled {
            if self.arrivals.take().is_some() || self.read_at.is_some() {
                super::screens::clear_notice();
            }
            self.read_at = None;
            self.core.forget();
            set_state(State::Off);
        } else if self.arrivals.is_none() && !self.tried {
            self.tried = true;
            // Never asked from here: Windows' own page decides (the card links to it).
            match super::notify_windows::access() {
                Access::Allowed => match super::notify_windows::watch(self.wake.clone()) {
                    Ok(a) => {
                        set_state(State::Listening {
                            by_event: a.by_event,
                        });
                        self.arrivals = Some(a);
                        self.core.forget();
                        // The first read primes: what's there already doesn't show.
                        self.read_at = Some(Instant::now());
                    }
                    Err(e) => set_state(State::Unavailable(e)),
                },
                Access::Denied | Access::Unasked => set_state(State::NoAccess),
                Access::Unavailable(e) => set_state(State::Unavailable(e)),
            }
        }
        let may_cue =
            self.settings.enabled && self.settings.light_cue && !super::away::away() && !paused();
        if self.cue.ends(Instant::now(), may_cue) {
            cue_end();
        }
    }

    /// The next deadline: a due read or the cue's end.
    fn next(&self) -> Option<Instant> {
        [self.read_at, self.cue.deadline()]
            .into_iter()
            .flatten()
            .min()
    }

    fn on_event(&mut self, event: Event) {
        match event {
            Event::Wake if self.arrivals.is_some() && !super::away::away() => {
                self.read_at.get_or_insert_with(|| Instant::now() + SETTLE);
            }
            Event::Wake => {}
            Event::Settings => {
                if let Some(s) = NotifySettings::try_load() {
                    if s != self.settings {
                        self.tried = false;
                    }
                    self.settings = s;
                }
            }
            Event::Retry => self.tried = false,
            Event::Hold => {
                if paused() {
                    super::screens::clear_notice();
                }
            }
            Event::Away => {
                if super::away::away() {
                    super::screens::clear_notice();
                    self.core.forget();
                    self.read_at = None;
                } else if self.arrivals.is_some() {
                    // Back: prime again, so what arrived meanwhile never shows.
                    self.read_at = Some(Instant::now());
                }
            }
        }
    }

    /// Reads the notifications when the read is due.
    fn read_if_due(&mut self) {
        let now = Instant::now();
        if !self.read_at.is_some_and(|t| t <= now) {
            return;
        }
        self.read_at = None;
        if super::away::away() {
            self.core.forget();
            return;
        }
        match self.core.read(&mut Windows, &self.settings, paused(), now) {
            Read::Nothing => {}
            Read::Restless => {
                eprintln!("Foxglove notifications: the folder keeps changing; stopped");
                self.arrivals = None;
                set_state(State::Unavailable(
                    "Windows' notification folder keeps changing; turn this off and on to try again".into(),
                ));
            }
            Read::Show(shown) => {
                note_recent(
                    &mut RECENT.lock().unwrap_or_else(|e| e.into_inner()),
                    shown.apps,
                );
                super::screens::show_notice(shown.notice, now + self.settings.shown_for());
                if shown.cue {
                    self.cue.shown(Instant::now(), cue_start);
                }
            }
        }
    }
}

fn run() {
    let (tx, rx) = mpsc::channel();
    *EVENTS.lock().unwrap_or_else(|e| e.into_inner()) = Some(tx.clone());
    let settings_tx = tx.clone();
    let _settings_watch = super::store::path().and_then(|p| {
        super::screens_watch::settings(
            &p,
            Arc::new(move || {
                let _ = settings_tx.send(Event::Settings);
            }),
        )
    });
    let away_tx = tx.clone();
    let _away = super::away::subscribe(move |_| {
        let _ = away_tx.send(Event::Away);
    });
    let wake: Arc<dyn Fn() + Send + Sync> = {
        let tx = Mutex::new(tx.clone());
        Arc::new(move || {
            let _ = tx
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .send(Event::Wake);
        })
    };
    drop(tx);
    let mut service = Service {
        settings: NotifySettings::load(),
        core: Core::default(),
        arrivals: None,
        tried: false,
        read_at: None,
        cue: Cue::default(),
        wake,
    };

    loop {
        service.settle();
        let event = match service.next() {
            Some(at) => match rx.recv_timeout(at.saturating_duration_since(Instant::now())) {
                Ok(e) => Some(e),
                Err(mpsc::RecvTimeoutError::Timeout) => None,
                Err(mpsc::RecvTimeoutError::Disconnected) => return,
            },
            None => match rx.recv() {
                Ok(e) => Some(e),
                Err(_) => return,
            },
        };
        if let Some(event) = event {
            service.on_event(event);
        }
        service.read_if_due();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fake notification centre: what it holds, whether it's quiet, and which apps' text was
    /// asked for.
    #[derive(Default)]
    struct Fake {
        toasts: Vec<Toast>,
        quiet: Option<Quiet>,
        reads: u32,
        text_asked: Vec<String>,
    }

    impl Fake {
        fn add(&mut self, id: u32, app: &str, text: &[&str]) {
            self.toasts.push(Toast {
                id,
                app: app.into(),
                text: text.iter().map(|t| t.to_string()).collect(),
            });
        }
    }

    impl Source for Fake {
        fn toasts(&mut self, text_for: &dyn Fn(&str) -> bool) -> Result<Vec<Toast>, String> {
            self.reads += 1;
            Ok(self
                .toasts
                .iter()
                .map(|t| {
                    let allowed = text_for(&t.app);
                    if allowed {
                        self.text_asked.push(t.app.clone());
                    }
                    Toast {
                        text: if allowed { t.text.clone() } else { Vec::new() },
                        ..t.clone()
                    }
                })
                .collect())
        }
        fn quiet(&mut self) -> Option<Quiet> {
            self.quiet
        }
    }

    fn on() -> NotifySettings {
        NotifySettings {
            enabled: true,
            ..NotifySettings::default()
        }
    }

    fn shown(r: Read) -> Shown {
        match r {
            Read::Show(s) => s,
            other => panic!("expected a notice, got {other:?}"),
        }
    }

    /// Off by default, and the first read primes: what was in the notification centre before
    /// never shows; a new one shows by its app's name only.
    #[test]
    fn opt_in_primes_then_shows_the_app_name_only() {
        assert!(!NotifySettings::default().enabled);
        assert!(!NotifySettings::default().light_cue);
        let t = Instant::now();
        let mut fake = Fake::default();
        fake.add(1, "Outlook", &["Old mail", "from before"]);
        let mut core = Core::default();
        assert_eq!(
            core.read(&mut fake, &NotifySettings::default(), false, t),
            Read::Nothing
        );
        assert_eq!(fake.reads, 0, "off: nothing is read");
        assert_eq!(core.read(&mut fake, &on(), false, t), Read::Nothing);
        fake.add(2, "Discord", &["Raid tonight", "Bring snacks"]);
        let s = shown(core.read(&mut fake, &on(), false, t + Duration::from_secs(5)));
        assert_eq!(
            s.notice,
            Notice {
                app: "Discord".into(),
                ..Notice::default()
            }
        );
        assert!(!s.cue);
        assert!(fake.text_asked.is_empty(), "no app's text was read");
        // The same notification again: nothing new.
        assert_eq!(
            core.read(&mut fake, &on(), false, t + Duration::from_secs(10)),
            Read::Nothing
        );
    }

    /// Text only for a ticked app, and only that app's text is ever read.
    #[test]
    fn text_shows_only_for_apps_the_user_ticked() {
        let t = Instant::now();
        let mut s = on();
        s.text_apps.insert("Discord".into());
        s.light_cue = true;
        let mut fake = Fake::default();
        let mut core = Core::default();
        core.read(&mut fake, &s, false, t);
        fake.add(7, "Outlook", &["Secret", "Do not show"]);
        fake.add(
            8,
            "Discord",
            &["Raid tonight", "Bring", "snacks\nand   water"],
        );
        let got = shown(core.read(&mut fake, &s, false, t + Duration::from_secs(3)));
        assert_eq!(
            got.notice,
            Notice {
                app: "Discord".into(),
                title: Some("Raid tonight".into()),
                body: Some("Bring snacks and water".into()),
                more: 1,
            }
        );
        assert!(got.cue);
        assert_eq!(got.apps, ["Outlook", "Discord"]);
        assert_eq!(fake.text_asked, ["Discord"]);
        // Newest is Outlook now: name only, though Discord's text was allowed.
        fake.add(9, "Outlook", &["Secret 2"]);
        let got = shown(core.read(&mut fake, &s, false, t + Duration::from_secs(9)));
        assert_eq!(got.notice.title, None);
        assert_eq!(got.notice.app, "Outlook");
    }

    /// Quiet (Do not disturb, full screen…), away or paused: nothing shows, and what arrived
    /// meanwhile doesn't show after.
    #[test]
    fn quiet_away_and_paused_show_nothing_now_or_later() {
        let t = Instant::now();
        let mut fake = Fake::default();
        let mut core = Core::default();
        core.read(&mut fake, &on(), false, t);
        fake.quiet = Some(Quiet::DoNotDisturb);
        fake.add(1, "Teams", &[]);
        assert_eq!(
            core.read(&mut fake, &on(), false, t + Duration::from_secs(3)),
            Read::Nothing
        );
        fake.quiet = None;
        assert_eq!(
            core.read(&mut fake, &on(), false, t + Duration::from_secs(6)),
            Read::Nothing,
            "a notification held back by Do not disturb doesn't show after"
        );
        // Away: the service doesn't read and forgets; back, the next read primes.
        fake.add(2, "Teams", &[]);
        core.forget();
        assert_eq!(
            core.read(&mut fake, &on(), false, t + Duration::from_secs(12)),
            Read::Nothing,
            "back: primes"
        );
        fake.add(3, "Slack", &[]);
        assert_eq!(
            shown(core.read(&mut fake, &on(), false, t + Duration::from_secs(15)))
                .notice
                .app,
            "Slack"
        );
        // Paused: read and taken as seen, no text asked, nothing shown; resumed, the next
        // arrival shows at once.
        let mut s = on();
        s.text_apps.insert("Teams".into());
        fake.add(4, "Teams", &["Hidden"]);
        assert_eq!(
            core.read(&mut fake, &s, true, t + Duration::from_secs(18)),
            Read::Nothing
        );
        assert!(fake.text_asked.is_empty(), "no text read while paused");
        assert_eq!(
            core.read(&mut fake, &s, false, t + Duration::from_secs(21)),
            Read::Nothing,
            "the paused one doesn't show after"
        );
        fake.add(5, "Slack", &[]);
        assert_eq!(
            shown(core.read(&mut fake, &s, false, t + Duration::from_secs(24)))
                .notice
                .app,
            "Slack"
        );
    }

    /// The light cue: one lease per run of notices, ended by its time, and at once when it may
    /// not show (turned off, away, paused); a refused take leaves nothing running.
    #[test]
    fn the_cue_starts_once_extends_and_always_ends() {
        let t = Instant::now();
        let takes = std::cell::Cell::new(0);
        let take = || {
            takes.set(takes.get() + 1);
            true
        };
        let mut cue = Cue::default();
        assert!(!cue.ends(t, true), "nothing showing");
        cue.shown(t, take);
        assert_eq!(cue.deadline(), Some(t + CUE));
        // Another notice mid-cue: runs on, no second lease.
        let mid = t + CUE / 2;
        cue.shown(mid, take);
        assert_eq!((takes.get(), cue.deadline()), (1, Some(mid + CUE)));
        assert!(!cue.ends(mid + CUE / 2, true));
        assert!(cue.ends(mid + CUE, true), "its time is up");
        assert!(!cue.ends(mid + CUE, true), "ended once");
        // Turned off, away or paused mid-cue: ends at once.
        cue.shown(t, take);
        assert!(cue.ends(t, false));
        assert_eq!(cue.deadline(), None);
        // The keyboard refused: nothing to end.
        let mut refused = Cue::default();
        refused.shown(t, || false);
        assert_eq!(refused.deadline(), None);
        assert!(!refused.ends(t + CUE, false));
    }

    /// A folder that keeps changing with nothing new stops the watch; reads far apart don't.
    #[test]
    fn a_restless_wake_stops_and_slow_ones_dont() {
        let t = Instant::now();
        let mut fake = Fake::default();
        let mut core = Core::default();
        // Far apart: never restless.
        for n in 0..50u64 {
            let r = core.read(&mut fake, &on(), false, t + Duration::from_secs(n * 10));
            assert_eq!(r, Read::Nothing);
        }
        // A read every 300 ms with nothing new.
        let start = t + Duration::from_secs(1000);
        let mut stopped = None;
        for n in 0..40u64 {
            let r = core.read(
                &mut fake,
                &on(),
                false,
                start + Duration::from_millis(n * 300),
            );
            if r == Read::Restless {
                stopped = Some(n);
                break;
            }
        }
        assert_eq!(stopped, Some(u64::from(RESTLESS_READS)));
    }

    #[test]
    fn recent_apps_are_names_newest_first_and_bounded() {
        let mut recent = Vec::new();
        note_recent(&mut recent, ["A".to_string(), "B".to_string()]);
        note_recent(&mut recent, ["A".to_string()]);
        assert_eq!(recent, ["A", "B"]);
        note_recent(&mut recent, (0..20).map(|n| n.to_string()));
        assert_eq!(recent.len(), MAX_RECENT);
        assert_eq!(recent[0], "19");
    }

    /// One test for the `[notifications]` section (the store's tests share one file).
    #[test]
    fn settings_round_trip_and_clamp() {
        super::super::store::test_config_dir();
        assert_eq!(NotifySettings::load(), NotifySettings::default());
        let mut s = on();
        s.text_apps.insert("Discord".into());
        s.seconds = 99;
        s.save().unwrap();
        let back = NotifySettings::load();
        assert_eq!(back, s);
        assert_eq!(back.shown_for(), Duration::from_secs(15));
    }
}
