//! The keyboard Foxglove last connected to, in the settings store's `[connection]` section, so
//! it can connect again by itself on the next start. Imported once from `connection.ini`.

use ini::Ini;
use serde::{Deserialize, Serialize};

use crate::protocols::{ConnectionSpec, ZmkTransportConfig};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct Connection {
    #[serde(skip_serializing_if = "Option::is_none")]
    last: Option<Last>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Last {
    vid: u16,
    pid: u16,
    /// `serial` or `ble`.
    transport: String,
    id: String,
}

impl Last {
    fn spec(&self) -> Option<ConnectionSpec> {
        let transport = match self.transport.as_str() {
            "serial" => ZmkTransportConfig::Serial(self.id.clone()),
            "ble" => ZmkTransportConfig::Ble(self.id.clone()),
            _ => return None,
        };
        Some(ConnectionSpec::Zmk {
            vid: self.vid,
            pid: self.pid,
            transport,
        })
    }
}

fn load() -> Connection {
    super::store::load_or_migrate("connection", || {
        let ini = crate::settings::Settings::config_file_path()?.with_file_name("connection.ini");
        let conf = Ini::load_from_file(&ini).ok()?;
        let s = conf.section(Some("last"))?;
        let last = Last {
            vid: s.get("vid")?.parse().ok()?,
            pid: s.get("pid")?.parse().ok()?,
            transport: s.get("transport")?.to_string(),
            id: s.get("id")?.to_string(),
        };
        Some((Connection { last: Some(last) }, vec![ini]))
    })
}

/// Remembers a ZMK keyboard (the only kind Foxglove reconnects to by itself).
pub fn remember(spec: &ConnectionSpec) {
    let ConnectionSpec::Zmk {
        vid,
        pid,
        transport,
    } = spec
    else {
        return;
    };
    let (kind, id) = match transport {
        ZmkTransportConfig::Serial(port) => ("serial", port),
        ZmkTransportConfig::Ble(device) => ("ble", device),
    };
    let last = Last {
        vid: *vid,
        pid: *pid,
        transport: kind.into(),
        id: id.clone(),
    };
    if load().last.as_ref() != Some(&last) {
        if let Err(e) = super::store::save("connection", &Connection { last: Some(last) }) {
            eprintln!("Foxglove: can't remember the keyboard: {e}");
        }
    }
    super::connection::refresh();
}

/// The last keyboard, if Foxglove has connected to one.
pub fn remembered() -> Option<ConnectionSpec> {
    load().last?.spec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn last_keyboard_round_trips_through_toml() {
        let c = Connection {
            last: Some(Last {
                vid: 0x1d50,
                pid: 0x615e,
                transport: "ble".into(),
                id: "BTHLE#x".into(),
            }),
        };
        let back: Connection = toml::from_str(&toml::to_string(&c).unwrap()).unwrap();
        assert_eq!(back, c);
        assert!(matches!(
            back.last.unwrap().spec(),
            Some(ConnectionSpec::Zmk {
                transport: ZmkTransportConfig::Ble(_),
                ..
            })
        ));
        assert_eq!(toml::to_string(&Connection::default()).unwrap(), "");
    }
}
