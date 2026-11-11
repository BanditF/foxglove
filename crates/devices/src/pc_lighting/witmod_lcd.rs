//! Witmod-firmware LCD pump caps (#614): an AIO cooler's pump head with a screen and an ARGB
//! ring, whose MCU speaks Witmod's HID protocol on a vendor usage page (the OMEN 45L's cap,
//! `03F0:7397`, is one). A class, not a product: every number this module knows comes from the
//! catalog's `[device.witmod]` table ([`Params`]): the interface, the report ids and lengths,
//! the claim op and how often the vendor's helper re-claims it, the LED counts, and the ring's
//! colour command once one is known.
//!
//! **A stub.** No ring command is known yet (only the screen's ops are), so this module sends
//! nothing: the driver it gives has no writer at all, and every colour it's asked for is
//! refused. A device with this protocol and no `ring_op` reads as not controllable
//! ([`Params::ring_known`], `Control::NotYet`), so scenes skip it and the page says it keeps its
//! own effect. The ring write lands only after a capture shows the vendor's own command (#614
//! step 4); it never guesses ops, since the same op set holds the firmware-update path.

use serde::Deserialize;

use super::{LightingDevice, ZoneSpec};
use crate::{DeviceError, Result, Rgb};

/// The zone id of the ring on the pump head.
pub const RING_ZONE: u8 = 1;
/// The zone id of the fans chained from the cap's ARGB out.
pub const FAN_ZONE: u8 = 2;

/// Whether a zone id names one of the cap's LED runs.
pub fn zone_ok(zone: u8) -> bool {
    zone == RING_ZONE || zone == FAN_ZONE
}

/// The cap's HID protocol, as data (`[device.witmod]` in the catalog).
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Params {
    /// The vendor interface's usage page and usage (not the cap's mouse-class collection).
    pub usage_page: u16,
    pub usage: u16,
    /// The report ids it uses.
    pub report_ids: Vec<u8>,
    /// Report lengths in bytes, without the report id.
    pub input_len: u16,
    pub output_len: u16,
    /// The op that claims the cap, and how often the vendor's helper sends it (a second writer
    /// would fight it).
    pub claim_op: u8,
    pub claim_every_ms: u32,
    /// LEDs on the cap's ring.
    pub ring_leds: u16,
    /// LEDs per chained fan, and how many fans the chain has.
    #[serde(default)]
    pub fan_leds: u16,
    #[serde(default)]
    pub fans: u8,
    /// The ring's colour command; unset while unknown, and the device isn't controllable.
    #[serde(default)]
    pub ring_op: Option<u8>,
}

impl Params {
    /// Whether the ring's colour command is known.
    pub fn ring_known(&self) -> bool {
        self.ring_op.is_some()
    }

    /// Every LED it drives: the ring, then each chained fan.
    pub fn leds(&self) -> u32 {
        u32::from(self.ring_leds) + u32::from(self.fan_leds) * u32::from(self.fans)
    }

    /// Refuses a table that can't describe a cap.
    pub fn check(&self) -> std::result::Result<(), String> {
        if self.report_ids.is_empty() {
            return Err("needs its report ids".into());
        }
        if self.input_len == 0 || self.output_len == 0 {
            return Err("report lengths can't be 0".into());
        }
        if self.claim_every_ms == 0 {
            return Err("claim_every_ms can't be 0".into());
        }
        if self.ring_leds == 0 {
            return Err("needs its ring's LED count".into());
        }
        if self.ring_op == Some(self.claim_op) {
            return Err("the ring op can't be the claim op".into());
        }
        Ok(())
    }

    /// The table as catalog TOML, for the user's own file.
    pub fn to_toml(&self) -> String {
        let ids: Vec<String> = self.report_ids.iter().map(|i| i.to_string()).collect();
        let mut out = format!(
            "[device.witmod]\nusage_page = 0x{:04X}\nusage = 0x{:02X}\nreport_ids = [{}]\n\
             input_len = {}\noutput_len = {}\nclaim_op = 0x{:02X}\nclaim_every_ms = {}\n\
             ring_leds = {}\nfan_leds = {}\nfans = {}\n",
            self.usage_page,
            self.usage,
            ids.join(", "),
            self.input_len,
            self.output_len,
            self.claim_op,
            self.claim_every_ms,
            self.ring_leds,
            self.fan_leds,
            self.fans
        );
        if let Some(op) = self.ring_op {
            out.push_str(&format!("ring_op = 0x{op:02X}\n"));
        }
        out
    }
}

/// The stub driver: it has no writer, so it can't send anything, and refuses every colour.
pub struct WitmodLcd {
    name: String,
    zones: Vec<ZoneSpec>,
}

impl WitmodLcd {
    pub fn new(name: String, zones: Vec<ZoneSpec>) -> Self {
        Self { name, zones }
    }
}

impl LightingDevice for WitmodLcd {
    fn name(&self) -> &str {
        &self.name
    }

    fn zones(&self) -> Vec<String> {
        self.zones.iter().map(|z| z.name.clone()).collect()
    }

    fn colors(&self) -> Vec<Option<Rgb>> {
        vec![None; self.zones.len()]
    }

