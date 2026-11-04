//! Who drives the keyboard's lights now (#491): a Foxglove scene, Windows Dynamic Lighting,
//! another app, or the keyboard itself. The Lights and Scenes pages show it in one line, and
//! the Keyboard health view has a "Lights owner" row (`foxglove-lab health` prints it).
//!
//! - The owner comes from the keyboard's owner byte in every D7 8C (SET_FRAME) and D7 8F
//!   (SET_EFFECT) report, replies and the keyboard's own events alike
//!   (`foxglove_link::lights_owner`). One watch on the keyboard's link reads them as they
//!   arrive ([`attach`]); nothing asks the keyboard and nothing polls. A keyboard connect
//!   forgets the owner until the keyboard next says.
//! - Which Foxglove part it is (a scene, a plugin's or tool's lease, the Lights page) comes from
//!   the lighting leases (#250, the Plugins tab's holder view #376).
//! - The yield rule, as Windows Settings > Dynamic Lighting orders its background controllers:
//!   while the owner byte says someone else has the lights (1 or 2, a LampArray: Windows
//!   Dynamic Lighting or an app it lets in), a running scene sends the keyboard nothing
//!   ([`State::yield_to`]). The event that frees them (any report saying 0 or 3) asks the scene
//!   for its frame or effect again. Firmware before LampArray (#115) only says 0 or 3, so it
//!   never yields.
//! - The pages repaint on the owner's change ([`set_wake`]), not on a timer.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard, Weak};

use foxglove_link::lights_owner::{Owner, Tracker};

// The shared row type of the Keyboard health view (#705).
pub use super::health::{HealthRow, Level};

/// The subsystem of the health row.
pub const SUBSYSTEM: &str = "lights.owner";

/// The owner as the keyboard last said, and what the scene did about it.
#[derive(Default)]
pub struct State {
    tracker: Tracker,
    /// Times a scene stepped aside: once per stretch of someone else holding the lights.
    yields: u32,
    /// A scene is stepping aside now.
    yielding: bool,
    /// Owner changes seen since Foxglove started.
    changes: u32,
    /// The "send again" flags of scenes stepping aside, set when the lights are free.
    waiting: Vec<Weak<AtomicBool>>,
    /// Repaints the pages that show the line.
    wake: Option<crate::ui_wake::UiWake>,
}

/// What [`State`] knows, as plain data (health rows, tests).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Now {
    pub owner: Option<Owner>,
    pub yields: u32,
    pub yielding: bool,
    pub changes: u32,
}

impl State {
    pub fn owner(&self) -> Option<Owner> {
        self.tracker.owner()
    }

    /// Someone other than Foxglove and the keyboard has the lights.
    pub fn busy(&self) -> bool {
        self.owner().is_some_and(Owner::elsewhere)
    }

    pub fn now(&self) -> Now {
        Now {
            owner: self.owner(),
            yields: self.yields,
            yielding: self.yielding,
            changes: self.changes,
        }
    }

    /// A report from the keyboard; true when the owner changed.
    pub fn observe(&mut self, r: &[u8]) -> bool {
        if !self.tracker.observe(r) {
            return false;
        }
        self.changes = self.changes.saturating_add(1);
        self.changed();
        true
    }

    /// A keyboard connected: nothing known until it next says. A scene stepping aside sends
    /// again, and the reply says who has the lights.
    pub fn connected(&mut self) {
        let had = self.owner().is_some();
        self.tracker.forget();
        if had || self.yielding {
            self.changed();
        }
    }

    fn changed(&mut self) {
        if !self.busy() {
            self.yielding = false;
            for flag in self.waiting.drain(..).filter_map(|w| w.upgrade()) {
                flag.store(true, Ordering::Relaxed);
            }
        }
        if let Some(w) = &self.wake {
            w.request_repaint();
        }
    }

