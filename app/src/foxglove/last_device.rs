//! The keyboard Foxglove last connected to, in `connection.ini` next to the settings, so it can
//! connect again by itself on the next start.

use ini::Ini;

use crate::protocols::{ConnectionSpec, ZmkTransportConfig};

fn path() -> Option<std::path::PathBuf> {
    crate::settings::Settings::config_file_path().map(|p| p.with_file_name("connection.ini"))
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
    let mut conf = Ini::new();
    conf.with_section(Some("last"))
        .set("vid", vid.to_string())
        .set("pid", pid.to_string())
        .set("transport", kind)
        .set("id", id.as_str());
    let Some(p) = path() else {
        return;
    };
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Err(e) = conf.write_to_file(&p) {
        eprintln!(
            "Foxglove: can't remember the keyboard in {}: {e}",
            p.display()
        );
    }
    super::connection::refresh();
}

/// The last keyboard, if Foxglove has connected to one.
pub fn remembered() -> Option<ConnectionSpec> {
    let conf = Ini::load_from_file(path()?).ok()?;
    let s = conf.section(Some("last"))?;
    let id = s.get("id")?.to_string();
    let transport = match s.get("transport")? {
        "serial" => ZmkTransportConfig::Serial(id),
        "ble" => ZmkTransportConfig::Ble(id),
        _ => return None,
    };
    Some(ConnectionSpec::Zmk {
        vid: s.get("vid")?.parse().ok()?,
        pid: s.get("pid")?.parse().ok()?,
        transport,
    })
}
