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
//! - The scene output stops streaming to the keyboard while it steps aside (#749): its "held"
//!   flag ([`State::watch_hold`]) is set while someone else has the lights, so no keyboard
//!   frame or effect is even made. It tries the keyboard again only on an event, never on a
//!   timer: the report that frees the lights, a keyboard (re)connect, or [`retry`] (the user
//!   acting, Windows changing a LampArray's availability), which lets exactly one send
//!   through; its reply holds the output again while Windows still has the lights.
//! - While a LampArray has them, the Lights page's Keyboard card says so in one amber line
//!   ([`NOTICE`], [`setting_path`]) with a button to the Windows Dynamic Lighting settings
//!   ([`DL_SETTINGS`]); it goes on the first report saying they're free. The Scenes page only
//!   names who has them.
//! - A keyboard that is also a LampArray Windows lists ([`paired_lamp_key`]) is drawn through
//!   Windows meanwhile (#749 b): by a running scene (`scene_output`) and by the Keyboard card's
//!   picks (`pc_lights`' worker, [`through_windows`]). The line then says so ([`THROUGH`]).
//!   The report that frees the lights lets the Keyboard card's LampArray go.
//! - The pages repaint on the owner's change ([`set_wake`]), not on a timer.
//! - With package identity (#179) Foxglove holds the keyboard's LampArrays itself
//!   (`lamp_hold`): an owner byte naming a LampArray transport Foxglove holds, with Windows'
//!   leave to drive it, is Foxglove ([`Who::FoxgloveLamp`], [`ours`]), never "Windows holds the
//!   lights".
//! - While Windows Dynamic Lighting has the lights and no scene waits, the Keyboard card's bar is
//!   quiet (an info bar, [`QUIET`]) with the button to the Dynamic Lighting settings; with
//!   identity its sentence says to put Foxglove first under Background light control. Foxglove
//!   never writes that setting.
//! - After a flash ([`FlashWatch`]): when the keyboard comes back with another firmware build
//!   and Windows Dynamic Lighting (not Foxglove's own hold) then takes its lights, which it
//!   didn't before, the health view gets a WARN row ([`flash_row`]) and a notification.

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
    /// Scene outputs' "held" flags (#749): true while someone else has the lights, so the
    /// output sends the keyboard nothing at all.
    holds: Vec<Weak<AtomicBool>>,
    /// A retry on an event (#749): one send goes through, and its reply decides.
    probe: Probe,
    /// Retries since Foxglove started.
    retries: u32,
    /// Whether Windows took the lights after a flash (#179).
    flash: FlashWatch,
}

/// Watches for Windows Dynamic Lighting taking the keyboard's lights after a flash (#179): a
/// new device instance (a re-pair after a settings reset, another port) can come back with
/// Windows' own controller first.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FlashWatch {
    /// The firmware build last connected.
    build: Option<String>,
    /// This connection follows a flash: its new build, and whether a LampArray had the lights
    /// before it.
    after: Option<(String, bool)>,
    /// Windows took the lights after the flash to this build.
    pub warn: Option<String>,
}

impl FlashWatch {
    /// A keyboard connected with firmware `build` (`None`: unreadable); `before` is its owner as
    /// last known. Another build than the last one is a flash.
    pub fn connected(&mut self, build: Option<String>, before: Option<Owner>) {
        let flashed = matches!((&self.build, &build), (Some(old), Some(new)) if old != new);
        if flashed {
            let was = before.is_some_and(Owner::lamp_array);
            self.after = build.clone().map(|b| (b, was));
        } else if build.is_some() {
            self.after = None;
        }
        self.warn = None;
        if build.is_some() {
            self.build = build;
        }
    }

    /// The keyboard says who has its lights; `ours` when it is Foxglove's own LampArray hold.
    /// True when the warning came or went.
    pub fn observed(&mut self, owner: Option<Owner>, ours: bool) -> bool {
        let windows = owner.is_some_and(Owner::lamp_array) && !ours;
        let warn = match &self.after {
            Some((build, false)) if windows => Some(build.clone()),
            _ => None,
        };
        let changed = warn != self.warn;
        self.warn = warn;
        changed
    }
}

/// Where a retry stands.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Probe {
    #[default]
    None,
    /// Asked for: the next send goes through.
    Armed,
    /// That send went: the rest wait for its reply.
    Sent,
}

