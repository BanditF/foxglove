//! Lighting leases (#236): plugins that signal a state on the underglow (mic muted, recording)
//! hold the lights while it lasts and give them back cleanly.
//!
//! - A lease names a colour (and effect) and a priority; the lights show the highest-priority
//!   lease, the newest among equals.
//! - When the first lease on a device starts, its lights as they were are kept; when the last
//!   one ends they come back: on/off, colour, brightness, effect and speed, each read back
//!   (#293). The snapshot is kept until the keyboard shows it again, so a restore that didn't
//!   take is tried again when the keyboard next connects, and a new lease meanwhile doesn't
//!   take a half-restored state for the original.
//! - A plugin's leases end when it releases them or its connection closes (stopped, crashed).
//! - Every change is read back and sent again if the keyboard didn't take it (a few times).
//! - While Foxglove is paused (#404) no lease shows: the lights go back as they were, leases
//!   still come, change and go, and the winner shows again on resume.

use std::collections::HashMap;
use std::sync::Mutex;

use foxglove_devices::{DeviceError, Effect, Lighting, LightingState};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Priority {
    Low,
    Normal,
    High,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Lease {
    pub id: u64,
    /// The connection that took it; it ends with the connection.
    pub conn: u64,
    pub device: String,
    pub priority: Priority,
    pub hue: u16,
    pub sat: u8,
    pub brightness: u8,
    pub effect: String,
}

#[derive(Default)]
pub struct Arbiter {
    leases: Vec<Lease>,
    /// Each leased device's lights from before its first lease, until they're shown again.
    base: HashMap<String, LightingState>,
    next: u64,
    /// Foxglove is paused: no lease shows.
    paused: bool,
}

/// What a device's lights should show now.
#[derive(Debug, PartialEq)]
enum Target<'a> {
    Lease(&'a Lease),
    Restore(LightingState),
    Nothing,
}

impl Arbiter {
    fn winner(&self, device: &str) -> Option<&Lease> {
        self.leases
            .iter()
            .filter(|l| l.device == device)
            .max_by_key(|l| (l.priority, l.id))
    }

    /// Takes the lights for `lease` (its `id` is assigned) and shows the winner.
    pub fn add(&mut self, mut lease: Lease, light: &dyn Lighting) -> Result<u64, DeviceError> {
        if !self.paused && !self.base.contains_key(&lease.device) {
            self.base.insert(lease.device.clone(), settled(light)?);
        }
        self.next += 1;
        lease.id = self.next;
        let (id, device) = (lease.id, lease.device.clone());
        self.leases.push(lease);
        self.apply(&device, light)?;
        Ok(id)
    }

    /// Changes a lease's colour and effect; returns its device when the lights need
    /// [`Arbiter::apply`] (it's the one showing), or an error if `conn` holds no such lease.
    pub fn update(
        &mut self,
        id: u64,
        conn: u64,
        hue: u16,
        sat: u8,
        brightness: u8,
        effect: &str,
    ) -> Result<Option<String>, String> {
        let lease = self
            .leases
            .iter_mut()
            .find(|l| l.id == id && l.conn == conn)
            .ok_or("no such lease")?;
        (lease.hue, lease.sat, lease.brightness, lease.effect) =
            (hue, sat, brightness, effect.to_string());
        let device = lease.device.clone();
        Ok(self
            .winner(&device)
            .is_some_and(|w| w.id == id && !self.paused)
            .then_some(device))
    }

    /// Foxglove paused or resumed (#404); returns the devices whose lights need
    /// [`Arbiter::apply`]: paused, each leased one gets its lights from before back; resumed,
    /// the winning lease shows again.
    pub fn set_paused(&mut self, paused: bool) -> Vec<String> {
        if self.paused == paused {
            return Vec::new();
        }
        self.paused = paused;
        let mut devices: Vec<String> = self.leases.iter().map(|l| l.device.clone()).collect();
        devices.sort();
        devices.dedup();
        devices
    }

    /// Ends the leases `ends` picks; returns the devices whose lights need [`Arbiter::apply`].
    pub fn end(&mut self, ends: impl Fn(&Lease) -> bool) -> Vec<String> {
        let mut devices: Vec<String> = self
            .leases
            .iter()
            .filter(|l| ends(l))
            .map(|l| l.device.clone())
            .collect();
        self.leases.retain(|l| !ends(l));
        devices.sort();
        devices.dedup();
        devices
    }

    fn target(&self, device: &str) -> Target<'_> {
        if let Some(l) = self.winner(device).filter(|_| !self.paused) {
            return Target::Lease(l);
        }
        match self.base.get(device) {
            Some(base) => Target::Restore(base.clone()),
            None => Target::Nothing,
        }
    }

    /// Shows the winning lease, or the lights from before the first lease once none is left.
    /// The snapshot goes only once the keyboard shows it.
    pub fn apply(&mut self, device: &str, light: &dyn Lighting) -> Result<(), DeviceError> {
        // Resumed (or a lease came while paused): keep the lights as they are first.
        if !self.paused && self.winner(device).is_some() && !self.base.contains_key(device) {
            self.base.insert(device.to_string(), settled(light)?);
        }
        match self.target(device) {
            Target::Lease(l) => show(light, &Want::lease(l), false).map(|_| ()),
            // Giving the lights back waits for both halves of a split keyboard.
            Target::Restore(s) => {
                if show(light, &Want::restore(&s), true)? {
                    self.base.remove(device);
                } else {
                    eprintln!("Foxglove: {device}'s lights didn't come back as they were; trying again when it next connects");
                }
                Ok(())
            }
            Target::Nothing => Ok(()),
        }
    }

    /// The device (re)connected: a restore that didn't take is tried again (#293). Started by
    /// the connection, stopped by the readback or [`RESENDS`].
    pub fn connected(&mut self, device: &str, light: &dyn Lighting) -> Result<(), DeviceError> {
        if (self.paused || self.winner(device).is_none()) && self.base.contains_key(device) {
            return self.apply(device, light);
        }
        Ok(())
    }
}

