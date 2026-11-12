//! The lighting arbiter's device classes beyond the keyboard (#1119, H4.5): any LampArray-class
//! device, and any catalog HID board, takes a lease like the keyboard does.
//!
//! The arbiter (`lighting_leases`) is already generic: it works on a device id and a
//! [`Lighting`]. What was missing is a way to reach a [`Lighting`] for an id that is neither the
//! host-link keyboard nor an open VIA one. This module is that seam:
//!
//! - [`register`] / [`unregister`]: a class provider (the Dynamic Lighting enumerator, a catalog
//!   board driver) hands the arbiter an open [`Device`]; its id is then a valid `lighting.lease`
//!   target, with priority, pause and restore exactly as for the keyboard.
//! - [`LampArrayDevice`]: a [`Device`] over any [`LampDevice`]s (one LampArray, or the several
//!   Windows lists for one piece of hardware). One colour for every lamp; no vendor logic.
//!   Systems, not products: what a vendor's lamps need is in the provider's data.
//!
//! A LampArray's colours can't be read back from Windows, so its lights are *handed back*, not
//! restored: the arbiter calls [`Lighting::release`] when the last lease ends or Foxglove
//! pauses (`restores_by_release`), the devices are let go and the OS's own controller takes the
//! lamps again, the same as a scene's end (`scene_lamps`). Devices that can say what they show
//! ([`LampDevice::current`]) give the arbiter a real read-back for each write.
//!
//! Not wired to Windows here: registering the real Dynamic Lighting devices belongs with the
//! feature-module wiring (H2.3); tests use fakes only.

// The providers register from H2.3's module wiring; until then only the tests call this.
#![allow(dead_code)]

use std::sync::Mutex;

use foxglove_devices::{
    BackendId, Caps, Device, DeviceError, DeviceId, DeviceInfo, DeviceKind, Effect, Lighting,
    LightingState, Result, Zone,
};

use super::pc_lights::hsv_rgb;
use super::scene_lamps::LampDevice;

static EXTRA: Mutex<Vec<Box<dyn Device>>> = Mutex::new(Vec::new());

fn extra() -> std::sync::MutexGuard<'static, Vec<Box<dyn Device>>> {
    EXTRA.lock().unwrap_or_else(|e| e.into_inner())
}

/// Makes `device` a lease target (replacing one with the same id).
pub fn register(device: Box<dyn Device>) {
    let mut all = extra();
    let id = device.info().id.clone();
    all.retain(|d| d.info().id != id);
    all.push(device);
}

/// Forgets the device `id` (unplugged). Its leases end through the usual hand-back.
pub fn unregister(id: &str) {
    extra().retain(|d| d.info().id.to_string() != id);
}

/// Runs `f` on the registered device `id`.
pub fn with_device<R>(id: &str, f: impl FnOnce(&dyn Device) -> R) -> Option<R> {
    let all = extra();
    let d = all.iter().find(|d| d.info().id.to_string() == id)?;
    Some(f(d.as_ref()))
}

/// Opens a LampArray's devices again after they were let go.
pub type Reopen = Box<dyn Fn() -> Vec<Box<dyn LampDevice>> + Send + Sync>;

/// [`Lighting`] over LampArray-class devices.
pub struct LampArrayLighting {
    reopen: Reopen,
    open: Mutex<Option<Vec<Box<dyn LampDevice>>>>,
    /// What was last written (the lamps cannot all be read).
    shadow: Mutex<LightingState>,
}

impl LampArrayLighting {
    pub fn new(reopen: Reopen) -> Self {
        LampArrayLighting {
            reopen,
            open: Mutex::new(None),
            shadow: Mutex::new(LightingState {
                on: false,
                hue: 0,
                sat: 0,
                brightness: 0,
                effect: Effect {
                    id: "0".into(),
                    params: vec![],
                },
                speed: 0,
                settled: true,
            }),
        }
    }

    /// Writes the shadow state to every lamp. Fails while no device is available to Foxglove
    /// (Windows or another app drives them): nothing is claimed that the lamps don't show.
    fn draw(&self) -> Result<()> {
        let mut open = self.open.lock().unwrap_or_else(|e| e.into_inner());
        if open.is_none() {
            *open = Some((self.reopen)());
        }
        let devices = open.as_ref().expect("opened above");
        let s = self
            .shadow
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let colour = if s.on {
            hsv_rgb(s.hue, s.sat, s.brightness)
        } else {
            [0, 0, 0]
        };
        let mut drew = false;
        for d in devices.iter().filter(|d| d.available()) {
            d.set(&[colour]).map_err(DeviceError::Backend)?;
            drew = true;
        }
        if drew {
            Ok(())
        } else {
            Err(DeviceError::Busy("another app drives these lamps".into()))
        }
    }