/// What [`State`] knows, as plain data (health rows, tests).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Now {
    pub owner: Option<Owner>,
    pub yields: u32,
    pub yielding: bool,
    pub changes: u32,
    pub retries: u32,
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
            retries: self.retries,
        }
    }

    /// A report from the keyboard; true when the owner changed. Any owner report ends a retry:
    /// the same busy owner again holds the scene output again.
    pub fn observe(&mut self, r: &[u8]) -> bool {
        let Some(said) = foxglove_link::lights_owner::said(r) else {
            return false;
        };
        if !self.tracker.apply(said) {
            if self.probe != Probe::None {
                self.probe = Probe::None;
                self.publish_holds();
            }
            return false;
        }
        self.changes = self.changes.saturating_add(1);
        self.changed();
        true
    }

    /// The scene outputs hold while someone else has the lights and no retry is out.
    fn held(&self) -> bool {
        self.busy() && self.probe == Probe::None
    }

    /// Sets every live hold flag; a held output is a scene stepping aside.
    fn publish_holds(&mut self) {
        self.holds.retain(|w| w.strong_count() > 0);
        let held = self.held();
        for flag in self.holds.iter().filter_map(Weak::upgrade) {
            flag.store(held, Ordering::Relaxed);
        }
        if held && !self.holds.is_empty() {
            self.step_aside();
        }
    }

    /// A scene's output stepping aside: counted once per stretch.
    fn step_aside(&mut self) {
        if !std::mem::replace(&mut self.yielding, true) {
            self.yields = self.yields.saturating_add(1);
            // The Scenes page's status says "Paused for ..." from now on.
            if let Some(w) = &self.wake {
                w.request_repaint();
            }
        }
    }

    /// A scene output's "held" flag: set now and on every change while it lives (#749).
    pub fn watch_hold(&mut self, held: &Arc<AtomicBool>) {
        self.holds.retain(|w| w.strong_count() > 0);
        if !self.holds.iter().any(|w| w.as_ptr() == Arc::as_ptr(held)) {
            self.holds.push(Arc::downgrade(held));
        }
        self.publish_holds();
    }

    /// An event worth trying the keyboard again on (#749): the user acted, or Windows changed a
    /// LampArray's availability. While someone else has the lights, the scene output sends once
    /// and its reply says who has them; true when that retry is out. Nothing repeats it.
    pub fn retry(&mut self) -> bool {
        if !self.busy() || self.probe == Probe::Armed {
            return false;
        }
        self.probe = Probe::Armed;
        self.retries = self.retries.saturating_add(1);
        self.publish_holds();
        // The output sends its frame or effect in full, not only what changed.
        for flag in self.waiting.iter().filter_map(Weak::upgrade) {
            flag.store(true, Ordering::Relaxed);
        }
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
        self.probe = Probe::None;
        self.publish_holds();
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
        let through = self.probe == Probe::Armed;
        if through {
            self.probe = Probe::Sent;
        } else {
            self.step_aside();
        }
        self.waiting.retain(|w| w.strong_count() > 0);
        if !self
            .waiting
            .iter()
            .any(|w| w.as_ptr() == Arc::as_ptr(again))
        {
            self.waiting.push(Arc::downgrade(again));
        }
        !through
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
            let now = s.owner();
            drop(s);
            eprintln!(
                "Foxglove: lights owner: {} (was {})",
                byte_words(now),
                byte_words(before)
            );
            // The lights moved to the keyboard's other LampArray (11/09): the scene reopens it
            // on that transport and draws it in full, and the Keyboard card lets go of the old
            // one (its next pick opens the new one).
            if super::lamp_hold::transport_moved(before, now) {
                eprintln!(
                    "Foxglove: lights owner: the keyboard's lights moved to its other                      LampArray; drawing there"
                );
                super::scene_run::devices_changed();
                super::pc_lights::send(super::pc_lights::Msg::KeyboardLampRelease);
            }
            // Windows let go (#749 b): the Lights page lets go of the keyboard's LampArray.
            if before.is_some_and(Owner::lamp_array) && !now.is_some_and(Owner::lamp_array) {
                super::pc_lights::send(super::pc_lights::Msg::KeyboardLampRelease);
                // And the Keyboard card's effect drawn through it ends (#768).
                super::scene_card::release();
            }
            // The lighting facts follow what Foxglove draws through Windows while it holds
            // the keyboard, and the keyboard's own report again when it lets go (#789).
            super::keyboard_facts::held(through_windows().is_some());
            // After a flash, Windows taking the lights is a WARN and a notification (#179).
            let ours = now.is_some_and(super::lamp_hold::holds);
            let warned = {
                let mut s = lock(&state);
                s.flash
                    .observed(now, ours)
                    .then(|| s.flash.warn.clone())
                    .flatten()
            };
            if let Some(build) = warned {
                eprintln!(
                    "Foxglove: lights owner: Windows Dynamic Lighting took the keyboard's lights \
                     after the flash to {build}"
                );
                super::health_notice::check(&super::health::all_rows());
            }
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
    let build = super::devices::firmware_build();
    {
        let mut s = lock(&STATE);
        let before = s.owner();
        s.flash.connected(build, before);
        s.connected();
    }
    // Nothing known until the keyboard says: its own report is the lighting facts' source.
    super::keyboard_facts::held(false);
    attach(&kb);
}

