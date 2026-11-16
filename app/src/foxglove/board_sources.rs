//! Catalog boards as lease targets (#1119, H2.3 slice S2): the Lights worker's scan
//! (`pc_lights::Worker::scan`, the one watch on HID arrivals) registers each board it opened a
//! driver for with the lighting arbiter's sources (`lighting_sources`), and forgets it when a
//! scan no longer finds it, ending its leases first.
//!
//! Systems, not products: a board is whatever the device catalog (data) describes with a
//! driver and zones. Nothing here knows a vendor; what a board needs is in its catalog entry
//! and its driver. A board the catalog lists without a way to light it (no driver yet, no
//! zones, no colour) is not registered.
//!
//! The lease draws one colour on every zone through the worker ([`Sink`]: the worker's queue in
//! the app, a fake in tests). The worker keeps the colours from before the lease and puts them
//! back when it ends (`restores_by_release`), so a lease never overwrites the user's colours.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use foxglove_devices::pc_lighting::{Color, Control, Found};
use foxglove_devices::{
    BackendId, Caps, Device, DeviceError, DeviceId, DeviceInfo, DeviceKind, Effect, Lighting,
    LightingState, Result, Zone,
};

use super::lighting_sources;
use super::pc_lights::hsv_rgb;

/// The backend name of devices registered here (part of their ids).
pub const BACKEND: &str = "pclights";

/// Where a lease's colours go: the board's worker.
pub trait Sink: Send + Sync {
    /// Shows `colours` (zone index, colour) on the board. Must not wait on the arbiter.
    fn show(&self, key: &str, colours: &[(usize, [u8; 3])]) -> Result<()>;
    /// The last lease on the board ended: its colours from before the lease go back.
    fn release(&self, key: &str);
}

/// The Lights worker's queue.
pub struct WorkerSink;

impl Sink for WorkerSink {
    fn show(&self, key: &str, colours: &[(usize, [u8; 3])]) -> Result<()> {
        let sent = super::pc_lights::try_send(super::pc_lights::Msg::LeaseShow {
            key: key.into(),
            colors: colours.to_vec(),
        });
        if sent {
            Ok(())
        } else {
            Err(DeviceError::Disconnected)
        }
    }

    fn release(&self, key: &str) {
        super::pc_lights::send(super::pc_lights::Msg::LeaseEnd(key.into()));
    }
}

/// A board the worker drives, as far as leasing needs it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Board {
    pub key: String,
    pub name: String,
    pub zones: usize,
}

/// The arbiter id of the board with this catalog key.
pub fn lease_id(key: &str) -> String {
    DeviceId::new(BackendId(BACKEND.into()), 0, 0, Some(key), key).to_string()
}

/// The leaseable boards among what a scan found: a driver controls it (`has_driver`: the worker
/// opened one), and the catalog says it has zones and a colour.
pub fn boards(found: &[Found], has_driver: impl Fn(&str) -> bool) -> Vec<Board> {
    found
        .iter()
        .filter(|f| matches!(f.control, Control::Driver(_)))
        .filter(|f| !f.zones.is_empty() && f.caps.color != Color::None)
        .filter(|f| has_driver(&f.key))
        .map(|f| Board {
            key: f.key.clone(),
            name: f.name.clone(),
            zones: f.zones.len(),
        })
        .collect()
}

/// Brings the registered boards in line with `now`: arrivals become lease targets; boards no
/// longer there end their leases and are forgotten. `known` is what this watcher registered.
pub fn sync(known: &mut BTreeSet<String>, now: &[Board], sink: &Arc<dyn Sink>) {
    for b in now {
        known.insert(b.key.clone());
        // A repeat scan must not let go of a board a lease is drawing on.
        if lighting_sources::with_device(&lease_id(&b.key), |_| ()).is_some() {
            continue;
        }
        lighting_sources::register(Box::new(BoardDevice::new(b, sink.clone())));
    }
    known.retain(|key| {
        if now.iter().any(|b| &b.key == key) {
            return true;
        }
        let id = lease_id(key);
        // Before unregistering, and with no registry lock held: ending a lease hands the board
        // back through the registered device.
        super::api::end_leases(|l| l.device == id);
        lighting_sources::unregister(&id);
        false
    });
}

/// [`Lighting`] over a board: one colour on every zone.
struct BoardLighting {
    key: String,
    zones: usize,
    sink: Arc<dyn Sink>,
    shadow: Mutex<LightingState>,
}

impl BoardLighting {
    fn change(&self, f: impl FnOnce(&mut LightingState)) -> Result<()> {
        let s = {
            let mut s = self.shadow.lock().unwrap_or_else(|e| e.into_inner());
            f(&mut s);
            s.clone()
        };
        let c = if s.on {
            hsv_rgb(s.hue, s.sat, s.brightness)
        } else {
            [0, 0, 0]
        };
        let all: Vec<_> = (0..self.zones).map(|i| (i, c)).collect();
        self.sink.show(&self.key, &all)
    }
}

impl Lighting for BoardLighting {
    fn layout(&self) -> Result<Vec<Zone>> {
        Ok(vec![])
    }

    /// What was last written: the board can't be read back.
    fn state(&self) -> Result<LightingState> {
        Ok(self
            .shadow
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone())
    }

    fn set_on(&self, on: bool) -> Result<()> {
        self.change(|s| s.on = on)
    }

    fn set_color(&self, hue: u16, sat: u8, brightness: u8) -> Result<()> {
        self.change(|s| (s.hue, s.sat, s.brightness) = (hue, sat, brightness))
    }

