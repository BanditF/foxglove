//! LampArray-class devices as lease targets (#1119, H2.3 slice S1): the LampArray watch
//! (`lamp_hold::Watched`, from `scene_lamps::watch_lamp_arrays`) registers each device Windows
//! lists with the lighting arbiter's sources (`lighting_sources`), and forgets it when Windows
//! stops listing it, ending its leases first.
//!
//! Systems, not products: any LampArray is registered by its OS device id, one colour for every
//! lamp (`LampArrayDevice`); nothing here knows a vendor. Registering opens nothing: the lamps
//! are opened on the first lease write and let go when the last lease ends, so a listed device
//! nobody leases is never claimed from Windows or another app.
//!
//! The source of devices is a seam ([`Arrays`]): Windows' in the app, a fake in tests.

use foxglove_devices::{BackendId, DeviceId};

use super::lamp_hold::Watched;
use super::lighting_sources::{self, LampArrayDevice, Reopen};

/// The backend name of devices registered here (part of their ids).
pub const BACKEND: &str = "lamparray";

/// Where the LampArrays come from: what the OS lists now, and a way to open one.
pub trait Arrays: Send + Sync {
    /// The device ids the OS lists now.
    fn listed(&self) -> Vec<String>;
    /// A device's display name and how to open its lamps (again), when it is still listed.
    fn open(&self, key: &str) -> Option<(String, Reopen)>;
}

/// The arbiter id of the LampArray with this OS device id.
pub fn lease_id(key: &str) -> String {
    DeviceId::new(BackendId(BACKEND.into()), 0, 0, Some(key), key).to_string()
}

/// Handles one watch event: arrivals become lease targets; removals end their leases and are
/// forgotten.
pub fn watched(event: &Watched, arrays: &dyn Arrays) {
    match event {
        Watched::Enumerated => {
            for key in arrays.listed() {
                arrived(&key, arrays);
            }
        }
        Watched::Added(key) => arrived(key, arrays),
        Watched::Removed(key) => removed(key),
    }
}

/// Makes the device `key` a lease target, unless it already is (a second event for one device
/// must not let go of lamps a lease is drawing on).
fn arrived(key: &str, arrays: &dyn Arrays) {
    if lighting_sources::with_device(&lease_id(key), |_| ()).is_some() {
        return;
    }
    let Some((name, reopen)) = arrays.open(key) else {
        eprintln!("Foxglove: LampArray {key} is listed but can't be opened; not leaseable");
        return;
    };
    lighting_sources::register(Box::new(LampArrayDevice::new(BACKEND, key, &name, reopen)));
}

/// The OS no longer lists `key`: its leases end (the lamps are handed back; nothing is kept to
/// restore), then it stops being a lease target.
fn removed(key: &str) {
    let id = lease_id(key);
    if lighting_sources::with_device(&id, |_| ()).is_none() {
        return;
    }
    // Before unregistering: ending a lease hands the lamps back through the registered device.
    super::api::end_leases(|l| l.device == id);
    lighting_sources::unregister(&id);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::foxglove::api::AppDevices;
    use crate::foxglove::scene_lamps::LampDevice;
    use foxglove_plugin_api::rpc::Error;
    use foxglove_plugin_host::Devices as _;
    use serde_json::{json, Value};
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
    use std::sync::{Arc, Mutex};

    struct Lamps {
        shows: Mutex<[u8; 3]>,
        dropped: AtomicU32,
        listed: AtomicBool,
    }

    struct Fake(Arc<Lamps>);

    impl LampDevice for Fake {
        fn available(&self) -> bool {
            true
        }
        fn set(&self, c: &[[u8; 3]]) -> Result<(), String> {
            *self.0.shows.lock().unwrap() = c[0];
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

    /// One fake LampArray the "OS" lists under `key`.
    struct Os {
        key: String,
        lamps: Arc<Lamps>,
    }

    impl Os {
        fn new(key: &str) -> Self {
            Os {
                key: key.into(),
                lamps: Arc::new(Lamps {
                    shows: Mutex::new([10, 20, 30]),
                    dropped: AtomicU32::new(0),
                    listed: AtomicBool::new(true),
                }),
            }
        }
    }

    impl Arrays for Os {
        fn listed(&self) -> Vec<String> {
            vec![self.key.clone()]
        }
        fn open(&self, key: &str) -> Option<(String, Reopen)> {
            if key != self.key || !self.lamps.listed.load(Ordering::SeqCst) {
                return None;
            }
            let l = self.lamps.clone();
            Some((
                "Fake lamps".into(),
                Box::new(move || vec![Box::new(Fake(l.clone())) as Box<dyn LampDevice>]),
            ))
        }
    }

    fn lease(conn: u64, device: &str) -> Result<Value, Error> {
        AppDevices.call_from(
            conn,
            "lighting.lease",
            json!({"device": device, "priority": "normal",
                   "color": {"hue": 0, "saturation": 100, "brightness": 100}}),
        )
    }

    fn leased(id: &str) -> bool {
        crate::foxglove::lighting_leases::with(|a| a.devices().iter().any(|d| d == id))
    }

    fn registered(key: &str) -> bool {
        lighting_sources::with_device(&lease_id(key), |_| ()).is_some()
    }

    // The registry and the arbiter are process-wide: each test uses its own device key and
    // connection number.

    #[test]
    fn an_arrival_makes_the_device_leaseable_and_a_removal_ends_its_leases() {
        let os = Os::new("fake-os-id-1");
        let id = lease_id(&os.key);
        assert!(lease(7101, &id).is_err(), "unknown before it arrives");

        watched(&Watched::Added(os.key.clone()), &os);
        assert!(registered(&os.key));
        assert!(lease(7101, &id).is_ok(), "leaseable as the Hub does it");
        assert_eq!(*os.lamps.shows.lock().unwrap(), [255, 0, 0]);
        assert!(leased(&id));

        watched(&Watched::Removed(os.key.clone()), &os);
        assert!(!leased(&id), "its leases ended");
        assert_eq!(os.lamps.dropped.load(Ordering::SeqCst), 1, "lamps let go");
        assert!(!registered(&os.key), "forgotten");
        assert!(lease(7101, &id).is_err(), "refused once gone");
    }

    #[test]
    fn the_first_enumeration_registers_what_is_already_there() {
        let os = Os::new("fake-os-id-2");
        let id = lease_id(&os.key);
        watched(&Watched::Enumerated, &os);
        assert!(lease(7102, &id).is_ok());
        watched(&Watched::Removed(os.key.clone()), &os);
        assert!(lease(7102, &id).is_err());
    }

    #[test]
    fn a_second_arrival_keeps_the_lamps_a_lease_draws_on() {
        let os = Os::new("fake-os-id-3");
        let id = lease_id(&os.key);
        watched(&Watched::Added(os.key.clone()), &os);
        lease(7103, &id).unwrap();
        watched(&Watched::Added(os.key.clone()), &os);
        assert_eq!(os.lamps.dropped.load(Ordering::SeqCst), 0);
        assert!(leased(&id));
        watched(&Watched::Removed(os.key.clone()), &os);
    }

    #[test]
    fn an_unopenable_device_is_not_registered_and_an_unknown_removal_is_quiet() {
        let os = Os::new("fake-os-id-4");
        os.lamps.listed.store(false, Ordering::SeqCst);
        watched(&Watched::Added(os.key.clone()), &os);
        assert!(!registered(&os.key));
        assert!(lease(7104, &lease_id(&os.key)).is_err());
        watched(&Watched::Removed("never-seen".into()), &os);
    }
}