/// An event worth trying the keyboard again on, in words for the log (#749): the user acted on
/// the lights or a scene, or Windows changed a Dynamic Lighting device's availability. Does
/// nothing unless someone else has the lights.
pub fn retry(why: &str) {
    if lock(&STATE).retry() {
        eprintln!("Foxglove: lights owner: {why}; the scene tries the keyboard once");
    }
}

/// Repaints the pages that show the line (Foxglove's own LampArray hold changed, #179).
pub fn repaint() {
    if let Some(w) = &lock(&STATE).wake {
        w.request_repaint();
    }
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
    /// Foxglove through its own hold on the keyboard's LampArray (#179): the owner byte says a
    /// LampArray, and it is Foxglove's.
    FoxgloveLamp,
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
            Who::FoxgloveLamp => "Foxglove, through Windows Dynamic Lighting".into(),
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
    /// Who drives them (the Keyboard card's chip takes its dot from it, #794).
    pub who: Who,
    /// The Keyboard card's short form (#794): the chip's text, or while Windows holds the
    /// lights the info bar's one line (60 characters or less); `text` is its hover and ⓘ.
    pub short: String,
    /// While Windows holds the lights: nothing waits for them and Foxglove doesn't draw through
    /// it, so the bar is a quiet info bar (#179); amber otherwise (#794).
    pub quiet: bool,
}

/// The info bar's line while Windows holds the lights (#794, K2), with the waiting scene's name
/// cut to fit the bar's 60 characters; [`QUIET`] when no scene waits.
fn windows_short(scene: Option<&str>) -> String {
    const HELD: &str = "Windows has the keyboard's lights";
    let Some(name) = scene else {
        return QUIET.to_string();
    };
    let room = super::theme::BAR_MAX_CHARS
        - HELD.chars().count()
        - " · \u{201c}\u{201d} waits".chars().count();
    let name: String = if name.chars().count() > room {
        name.chars().take(room - 1).chain(['…']).collect()
    } else {
        name.to_string()
    };
    format!("{HELD} · \u{201c}{name}\u{201d} waits")
}

/// The info bar's line while Foxglove draws through Windows (#794, K3).
fn through_short(identity: bool) -> String {
    format!(
        "Windows has the lights · {}",
        if identity {
            "Foxglove draws through it"
        } else {
            "Foxglove draws through it in front"
        }
    )
}

pub const TIP: &str = "Who drives the keyboard's lights, as the keyboard says. Windows Dynamic \
                       Lighting comes first. While it holds them, Foxglove draws through it if \
                       Windows lists the keyboard as a Dynamic Lighting device (only while \
                       Foxglove's window is in front, unless Foxglove has package identity); \
                       otherwise a running scene stops sending and comes back when Windows \
                       lets go.";

/// What the pages say while a LampArray (Windows Dynamic Lighting) has the keyboard's lights
/// (#749).
pub const NOTICE: &str = "Windows Dynamic Lighting holds the keyboard's lights";

/// The Keyboard card's quiet line while Windows Dynamic Lighting has the lights and nothing waits
/// for them (#179).
pub const QUIET: &str = "Windows Dynamic Lighting is controlling this keyboard";

/// [`windows_line`] with package identity (#179): Foxglove can be the background controller, so
/// the fix is the order, which only the user sets.
pub fn identity_line(device: Option<&str>) -> String {
    let device = device.map(str::trim).filter(|d| !d.is_empty());
    format!(
        "{QUIET}: another controller comes before Foxglove for it. In Settings > Personalization > \
         Dynamic Lighting > Background light control, drag Foxglove to the top, and check {}'s \
         own device pages (USB and Bluetooth). Foxglove never changes that setting.",
        device.unwrap_or("the keyboard")
    )
}

/// The Windows setting that gives the lights back, for the keyboard as Windows lists it (its
/// USB product name; "your keyboard" while unknown). Foxglove never changes it.
pub fn setting_path(device: Option<&str>) -> String {
    let device = device.map(str::trim).filter(|d| !d.is_empty());
    format!(
        "Settings > Personalization > Dynamic Lighting > {} > \u{201c}Use Dynamic Lighting on my devices\u{201d}",
        device.unwrap_or("your keyboard")
    )
}

