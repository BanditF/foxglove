//! Lighting leases (#236): plugins that signal a state on the underglow (mic muted, recording)
//! hold the lights while it lasts and give them back cleanly.
//!
//! - A lease names a colour (and effect) and a priority; the lights show the highest-priority
//!   lease, the newest among equals.
//! - When the first lease on a device starts, its lights as they were are kept; when the last
//!   one ends they come back.
//! - A plugin's leases end when it releases them or its connection closes (stopped, crashed).
//! - Every change is read back and sent again if the keyboard didn't take it (a few times).

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
    /// Each leased device's lights from before its first lease.
    base: HashMap<String, LightingState>,
    next: u64,
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
        self.leases.iter().filter(|l| l.device == device).max_by_key(|l| (l.priority, l.id))
    }

    /// Takes the lights for `lease` (its `id` is assigned) and shows the winner.
    pub fn add(&mut self, mut lease: Lease, light: &dyn Lighting) -> Result<u64, DeviceError> {
        if !self.base.contains_key(&lease.device) {
            self.base.insert(lease.device.clone(), light.state()?);
        }
        self.next += 1;
        lease.id = self.next;
        let (id, device) = (lease.id, lease.device.clone());
        self.leases.push(lease);
        self.apply(&device, light)?;
        Ok(id)
    }

    /// Ends the leases `ends` picks; returns the devices whose lights need [`Arbiter::apply`].
    pub fn end(&mut self, ends: impl Fn(&Lease) -> bool) -> Vec<String> {
        let mut devices: Vec<String> = self.leases.iter().filter(|l| ends(l)).map(|l| l.device.clone()).collect();
        self.leases.retain(|l| !ends(l));
        devices.sort();
        devices.dedup();
        devices
    }

    fn target(&mut self, device: &str) -> Target<'_> {
        if self.winner(device).is_some() {
            return Target::Lease(self.winner(device).expect("just checked"));
        }
        match self.base.remove(device) {
            Some(base) => Target::Restore(base),
            None => Target::Nothing,
        }
    }

    /// Shows the winning lease, or the lights from before the first lease once none is left.
    pub fn apply(&mut self, device: &str, light: &dyn Lighting) -> Result<(), DeviceError> {
        match self.target(device) {
            Target::Lease(l) => show(light, true, l.hue, l.sat, l.brightness, &l.effect),
            Target::Restore(s) => show(light, s.on, s.hue, s.sat, s.brightness, &s.effect.id),
            Target::Nothing => Ok(()),
        }
    }
}

/// How often a change the keyboard didn't take is sent again.
const RESENDS: u32 = 3;

fn shows(state: &LightingState, on: bool, hue: u16, sat: u8) -> bool {
    let near = |a: u16, b: u16, wrap: u16| {
        let d = a.abs_diff(b);
        d.min(wrap.saturating_sub(d)) <= 2
    };
    state.on == on && (!on || (near(state.hue, hue, 360) && near(state.sat.into(), sat.into(), 1000)))
}

fn show(light: &dyn Lighting, on: bool, hue: u16, sat: u8, brightness: u8, effect: &str) -> Result<(), DeviceError> {
    let send = || -> Result<(), DeviceError> {
        light.set_on(on)?;
        light.set_color(hue, sat, brightness)?;
        light.set_effect(Effect { id: effect.into(), params: vec![] })
    };
    send()?;
    for _ in 0..RESENDS {
        match light.state() {
            Ok(s) if !shows(&s, on, hue, sat) => {
                std::thread::sleep(std::time::Duration::from_millis(100));
                send()?;
            }
            _ => break,
        }
    }
    Ok(())
}

pub static ARBITER: Mutex<Option<Arbiter>> = Mutex::new(None);

pub fn with<R>(f: impl FnOnce(&mut Arbiter) -> R) -> R {
    f(ARBITER.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(Arbiter::default))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// Lights in memory; `lose` drops that many writes.
    struct Fake {
        state: RefCell<LightingState>,
        lose: RefCell<u32>,
    }

    impl Fake {
        fn new() -> Fake {
            Fake {
                state: RefCell::new(LightingState { on: false, hue: 160, sat: 50, brightness: 40, effect: Effect { id: "3".into(), params: vec![] }, speed: 1 }),
                lose: RefCell::new(0),
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
            self.state.borrow_mut().effect = effect;
            Ok(())
        }
    }

    fn lease(conn: u64, priority: Priority, hue: u16) -> Lease {
        Lease { id: 0, conn, device: "kb".into(), priority, hue, sat: 100, brightness: 80, effect: "0".into() }
    }

    fn hue(f: &Fake) -> (bool, u16) {
        let s = f.state.borrow();
        (s.on, s.hue)
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
        assert_eq!((s.on, s.hue, s.sat, s.effect.id.as_str()), (false, 160, 50, "3"));
        assert!(a.leases.is_empty() && a.base.is_empty());
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