    fn set_zones(&mut self, colors: &[(usize, Rgb)]) -> Result<()> {
        if colors.is_empty() {
            return Ok(());
        }
        Err(DeviceError::Unsupported)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pc_lighting::omen_tracer::tests::Recorder;
    use crate::pc_lighting::{
        discover, open, test_zone, Catalog, Control, HidEntry, Protocol, BLACK, WHITE,
    };

    /// The 45L cap's vendor interface, as an enumeration lists it (no device is opened).
    fn cap() -> HidEntry {
        HidEntry {
            vid: 0x03f0,
            pid: 0x7397,
            usage_page: 0xff1a,
            usage: 0x93,
            path: "pump-vendor".into(),
            ..Default::default()
        }
    }

    #[test]
    fn the_45l_cap_is_data_and_reads_as_not_controllable() {
        let c = Catalog::bundled();
        let d = c.find(0x03f0, 0x7397).unwrap();
        assert_eq!(d.protocol, Protocol::WitmodLcd);
        let w = d.witmod.as_ref().expect("its protocol table");
        assert_eq!((w.usage_page, w.usage), (0xff1a, 0x93));
        assert_eq!(w.report_ids, [1, 2]);
        assert_eq!((w.input_len, w.output_len), (512, 1024));
        assert_eq!((w.claim_op, w.claim_every_ms), (0x41, 1000));
        assert_eq!(w.leds(), 16 + 10 * 3);
        assert!(!w.ring_known());
        assert!(!d.controllable());
        let found = discover(&c, &[cap()]);
        assert_eq!(found[0].control, Control::NotYet);
    }

    #[test]
    fn the_stub_never_writes() {
        let c = Catalog::bundled();
        let rec = Recorder::default();
        // Even opened as a driver (as it would be once a catalog names a ring op), it sends
        // nothing: it has no writer.
        let mut found = discover(&c, &[cap()]).remove(0);
        found.control = Control::Driver(Protocol::WitmodLcd);
        let mut d = open(&found, Box::new(rec.clone())).expect("the stub opens");
        assert_eq!(d.zones(), ["Pump ring", "Radiator fans"]);
        assert_eq!(d.set_all(WHITE), Err(DeviceError::Unsupported));
        assert_eq!(d.off(), Err(DeviceError::Unsupported));
        assert_eq!(d.set_zones(&[(0, BLACK)]), Err(DeviceError::Unsupported));
        assert!(d.identify(&mut |_| {}).is_err());
        assert_eq!(d.colors(), vec![None, None]);
        assert_eq!(
            test_zone(
                Protocol::WitmodLcd,
                RING_ZONE,
                None,
                Box::new(rec.clone()),
                &mut |_| {}
            ),
            Err(DeviceError::Unsupported)
        );
        assert!(rec.sent.lock().unwrap().is_empty(), "nothing written");
        assert!(Protocol::WitmodLcd.zones().is_empty(), "no zone test");
        assert!(
            !Protocol::ALL.contains(&Protocol::WitmodLcd),
            "not a manual add"
        );
    }

    #[test]
    fn a_known_ring_op_makes_it_controllable_and_bad_tables_are_refused() {
        let entry = |table: &str| {
            format!(
                "[[device]]\nname = \"Cap\"\nvid = 1\npid = 2\nprotocol = \"witmod-lcd\"\n\
                 kind = \"cooler\"\nzones = [{{ id = 1, name = \"Ring\" }}]\n{table}"
            )
        };
        let table = "[device.witmod]\nusage_page = 0xFF1A\nusage = 0x93\nreport_ids = [1]\n\
                     input_len = 64\noutput_len = 64\nclaim_op = 0x41\nclaim_every_ms = 1000\n\
                     ring_leds = 8\n";
        let c = Catalog::parse(&entry(table)).unwrap();
        assert!(!c.devices[0].controllable());
        let known = Catalog::parse(&entry(&format!("{table}ring_op = 0x50\n"))).unwrap();
        assert!(known.devices[0].controllable());
        // Written back the same, in the user's own file.
        let back = Catalog::parse(&known.devices[0].to_toml()).unwrap();
        assert_eq!(back.devices[0].witmod, known.devices[0].witmod);
        // No table, a table on another protocol, a bad zone or a bad table: refused.
        assert!(Catalog::parse(&entry("")).is_err());
        let other = entry(table).replace("witmod-lcd", "none");
        assert!(Catalog::parse(&other).is_err());
        assert!(Catalog::parse(&entry(table).replace("id = 1", "id = 3")).is_err());
        let bad = |from: &str, to: &str| Catalog::parse(&entry(&table.replace(from, to)));
        assert!(bad("report_ids = [1]", "report_ids = []").is_err());
        assert!(bad("ring_leds = 8", "ring_leds = 0").is_err());
        assert!(bad("claim_every_ms = 1000", "claim_every_ms = 0").is_err());
        assert!(Catalog::parse(&entry(&format!("{table}ring_op = 0x41\n"))).is_err());
        assert!(bad("usage = 0x93", "usage = 0x93\nled_op = 1").is_err());
    }
}