/// The one line while Windows holds the lights: [`NOTICE`], the setting that gives them back,
/// and the running scene that waits for them.
pub fn windows_line(device: Option<&str>, scene: Option<&str>) -> String {
    let mut text = format!(
        "{NOTICE}. Turn off {} to give them back.",
        setting_path(device)
    );
    if let Some(name) = scene {
        text.push_str(&format!(
            " The scene \u{201c}{name}\u{201d} comes back then."
        ));
    }
    text
}

/// What Foxglove does meanwhile when the keyboard is also a LampArray (#749 b): it draws through
/// Windows, which lets an app without package identity do so only while its window is in front.
pub const THROUGH: &str = "Foxglove draws through it while its window is in front";

/// [`THROUGH`] with package identity (#179): Windows' background order decides, not the window.
pub const THROUGH_IDENTITY: &str = "Foxglove draws through it";

/// The one line while Windows holds the lights and Foxglove draws through the keyboard's own
/// LampArray (#749 b): [`NOTICE`], [`THROUGH`] (or [`THROUGH_IDENTITY`]), and the setting that
/// gives the lights back. No scene waits: a running one draws through Windows too.
pub fn through_line(device: Option<&str>, identity: bool) -> String {
    format!(
        "{NOTICE}; {}. Turn off {} to give them back.",
        if identity { THROUGH_IDENTITY } else { THROUGH },
        setting_path(device)
    )
}

/// The line for `who`, with the running scene's name and the keyboard's name (for the
/// Windows setting). The app draws [`line_through`]; the tests and previews draw this one.
#[cfg_attr(not(test), allow(dead_code))]
pub fn line(who: &Who, scene: Option<&str>, device: Option<&str>) -> Line {
    line_through(who, scene, device, None)
}

/// [`line`], with `through` `Some(identity)` while the keyboard has its own LampArray that
/// Foxglove draws through as Windows holds the lights (#749 b).
pub fn line_through(
    who: &Who,
    scene: Option<&str>,
    device: Option<&str>,
    through: Option<bool>,
) -> Line {
    if matches!(who, Who::Windows) {
        let (text, short) = match through {
            Some(identity) => (through_line(device, identity), through_short(identity)),
            None => (windows_line(device, scene), windows_short(scene)),
        };
        return Line {
            text,
            warn: true,
            who: who.clone(),
            short,
            quiet: scene.is_none() && through.is_none(),
        };
    }
    let text = format!("Lights now: {}", who.words());
    Line {
        short: text.clone(),
        text,
        warn: false,
        who: who.clone(),
        quiet: false,
    }
}