/// What the lights should show, and what a readback has to match.
struct Want<'a> {
    on: bool,
    hue: u16,
    sat: u8,
    brightness: u8,
    effect: &'a str,
    /// A restore puts the speed back too; a lease leaves it.
    speed: Option<u8>,
}

impl<'a> Want<'a> {
    fn lease(l: &'a Lease) -> Self {
        Want {
            on: true,
            hue: l.hue,
            sat: l.sat,
            brightness: l.brightness,
            effect: &l.effect,
            speed: None,
        }
    }

    fn restore(s: &'a LightingState) -> Self {
        Want {
            on: s.on,
            hue: s.hue,
            sat: s.sat,
            brightness: s.brightness,
            effect: &s.effect.id,
            speed: Some(s.speed),
        }
    }
}

/// How often a change the keyboard didn't take is sent again.
const RESENDS: u32 = 3;

fn shows(state: &LightingState, want: &Want) -> bool {
    let near = |a: u16, b: u16, wrap: u16| {
        let d = a.abs_diff(b);
        d.min(wrap.saturating_sub(d)) <= 2
    };
    // The colour counts even with the lights off: the keyboard keeps it for when they come back.
    state.on == want.on
        && near(state.hue, want.hue, 360)
        && near(state.sat.into(), want.sat.into(), 1000)
        && near(state.brightness.into(), want.brightness.into(), 1000)
        && state.effect.id == want.effect
        && want.speed.is_none_or(|s| state.speed == s)
}

/// How long to wait for a split keyboard's other half to catch up (1.7 s typical, 5.9 s seen).
const SETTLE: std::time::Duration = std::time::Duration::from_secs(8);
const SETTLE_STEP: std::time::Duration = std::time::Duration::from_millis(250);

