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
//! - A lease may draw per-LED frames over its colour (a scene with SET_FRAME, #529): frames hide
//!   every colour the keyboard is sent, so they are handed back (`Lighting::release`) before
//!   anything else shows, and the lease is asked to draw again when it shows again.
//! - Colours the user sets while a lease holds the lights are what comes back at the end.
//! - The user's choice (what the Lights page or the followed accent set, [`Chosen`]) is the
//!   only source for what the keyboard shows when no lease holds it (#624, #625). A snapshot
//!   gets it laid over, so one taken while the LEDs were idle-dark or showed something else
//!   doesn't bring that back. A restore that didn't take is tried again on a later event only
//!   while the keyboard still shows nothing but what the arbiter wrote; once anything else
//!   changed it (a key, a tool, the user), the old snapshot is dropped and nothing is sent.

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
    /// Devices whose lights a lease draws as frames (#529).
    frames: HashMap<String, Frames>,
    /// Each device's lights as the user chose them (#624): the fields they set.
    chosen: HashMap<String, Chosen>,
    /// What the arbiter wrote to each device while its snapshot is kept (the leases' looks and
    /// each restore): a later retry is only safe while the keyboard shows nothing else.
    wrote: HashMap<String, Vec<Look>>,
}

/// The fields of a device's lights the user set (the Lights page, the followed accent), as
/// [`Arbiter::user_changed`] saw them. A field never set is `None`.
#[derive(Clone, Debug, Default, PartialEq)]
struct Chosen {
    on: Option<bool>,
    hue: Option<u16>,
    sat: Option<u8>,
    brightness: Option<u8>,
    effect: Option<String>,
    speed: Option<u8>,
}

impl Chosen {
    /// A state to run a change on: the chosen values, and out-of-range ones for fields never
    /// set, so every field the change sets is seen (the change runs once: it is `FnOnce`).
    fn probe(&self) -> LightingState {
        LightingState {
            on: self.on.unwrap_or(false),
            hue: self.hue.unwrap_or(u16::MAX),
            sat: self.sat.unwrap_or(u8::MAX),
            brightness: self.brightness.unwrap_or(u8::MAX),
            effect: Effect {
                id: self.effect.clone().unwrap_or_default(),
                params: vec![],
            },
            speed: self.speed.unwrap_or(u8::MAX),
            settled: true,
        }
    }

    /// The fields `after` (a [`Chosen::probe`] a change ran on) differs in. Setting a field to
    /// the value it already had changes nothing, so it needs no entry.
    fn changed(&self, after: &LightingState) -> Chosen {
        let before = self.probe();
        Chosen {
            on: (before.on != after.on).then_some(after.on),
            hue: (before.hue != after.hue).then_some(after.hue),
            sat: (before.sat != after.sat).then_some(after.sat),
            brightness: (before.brightness != after.brightness).then_some(after.brightness),
            effect: (before.effect.id != after.effect.id).then(|| after.effect.id.clone()),
            speed: (before.speed != after.speed).then_some(after.speed),
        }
    }

    /// Takes every field `other` sets.
    fn merge(&mut self, other: &Chosen) {
        self.on = other.on.or(self.on);
        self.hue = other.hue.or(self.hue);
        self.sat = other.sat.or(self.sat);
        self.brightness = other.brightness.or(self.brightness);
        self.effect = other.effect.clone().or(self.effect.take());
        self.speed = other.speed.or(self.speed);
    }

    /// Lays the chosen fields over `s`.
    fn overlay(&self, s: &mut LightingState) {
        s.on = self.on.unwrap_or(s.on);
        s.hue = self.hue.unwrap_or(s.hue);
        s.sat = self.sat.unwrap_or(s.sat);
        s.brightness = self.brightness.unwrap_or(s.brightness);
        if let Some(e) = &self.effect {
            s.effect.id.clone_from(e);
        }
        s.speed = self.speed.unwrap_or(s.speed);
    }
}

/// One look the arbiter wrote (a lease's, or a restore); `speed` only for a restore.
#[derive(Clone, Debug, PartialEq)]
struct Look {
    on: bool,
    hue: u16,
    sat: u8,
    brightness: u8,
    effect: String,
    speed: Option<u8>,
}

/// At most this many looks are kept per device (`Arbiter::wrote`): enough for a lease
/// stack's changes; the oldest go first.
const WROTE_KEPT: usize = 16;