    /// Before a scene's send to the keyboard: true when it steps aside (someone else has the
    /// lights). `again` is set when they are free, so the scene sends its frame or effect again.
    pub fn yield_to(&mut self, again: &Arc<AtomicBool>) -> bool {
        if !self.busy() {
            return false;
        }
        if !std::mem::replace(&mut self.yielding, true) {
            self.yields = self.yields.saturating_add(1);
            // The Scenes page's status says "Paused for ..." from now on.
            if let Some(w) = &self.wake {
                w.request_repaint();
            }
        }
        self.waiting.retain(|w| w.strong_count() > 0);
        if !self
            .waiting
            .iter()
            .any(|w| w.as_ptr() == Arc::as_ptr(again))
        {
            self.waiting.push(Arc::downgrade(again));
        }
        true
    }

    pub fn set_wake(&mut self, wake: crate::ui_wake::UiWake) {
        self.wake = Some(wake);
    }
}

static STATE: LazyLock<Arc<Mutex<State>>> = LazyLock::new(Arc::default);

/// The app's owner state (a running scene's writer holds it).
pub fn shared() -> Arc<Mutex<State>> {
    STATE.clone()
}

pub fn lock(s: &Mutex<State>) -> MutexGuard<'_, State> {
    s.lock().unwrap_or_else(|e| e.into_inner())
}

/// The link whose reports feed [`STATE`], and the watch.
#[allow(clippy::type_complexity)]
static TAP: Mutex<
    Option<(
        Arc<foxglove_link::link::Link>,
        foxglove_link::link::EventSubscription,
    )>,
> = Mutex::new(None);

/// Watches this keyboard's link for its owner byte, once per link (again for another one).
pub fn attach(kb: &foxglove_link::devices::FoxgloveLinkDevice) {
    let mut tap = TAP.lock().unwrap_or_else(|e| e.into_inner());
    if tap.as_ref().is_some_and(|(l, _)| Arc::ptr_eq(l, kb.link())) {
        return;
    }
    let state = shared();
    let watch = kb.link().subscribe_reports(move |_, r| {
        let mut s = lock(&state);
        let before = s.owner();
        if s.observe(r) {
            eprintln!(
                "Foxglove: lights owner: {} (was {})",
                byte_words(s.owner()),
                byte_words(before)
            );
        }
    });
    *tap = Some((kb.link().clone(), watch));
}

/// A device connected (the device bus's Connected, `device` its id): when it is the keyboard,
/// forget the owner and watch its link. Off the UI thread (it may look for the keyboard).
pub fn device_connected(device: &str) {
    use foxglove_devices::Device as _;
    let Some(kb) = super::devices::device() else {
        return;
    };
    if kb.info().id.to_string() != device {
        return;
    }
    lock(&STATE).connected();
    attach(&kb);
}

/// Repaints the page showing the line when the owner changes.
#[cfg_attr(test, allow(dead_code))]
pub fn set_wake(ctx: &egui::Context) {
    let mut s = lock(&STATE);
    if s.wake.is_none() {
        s.set_wake(crate::ui_wake::UiWake::from_ctx(ctx));
    }
}

fn byte_words(o: Option<Owner>) -> String {
    match o {
        None => "not said".into(),
        Some(o) => format!("{o:?} ({})", o.byte()),
    }
}

/// Who drives the lights, in the pages' terms.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Who {
    /// A Foxglove scene, by name.
    Scene(String),
    /// Windows Dynamic Lighting (a LampArray owner).
    Windows,
    /// Another app: a plugin's or tool's lease, or an owner Foxglove doesn't know.
    OtherApp,
    /// Foxglove itself without a scene (the Lights page, Find my LEDs).
    Foxglove,
    /// The keyboard's own effect.
    Keyboard,
}