    fn set_effect(&self, effect: Effect) -> Result<()> {
        self.shadow.lock().unwrap_or_else(|e| e.into_inner()).effect = effect;
        Ok(())
    }

    fn release(&self) -> Result<()> {
        self.shadow.lock().unwrap_or_else(|e| e.into_inner()).on = false;
        self.sink.release(&self.key);
        Ok(())
    }

    fn restores_by_release(&self) -> bool {
        true
    }
}

/// A board as a [`Device`] with only lighting.
struct BoardDevice {
    info: DeviceInfo,
    lighting: BoardLighting,
}

impl BoardDevice {
    fn new(b: &Board, sink: Arc<dyn Sink>) -> Self {
        let backend = BackendId(BACKEND.into());
        BoardDevice {
            info: DeviceInfo {
                id: DeviceId::new(backend.clone(), 0, 0, Some(&b.key), &b.key),
                backend,
                name: b.name.clone(),
                kind: DeviceKind::Other,
                firmware: None,
            },
            lighting: BoardLighting {
                key: b.key.clone(),
                zones: b.zones,
                sink,
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
            },
        }
    }
}

impl Device for BoardDevice {
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
    use crate::foxglove::api::AppDevices;
    use foxglove_devices::pc_lighting::{Catalog, HidEntry};
    use foxglove_plugin_api::rpc::Error;
    use foxglove_plugin_host::Devices as _;
    use serde_json::{json, Value};

    type Shows = Vec<(String, Vec<(usize, [u8; 3])>)>;

    #[derive(Default)]
    pub(crate) struct Fake {
        shows: Mutex<Shows>,
        releases: Mutex<Vec<String>>,
    }

    impl Sink for Fake {
        fn show(&self, key: &str, c: &[(usize, [u8; 3])]) -> Result<()> {
            self.shows.lock().unwrap().push((key.into(), c.to_vec()));
            Ok(())
        }
        fn release(&self, key: &str) {
            self.releases.lock().unwrap().push(key.into());
        }
    }

    fn board(key: &str) -> Board {
        Board {
            key: key.into(),
            name: "Fake board".into(),
            zones: 3,
        }
    }

    fn lease(conn: u64, device: &str) -> std::result::Result<Value, Error> {
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

    fn sink() -> (Arc<Fake>, Arc<dyn Sink>) {
        let f = Arc::new(Fake::default());
        (f.clone(), f)
    }

    // The registry and the arbiter are process-wide: each test uses its own key and connection.

    #[test]
    fn an_arrival_makes_the_board_leaseable_and_a_removal_ends_its_lease() {
        let (fake, s) = sink();
        let mut known = BTreeSet::new();
        let id = lease_id("board-1");
        assert!(lease(7201, &id).is_err(), "unknown before it arrives");

        sync(&mut known, &[board("board-1")], &s);
        assert!(registered("board-1"));
        assert!(lease(7201, &id).is_ok(), "leaseable as the Hub does it");
        assert!(leased(&id));
        let shows = fake.shows.lock().unwrap().clone();
        let red = [255, 0, 0];
        assert_eq!(shows.last().unwrap().1, vec![(0, red), (1, red), (2, red)]);

        sync(&mut known, &[], &s);
        assert!(!leased(&id), "its lease ended");
        assert_eq!(*fake.releases.lock().unwrap(), vec!["board-1".to_string()]);
        assert!(!registered("board-1"), "forgotten");
        assert!(lease(7201, &id).is_err(), "refused once gone");
        assert!(known.is_empty());
    }

    #[test]
    fn an_unknown_id_is_refused() {
        assert!(lease(7202, &lease_id("never-seen")).is_err());
    }

    #[test]
    fn a_repeat_arrival_keeps_an_active_lease() {
        let (fake, s) = sink();
        let mut known = BTreeSet::new();
        let id = lease_id("board-3");
        sync(&mut known, &[board("board-3")], &s);
        lease(7203, &id).unwrap();
        sync(&mut known, &[board("board-3")], &s);
        assert!(leased(&id));
        assert!(fake.releases.lock().unwrap().is_empty());
        sync(&mut known, &[], &s);
    }

    /// The bundled catalog: the case board has a driver and zones; the pump cap is recognised
    /// but not controllable yet; a LampArray is S1's, not a catalog board.
    #[test]
    fn only_boards_the_catalog_can_light_are_leaseable() {
        let entry = |vid, pid, usage_page, usage, path: &str| HidEntry {
            vid,
            pid,
            usage_page,
            usage,
            path: path.into(),
            ..Default::default()
        };
        let hid = vec![
            entry(0x103c, 0x84fd, 1, 0, "a"),
            entry(0x03f0, 0x7397, 1, 0, "b"),
            entry(0x046d, 0xc332, 0x59, 1, "c"),
        ];
        let found = foxglove_devices::pc_lighting::discover(&Catalog::bundled(), &hid);
        assert_eq!(found.len(), 3);
        let got = boards(&found, |_| true);
        assert_eq!(got.len(), 1, "{got:?}");
        assert_eq!(got[0].key, "103c:84fd");
        assert!(
            boards(&found, |_| false).is_empty(),
            "no driver opened, no board"
        );

        let (_fake, s) = sink();
        let mut known = BTreeSet::new();
        sync(&mut known, &got, &s);
        assert!(registered("103c:84fd"));
        assert!(!registered("03f0:7397"));
        sync(&mut known, &[], &s);
    }
}