    fn change(&self, f: impl FnOnce(&mut LightingState)) -> Result<()> {
        f(&mut self.shadow.lock().unwrap_or_else(|e| e.into_inner()));
        self.draw()
    }
}

impl Lighting for LampArrayLighting {
    fn layout(&self) -> Result<Vec<Zone>> {
        Ok(vec![])
    }

    /// What the lamps show: read from the first device that can say, else what was written.
    fn state(&self) -> Result<LightingState> {
        let mut s = self
            .shadow
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let open = self.open.lock().unwrap_or_else(|e| e.into_inner());
        let seen = open.as_ref().and_then(|ds| {
            ds.iter()
                .filter(|d| d.available())
                .find_map(|d| d.current())
        });
        if let Some(rgb) = seen {
            if rgb == [0, 0, 0] {
                s.on = false;
            } else {
                let (h, sat, v) =
                    foxglove_devices::calibration::rgb_to_hsv(rgb.map(|c| f32::from(c) / 255.0));
                s.on = true;
                s.hue = (h.round() as u16) % 360;
                s.sat = (sat * 100.0).round() as u8;
                s.brightness = (v * 100.0).round() as u8;
            }
        }
        Ok(s)
    }

    fn set_on(&self, on: bool) -> Result<()> {
        self.change(|s| s.on = on)
    }

    fn set_color(&self, hue: u16, sat: u8, brightness: u8) -> Result<()> {
        self.change(|s| (s.hue, s.sat, s.brightness) = (hue, sat, brightness))
    }

    fn set_effect(&self, effect: Effect) -> Result<()> {
        // A LampArray has no effects of its own; the id is only remembered for the read-back.
        self.shadow.lock().unwrap_or_else(|e| e.into_inner()).effect = effect;
        Ok(())
    }

    /// Lets every device go: the OS's controller has the lamps again.
    fn release(&self) -> Result<()> {
        *self.open.lock().unwrap_or_else(|e| e.into_inner()) = None;
        Ok(())
    }

    fn restores_by_release(&self) -> bool {
        true
    }
}

/// A [`Device`] with only lighting, over [`LampArrayLighting`].
pub struct LampArrayDevice {
    info: DeviceInfo,
    lighting: LampArrayLighting,
}

impl LampArrayDevice {
    pub fn new(backend: &str, key: &str, name: &str, reopen: Reopen) -> Self {
        let backend = BackendId(backend.into());
        LampArrayDevice {
            info: DeviceInfo {
                id: DeviceId::new(backend.clone(), 0, 0, Some(key), key),
                backend,
                name: name.into(),
                kind: DeviceKind::Other,
                firmware: None,
            },
            lighting: LampArrayLighting::new(reopen),
        }
    }
}

impl Device for LampArrayDevice {
    fn info(&self) -> &DeviceInfo {
        &self.info
    }

    fn caps(&self) -> Caps {
        Caps::LIGHTING
    }