/// Who drives the lights from the owner byte, the running scene and the keyboard's winning
/// lease connection (`None` with no lease, or while Foxglove is paused).
pub fn who(owner: Option<Owner>, scene: Option<&str>, lease: Option<u64>) -> Who {
    let scene_conn = super::scene_run::SCENE_CONN;
    match owner {
        Some(o) if o.lamp_array() => Who::Windows,
        Some(Owner::Other(_)) => Who::OtherApp,
        // Frames or an effect, or a lease's colour on the keyboard's own effect.
        _ => match (lease, scene) {
            (Some(c), Some(name)) if c == scene_conn => Who::Scene(name.to_string()),
            (Some(c), None) if c == scene_conn => Who::Foxglove,
            (Some(_), _) => Who::OtherApp,
            (None, _) if owner == Some(Owner::HostLink) => Who::Foxglove,
            (None, _) => Who::Keyboard,
        },
    }
}

impl Who {
    pub fn words(&self) -> String {
        match self {
            Who::Scene(name) => format!("the Foxglove scene \u{201c}{name}\u{201d}"),
            Who::Windows => "Windows Dynamic Lighting".into(),
            Who::OtherApp => "another app".into(),
            Who::Foxglove => "Foxglove".into(),
            Who::Keyboard => "the keyboard itself (its own effect)".into(),
        }
    }
}

/// The pages' owner line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    pub text: String,
    /// A scene is waiting for the lights.
    pub warn: bool,
}

pub const TIP: &str = "Who drives the keyboard's lights, as the keyboard says. Windows Dynamic \
                       Lighting comes first: a running scene stops sending while Windows has \
                       them, and comes back when Windows lets go.";

/// The line for `who`, with the running scene's name.
pub fn line(who: &Who, scene: Option<&str>) -> Line {
    let mut text = format!("Lights now: {}", who.words());
    let waits = matches!(who, Who::Windows) && scene.is_some();
    if let Some(name) = scene.filter(|_| waits) {
        text.push_str(&format!(
            ". The scene \u{201c}{name}\u{201d} steps aside and comes back when Windows lets go."
        ));
    }
    Line { text, warn: waits }
}

/// For whom a running scene steps aside now, in words: `None` while it doesn't.
pub fn yielded_words(now: &Now) -> Option<&'static str> {
    if !now.yielding {
        return None;
    }
    Some(match now.owner {
        Some(o) if o.lamp_array() => "Windows Dynamic Lighting",
        _ => "another app",
    })
}

/// [`yielded_words`] for the app's owner now (the Scenes page's status).
#[cfg_attr(test, allow(dead_code))]
pub fn yielded_to() -> Option<&'static str> {
    yielded_words(&lock(&STATE).now())
}

/// The keyboard's winning lease connection, `None` while paused or with none.
fn lease_now() -> Option<u64> {
    let held = super::lighting_leases::holding();
    if held.paused {
        return None;
    }
    let kb = super::devices::keyboard_id()?;
    held.devices
        .iter()
        .find(|d| d.device == kb)
        .and_then(|d| d.leases.first())
        .map(|(conn, _)| *conn)
}

/// Who drives the lights now, and the running scene's name.
fn who_now(owner: Option<Owner>) -> (Who, Option<String>) {
    let scene = super::scene_run::running().map(|(name, _)| name);
    (who(owner, scene.as_deref(), lease_now()), scene)
}

/// The line as it stands now (no blocking: the arbiter's published holders, the cached device).
#[cfg_attr(test, allow(dead_code))]
pub fn line_now() -> Line {
    let owner = lock(&STATE).owner();
    let (who, scene) = who_now(owner);
    line(&who, scene.as_deref())
}

#[cfg(test)]
thread_local! {
    /// The previews' line; without one, tests draw none (they never read the app's state).
    pub static PREVIEW: std::cell::RefCell<Option<Line>> = const { std::cell::RefCell::new(None) };
}