/// The lights once every part of the device agrees, or as they are after [`SETTLE`].
fn settled(light: &dyn Lighting) -> Result<LightingState, DeviceError> {
    let until = std::time::Instant::now() + SETTLE;
    loop {
        let s = light.state()?;
        if s.settled || std::time::Instant::now() >= until {
            return Ok(s);
        }
        std::thread::sleep(SETTLE_STEP);
    }
}

/// Sends the lights and reads them back, sending again (a few times) if the keyboard didn't
/// take them; with `settle`, also until a split keyboard's other half agrees (re-sent half way
/// through [`SETTLE`] if it still hasn't). True once the readback shows them.
fn show(light: &dyn Lighting, want: &Want, settle: bool) -> Result<bool, DeviceError> {
    let send = || -> Result<(), DeviceError> {
        light.set_on(want.on)?;
        light.set_color(want.hue, want.sat, want.brightness)?;
        light.set_effect(Effect {
            id: want.effect.into(),
            params: vec![],
        })?;
        match want.speed.map(|s| light.set_speed(s)) {
            // Lights without a speed control have nothing to put back.
            Some(Err(DeviceError::Unsupported)) | None => Ok(()),
            Some(r) => r,
        }
    };
    send()?;
    let start = std::time::Instant::now();
    let mut resent = 0;
    loop {
        let Ok(s) = light.state() else {
            return Ok(false);
        };
        let left_ok = shows(&s, want);
        if left_ok && (!settle || s.settled) {
            return Ok(true);
        }
        let late_right = left_ok && start.elapsed() >= SETTLE / 2;
        if resent >= RESENDS || (settle && start.elapsed() >= SETTLE) || (!settle && left_ok) {
            return Ok(left_ok && !settle);
        }
        if !left_ok || late_right {
            resent += 1;
            std::thread::sleep(std::time::Duration::from_millis(100));
            send()?;
        }
        if settle {
            std::thread::sleep(SETTLE_STEP);
        }
    }
}

pub static ARBITER: Mutex<Option<Arbiter>> = Mutex::new(None);

pub fn with<R>(f: impl FnOnce(&mut Arbiter) -> R) -> R {
    let mut guard = ARBITER.lock().unwrap_or_else(|e| e.into_inner());
    let a = guard.get_or_insert_with(Arbiter::default);
    let r = f(a);
    // Every lease grant, release and pause passes through here: that's the event (#376).
    publish(a, &HOLDING, WAKE.get());
    r
}

/// One device's leases, the one showing first: each one's connection and priority (#376).
#[derive(Clone, Debug, PartialEq)]
pub struct Holding {
    pub device: String,
    pub leases: Vec<(u64, Priority)>,
}

/// Who holds the lights, and whether Foxglove is paused, as of the last lease change. The
/// Plugins tab reads this instead of the arbiter, whose lock a lease keeps while the keyboard
/// settles (seconds).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Holders {
    pub devices: Vec<Holding>,
    pub paused: bool,
}

static HOLDING: Mutex<Holders> = Mutex::new(Holders {
    devices: Vec::new(),
    paused: false,
});
/// Repaints the Plugins tab when the holders change. Set by the tab.
static WAKE: std::sync::OnceLock<crate::ui_wake::UiWake> = std::sync::OnceLock::new();

pub fn set_wake(wake: crate::ui_wake::UiWake) {
    let _ = WAKE.set(wake);
}