/// A lease that draws frames over its colour.
struct Frames {
    lease: u64,
    /// Frames may be showing: a frame went out since the last hand-back.
    held: bool,
    /// Asks the lease's owner for a frame again (its lease shows again).
    again: Box<dyn Fn() + Send>,
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
            let snapshot = self.snapshot(&lease.device, light)?;
            self.base.insert(lease.device.clone(), snapshot);
        }
        self.next += 1;
        lease.id = self.next;
        let (id, device) = (lease.id, lease.device.clone());
        self.leases.push(lease);
        self.apply(&device, light)?;
        Ok(id)
    }

    /// The device's lights from before its first lease, kept now if no lease holds it yet:
    /// what comes back when the last one ends. A lease that only draws frames over its colour
    /// takes this as its colour, so a hand-back mid-way shows the user's own lights and ending
    /// it changes nothing (the LED map's dot test).
    pub fn lights_before(
        &mut self,
        device: &str,
        light: &dyn Lighting,
    ) -> Result<LightingState, DeviceError> {
        if let Some(base) = self.base.get(device) {
            return Ok(base.clone());
        }
        let now = self.snapshot(device, light)?;
        if !self.paused {
            self.base.insert(device.to_string(), now.clone());
        }
        Ok(now)
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
            let snapshot = self.snapshot(device, light)?;
            self.base.insert(device.to_string(), snapshot);
        }
        let showing = match self.target(device) {
            Target::Lease(l) => Some(l.id),
            _ => None,
        };
        // Frames hide every colour: they go before anything else shows.
        self.hand_back_frames(device, showing, light);
        let shown = match self.target(device) {
            Target::Lease(l) => {
                let want = Want::lease(l);
                let look = want.look();
                let shown = show(light, &want, false).map(|_| ());
                self.wrote_look(device, look);
                shown
            }
            // Giving the lights back waits for both halves of a split keyboard.
            Target::Restore(s) => {
                let want = Want::restore(&s);
                self.wrote_look(device, want.look());
                if show(light, &want, true)? {
                    self.forget_restore(device);
                } else {
                    eprintln!("Foxglove: {device}'s lights didn't come back as they were; trying again when it next connects");
                }
                Ok(())
            }
            Target::Nothing => Ok(()),
        };
        self.frames_again(device, showing);
        shown
    }

    /// The device (re)connected: a restore that didn't take is tried again (#293), only while
    /// the keyboard shows nothing but what the arbiter wrote (`Arbiter::retry_restore`).
    /// Started by the connection, stopped by the readback or [`RESENDS`]. A lease drawing
    /// frames draws again (an unplug hands them back).
    pub fn connected(&mut self, device: &str, light: &dyn Lighting) -> Result<(), DeviceError> {
        if (self.paused || self.winner(device).is_none()) && self.base.contains_key(device) {
            return self
                .retry_restore(device, light, "keyboard connected")
                .map(|_| ());
        }
        if let (Some(f), Some(w)) = (self.frames.get(device), self.winner(device)) {
            if f.lease == w.id && !self.paused {
                (f.again)();
            }
        }
        Ok(())
    }

    /// The devices whose lights the arbiter owns now: leased, or with a restore to finish.
    pub fn devices(&self) -> Vec<String> {
        let mut devices: Vec<String> = self
            .leases
            .iter()
            .map(|l| l.device.clone())
            .chain(self.base.keys().cloned())
            .collect();
        devices.sort();
        devices.dedup();
        devices
    }

    /// A re-apply (`reassert.rs`, #618): the winning lease shows again when `device`'s lights
    /// don't show it (a reboot or a re-link lost it), and a restore that didn't take is tried
    /// again, unless the lights already show it. A keyboard connecting retries the restore
    /// through [`Arbiter::connected`], so `connected` says not to here. Leases drawing frames
    /// keep their own path. True when something was sent.
    pub fn reassert(
        &mut self,
        device: &str,
        light: &dyn Lighting,
        connected: bool,
    ) -> Result<bool, DeviceError> {
        if self.frames.contains_key(device) {
            return Ok(false);
        }
        match self.target(device) {
            Target::Lease(l) => {
                if light.state().is_ok_and(|s| shows(&s, &Want::lease(l))) {
                    return Ok(false);
                }
            }
            Target::Restore(_) if connected => return Ok(false),
            // The same rule as a connect: never an old snapshot over a changed keyboard.
            Target::Restore(_) => return self.retry_restore(device, light, "re-apply"),
            Target::Nothing => return Ok(false),
        }
        self.apply(device, light).map(|()| true)
    }

    /// A snapshot of `device`'s lights for a lease: as they read once settled, with the user's
    /// choice laid over (#624). A read while idle-dark (off), under a left-running effect, or
    /// with a half behind doesn't become what comes back.
    fn snapshot(&self, device: &str, light: &dyn Lighting) -> Result<LightingState, DeviceError> {
        let mut s = settled(light)?;
        if let Some(c) = self.chosen.get(device) {
            c.overlay(&mut s);
        }
        Ok(s)
    }

    /// Notes a look sent to `device` (a lease's or a restore), for `Arbiter::only_ours`.
    fn wrote_look(&mut self, device: &str, look: Look) {
        let wrote = self.wrote.entry(device.to_string()).or_default();
        if wrote.last() != Some(&look) {
            wrote.push(look);
        }
        if wrote.len() > WROTE_KEPT {
            wrote.remove(0);
        }
    }

    /// The restore is done (or dropped): no snapshot and no record of writes are kept.
    fn forget_restore(&mut self, device: &str) {
        self.base.remove(device);
        self.wrote.remove(device);
    }

    /// Whether the keyboard shows nothing but the arbiter's writes to `device` while its
    /// snapshot was kept: one look it wrote, whole (a lease's colour left on), or a restore
    /// that half took (the snapshot's on/off, and each other field one the arbiter wrote or
    /// the snapshot's own). Anything else means someone changed the keyboard since.
    fn only_ours(&self, device: &str, s: &LightingState, base: &LightingState) -> bool {
        let near = |a: u16, b: u16, wrap: u16| {
            let d = a.abs_diff(b);
            d.min(wrap.saturating_sub(d)) <= 2
        };
        let base = Want::restore(base).look();
        let wrote = self.wrote.get(device).map_or(&[][..], Vec::as_slice);
        let looks = || wrote.iter().chain(std::iter::once(&base));
        let whole = |l: &Look| {
            l.on == s.on
                && near(l.hue, s.hue, 360)
                && near(l.sat.into(), s.sat.into(), 1000)
                && near(l.brightness.into(), s.brightness.into(), 1000)
                && l.effect == s.effect.id
                && l.speed.or(base.speed) == Some(s.speed)
        };
        if looks().any(whole) {
            return true;
        }
        // The restore sends on/off first: a half-taken one shows the snapshot's.
        s.on == base.on
            && looks().any(|l| near(l.hue, s.hue, 360))
            && looks().any(|l| near(l.sat.into(), s.sat.into(), 1000))
            && looks().any(|l| near(l.brightness.into(), s.brightness.into(), 1000))
            && looks().any(|l| l.effect == s.effect.id)
            && looks().any(|l| l.speed == Some(s.speed))
    }

    /// A restore that didn't take, tried again on a later event (`why`, for the log): sent
    /// (the snapshot with the user's choice over it, logged) only while the keyboard shows
    /// nothing but the arbiter's own writes. A keyboard that already shows it, or that
    /// something else changed since (#625: the user's own look), is left alone and the
    /// snapshot dropped. True when something was sent.
    fn retry_restore(
        &mut self,
        device: &str,
        light: &dyn Lighting,
        why: &str,
    ) -> Result<bool, DeviceError> {
        let Some(base) = self.base.get(device).cloned() else {
            return Ok(false);
        };
        let s = light.state()?;
        if s.settled && shows(&s, &Want::restore(&base)) {
            self.forget_restore(device);
            return Ok(false);
        }
        if !self.only_ours(device, &s, &base) {
            eprintln!(
                "Foxglove: {device}'s lights changed since a lease ended; its old snapshot is dropped, nothing sent ({why})"
            );
            self.forget_restore(device);
            return Ok(false);
        }
        eprintln!(
            "Foxglove: giving back {device}'s lights after a lease ({why}): {} · effect {} · {} % · hue {} · sat {} · speed {}",
            if base.on { "on" } else { "off" },
            base.effect.id,
            base.brightness,
            base.hue,
            base.sat,
            base.speed
        );
        self.apply(device, light).map(|()| true)
    }

    /// Lease `id` draws frames on its device (#529). `again` asks it for a frame once its lease
    /// shows again (a notification ended, Foxglove resumed, the keyboard reconnected); it runs
    /// under the arbiter's lock, so it must only flag work.
    pub fn draws_frames(&mut self, id: u64, again: impl Fn() + Send + 'static) {
        let Some(device) = self
            .leases
            .iter()
            .find(|l| l.id == id)
            .map(|l| l.device.clone())
        else {
            return;
        };
        self.frames.insert(
            device,
            Frames {
                lease: id,
                held: false,
                again: Box::new(again),
            },
        );
    }

    /// Whether lease `id` is the one its device shows now (and Foxglove isn't paused).
    pub fn shows(&self, id: u64) -> bool {
        !self.paused
            && self
                .leases
                .iter()
                .find(|l| l.id == id)
                .and_then(|l| self.winner(&l.device))
                .is_some_and(|w| w.id == id)
    }

    /// Whether a lease's frames (or effect) may be showing on `device` now: sent and not handed
    /// back since.
    pub fn holds(&self, device: &str) -> bool {
        self.frames.get(device).is_some_and(|f| f.held)
    }

    /// A frame went out on `device`: it hides the keyboard's colour until handed back.
    pub fn frame_sent(&mut self, device: &str) {
        if let Some(f) = self.frames.get_mut(device) {
            f.held = true;
        }
    }

    /// Hands `device`'s frames back now (the user is away, or the keyboard has none): its
    /// lease's colour shows instead. The lease keeps drawing frames when asked again.
    pub fn release_frames(&mut self, device: &str, light: &dyn Lighting) {
        let Some(f) = self.frames.get(device) else {
            return;
        };
        let (held, lease) = (f.held, f.lease);
        self.hand_back_frames(device, None, light);
        // Frames changed the lease's colour without sending it: the keyboard's own colour is
        // the one last shown. Show the lease's colour now.
        let current = self
            .winner(device)
            .filter(|w| w.id == lease && !self.paused)
            .cloned();
        if let (true, Some(w)) = (held, current) {
            let want = Want::lease(&w);
            self.wrote_look(device, want.look());
            if let Err(e) = show(light, &want, false) {
                eprintln!("Foxglove: {device}'s colour after its frames: {e}");
            }
        }
    }

    /// The device draws no frames after all (firmware without SET_FRAME).
    pub fn no_frames(&mut self, device: &str) {
        self.frames.remove(device);
    }

    /// Releases frames that may be showing unless `showing` is the lease drawing them; forgets
    /// a lease that ended.
    fn hand_back_frames(&mut self, device: &str, showing: Option<u64>, light: &dyn Lighting) {
        let Some(f) = self.frames.get_mut(device) else {
            return;
        };
        if f.held && showing != Some(f.lease) {
            // Not taken back (a lost reply): the keyboard's own timeout hands them back.
            if let Err(e) = light.release() {
                eprintln!("Foxglove: handing {device}'s frames back: {e}");
            }
            f.held = false;
        }
        let lease = f.lease;
        if !self.leases.iter().any(|l| l.id == lease) {
            self.frames.remove(device);
        }
    }

    /// The lease drawing frames shows again without them: ask it for one.
    fn frames_again(&self, device: &str, showing: Option<u64>) {
        if let Some(f) = self.frames.get(device) {
            if !f.held && showing == Some(f.lease) {
                (f.again)();
            }
        }
    }

    /// A colour the lights follow (the wallpaper's accent, Windows' accent, #70, #93) for
    /// `device`: its hue and saturation go on now, at `brightness` (the lights' own, read by the
    /// caller outside the lock), unless a lease holds them (a running scene, a notification),
    /// which wins: then they are only what comes back when it ends. True when sent.
    pub fn follow(
        &mut self,
        device: &str,
        (hue, sat, brightness): (u16, u8, u8),
        light: &dyn Lighting,
    ) -> Result<bool, DeviceError> {
        self.user_changed(device, |s| (s.hue, s.sat) = (hue, sat));
        if self.leases.iter().any(|l| l.device == device) {
            return Ok(false);
        }
        light.set_color(hue, sat, brightness.max(1))?;
        Ok(true)
    }

    /// The user changed `device`'s lights (the Lights page, the followed accent): their
    /// choice from now on (#624), and, while leases hold the lights, what comes back when the
    /// last one ends, as for the case's colours under a scene.
    /// The looks the user chose (#670): each device's effect and colour fields they set.
    pub fn saved_looks(&self) -> Vec<(String, super::effect_controls::Saved)> {
        let mut out: Vec<_> = self
            .chosen
            .iter()
            .map(|(device, c)| {
                let saved = super::effect_controls::Saved {
                    effect: c.effect.as_deref().and_then(|e| e.parse().ok()),
                    hue: c.hue,
                    sat: c.sat,
                };
                (device.clone(), saved)
            })
            .filter(|(_, s)| *s != super::effect_controls::Saved::default())
            .collect();
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    pub fn user_changed(&mut self, device: &str, change: impl FnOnce(&mut LightingState)) {
        let chosen = self.chosen.entry(device.to_string()).or_default();
        let mut probe = chosen.probe();
        change(&mut probe);
        let set = chosen.changed(&probe);
        chosen.merge(&set);
        if let Some(base) = self.base.get_mut(device) {
            set.overlay(base);
        }
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

    fn look(&self) -> Look {
        Look {
            on: self.on,
            hue: self.hue,
            sat: self.sat,
            brightness: self.brightness,
            effect: self.effect.to_string(),
            speed: self.speed,
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

/// Registers the lights' re-apply hook (`reassert.rs`): each trigger goes to a thread that
/// sleeps on its channel and runs [`Arbiter::reassert`] for every device the arbiter owns.
pub fn register_reassert() {
    use super::reassert::{self, Trigger};
    let (tx, rx) = std::sync::mpsc::channel::<Trigger>();
    let tx = Mutex::new(tx);
    reassert::register("lights", move |t| {
        let _ = tx.lock().unwrap_or_else(|e| e.into_inner()).send(t);
    });
    super::services::spawn_named("reassert-lights", move || {
        for t in rx {
            with(|a| {
                for device in a.devices() {
                    let shown = with_light(&device, |light| reassert_on(a, &device, light, t));
                    match shown {
                        Some(Ok(true)) => {
                            eprintln!("{}", reassert::line(&format!("{device}'s lights"), t));
                        }
                        Some(Err(e)) => eprintln!(
                            "Foxglove: re-applying {device}'s lights ({}): {}",
                            t.label(),
                            super::devices::describe(&e)
                        ),
                        _ => {}
                    }
                }
            });
        }
    });
}

/// The lights hook's work for one device and trigger (#622): a connect leaves a pending
/// restore to [`Arbiter::connected`], which the device bus's Connected event runs (`api.rs`).
fn reassert_on(
    a: &mut Arbiter,
    device: &str,
    light: &dyn Lighting,
    t: super::reassert::Trigger,
) -> Result<bool, DeviceError> {
    a.reassert(
        device,
        light,
        t == super::reassert::Trigger::KeyboardConnected,
    )
}

/// Runs `f` on the lights of device `id`: the host-link keyboard or a VIA one.
fn with_light<R>(id: &str, f: impl FnOnce(&dyn Lighting) -> R) -> Option<R> {
    use foxglove_devices::Device as _;
    if let Some(d) = super::devices::device().filter(|d| d.info().id.to_string() == id) {
        return d.lighting().map(f);
    }
    super::via_host::with_device(id, |d| d.lighting().map(f)).flatten()
}

pub static ARBITER: Mutex<Option<Arbiter>> = Mutex::new(None);

pub fn with<R>(f: impl FnOnce(&mut Arbiter) -> R) -> R {
    let mut guard = ARBITER.lock().unwrap_or_else(|e| e.into_inner());
    let a = guard.get_or_insert_with(Arbiter::default);
    let r = f(a);
    // Every lease grant, release and pause passes through here: that's the event (#376).
    publish(a, &HOLDING, WAKE.get());
    // So does every choice of the user's: the Lights page reads it without this lock (#670).
    let saved = a.saved_looks();
    *SAVED.lock().unwrap_or_else(|e| e.into_inner()) = saved;
    r
}

/// Each device's saved look as of the last pass through [`with`] (#670).
static SAVED: Mutex<Vec<(String, super::effect_controls::Saved)>> = Mutex::new(Vec::new());

/// The look the user chose for `device` (the Lights page, the followed accent), without the
/// arbiter's lock: the Lights page names it when the keyboard shows something else.
pub fn saved(device: &str) -> Option<super::effect_controls::Saved> {
    SAVED
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .find(|(d, _)| d == device)
        .map(|(_, s)| s.clone())
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
        /// Every write, in order (#624: what a connect sent).
        log: RefCell<Vec<String>>,
        /// Unplugged: every write fails.
        offline: RefCell<bool>,
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
                log: RefCell::new(Vec::new()),
                offline: RefCell::new(false),
            }
        }
        /// Notes a write; an unplugged keyboard refuses it.
        fn write(&self, what: String) -> foxglove_devices::Result<()> {
            if *self.offline.borrow() {
                return Err(DeviceError::Disconnected);
            }
            self.log.borrow_mut().push(what);
            Ok(())
        }
        /// The writes so far, emptied.
        fn writes(&self) -> Vec<String> {
            std::mem::take(&mut *self.log.borrow_mut())
        }
        /// Sets the whole look, as a key or another tool would (no write is logged).
        fn set(&self, on: bool, (hue, sat, brightness): (u16, u8, u8), effect: &str, speed: u8) {
            let mut s = self.state.borrow_mut();
            (s.on, s.hue, s.sat, s.brightness, s.speed) = (on, hue, sat, brightness, speed);
            s.effect.id = effect.into();
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
            self.write(format!("on {on}"))?;
            if !self.lost() {
                self.state.borrow_mut().on = on;
            }
            Ok(())
        }
        fn set_color(&self, hue: u16, sat: u8, brightness: u8) -> foxglove_devices::Result<()> {
            self.write(format!("colour {hue} {sat} {brightness}"))?;
            let mut s = self.state.borrow_mut();
            (s.hue, s.sat, s.brightness) = (hue, sat, brightness);
            Ok(())
        }
        fn set_effect(&self, effect: Effect) -> foxglove_devices::Result<()> {
            self.write(format!("effect {}", effect.id))?;
            if !*self.stuck_effect.borrow() {
                self.state.borrow_mut().effect = effect;
            }
            Ok(())
        }
        fn set_speed(&self, speed: u8) -> foxglove_devices::Result<()> {
            self.write(format!("speed {speed}"))?;
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
    fn what_the_user_sets_under_a_lease_comes_back_at_its_end() {
        let kb = Fake::new();
        let mut a = Arbiter::default();
        a.add(lease(1, Priority::Low, 30), &kb).unwrap();
        // The Lights page: another colour and effect while the lease holds the lights.
        a.user_changed("kb", |s| {
            (s.hue, s.sat, s.brightness) = (300, 20, 70);
            s.effect.id = "1".into();
        });
        a.user_changed("mouse", |s| s.hue = 1);
        for d in a.end(|_| true) {
            a.apply(&d, &kb).unwrap();
        }
        assert_eq!(all(&kb), (false, 300, 20, 70, "1".into(), 1));
        // No lease: nothing kept, nothing changes.
        a.user_changed("kb", |s| s.hue = 5);
        assert!(a.base.is_empty());
    }

    /// #70, #93: a followed accent goes on at the lights' own brightness; under a lease (a
    /// running scene) it waits, and is what comes back when the lease ends.
    #[test]
    fn a_followed_colour_waits_for_a_lease_to_end() {
        let kb = Fake::new();
        let mut a = Arbiter::default();
        assert_eq!(a.follow("kb", (180, 90, 40), &kb), Ok(true));
        assert_eq!(
            all(&kb),
            (false, 180, 90, 40, "3".into(), 1),
            "brightness kept"
        );
        let scene = a.add(lease(7, Priority::Low, 30), &kb).unwrap();
        assert_eq!(hue(&kb), (true, 30));
        assert_eq!(
            a.follow("kb", (210, 70, 40), &kb),
            Ok(false),
            "the scene wins"
        );
        assert_eq!(hue(&kb), (true, 30));
        for d in a.end(|l| l.id == scene) {
            a.apply(&d, &kb).unwrap();
        }
        assert_eq!(all(&kb), (false, 210, 70, 40, "3".into(), 1));
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

    /// A re-apply (#618) shows the lease again only when the keyboard lost it, and retries a
    /// restore that didn't take, except on a connect (which `connected` covers).
    #[test]
    fn a_reassert_puts_back_only_what_the_keyboard_lost() {
        let kb = Fake::new();
        let mut a = Arbiter::default();
        assert_eq!(a.devices(), Vec::<String>::new());
        a.add(lease(1, Priority::Normal, 0), &kb).unwrap();
        assert_eq!(a.devices(), ["kb"]);
        assert!(!a.reassert("kb", &kb, false).unwrap(), "shows it already");
        // The keyboard rebooted to its saved lights.
        {
            let mut s = kb.state.borrow_mut();
            (s.on, s.hue) = (false, 160);
        }
        assert!(a.reassert("kb", &kb, false).unwrap());
        assert_eq!(hue(&kb), (true, 0));
        for connected in [true, false, false] {
            assert!(!a.reassert("kb", &kb, connected).unwrap(), "no spam");
        }
        // A restore that didn't take: a connect leaves it to `connected`, others retry it.
        *kb.stuck_effect.borrow_mut() = true;
        for d in a.end(|_| true) {
            a.apply(&d, &kb).unwrap();
        }
        assert!(a.base.contains_key("kb"));
        assert!(!a.reassert("kb", &kb, true).unwrap());
        *kb.stuck_effect.borrow_mut() = false;
        assert!(a.reassert("kb", &kb, false).unwrap());
        assert!(a.base.is_empty(), "restored");
        assert!(!a.reassert("kb", &kb, false).unwrap(), "nothing left to do");
        assert_eq!(a.devices(), Vec::<String>::new());
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
    /// The #625 look on file: off · Spectrum (2) · hue 4 · sat 100 · 9 % · speed 3.
    const STALE: (bool, (u16, u8, u8), &str, u8) = (false, (4, 100, 9), "2", 3);
    /// The user's: on · Swirl (3) · the accent 39/70 · 42 % · speed 3.
    const USERS: (bool, (u16, u8, u8), &str, u8) = (true, (39, 70, 42), "3", 3);

    fn look(f: &Fake) -> (bool, (u16, u8, u8), String, u8) {
        let s = f.state.borrow();
        (
            s.on,
            (s.hue, s.sat, s.brightness),
            s.effect.id.clone(),
            s.speed,
        )
    }

    fn users() -> (bool, (u16, u8, u8), String, u8) {
        (USERS.0, USERS.1, USERS.2.into(), USERS.3)
    }

    /// A lease taken while the keyboard read the #625 look, whose restore didn't take (the
    /// effect stuck), so that look is kept for a later retry.
    fn stale_snapshot_on_file(kb: &Fake, a: &mut Arbiter) {
        kb.set(STALE.0, STALE.1, STALE.2, STALE.3);
        a.add(lease(1, Priority::Low, 0), kb).unwrap();
        *kb.stuck_effect.borrow_mut() = true;
        for d in a.end(|_| true) {
            a.apply(&d, kb).unwrap();
        }
        *kb.stuck_effect.borrow_mut() = false;
        assert_eq!(a.base["kb"].effect.id, "2", "the stale snapshot is on file");
        kb.writes();
    }

    /// #625, exactly: the keyboard is at the user's on · Swirl · 42 % (set on the keyboard,
    /// not through the app), with a stale off · Spectrum · 9 % snapshot on file. It connects,
    /// then the restore runs: nothing is written, and the effect and brightness stay.
    #[test]
    fn the_625_connect_and_restore_write_nothing_over_the_users_look() {
        let kb = Fake::new();
        let mut a = Arbiter::default();
        stale_snapshot_on_file(&kb, &mut a);
        kb.set(USERS.0, USERS.1, USERS.2, USERS.3);
        a.connected("kb", &kb).unwrap();
        assert_eq!(kb.writes(), Vec::<String>::new(), "nothing written");
        assert_eq!(look(&kb), users(), "effect and brightness kept");
        assert!(a.base.is_empty(), "the stale snapshot is dropped");
        // A later connect has nothing left to send.
        a.connected("kb", &kb).unwrap();
        assert_eq!(kb.writes(), Vec::<String>::new());

        // The same through the Lights page: the user's choice goes into the snapshot, and the
        // keyboard that shows it gets nothing.
        let kb = Fake::new();
        let mut a = Arbiter::default();
        stale_snapshot_on_file(&kb, &mut a);
        kb.set(USERS.0, USERS.1, USERS.2, USERS.3);
        a.user_changed("kb", |s| s.on = true);
        a.user_changed("kb", |s| (s.hue, s.sat, s.brightness) = (39, 70, 42));
        a.user_changed("kb", |s| s.effect.id = "3".into());
        a.connected("kb", &kb).unwrap();
        assert_eq!(kb.writes(), Vec::<String>::new());
        assert_eq!(look(&kb), users());
        assert!(a.base.is_empty());
    }

    /// #622's re-apply hooks (connect, relay ready, PC back, display change, taskbar restart),
    /// in any order and repeated, can't replay a stale snapshot through the lights hook.
    #[test]
    fn the_reapply_hooks_never_replay_a_stale_snapshot() {
        use super::super::reassert::Trigger;
        // The relay comes up first (the #624 re-link), then the rest, twice.
        let mut order = vec![Trigger::RightRelayReady];
        order.extend(Trigger::ALL);
        order.extend(Trigger::ALL);
        for first in Trigger::ALL {
            let kb = Fake::new();
            let mut a = Arbiter::default();
            stale_snapshot_on_file(&kb, &mut a);
            kb.set(USERS.0, USERS.1, USERS.2, USERS.3);
            for &t in std::iter::once(&first).chain(&order) {
                for device in a.devices() {
                    assert_eq!(reassert_on(&mut a, &device, &kb, t), Ok(false), "{t:?}");
                }
                if t == Trigger::KeyboardConnected {
                    a.connected("kb", &kb).unwrap();
                }
            }
            assert_eq!(kb.writes(), Vec::<String>::new(), "first {first:?}");
            assert_eq!(look(&kb), users());
            assert_eq!(
                a.devices(),
                Vec::<String>::new(),
                "nothing left to re-apply"
            );
        }
    }

    /// #624: a snapshot read while the LEDs were idle-dark (off), dimmed, or under an effect a
    /// scene left running gets the user's choice laid over; its off · Spectrum · 9 % is never
    /// written, on the lease's end or on a connect.
    #[test]
    fn a_snapshot_taken_while_idle_dimmed_or_under_a_scene_is_never_written() {
        let kb = Fake::new();
        let mut a = Arbiter::default();
        // The Lights page: on · Swirl · 42 %, before any lease.
        kb.set(USERS.0, USERS.1, USERS.2, USERS.3);
        a.user_changed("kb", |s| s.on = true);
        a.user_changed("kb", |s| (s.hue, s.sat, s.brightness) = (39, 70, 42));
        a.user_changed("kb", |s| s.effect.id = "3".into());
        assert_eq!(a.chosen["kb"].brightness, Some(42));
        // Idle-dark, at a dim, under a scene's Spectrum: the keyboard reads the #625 look.
        kb.set(STALE.0, STALE.1, STALE.2, STALE.3);
        a.add(lease(1, Priority::Low, 0), &kb).unwrap();
        assert_eq!(
            (
                a.base["kb"].on,
                a.base["kb"].brightness,
                a.base["kb"].effect.id.as_str()
            ),
            (true, 42, "3"),
            "the snapshot is the user's choice"
        );
        // Unplugged when the lease ends: the restore doesn't take, the lease's red stays.
        *kb.offline.borrow_mut() = true;
        for d in a.end(|_| true) {
            assert!(a.apply(&d, &kb).is_err());
        }
        *kb.offline.borrow_mut() = false;
        kb.writes();
        a.connected("kb", &kb).unwrap();
        let sent = kb.writes();
        for stale in ["on false", "effect 2", "colour 4 100 9"] {
            assert!(!sent.contains(&stale.to_string()), "{stale} in {sent:?}");
        }
        assert_eq!(look(&kb), users());
    }

    /// A connect with no lease: a keyboard that matches (or one no lease ever touched) gets
    /// nothing; one a lease left its colour on gets the user's choice, once, and then nothing.
    #[test]
    fn a_connect_sends_the_users_choice_only_over_a_leases_leftover_and_once() {
        let kb = Fake::new();
        let mut a = Arbiter::default();
        a.user_changed("kb", |s| s.on = true);
        a.user_changed("kb", |s| (s.hue, s.sat, s.brightness) = (39, 70, 42));
        a.user_changed("kb", |s| s.effect.id = "3".into());
        // No lease ever: the keyboard is the user's to change (keys, other tools).
        kb.set(STALE.0, STALE.1, STALE.2, STALE.3);
        a.connected("kb", &kb).unwrap();
        assert_eq!(kb.writes(), Vec::<String>::new(), "no lease, nothing sent");

        // A restore pending, and the keyboard already shows it: nothing.
        kb.set(USERS.0, USERS.1, USERS.2, USERS.3);
        a.add(lease(1, Priority::Normal, 0), &kb).unwrap();
        *kb.offline.borrow_mut() = true;
        for d in a.end(|_| true) {
            assert!(a.apply(&d, &kb).is_err());
        }
        *kb.offline.borrow_mut() = false;
        kb.set(USERS.0, USERS.1, USERS.2, USERS.3);
        kb.writes();
        a.connected("kb", &kb).unwrap();
        assert_eq!(kb.writes(), Vec::<String>::new(), "matches: nothing sent");
        assert!(a.base.is_empty());

        // A restore pending, and the keyboard still shows the lease's red: the user's choice
        // goes out once.
        a.add(lease(1, Priority::Normal, 0), &kb).unwrap();
        *kb.offline.borrow_mut() = true;
        for d in a.end(|_| true) {
            assert!(a.apply(&d, &kb).is_err());
        }
        *kb.offline.borrow_mut() = false;
        assert_eq!(hue(&kb), (true, 0), "the lease's red is left on");
        kb.writes();
        a.connected("kb", &kb).unwrap();
        assert_eq!(
            kb.writes(),
            ["on true", "colour 39 70 42", "effect 3", "speed 3"],
            "the user's choice, once"
        );
        assert_eq!(look(&kb), users());
        a.connected("kb", &kb).unwrap();
        assert_eq!(kb.writes(), Vec::<String>::new(), "then nothing");
    }

    /// A key that turns the lights back on after a restore half took (the on/off differs from
    /// the snapshot) counts as the user's: the old snapshot isn't sent.
    #[test]
    fn a_key_after_a_half_taken_restore_is_left_alone() {
        let kb = Fake::new();
        let mut a = Arbiter::default();
        stale_snapshot_on_file(&kb, &mut a);
        // The restore turned the lights off; the user turns them on with a key.
        kb.state.borrow_mut().on = true;
        a.connected("kb", &kb).unwrap();
        assert_eq!(kb.writes(), Vec::<String>::new());
        assert!(kb.state.borrow().on);
    }

    /// The Lights page's changes are the user's choice field by field, also when set back.
    #[test]
    fn the_users_choice_is_kept_field_by_field() {
        let mut a = Arbiter::default();
        a.user_changed("kb", |s| s.on = true);
        a.user_changed("kb", |s| s.speed = 4);
        a.user_changed("kb", |s| s.on = false);
        a.user_changed("kb", |s| s.on = false);
        let c = &a.chosen["kb"];
        assert_eq!(
            (c.on, c.speed, c.hue, c.effect.as_deref()),
            (Some(false), Some(4), None, None)
        );
    }

    /// #670: the saved look the Lights page names is the user's effect and colour fields, per
    /// device; brightness or on/off alone save no look.
    #[test]
    fn the_saved_look_is_the_users_effect_and_colour() {
        use crate::foxglove::effect_controls::Saved;
        let mut a = Arbiter::default();
        assert!(a.saved_looks().is_empty());
        a.user_changed("mouse", |s| s.brightness = 20);
        a.user_changed("kb", |s| (s.hue, s.sat) = (39, 70));
        a.user_changed("kb", |s| s.effect.id = "0".into());
        assert_eq!(
            a.saved_looks(),
            vec![(
                "kb".to_string(),
                Saved {
                    effect: Some(0),
                    hue: Some(39),
                    sat: Some(70),
                }
            )]
        );
    }
}