    fn lighting(&self) -> Option<&dyn Lighting> {
        Some(&self.lighting)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::foxglove::lighting_leases::{Arbiter, Lease, Priority};
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
    use std::sync::Arc;

    /// A LampArray in memory: the colour its lamps show, and whether it lets Foxglove drive.
    struct Lamps {
        available: AtomicBool,
        shows: Mutex<[u8; 3]>,
        dropped: AtomicU32,
    }

    struct Fake(Arc<Lamps>);

    impl LampDevice for Fake {
        fn available(&self) -> bool {
            self.0.available.load(Ordering::SeqCst)
        }
        fn set(&self, colours: &[[u8; 3]]) -> std::result::Result<(), String> {
            *self.0.shows.lock().unwrap() = colours[0];
            Ok(())
        }
        fn current(&self) -> Option<[u8; 3]> {
            Some(*self.0.shows.lock().unwrap())
        }
    }

    impl Drop for Fake {
        fn drop(&mut self) {
            self.0.dropped.fetch_add(1, Ordering::SeqCst);
        }
    }

    /// The OS's own colour, before any lease.
    const OWN: [u8; 3] = [10, 20, 30];

    fn array(key: &str) -> (LampArrayDevice, Arc<Lamps>) {
        let lamps = Arc::new(Lamps {
            available: AtomicBool::new(true),
            shows: Mutex::new(OWN),
            dropped: AtomicU32::new(0),
        });
        let l = lamps.clone();
        let reopen: Reopen =
            Box::new(move || vec![Box::new(Fake(l.clone())) as Box<dyn LampDevice>]);
        (
            LampArrayDevice::new("fake-lamparray", key, "Fake lamps", reopen),
            lamps,
        )
    }

    fn lease(conn: u64, device: &str, priority: Priority, hue: u16) -> Lease {
        Lease {
            id: 0,
            conn,
            device: device.into(),
            priority,
            hue,
            sat: 100,
            brightness: 100,
            effect: "0".into(),
        }
    }

    fn shown(l: &Lamps) -> [u8; 3] {
        *l.shows.lock().unwrap()
    }

    #[test]
    fn a_lease_shows_on_a_lamp_array_and_release_hands_it_back() {
        let (dev, lamps) = array("a");
        let id = dev.info().id.to_string();
        let light = dev.lighting().unwrap();
        let mut a = Arbiter::default();
        a.add(lease(1, &id, Priority::Normal, 0), light).unwrap();
        assert_eq!(shown(&lamps), [255, 0, 0], "the lease shows, read back");
        assert_eq!(lamps.dropped.load(Ordering::SeqCst), 0);
        for d in a.end(|l| l.conn == 1) {
            a.apply(&d, light).unwrap();
        }
        assert_eq!(
            lamps.dropped.load(Ordering::SeqCst),
            1,
            "let go: the OS has them"
        );
        assert!(a.devices().is_empty(), "no snapshot is kept");
    }

    #[test]
    fn highest_priority_then_newest_wins_on_a_lamp_array() {
        let (dev, lamps) = array("b");
        let id = dev.info().id.to_string();
        let light = dev.lighting().unwrap();
        let mut a = Arbiter::default();
        a.add(lease(1, &id, Priority::Normal, 0), light).unwrap();
        a.add(lease(2, &id, Priority::Normal, 120), light).unwrap();
        assert_eq!(shown(&lamps), [0, 255, 0], "newest among equals");
        a.add(lease(3, &id, Priority::High, 240), light).unwrap();
        a.add(lease(4, &id, Priority::Low, 0), light).unwrap();
        assert_eq!(shown(&lamps), [0, 0, 255], "high beats newer low");
        // The high one ends with its connection: the newest normal shows again.
        for d in a.end(|l| l.conn == 3) {
            a.apply(&d, light).unwrap();
        }
        assert_eq!(shown(&lamps), [0, 255, 0]);
    }

    #[test]
    fn pause_hands_the_lamps_back_and_resume_shows_the_lease_again() {
        let (dev, lamps) = array("c");
        let id = dev.info().id.to_string();
        let light = dev.lighting().unwrap();
        let mut a = Arbiter::default();
        a.add(lease(1, &id, Priority::Normal, 0), light).unwrap();
        for d in a.set_paused(true) {
            a.apply(&d, light).unwrap();
        }
        assert_eq!(
            lamps.dropped.load(Ordering::SeqCst),
            1,
            "paused: handed back"
        );
        for d in a.set_paused(false) {
            a.apply(&d, light).unwrap();
        }
        assert_eq!(shown(&lamps), [255, 0, 0], "resumed: the winner again");
        for d in a.end(|_| true) {
            a.apply(&d, light).unwrap();
        }
        assert_eq!(lamps.dropped.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn leases_on_one_device_leave_another_alone() {
        let (d1, l1) = array("d1");
        let (d2, l2) = array("d2");
        let (i1, i2) = (d1.info().id.to_string(), d2.info().id.to_string());
        let mut a = Arbiter::default();
        a.add(lease(1, &i1, Priority::Normal, 0), d1.lighting().unwrap())
            .unwrap();
        assert_eq!(shown(&l2), OWN, "the other array is untouched");
        a.add(lease(2, &i2, Priority::Normal, 240), d2.lighting().unwrap())
            .unwrap();
        for d in a.end(|l| l.conn == 1) {
            a.apply(&d, d1.lighting().unwrap()).unwrap();
        }
        assert_eq!(shown(&l2), [0, 0, 255], "the second lease still shows");
        assert_eq!(l1.dropped.load(Ordering::SeqCst), 1);
        assert_eq!(l2.dropped.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn a_lamp_array_another_app_drives_refuses_the_lease_write() {
        let (dev, lamps) = array("e");
        lamps.available.store(false, Ordering::SeqCst);
        let id = dev.info().id.to_string();
        let mut a = Arbiter::default();
        let r = a.add(lease(1, &id, Priority::Normal, 0), dev.lighting().unwrap());
        assert!(matches!(r, Err(DeviceError::Busy(_))), "{r:?}");
        assert_eq!(shown(&lamps), OWN, "nothing was drawn on it");
    }

    #[test]
    fn the_registry_finds_a_device_by_id_and_forgets_it() {
        let (dev, _) = array("registry-test");
        let id = dev.info().id.to_string();
        register(Box::new(dev));
        assert_eq!(with_device(&id, |d| d.caps()), Some(Caps::LIGHTING));
        assert_eq!(
            super::super::lighting_leases::with_light_for_test(&id),
            Some(true),
            "the arbiter reaches it by id"
        );
        unregister(&id);
        assert!(with_device(&id, |_| ()).is_none());
    }
}