/// The lights' holders as of the last lease change (#376).
pub fn holding() -> Holders {
    HOLDING.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

impl Arbiter {
    /// Each leased device's leases, highest priority first and the newest first among equals,
    /// so the first one is the one showing (unless paused).
    pub fn holders(&self) -> Holders {
        let mut devices: Vec<String> = self.leases.iter().map(|l| l.device.clone()).collect();
        devices.sort();
        devices.dedup();
        let devices = devices
            .into_iter()
            .map(|device| {
                let mut leases: Vec<&Lease> =
                    self.leases.iter().filter(|l| l.device == device).collect();
                leases.sort_by_key(|l| std::cmp::Reverse((l.priority, l.id)));
                Holding {
                    leases: leases.iter().map(|l| (l.conn, l.priority)).collect(),
                    device,
                }
            })
            .collect();
        Holders {
            devices,
            paused: self.paused,
        }
    }
}

/// Stores `a`'s holders in `slot` and wakes the tab, if they changed. True when they did.
fn publish(a: &Arbiter, slot: &Mutex<Holders>, wake: Option<&crate::ui_wake::UiWake>) -> bool {
    let now = a.holders();
    let mut held = slot.lock().unwrap_or_else(|e| e.into_inner());
    if *held == now {
        return false;
    }
    *held = now;
    drop(held);
    if let Some(w) = wake {
        w.request_repaint();
    }
    true
}

/// Each leased device's winning lease: device, connection and priority (#371, diagnostics).
pub fn holders() -> Vec<(String, u64, Priority)> {
    with(|a| {
        let mut devices: Vec<String> = a.leases.iter().map(|l| l.device.clone()).collect();
        devices.sort();
        devices.dedup();
        devices
            .iter()
            .filter_map(|d| a.winner(d))
            .map(|l| (l.device.clone(), l.conn, l.priority))
            .collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// Lights in memory; `lose` drops that many writes.
    struct Fake {
        state: RefCell<LightingState>,
        lose: RefCell<u32>,
        /// Keeps the effect from changing (a keyboard that won't take it).
        stuck_effect: RefCell<bool>,
    }

    impl Fake {
        fn new() -> Fake {
            Fake {
                state: RefCell::new(LightingState {
                    on: false,
                    hue: 160,
                    sat: 50,
                    brightness: 40,
                    effect: Effect {
                        id: "3".into(),
                        params: vec![],
                    },
                    speed: 1,
                    settled: true,
                }),
                lose: RefCell::new(0),
                stuck_effect: RefCell::new(false),
            }
        }
        fn lost(&self) -> bool {
            let mut l = self.lose.borrow_mut();
            let lost = *l > 0;
            *l = l.saturating_sub(1);
            lost
        }
    }

    impl Lighting for Fake {
        fn layout(&self) -> foxglove_devices::Result<Vec<foxglove_devices::Zone>> {
            Ok(vec![])
        }
        fn state(&self) -> foxglove_devices::Result<LightingState> {
            Ok(self.state.borrow().clone())
        }
        fn set_on(&self, on: bool) -> foxglove_devices::Result<()> {
            if !self.lost() {
                self.state.borrow_mut().on = on;
            }
            Ok(())
        }
        fn set_color(&self, hue: u16, sat: u8, brightness: u8) -> foxglove_devices::Result<()> {
            let mut s = self.state.borrow_mut();
            (s.hue, s.sat, s.brightness) = (hue, sat, brightness);
            Ok(())
        }
        fn set_effect(&self, effect: Effect) -> foxglove_devices::Result<()> {
            if !*self.stuck_effect.borrow() {
                self.state.borrow_mut().effect = effect;
            }
            Ok(())
        }
        fn set_speed(&self, speed: u8) -> foxglove_devices::Result<()> {
            self.state.borrow_mut().speed = speed;
            Ok(())
        }
    }

    fn lease(conn: u64, priority: Priority, hue: u16) -> Lease {
        Lease {
            id: 0,
            conn,
            device: "kb".into(),
            priority,
            hue,
            sat: 100,
            brightness: 80,
            effect: "0".into(),
        }
    }

    fn hue(f: &Fake) -> (bool, u16) {
        let s = f.state.borrow();
        (s.on, s.hue)
    }

    fn all(f: &Fake) -> (bool, u16, u8, u8, String, u8) {
        let s = f.state.borrow();
        (
            s.on,
            s.hue,
            s.sat,
            s.brightness,
            s.effect.id.clone(),
            s.speed,
        )
    }

    #[test]
    fn pausing_gives_the_lights_back_and_resuming_shows_the_lease_again() {
        let kb = Fake::new();
        let before = all(&kb);
        let mut a = Arbiter::default();
        let mic = a.add(lease(1, Priority::High, 0), &kb).unwrap();
        assert_eq!(hue(&kb), (true, 0));
        for d in a.set_paused(true) {
            a.apply(&d, &kb).unwrap();
        }
        assert_eq!(all(&kb), before, "paused: the keyboard's own lights");
        assert!(a.set_paused(true).is_empty(), "already paused");
        // While paused, leases come and change without touching the lights.
        a.add(lease(2, Priority::Normal, 120), &kb).unwrap();
        assert_eq!(a.update(mic, 1, 30, 100, 80, "0"), Ok(None));
        assert_eq!(all(&kb), before);
        // Resumed: the winner (the updated mic lease) shows again.
        for d in a.set_paused(false) {
            a.apply(&d, &kb).unwrap();
        }
        assert_eq!(hue(&kb), (true, 30));
        // And when it all ends, the lights from before come back.
        for d in a.end(|_| true) {
            a.apply(&d, &kb).unwrap();
        }
        assert_eq!(all(&kb), before);
        assert!(a.base.is_empty());
    }

    #[test]
    fn a_lease_taken_while_paused_shows_on_resume_and_gives_back_what_was_there() {
        let kb = Fake::new();
        let before = all(&kb);
        let mut a = Arbiter::default();
        assert!(
            a.set_paused(true).is_empty(),
            "no leases: nothing to give back"
        );
        a.add(lease(1, Priority::Normal, 200), &kb).unwrap();
        assert_eq!(all(&kb), before, "nothing sent while paused");
        assert!(a.base.is_empty(), "no snapshot taken while paused");
        // A keyboard reconnecting while paused gets nothing either.
        a.connected("kb", &kb).unwrap();
        assert_eq!(all(&kb), before);
        for d in a.set_paused(false) {
            a.apply(&d, &kb).unwrap();
        }
        assert_eq!(hue(&kb), (true, 200));
        for d in a.end(|_| true) {
            a.apply(&d, &kb).unwrap();
        }
        assert_eq!(all(&kb), before);
    }

    #[test]
    fn highest_priority_shows_and_the_lights_come_back() {
        let kb = Fake::new();
        let mut a = Arbiter::default();
        let mic = a.add(lease(1, Priority::High, 0), &kb).unwrap(); // muted: red
        assert_eq!(hue(&kb), (true, 0));
        a.add(lease(2, Priority::Normal, 120), &kb).unwrap(); // recording: green, lower
        assert_eq!(hue(&kb), (true, 0), "the higher one stays");
        for d in a.end(|l| l.id == mic) {
            a.apply(&d, &kb).unwrap();
        }
        assert_eq!(hue(&kb), (true, 120), "the next one shows");
        // The other plugin's connection closes: back to the lights from before.
        for d in a.end(|l| l.conn == 2) {
            a.apply(&d, &kb).unwrap();
        }
        let s = kb.state.borrow().clone();
        assert_eq!(
            (
                s.on,
                s.hue,
                s.sat,
                s.brightness,
                s.effect.id.as_str(),
                s.speed
            ),
            (false, 160, 50, 40, "3", 1)
        );
        assert!(a.leases.is_empty() && a.base.is_empty());
    }

    #[test]
    fn speed_comes_back_and_a_restore_that_didnt_take_is_kept_for_the_next_connection() {
        let kb = Fake::new();
        let mut a = Arbiter::default();
        a.add(lease(1, Priority::Normal, 0), &kb).unwrap();
        kb.state.borrow_mut().speed = 4; // someone changed it meanwhile
                                         // The keyboard won't take the effect: the snapshot stays.
        *kb.stuck_effect.borrow_mut() = true;
        for d in a.end(|_| true) {
            a.apply(&d, &kb).unwrap();
        }
        assert_eq!(kb.state.borrow().speed, 1, "speed restored");
        assert!(
            a.base.contains_key("kb"),
            "kept: the effect didn't come back"
        );
        // A new lease meanwhile keeps the original, not the half-restored state.
        a.add(lease(2, Priority::Normal, 120), &kb).unwrap();
        assert_eq!(a.base["kb"].effect.id, "3");
        for d in a.end(|_| true) {
            a.apply(&d, &kb).unwrap();
        }
        // It connects again and takes it now.
        *kb.stuck_effect.borrow_mut() = false;
        a.connected("kb", &kb).unwrap();
        assert_eq!(kb.state.borrow().effect.id, "3");
        assert!(a.base.is_empty());
        // Nothing to do on a later connection.
        a.connected("kb", &kb).unwrap();
    }

    #[test]
    fn a_lease_changes_colour_in_place() {
        let kb = Fake::new();
        let mut a = Arbiter::default();
        let low = a.add(lease(1, Priority::Low, 0), &kb).unwrap();
        assert_eq!(
            a.update(low, 1, 120, 100, 80, "0").unwrap().as_deref(),
            Some("kb")
        );
        a.apply("kb", &kb).unwrap();
        assert_eq!(hue(&kb), (true, 120));
        assert_eq!(a.leases.len(), 1, "no new lease");
        // Under a higher lease it changes, but nothing is shown yet.
        a.add(lease(2, Priority::High, 300), &kb).unwrap();
        assert_eq!(a.update(low, 1, 60, 100, 80, "0").unwrap(), None);
        assert_eq!(hue(&kb), (true, 300));
        assert!(
            a.update(low, 2, 60, 100, 80, "0").is_err(),
            "only its own connection"
        );
    }

    /// The E1.31 receiver (#330) under mic mute (#210): its stream holds a low lease and changes
    /// it in place; a mute's high lease wins meanwhile; the stream's colour is back on unmute;
    /// and when the stream ends (terminated, or silent past 2.5 s: both end its lease) the
    /// lights are as they were before it.
    #[test]
    fn an_e131_stream_under_mic_mute_then_its_end_restores_the_lights() {
        const E131: u64 = 1;
        const MIC: u64 = 2;
        let kb = Fake::new();
        let mut a = Arbiter::default();
        let stream = a.add(lease(E131, Priority::Low, 0), &kb).unwrap(); // first frame: red
        assert_eq!(hue(&kb), (true, 0));
        for frame_hue in [30, 60] {
            let shown = a.update(stream, E131, frame_hue, 100, 100, "0").unwrap();
            a.apply(&shown.unwrap(), &kb).unwrap();
        }
        assert_eq!(hue(&kb), (true, 60), "frames change the lease in place");
        assert_eq!(a.leases.len(), 1);
        // Muted: the mic's high lease shows over the stream, which keeps changing underneath.
        let mute = a.add(lease(MIC, Priority::High, 0), &kb).unwrap();
        assert_eq!(hue(&kb), (true, 0));
        assert_eq!(
            a.update(stream, E131, 240, 100, 100, "0").unwrap(),
            None,
            "hidden under the mute"
        );
        assert_eq!(hue(&kb), (true, 0));
        // Unmuted: the stream's latest colour comes back.
        for d in a.end(|l| l.id == mute) {
            a.apply(&d, &kb).unwrap();
        }
        assert_eq!(hue(&kb), (true, 240));
        // The stream ends: the lights before it.
        for d in a.end(|l| l.conn == E131) {
            a.apply(&d, &kb).unwrap();
        }
        let s = kb.state.borrow().clone();
        assert_eq!(
            (
                s.on,
                s.hue,
                s.sat,
                s.brightness,
                s.effect.id.as_str(),
                s.speed
            ),
            (false, 160, 50, 40, "3", 1)
        );
        assert!(a.leases.is_empty() && a.base.is_empty());
    }

    #[test]
    fn a_mute_during_a_stream_that_ends_first_keeps_red_then_restores() {
        let kb = Fake::new();
        let mut a = Arbiter::default();
        a.add(lease(1, Priority::Low, 120), &kb).unwrap(); // the stream, green
        let mute = a.add(lease(2, Priority::High, 0), &kb).unwrap();
        // The stream goes silent while muted: its lease ends, the mute's red stays.
        for d in a.end(|l| l.conn == 1) {
            a.apply(&d, &kb).unwrap();
        }
        assert_eq!(hue(&kb), (true, 0));
        // Unmuted: no lease left, so the original lights, not the stream's green.
        for d in a.end(|l| l.id == mute) {
            a.apply(&d, &kb).unwrap();
        }
        assert_eq!(kb.state.borrow().hue, 160);
        assert!(!kb.state.borrow().on);
    }

    /// #376: a lease's grant and release change who the Plugins tab says holds the lights, and
    /// only a change wakes the tab.
    #[test]
    fn grants_and_releases_publish_who_holds_the_lights() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;
        let kb = Fake::new();
        let mut a = Arbiter::default();
        let slot = Mutex::new(Holders::default());
        let wakes = Arc::new(AtomicUsize::new(0));
        let w = wakes.clone();
        let wake = crate::ui_wake::UiWake::new(Arc::new(move || {
            w.fetch_add(1, Ordering::Relaxed);
        }));
        assert!(
            !publish(&a, &slot, Some(&wake)),
            "nothing held, nothing changed"
        );
        let stream = a.add(lease(1, Priority::Low, 120), &kb).unwrap(); // OpenRGB
        assert!(publish(&a, &slot, Some(&wake)));
        a.add(lease(2, Priority::High, 0), &kb).unwrap(); // Mic mute
        a.add(lease(3, Priority::Low, 30), &kb).unwrap(); // Pomodoro, newer than the stream
        assert!(publish(&a, &slot, Some(&wake)));
        let held = slot.lock().unwrap().clone();
        assert_eq!(held.devices.len(), 1);
        assert_eq!(
            held.devices[0].leases,
            [(2, Priority::High), (3, Priority::Low), (1, Priority::Low)],
            "the one showing first"
        );
        // A colour change in place isn't a change of holder.
        a.update(stream, 1, 60, 100, 80, "0").unwrap();
        assert!(!publish(&a, &slot, Some(&wake)));
        assert_eq!(wakes.load(Ordering::Relaxed), 2);
        for d in a.end(|l| l.conn == 2) {
            a.apply(&d, &kb).unwrap();
        }
        assert!(publish(&a, &slot, Some(&wake)));
        assert_eq!(
            slot.lock().unwrap().devices[0].leases[0],
            (3, Priority::Low)
        );
        a.set_paused(true);
        assert!(publish(&a, &slot, Some(&wake)), "paused is part of it");
        a.set_paused(false);
        for d in a.end(|_| true) {
            a.apply(&d, &kb).unwrap();
        }
        assert!(publish(&a, &slot, None));
        assert_eq!(
            *slot.lock().unwrap(),
            Holders::default(),
            "nobody: the lights are the user's"
        );
    }

    #[test]
    fn newest_wins_among_equals_and_lost_writes_are_sent_again() {
        let kb = Fake::new();
        let mut a = Arbiter::default();
        a.add(lease(1, Priority::Normal, 0), &kb).unwrap();
        *kb.lose.borrow_mut() = 1; // the keyboard misses the next "on"... (already on, so no harm)
        a.add(lease(2, Priority::Normal, 240), &kb).unwrap();
        assert_eq!(hue(&kb), (true, 240));
        // After the restore turned the lights off, a new lease whose "on" is lost still ends lit.
        for d in a.end(|_| true) {
            a.apply(&d, &kb).unwrap();
        }
        assert!(!kb.state.borrow().on);
        *kb.lose.borrow_mut() = 1;
        a.add(lease(3, Priority::Low, 60), &kb).unwrap();
        assert_eq!(hue(&kb), (true, 60));
    }
}
