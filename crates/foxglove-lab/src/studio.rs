//! The lab's own Studio read of the left half's behavior ids (#618): over its USB serial
//! (CDC ACM) port, `list_all_behaviors` then `get_behavior_details` for each, as the app's keymap
//! read does. Used when the app's saved keymap can't be trusted for the running build
//! ([`crate::behaviors::resolve`]). Each call has a time limit, and the list is capped.
use crate::behaviors::Catalog;
use anyhow::{bail, ensure, Result};
use std::time::Duration;

/// The running firmware's USB ids (host.rs RUNNING).
const VID: u16 = 0x1D50;
const PID: u16 = 0x615E;
/// Per Studio call, re-sends included.
const CALL_TIMEOUT: Duration = Duration::from_secs(3);
/// More behaviors than any keymap has: a reply past this is not trusted.
const MAX_BEHAVIORS: usize = 256;

/// A USB serial port as the OS lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Port {
    pub name: String,
    pub vid: u16,
    pub pid: u16,
    pub serial: Option<String>,
}

/// The left half's Studio port: the running firmware's port with the left's USB serial, else
/// the only port of the running firmware (Windows can list a composite device's interface
/// serial instead of the device's). Several and none match: an error naming them.
pub fn pick_port(ports: &[Port], left_serial: &str) -> Result<String> {
    let ours: Vec<_> = ports
        .iter()
        .filter(|p| (p.vid, p.pid) == (VID, PID))
        .collect();
    if let Some(p) = ours.iter().find(|p| {
        p.serial
            .as_deref()
            .is_some_and(|s| s.eq_ignore_ascii_case(left_serial))
    }) {
        return Ok(p.name.clone());
    }
    match ours.as_slice() {
        [one] => Ok(one.name.clone()),
        [] => bail!(
            "no USB serial port of the keyboard ({VID:04X}:{PID:04X}); the left needs USB for a \
             Studio read (or pass --keymap with a copy of this build)"
        ),
        many => bail!(
            "several keyboard serial ports and none with the left's serial {left_serial}: {}; \
             pass --studio-port",
            many.iter()
                .map(|p| p.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

/// The USB serial ports on this PC.
pub fn ports() -> Result<Vec<Port>> {
    Ok(serialport::available_ports()?
        .into_iter()
        .filter_map(|p| match p.port_type {
            serialport::SerialPortType::UsbPort(u) => Some(Port {
                name: p.port_name,
                vid: u.vid,
                pid: u.pid,
                serial: u.serial_number,
            }),
            _ => None,
        })
        .collect())
}

/// Reads the behavior ids from the Studio port `port` (the left half's, or `--studio-port`).
pub fn read(port: Option<&str>) -> Result<Catalog> {
    let port = match port {
        Some(p) => p.to_string(),
        None => pick_port(&ports()?, &crate::host::serials().left)?,
    };
    let mut client = zmk_studio_api::StudioClient::open_serial(&port).map_err(|e| {
        anyhow::anyhow!("can't open {port} for Studio: {e} (the app or ZMK Studio may hold it)")
    })?;
    client.set_call_timeout(CALL_TIMEOUT);
    let studio = |e: zmk_studio_api::ClientError| anyhow::anyhow!("Studio on {port}: {e:?}");
    let ids = client.list_all_behaviors().map_err(studio)?;
    ensure!(
        ids.len() <= MAX_BEHAVIORS,
        "Studio on {port} listed {} behaviors",
        ids.len()
    );
    let mut details = Vec::with_capacity(ids.len());
    for id in ids {
        let d = client.get_behavior_details(id).map_err(studio)?;
        details.push((id, d.display_name));
    }
    drop(client);
    let mut c = Catalog::from_details(details)?;
    c.source = format!("Studio read of {port}");
    Ok(c)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn port(name: &str, vid: u16, serial: Option<&str>) -> Port {
        Port {
            name: name.into(),
            vid,
            pid: PID,
            serial: serial.map(String::from),
        }
    }

    #[test]
    fn picks_the_left_port_by_serial_then_alone() {
        let left = port("COM5", VID, Some("ABC123"));
        let right = port("COM6", VID, Some("DEF456"));
        let other = port("COM3", 0x2341, Some("ABC123"));
        assert_eq!(
            pick_port(&[other.clone(), right.clone(), left], "abc123").unwrap(),
            "COM5"
        );
        // Windows' interface serial: the only keyboard port.
        let odd = port("COM7", VID, Some("6&1F2E"));
        assert_eq!(
            pick_port(&[other.clone(), odd.clone()], "ABC123").unwrap(),
            "COM7"
        );
        let e = pick_port(&[odd, right], "ABC123").unwrap_err().to_string();
        assert!(e.contains("COM6") && e.contains("--studio-port"), "{e}");
        assert!(pick_port(&[other], "ABC123").is_err());
    }
}