/// Draws the owner line (the Lights page's Keyboard card, the Scenes page) after `space` points;
/// true when it drew one.
pub fn show_line(ui: &mut egui::Ui, space: f32) -> bool {
    #[cfg(test)]
    let line = match PREVIEW.with_borrow(|l| l.clone()) {
        Some(l) => l,
        None => return false,
    };
    #[cfg(not(test))]
    let line = {
        set_wake(ui.ctx());
        line_now()
    };
    let p = super::theme::palette(ui);
    let color = if line.warn { p.warn } else { p.text };
    ui.add_space(space);
    ui.add(egui::Label::new(egui::RichText::new(line.text).color(color)).wrap())
        .on_hover_text(TIP);
    true
}

/// The health view's row now (registered with `health::register` as "lights_owner").
pub fn rows_now() -> Vec<HealthRow> {
    let now = lock(&STATE).now();
    let (who, _) = who_now(now.owner);
    rows(&now, &who)
}

/// The health view's row: who has the lights, and how often a scene stepped aside.
pub fn rows(now: &Now, who: &Who) -> Vec<HealthRow> {
    let (level, owner) = match now.owner {
        None => (
            Level::Ok,
            format!(
                "{}; the keyboard hasn't said yet (it does with Foxglove's next frame or effect)",
                who.words()
            ),
        ),
        Some(Owner::Other(b)) => (
            Level::Warn,
            format!("an owner Foxglove doesn't know (byte {b})"),
        ),
        Some(o) => (Level::Ok, format!("{} (owner {})", who.words(), o.byte())),
    };
    let yields = format!(
        "{} yield{} to Windows Dynamic Lighting{}",
        now.yields,
        if now.yields == 1 { "" } else { "s" },
        if now.yielding {
            " (the scene steps aside now)"
        } else {
            ""
        }
    );
    vec![HealthRow::new(
        SUBSYSTEM,
        "Lights owner",
        level,
        format!("{owner}; {yields}; {} owner changes", now.changes),
    )]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU32;

    /// A D7 8C report: a reply, or with `event` the keyboard's own.
    fn frame_report(owner: u8, event: bool) -> [u8; 32] {
        let mut r = [0u8; 32];
        r[..7].copy_from_slice(&[0xD7, 0x8C, 0, owner, 30, 1, u8::from(event)]);
        r
    }

    /// Busy (owner 1 or 2) stops the sends; the event that frees the lights asks for them again.
    #[test]
    fn busy_steps_aside_and_the_release_asks_again() {
        let mut s = State::default();
        let again = Arc::new(AtomicBool::new(false));
        assert!(!s.yield_to(&again), "nothing said yet: send");
        s.observe(&frame_report(3, false));
        assert!(!s.yield_to(&again), "Foxglove has them: send");

        for owner in [1, 2] {
            assert!(s.observe(&frame_report(owner, true)), "Windows takes them");
            assert!(s.busy());
            assert!(s.yield_to(&again), "owner {owner}: step aside");
            assert!(s.yield_to(&again), "still");
            assert!(!again.load(Ordering::Relaxed));
            assert!(s.now().yielding);
            // Free again: the keyboard's own effect (its event), or its reply saying so.
            assert!(s.observe(&frame_report(0, true)));
            assert!(again.swap(false, Ordering::Relaxed), "asked to send again");
            assert!(!s.yield_to(&again), "sends resume");
            assert!(!s.now().yielding);
        }
        assert_eq!(s.now().yields, 2, "one per stretch, not per frame");
        assert_eq!(s.now().changes, 5);
    }

    /// A connect while stepping aside lets the scene send again (its reply then says).
    #[test]
    fn a_connect_forgets_the_owner() {
        let mut s = State::default();
        let again = Arc::new(AtomicBool::new(false));
        s.observe(&frame_report(1, false));
        assert!(s.yield_to(&again));
        s.connected();
        assert_eq!(s.owner(), None);
        assert!(again.load(Ordering::Relaxed));
        assert!(!s.yield_to(&Arc::new(AtomicBool::new(false))));
    }

    /// The pages' line changes on the keyboard's event, which repaints them; a report that
    /// changes nothing repaints nothing.
    #[test]
    fn the_line_updates_on_the_event() {
        let repaints = Arc::new(AtomicU32::new(0));
        let n = repaints.clone();
        let mut s = State::default();
        s.set_wake(crate::ui_wake::UiWake::new(Arc::new(move || {
            n.fetch_add(1, Ordering::SeqCst);
        })));
        let scene = Some("Bandit scanline");
        let now = |s: &State| {
            line(
                &who(s.owner(), scene, Some(super::super::scene_run::SCENE_CONN)),
                scene,
            )
        };
        s.observe(&frame_report(3, false));
        assert_eq!(
            now(&s).text,
            "Lights now: the Foxglove scene \u{201c}Bandit scanline\u{201d}"
        );
        assert_eq!(repaints.load(Ordering::SeqCst), 1);
        s.observe(&frame_report(3, false));
        assert_eq!(repaints.load(Ordering::SeqCst), 1, "no change, no repaint");
        s.observe(&frame_report(1, true));
        assert_eq!(repaints.load(Ordering::SeqCst), 2);
        let l = now(&s);
        assert!(l.warn);
        assert!(
            l.text
                .starts_with("Lights now: Windows Dynamic Lighting. The scene"),
            "{}",
            l.text
        );
        s.observe(&frame_report(0, true));
        assert_eq!(repaints.load(Ordering::SeqCst), 3);
        assert!(!now(&s).warn);
    }

    #[test]
    fn who_drives_the_lights() {
        let scene = super::super::scene_run::SCENE_CONN;
        let s = Some("Desk");
        assert_eq!(
            who(Some(Owner::HostLink), s, Some(scene)),
            Who::Scene("Desk".into())
        );
        assert_eq!(
            who(None, s, Some(scene)),
            Who::Scene("Desk".into()),
            "one colour"
        );
        assert_eq!(who(Some(Owner::LampArrayUsb), s, Some(scene)), Who::Windows);
        assert_eq!(who(Some(Owner::LampArrayBle), None, None), Who::Windows);
        assert_eq!(
            who(Some(Owner::Keyboard), s, Some(7)),
            Who::OtherApp,
            "a plugin's lease"
        );
        assert_eq!(who(Some(Owner::Other(9)), None, None), Who::OtherApp);
        assert_eq!(
            who(Some(Owner::HostLink), None, None),
            Who::Foxglove,
            "Find my LEDs"
        );
        assert_eq!(who(Some(Owner::Keyboard), None, None), Who::Keyboard);
        assert_eq!(who(None, None, None), Who::Keyboard);
        assert_eq!(
            line(&Who::Keyboard, None).text,
            "Lights now: the keyboard itself (its own effect)"
        );
        assert!(!line(&Who::Windows, None).warn, "no scene waits");
    }

    /// The health row carries the owner and the yield count.
    #[test]
    fn the_health_row_says_the_owner_and_the_yields() {
        let mut s = State::default();
        let row = |s: &State, w: &Who| rows(&s.now(), w).remove(0);
        let r = row(&s, &Who::Keyboard);
        assert_eq!(
            (r.subsystem.as_str(), r.item.as_str()),
            (SUBSYSTEM, "Lights owner")
        );
        assert_eq!(r.level, Level::Ok);
        assert!(r.detail.contains("hasn't said yet"), "{}", r.detail);
        let again = Arc::new(AtomicBool::new(false));
        s.observe(&frame_report(1, true));
        s.yield_to(&again);
        let r = row(&s, &Who::Windows);
        assert_eq!(
            r.detail,
            "Windows Dynamic Lighting (owner 1); 1 yield to Windows Dynamic Lighting \
             (the scene steps aside now); 1 owner changes"
        );
        s.observe(&frame_report(9, false));
        assert_eq!(row(&s, &Who::OtherApp).level, Level::Warn);
    }
}