/// [`line_through`] as the app draws it: with package identity and nothing drawn through
/// Windows, the sentence says to put Foxglove first ([`identity_line`]).
pub fn line_for(
    who: &Who,
    scene: Option<&str>,
    device: Option<&str>,
    through: Option<bool>,
    identity: bool,
) -> Line {
    let mut line = line_through(who, scene, device, through);
    if identity && matches!(who, Who::Windows) && through.is_none() && scene.is_none() {
        line.text = identity_line(device);
    }
    line
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

/// [`who`]'s answer when Foxglove itself holds the LampArray the owner byte names
/// (`held_by_foxglove`, #179): Foxglove (or its running scene), not Windows.
pub fn ours(who: Who, held_by_foxglove: bool, scene: Option<&str>) -> Who {
    match who {
        Who::Windows if held_by_foxglove => {
            scene.map_or(Who::FoxgloveLamp, |name| Who::Scene(name.to_string()))
        }
        other => other,
    }
}

/// Who drives the lights now, and the running scene's name.
fn who_now(owner: Option<Owner>) -> (Who, Option<String>) {
    let scene = super::scene_run::running().map(|(name, _)| name);
    let held = owner.is_some_and(super::lamp_hold::holds);
    let who = ours(
        who(owner, scene.as_deref(), lease_now()),
        held,
        scene.as_deref(),
    );
    (who, scene)
}

/// The line as it stands now (no blocking: the arbiter's published holders, the cached device).
#[cfg_attr(test, allow(dead_code))]
pub fn line_now() -> Line {
    let owner = lock(&STATE).owner();
    let (who, scene) = who_now(owner);
    let identity = super::scene_lamps::identity();
    let through = (who == Who::Windows)
        .then(paired_lamp_key)
        .flatten()
        .map(|_| identity);
    line_for(
        &who,
        scene.as_deref(),
        super::devices::keyboard_name().as_deref(),
        through,
        identity,
    )
}

/// The host-link keyboard's own LampArray as Windows lists it (#749 b), paired by VID/PID
/// (`scene_output::keyboard_lamp_key`): the key the Lights page and a scene draw it through
/// while Windows holds its lights. Never blocks or looks for the keyboard.
pub fn paired_lamp_key() -> Option<String> {
    let keyboard = super::devices::keyboard_key()?;
    super::scene_output::keyboard_lamp_key(
        &keyboard,
        &super::pc_lights::snapshot().keyboard_lamp_arrays,
    )
}

/// While Windows Dynamic Lighting holds the keyboard's lights (the owner report says a
/// LampArray) and the keyboard has its own LampArray: its key, for the Keyboard card to draw
/// through (#749 b). Only on the owner's word, never on a timer.
pub fn through_windows() -> Option<String> {
    if !lock(&STATE).owner().is_some_and(Owner::lamp_array) {
        return None;
    }
    paired_lamp_key()
}

#[cfg(test)]
thread_local! {
    /// The previews' line; without one, tests draw none (they never read the app's state).
    pub static PREVIEW: std::cell::RefCell<Option<Line>> = const { std::cell::RefCell::new(None) };
}

/// The Windows Settings page for Dynamic Lighting (#749).
pub const DL_SETTINGS: &str = "ms-settings:personalization-lighting";

/// The button after the notice that opens [`DL_SETTINGS`].
pub const DL_BUTTON: &str = "Open Dynamic Lighting settings";

/// The owner line now, for the pages to draw; `None` in tests without a preview line (they never
/// read the app's state).
pub fn current(ctx: &egui::Context) -> Option<Line> {
    #[cfg(test)]
    {
        let _ = ctx;
        PREVIEW.with_borrow(|l| l.clone())
    }
    #[cfg(not(test))]
    {
        set_wake(ctx);
        Some(line_now())
    }
}

/// Draws the Scenes page's owner line after `space` points; true when it drew one. Windows
/// holding the lights is only named there: the Lights page's Keyboard card has the notice.
pub fn show_line(ui: &mut egui::Ui, space: f32) -> bool {
    let Some(line) = current(ui.ctx()) else {
        return false;
    };
    let p = super::theme::palette(ui);
    ui.add_space(space);
    let text = if line.warn {
        format!("Lights now: {}", Who::Windows.words())
    } else {
        line.text
    };
    ui.add(egui::Label::new(egui::RichText::new(text).color(p.text)).wrap())
        .on_hover_text(TIP);
    true
}

/// The Keyboard card's amber notice while Windows holds the lights (#749, #794 K2/K3): one info
/// bar with the short line, the whole sentence (and what the owner line means, [`TIP`]) on its
/// ⓘ, and the button to the Windows setting. Never collapsed.
pub fn notice_bar(ui: &mut egui::Ui, line: &Line) {
    let full = format!("{}\n\n{TIP}", line.text);
    let level = if line.quiet {
        super::theme::Level::Info
    } else {
        super::theme::Level::Warn
    };
    let bar = super::theme::info_bar(ui, level, &line.short, &full, Some(DL_BUTTON));
    if bar.clicked() {
        super::notifications_ui::open_settings_page(DL_SETTINGS);
    }
}

/// The Keyboard card's owner chip (#794 K1): "Lights now: Foxglove" with a green dot while
/// Foxglove (or one of its scenes) drives the lights, grey otherwise; [`TIP`] on hover.
pub fn chip(ui: &mut egui::Ui, line: &Line) -> egui::Response {
    use super::theme::Dot;
    let dot = match line.who {
        Who::Foxglove | Who::FoxgloveLamp | Who::Scene(_) => Dot::Ok,
        _ => Dot::Muted,
    };
    super::theme::hint_chip(ui, &line.short, Some(TIP), Some(dot))
}

/// The health view's row now (registered with `health::register` as "lights_owner").
pub fn rows_now() -> Vec<HealthRow> {
    let (now, warn) = {
        let s = lock(&STATE);
        (s.now(), s.flash.warn.clone())
    };
    let (who, _) = who_now(now.owner);
    let mut out = rows(&now, &who);
    out.extend(flash_row(warn.as_deref()));
    out
}

/// The WARN row while Windows Dynamic Lighting has the lights it took after a flash to `build`
/// (#179). Foxglove never changes the setting; the row says where it is.
pub fn flash_row(build: Option<&str>) -> Option<HealthRow> {
    build.map(|b| {
        HealthRow::new(
            SUBSYSTEM,
            "Lights owner after the flash",
            Level::Warn,
            format!(
                "Windows Dynamic Lighting took the keyboard's lights after the flash to {b}: \
                 open Settings > Personalization > Dynamic Lighting and put Foxglove first under \
                 Background light control for both keyboard entries (or turn off \
                 \u{201c}Use Dynamic Lighting on my devices\u{201d} on them)"
            ),
        )
    })
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
                Some("Eyelash Corne"),
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
        assert_eq!(
            l.text,
            "Windows Dynamic Lighting holds the keyboard's lights. Turn off Settings > \
             Personalization > Dynamic Lighting > Eyelash Corne > \u{201c}Use Dynamic Lighting \
             on my devices\u{201d} to give them back. The scene \u{201c}Bandit scanline\u{201d} \
             comes back then."
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
            line(&Who::Keyboard, None, None).text,
            "Lights now: the keyboard itself (its own effect)"
        );
        assert!(!line(&Who::Keyboard, None, None).warn);
        let w = line(&Who::Windows, None, None);
        assert!(w.warn, "Windows holding the lights is worth a look");
        assert!(w.text.starts_with(NOTICE), "{}", w.text);
        assert!(
            w.text.contains("> your keyboard >"),
            "no name yet: {}",
            w.text
        );
    }

    /// #794: the Keyboard card's short forms. The chip says who; the info bar's line names the
    /// problem in 60 characters or less, a long scene name cut to fit (it is whole in `text`).
    #[test]
    fn the_short_lines_fit_the_info_bar() {
        use super::super::theme::BAR_MAX_CHARS;
        let long = "A scene with a very long name indeed, longer than the bar";
        let lines = [
            line(&Who::Windows, Some(long), None),
            line(&Who::Windows, Some("Desk"), None),
            line(&Who::Windows, None, None),
            line_through(&Who::Windows, None, None, Some(false)),
            line_through(&Who::Windows, None, None, Some(true)),
        ];
        for l in &lines {
            assert!(l.short.chars().count() <= BAR_MAX_CHARS, "{}", l.short);
            assert!(l.warn && l.who == Who::Windows);
        }
        assert_eq!(
            lines[1].short,
            "Windows has the keyboard's lights · \u{201c}Desk\u{201d} waits"
        );
        assert!(
            lines[0].short.ends_with("…\u{201d} waits"),
            "{}",
            lines[0].short
        );
        assert!(lines[0].text.contains(long));
        assert_eq!(lines[2].short, QUIET);
        assert_eq!(
            lines[3].short,
            "Windows has the lights · Foxglove draws through it in front"
        );
        assert_eq!(
            lines[4].short,
            "Windows has the lights · Foxglove draws through it"
        );
        let mine = line(&Who::Foxglove, None, None);
        assert_eq!(mine.short, "Lights now: Foxglove");
        assert_eq!(mine.short, mine.text);
    }

    /// #749: the notice names the Windows setting for the keyboard as Windows lists it, shows
    /// only for a LampArray owner, and goes on the first report saying the lights are free.
    #[test]
    fn the_notice_follows_the_replies() {
        let notice = |o: Option<Owner>, d: Option<&str>| {
            let l = line(&who(o, None, None), None, d);
            l.warn.then_some(l.text)
        };
        assert_eq!(
            setting_path(Some(" Eyelash Corne ")),
            "Settings > Personalization > Dynamic Lighting > Eyelash Corne > \u{201c}Use \
             Dynamic Lighting on my devices\u{201d}"
        );
        assert!(setting_path(Some("")).contains("> your keyboard >"));
        for o in [
            None,
            Some(Owner::Keyboard),
            Some(Owner::HostLink),
            Some(Owner::Other(9)),
        ] {
            assert_eq!(notice(o, Some("K")), None, "{o:?}");
        }
        let mut s = State::default();
        // A SET_FRAME SHOW refused busy, owner USB LampArray (status 1): the notice shows.
        let mut busy = frame_report(1, false);
        busy[2] = 1;
        s.observe(&busy);
        let n = notice(s.owner(), Some("Eyelash Corne")).expect("the notice");
        assert!(n.starts_with(NOTICE) && n.contains("Eyelash Corne"), "{n}");
        // The same from SET_EFFECT (D7 8F), owner BLE LampArray.
        let mut s2 = State::default();
        let mut effect = [0u8; 32];
        effect[..7].copy_from_slice(&[0xD7, 0x8F, 1, 2, 0, 0, 0]);
        s2.observe(&effect);
        assert!(notice(s2.owner(), None).is_some());
        // The first reply saying they're free (Foxglove's frame shown) clears it.
        s.observe(&frame_report(3, false));
        assert_eq!(notice(s.owner(), Some("Eyelash Corne")), None);
    }

    /// #749 b: while Foxglove draws through the keyboard's own LampArray, the notice says so
    /// (foreground only without package identity) and still names the setting; no scene
    /// waits. Without a paired LampArray it reads as before; other owners are untouched.
    #[test]
    fn the_notice_says_foxglove_draws_through_windows() {
        let scene = Some("Bandit scanline");
        let corne = Some("Eyelash Corne");
        let l = line_through(&Who::Windows, scene, corne, Some(false));
        assert!(l.warn);
        assert_eq!(
            l.text,
            "Windows Dynamic Lighting holds the keyboard's lights; Foxglove draws through it \
             while its window is in front. Turn off Settings > Personalization > Dynamic \
             Lighting > Eyelash Corne > \u{201c}Use Dynamic Lighting on my devices\u{201d} to \
             give them back."
        );
        let id = line_through(&Who::Windows, scene, corne, Some(true)).text;
        assert!(
            id.starts_with("Windows Dynamic Lighting holds the keyboard's lights; Foxglove draws through it. Turn off"),
            "{id}"
        );
        assert_eq!(
            line_through(&Who::Windows, scene, corne, None),
            line(&Who::Windows, scene, corne)
        );
        assert!(line(&Who::Windows, scene, corne)
            .text
            .contains("comes back then"));
        assert_eq!(
            line_through(&Who::Keyboard, None, corne, Some(false)),
            line(&Who::Keyboard, None, corne)
        );
        // The tooltip says the same: Foxglove draws through Windows, front-only without
        // package identity; a scene waits only when the keyboard isn't a LampArray.
        assert!(TIP.contains("Foxglove draws through it"), "{TIP}");
        assert!(TIP.contains("window is in front"), "{TIP}");
        assert!(
            TIP.contains("otherwise a running scene stops sending"),
            "{TIP}"
        );
    }

    /// #749: the scene output's hold flag is set while Windows has the lights (so it streams
    /// nothing), cleared by the report that frees them and by a reconnect; never by time.
    #[test]
    fn the_output_holds_while_busy_and_lets_go_on_events() {
        let mut s = State::default();
        let held = Arc::new(AtomicBool::new(true));
        s.watch_hold(&held);
        assert!(!held.load(Ordering::Relaxed), "nothing said yet: stream");
        s.observe(&frame_report(1, true));
        assert!(held.load(Ordering::Relaxed), "Windows took them: hold");
        assert!(s.now().yielding, "a held output is a scene stepping aside");
        assert_eq!(s.now().yields, 1);
        // Further busy replies change nothing.
        s.observe(&frame_report(1, false));
        assert!(held.load(Ordering::Relaxed));
        assert_eq!(s.now().yields, 1);
        // Freed by the keyboard's event.
        s.observe(&frame_report(0, true));
        assert!(!held.load(Ordering::Relaxed));
        assert!(!s.now().yielding);
        // Busy again, then a reconnect: nothing known, so the output tries.
        s.observe(&frame_report(2, true));
        assert!(held.load(Ordering::Relaxed));
        s.connected();
        assert!(!held.load(Ordering::Relaxed));
        // A new scene's output registered while busy starts held.
        s.observe(&frame_report(1, true));
        let late = Arc::new(AtomicBool::new(false));
        s.watch_hold(&late);
        assert!(late.load(Ordering::Relaxed));
        // A dropped output is forgotten.
        drop(late);
        s.observe(&frame_report(0, true));
        assert_eq!(s.holds.len(), 1);
    }

    /// #749: a retry (the user acted, a LampArray's availability changed) lets exactly one
    /// send through; its busy reply holds the output again, a free one ends the wait.
    #[test]
    fn a_retry_sends_once_and_the_reply_decides() {
        let mut s = State::default();
        let held = Arc::new(AtomicBool::new(false));
        let again = Arc::new(AtomicBool::new(false));
        s.watch_hold(&held);
        assert!(!s.retry(), "nobody else has the lights: nothing to retry");
        s.observe(&frame_report(1, true));
        assert!(held.load(Ordering::Relaxed));
        assert!(s.yield_to(&again), "held: the writer steps aside too");

        assert!(s.retry());
        assert!(!s.retry(), "one retry at a time");
        assert!(!held.load(Ordering::Relaxed), "the output sends once");
        assert!(again.swap(false, Ordering::Relaxed), "in full");
        assert!(!s.yield_to(&again), "the one send goes through");
        assert!(s.yield_to(&again), "the rest wait for its reply");
        // Still busy: held again; the notice stays.
        s.observe(&frame_report(1, false));
        assert!(held.load(Ordering::Relaxed));
        assert!(s.busy());
        assert_eq!(s.now().retries, 1);

        // Another retry, and this time the reply says Foxglove's frame is shown.
        assert!(s.retry());
        assert!(!s.yield_to(&again));
        s.observe(&frame_report(3, false));
        assert!(!held.load(Ordering::Relaxed));
        assert!(!s.yield_to(&again), "streaming again");
        assert!(!s.now().yielding);
        assert_eq!(s.now().retries, 2);
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

    /// #179: with identity Foxglove holds the keyboard's LampArrays itself; an owner byte naming
    /// one of them is Foxglove (or its scene), with no "Windows holds the lights" notice.
    #[test]
    fn foxgloves_own_lamparray_hold_is_foxglove() {
        let lamp = Some(Owner::LampArrayBle);
        let windows = who(lamp, None, None);
        assert_eq!(windows, Who::Windows);
        assert_eq!(ours(windows.clone(), false, None), Who::Windows, "not ours");
        let me = ours(windows.clone(), true, None);
        assert_eq!(me, Who::FoxgloveLamp);
        assert_eq!(
            ours(windows, true, Some("Ember")),
            Who::Scene("Ember".into())
        );
        assert_eq!(ours(Who::Keyboard, true, None), Who::Keyboard);
        let l = line(&me, None, Some("Eyelash Corne"));
        assert!(!l.warn, "no amber notice about Foxglove's own hold");
        assert!(!l.text.contains(NOTICE), "{}", l.text);
        assert!(l.text.contains("Foxglove"), "{}", l.text);
        let r = rows(
            &Now {
                owner: lamp,
                ..Now::default()
            },
            &me,
        );
        assert_eq!(r[0].level, Level::Ok);
        assert!(r[0].detail.contains("Foxglove, through"), "{}", r[0].detail);
    }

    /// #179: with nothing waiting the bar is quiet and says Windows is controlling the keyboard;
    /// a waiting scene keeps the amber bar; with identity the sentence says to put Foxglove first
    /// and never offers to change the setting itself.
    #[test]
    fn the_windows_line_is_quiet_unless_a_scene_waits() {
        let corne = Some("Eyelash Corne");
        let quiet = line(&Who::Windows, None, corne);
        assert!(quiet.warn && quiet.quiet);
        assert_eq!(quiet.short, QUIET);
        let waits = line(&Who::Windows, Some("Ember"), corne);
        assert!(waits.warn && !waits.quiet, "a scene waits: amber");
        let through = line_through(&Who::Windows, None, corne, Some(true));
        assert!(!through.quiet, "drawn through Windows: amber stays (#794)");
        let ranked = line_for(&Who::Windows, None, corne, None, true);
        assert!(
            ranked.text.contains("drag Foxglove to the top"),
            "{}",
            ranked.text
        );
        assert!(ranked.text.contains("never changes"), "{}", ranked.text);
        assert_eq!(ranked.short, QUIET);
        let plain = line_for(&Who::Windows, None, corne, None, false);
        assert_eq!(plain.text, quiet.text, "unpackaged: as before");
        assert!(QUIET.chars().count() <= crate::foxglove::theme::BAR_MAX_CHARS);
    }

    /// #179: after a flash (another build), Windows taking the lights is a WARN until it lets go;
    /// not when it had them before the flash, not for Foxglove's own hold, not on a plain
    /// reconnect.
    #[test]
    fn windows_taking_the_lights_after_a_flash_warns() {
        let lamp = Some(Owner::LampArrayUsb);
        let mut w = FlashWatch::default();
        w.connected(Some("0.3.0 dev 5be702c".into()), None);
        assert!(!w.observed(lamp, false), "the first build seen: no flash");
        w.connected(Some("0.3.0 dev c9bf83b".into()), Some(Owner::HostLink));
        assert!(!w.observed(Some(Owner::HostLink), false));
        assert!(w.observed(lamp, false), "Windows took them after the flash");
        assert_eq!(w.warn.as_deref(), Some("0.3.0 dev c9bf83b"));
        assert_eq!(flash_row(w.warn.as_deref()).unwrap().level, Level::Warn);
        assert!(
            w.observed(Some(Owner::HostLink), false),
            "it let go: the row goes"
        );
        assert_eq!(w.warn, None);
        assert!(!w.observed(lamp, true), "Foxglove's own hold isn't Windows");
        // A reconnect with the same build isn't a flash.
        w.connected(Some("0.3.0 dev c9bf83b".into()), lamp);
        assert!(!w.observed(lamp, false));
        // Windows had them before the flash too: nothing new to say.
        w.connected(Some("0.3.1 dev 0000000".into()), lamp);
        assert!(!w.observed(lamp, false));
        assert_eq!(flash_row(None), None);
    }
}
