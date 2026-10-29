//! The local API on the named pipe (#69, docs/local-api.md): outside tools and foxglove-cli
//! reach the app's facts and devices. The listener sleeps in the kernel until a client
//! connects; each client gets its own thread.

use std::sync::Arc;

use foxglove_devices::{Caps, Device as _};
use foxglove_plugin_api::methods::{self, ClientInfo};
use foxglove_plugin_api::permission::Permission;
use foxglove_plugin_api::rpc::{code, Error};
use foxglove_plugin_host::{Client, Context, Devices, Limits};
use serde_json::{json, Value};

/// Starts the pipe listener (Windows; `FOXGLOVE_DISABLE=api` leaves it out).
pub fn start() {
    if super::repaint_trace::disabled("api") {
        return;
    }
    restore_on_connect();
    // Supervised (#587): a restart opens the pipe again.
    #[cfg(windows)]
    let _ = super::services::spawn_supervised("api-pipe", || {
        let result = foxglove_plugin_host::pipe::listen(|| Context {
            facts: super::devices::bus().facts().clone(),
            devices: Arc::new(AppDevices),
            client: Client::Tool(Arc::new(approve)),
            app_version: env!("CARGO_PKG_VERSION").into(),
            limits: Limits::default(),
        });
        if let Err(e) = result {
            eprintln!("Foxglove API: the pipe isn't available: {e}");
        }
    });
}

/// A keyboard (re)connecting gets back lights whose restore didn't take (#293): the retry is
/// started by the connection and stopped by the readback.
fn restore_on_connect() {
    let events = super::services::First::new(super::devices::bus().subscribe(false));
    let _ = super::services::spawn_supervised("lease-restore", move || {
        let events = events.take_or(|| super::devices::bus().subscribe(false));
        for e in events {
            if e.event != foxglove_devices::DeviceEvent::Connected {
                continue;
            }
            let device = e.device.to_string();
            super::lighting_leases::with(|a| {
                let shown = with_device(&device, |d| {
                    let light = d.lighting().ok_or_else(unsupported)?;
                    a.connected(&device, light).map_err(device_error)
                });
                if let Err(e) = shown {
                    eprintln!("Foxglove: giving back {device}'s lights: {}", e.message);
                }
            });
            // Outside the arbiter's lock: a scene sends its effect again (#542).
            super::scene_run::keyboard_connected();
        }
    });
}

/// Until the Plugins tab asks the user (next PR), tools get what they ask for except the sensitive
/// permissions (key events, raw HID), which stay off. Any program of this user could already
/// read the same state; the pipe admits no one else.
fn approve(_client: &ClientInfo, asked: &[Permission]) -> Vec<Permission> {
    asked.iter().filter(|p| !p.sensitive()).cloned().collect()
}

fn caps(c: Caps) -> Vec<String> {
    [
        (Caps::KEYMAP, "keymap"),
        (Caps::LAYERS, "layers"),
        (Caps::KEY_EVENTS, "key_events"),
        (Caps::BEHAVIORS, "behaviors"),
        (Caps::LIGHTING, "lighting"),
        (Caps::COLOR_IN, "color_in"),
        (Caps::SCREENS, "screens"),
        (Caps::POWER, "power"),
        (Caps::SETTINGS, "settings"),
    ]
    .into_iter()
    .filter(|(cap, _)| c.contains(*cap))
    .map(|(_, name)| name.to_string())
    .collect()
}

pub(crate) struct AppDevices;

fn params<T: serde::de::DeserializeOwned>(p: Value) -> Result<T, Error> {
    serde_json::from_value(p).map_err(|e| Error {
        code: code::INVALID_PARAMS,
        message: e.to_string(),
        data: None,
    })
}

fn unsupported() -> Error {
    Error {
        code: code::UNSUPPORTED,
        message: "this device has no lighting".into(),
        data: None,
    }
}

fn device_error(e: foxglove_devices::DeviceError) -> Error {
    let code = match e {
        foxglove_devices::DeviceError::Unsupported => code::UNSUPPORTED,
        foxglove_devices::DeviceError::Disconnected => code::DISCONNECTED,
        foxglove_devices::DeviceError::Busy(_) => code::BUSY,
        foxglove_devices::DeviceError::Timeout => code::TIMEOUT,
        foxglove_devices::DeviceError::Denied(_) => code::DENIED,
        foxglove_devices::DeviceError::Backend(_) => code::BACKEND,
    };
    Error {
        code,
        message: super::devices::describe(&e),
        data: None,
    }
}

/// The host-link keyboard, found now if nothing has looked for it yet: plugins and tools must
/// see it without the Keyboard panel or another feature having connected first. API calls run
/// on their connection's own thread, so the discovery can wait there.
fn keyboard() -> Option<std::sync::Arc<foxglove_link::devices::FoxgloveLinkDevice>> {
    super::devices::device()
}

/// Ends the leases `ends` picks and shows what's next on each device they held.
pub(crate) fn end_leases(ends: impl Fn(&super::lighting_leases::Lease) -> bool) {
    super::lighting_leases::with(|a| {
        for device in a.end(ends) {
            let shown = with_device(&device, |d| {
                let light = d.lighting().ok_or_else(unsupported)?;
                a.apply(&device, light).map_err(device_error)
            });
            if let Err(e) = shown {
                eprintln!("Foxglove: giving back {device}'s lights: {}", e.message);
            }
        }
    });
}

/// Foxglove paused or resumed (#404): leased lights go back to the keyboard's own, or the
/// winning lease shows again. On its own thread, since giving lights back waits for both
/// halves. It takes the pause state when it runs, so a quick pause and resume ends resumed.
pub fn pause_leases() {
    super::services::spawn_named("lease-pause", || {
        super::lighting_leases::with(|a| {
            for device in a.set_paused(super::pause::paused()) {
                let shown = with_device(&device, |d| {
                    let light = d.lighting().ok_or_else(unsupported)?;
                    a.apply(&device, light).map_err(device_error)
                });
                if let Err(e) = shown {
                    eprintln!(
                        "Foxglove: {device}'s lights on pause or resume: {}",
                        e.message
                    );
                }
            }
        });
    });
}

/// A write refused because Foxglove is paused (#404).
fn paused_error() -> Error {
    Error {
        code: code::BUSY,
        message: super::pause::REFUSED.into(),
        data: None,
    }
}

/// Runs `f` on the device a call names: the host-link keyboard or an open VIA keyboard.
fn with_device<R>(
    id: &str,
    f: impl FnOnce(&dyn foxglove_devices::Device) -> Result<R, Error>,
) -> Result<R, Error> {
    if let Some(d) = keyboard().filter(|d| d.info().id.to_string() == id) {
        return f(d.as_ref());
    }
    super::via_host::with_device(id, f).unwrap_or_else(|| {
        Err(Error {
            code: code::DISCONNECTED,
            message: format!("no device {id}"),
            data: None,
        })
    })
}

impl Devices for AppDevices {
    fn call_from(&self, conn: u64, method: &str, params: Value) -> Result<Value, Error> {
        use super::lighting_leases::{self as leases, Priority};
        match method {
            "lighting.lease" => {
                let p: methods::LeaseParams =
                    serde_json::from_value(params).map_err(|e| Error {
                        code: code::INVALID_PARAMS,
                        message: e.to_string(),
                        data: None,
                    })?;
                let priority = match p.priority {
                    methods::LeasePriority::Low => Priority::Low,
                    methods::LeasePriority::Normal => Priority::Normal,
                    methods::LeasePriority::High => Priority::High,
                };
                let lease = leases::Lease {
                    id: 0,
                    conn,
                    device: p.device.clone(),
                    priority,
                    hue: p.color.hue % 360,
                    sat: p.color.saturation.min(100),
                    brightness: p.color.brightness.min(100),
                    effect: p.effect.unwrap_or_else(|| "0".into()),
                };
                let id = with_device(&p.device, |d| {
                    let light = d.lighting().ok_or_else(unsupported)?;
                    leases::with(|a| a.add(lease, light)).map_err(device_error)
                })?;
                Ok(json!(methods::Lease { lease: id }))
            }
            "lighting.update" => {
                let p: methods::LeaseUpdate =
                    serde_json::from_value(params).map_err(|e| Error {
                        code: code::INVALID_PARAMS,
                        message: e.to_string(),
                        data: None,
                    })?;
                let effect = p.effect.unwrap_or_else(|| "0".into());
                let (hue, sat, brightness) = (
                    p.color.hue % 360,
                    p.color.saturation.min(100),
                    p.color.brightness.min(100),
                );
                leases::with(|a| {
                    let showing = a
                        .update(p.lease, conn, hue, sat, brightness, &effect)
                        .map_err(|e| Error {
                            code: code::INVALID_PARAMS,
                            message: e,
                            data: None,
                        })?;
                    match showing {
                        Some(device) => with_device(&device, |d| {
                            let light = d.lighting().ok_or_else(unsupported)?;
                            a.apply(&device, light).map_err(device_error)
                        }),
                        None => Ok(()),
                    }
                })?;
                Ok(json!({}))
            }
            "lighting.release" => {
                let p: methods::Lease = serde_json::from_value(params).map_err(|e| Error {
                    code: code::INVALID_PARAMS,
                    message: e.to_string(),
                    data: None,
                })?;
                end_leases(|l| l.id == p.lease && l.conn == conn);
                Ok(json!({}))
            }
            "screens.panel_override" => {
                use base64::Engine;
                let p: methods::PanelOverrideParams =
                    serde_json::from_value(params).map_err(|e| Error {
                        code: code::INVALID_PARAMS,
                        message: e.to_string(),
                        data: None,
                    })?;
                // API 1.4: a screen id must be an info screen Foxglove draws (the first one;
                // a second info screen isn't driven yet). No id: the first, as before.
                if let Some(id) = p.screen {
                    let screens = super::screen_roles::current().unwrap_or_default();
                    check_panel_screen(&screens, id).map_err(|message| Error {
                        code: code::INVALID_PARAMS,
                        message,
                        data: None,
                    })?;
                }
                let bits = base64::engine::general_purpose::STANDARD
                    .decode(&p.bitmap)
                    .map_err(|e| Error {
                        code: code::INVALID_PARAMS,
                        message: format!("bad base64: {e}"),
                        data: None,
                    })?;
                // 0: held until replaced, released or the connection closes.
                let ttl =
                    (p.ttl_ms > 0).then(|| std::time::Duration::from_millis(u64::from(p.ttl_ms)));
                super::screens::set_override(conn, p.width, p.height, bits, ttl).map_err(|e| {
                    Error {
                        code: code::INVALID_PARAMS,
                        message: e,
                        data: None,
                    }
                })?;
                Ok(json!({}))
            }
            "screens.panel_release" => {
                super::screens::clear_override(conn);
                Ok(json!({}))
            }
            _ => self.call(method, params),
        }
    }

    fn closed(&self, conn: u64) {
        end_leases(|l| l.conn == conn);
        super::screens::clear_override(conn);
    }

    fn call(&self, method: &str, _params: Value) -> Result<Value, Error> {
        match method {
            "screens.list" => {
                let p: methods::DeviceParams = params(_params)?;
                // The host-link keyboard's screens; a VIA keyboard has none Foxglove knows.
                if !keyboard().is_some_and(|d| d.info().id.to_string() == p.device) {
                    with_device(&p.device, |_| Ok(()))?;
                    return Ok(json!(methods::Screens { screens: vec![] }));
                }
                let screens = super::screen_roles::current().unwrap_or_default();
                Ok(json!(screens_list(&screens)))
            }
            "devices.list" => {
                let devices = keyboard()
                    .map(|d| {
                        let info = d.info();
                        methods::Device {
                            id: info.id.to_string(),
                            name: info.name.clone(),
                            kind: format!("{:?}", info.kind).to_lowercase(),
                            backend: info.backend.to_string(),
                            connected: d.link().state().connected.is_some(),
                            caps: caps(d.caps()),
                        }
                    })
                    .into_iter()
                    .chain(super::via_host::devices().into_iter().map(|(info, c)| {
                        methods::Device {
                            id: info.id.to_string(),
                            name: info.name.clone(),
                            kind: format!("{:?}", info.kind).to_lowercase(),
                            backend: info.backend.to_string(),
                            connected: true,
                            caps: caps(c),
                        }
                    }))
                    .collect();
                Ok(json!(methods::Devices { devices }))
            }
            "audio.inputs.set_mute" => {
                let p: methods::SetMuteParams = params(_params)?;
                foxglove_panel::mic::set_mute(p.muted).map_err(|e| Error {
                    code: code::BACKEND,
                    message: e,
                    data: None,
                })?;
                Ok(json!({}))
            }
            "lighting.state" => {
                let p: methods::DeviceParams = params(_params)?;
                let st = with_device(&p.device, |d| {
                    d.lighting()
                        .ok_or_else(unsupported)?
                        .state()
                        .map_err(device_error)
                })?;
                Ok(json!(methods::LightingState {
                    on: st.on,
                    hue: st.hue,
                    saturation: st.sat,
                    brightness: st.brightness,
                    effect: st.effect.id,
                    speed: st.speed,
                    settled: st.settled,
                }))
            }
            "lighting.set" => {
                let p: methods::SetLightingParams = params(_params)?;
                if super::pause::paused() {
                    return Err(paused_error());
                }
                with_device(&p.device, |d| {
                    let lighting = d.lighting().ok_or_else(unsupported)?;
                    if let Some(on) = p.on {
                        lighting.set_on(on).map_err(device_error)?;
                    }
                    if let Some(c) = p.color {
                        lighting
                            .set_color(c.hue, c.saturation, c.brightness)
                            .map_err(device_error)?;
                    }
                    if let Some(effect) = p.effect {
                        lighting
                            .set_effect(foxglove_devices::Effect {
                                id: effect,
                                params: vec![],
                            })
                            .map_err(device_error)?;
                    }
                    Ok(json!({}))
                })
            }
            "lighting.sync" | "lighting.set_sync" => {
                let (device, set) = if method == "lighting.sync" {
                    let p: methods::DeviceParams = params(_params)?;
                    (p.device, None)
                } else {
                    let p: methods::SetSyncParams = params(_params)?;
                    (p.device, Some(p.on))
                };
                if set.is_some() && super::pause::paused() {
                    return Err(paused_error());
                }
                let d = keyboard()
                    .filter(|d| d.info().id.to_string() == device)
                    .ok_or_else(|| Error {
                        code: code::DISCONNECTED,
                        message: format!("no keyboard {device} with Foxglove's host link"),
                        data: None,
                    })?;
                let state = super::keyboard_settings::sync_now(Some(&d), set);
                Ok(json!(sync_result(&device, state)?))
            }
            "pc_lighting.list" => Ok(json!(pc_devices(&super::pc_lights::snapshot()))),
            "pc_lighting.set_zone" => {
                let p: methods::SetZoneParams = params(_params)?;
                if super::pause::paused() {
                    return Err(paused_error());
                }
                let (key, index, color) = find_zone(&super::pc_lights::snapshot(), &p)?;
                // Through the worker, as the page's Apply: saved, logged, and answered once
                // the worker published its snapshot (a condvar, no polling).
                let after = super::pc_lights::send_and_wait(
                    super::pc_lights::Msg::Set {
                        key: key.clone(),
                        colors: vec![(index, color)],
                    },
                    std::time::Duration::from_secs(10),
                )
                .ok_or_else(|| Error {
                    code: code::TIMEOUT,
                    message: "the PC lighting worker didn't answer".into(),
                    data: None,
                })?;
                Ok(json!(zone_readback(&after, &key, index, color)?))
            }
            "profiles.list" => {
                let (names, active) = super::store::profiles();
                Ok(json!(methods::Profiles { names, active }))
            }
            "profiles.activate" => {
                let p: methods::ActivateProfileParams =
                    serde_json::from_value(_params).map_err(|e| Error {
                        code: code::INVALID_PARAMS,
                        message: e.to_string(),
                        data: None,
                    })?;
                let (names, _) = super::store::profiles();
                if p.name.as_ref().is_some_and(|n| !names.contains(n)) {
                    return Err(Error {
                        code: code::INVALID_PARAMS,
                        message: "no such profile".into(),
                        data: None,
                    });
                }
                super::store::set_active(p.name.as_deref()).map_err(|e| Error {
                    code: code::BACKEND,
                    message: e.to_string(),
                    data: None,
                })?;
                let (names, active) = super::store::profiles();
                Ok(json!(methods::Profiles { names, active }))
            }
            _ => Err(Error {
                code: code::UNSUPPORTED,
                message: format!("{method} is not available yet"),
                data: None,
            }),
        }
    }
}

/// `lighting.sync`'s answer from what the keyboard read back.
fn sync_result(
    device: &str,
    state: super::keyboard_settings::RgbSync,
) -> Result<methods::SyncState, Error> {
    use super::keyboard_settings::RgbSync;
    match state {
        RgbSync::Known { on } => Ok(methods::SyncState {
            device: device.into(),
            on,
        }),
        RgbSync::NoKeyboard => Err(Error {
            code: code::DISCONNECTED,
            message: "no keyboard with Foxglove's host link".into(),
            data: None,
        }),
        _ => Err(Error {
            code: code::UNSUPPORTED,
            message: "this keyboard's firmware has no lighting sync (or didn't answer)".into(),
            data: None,
        }),
    }
}

/// Whether Foxglove writes this device's colours itself.
fn writable(r: &super::pc_lights::Row) -> bool {
    matches!(r.control, foxglove_devices::pc_lighting::Control::Driver(_))
}

/// `pc_lighting.list` (#617): every PC lighting device the Lights page lists.
fn pc_devices(s: &super::pc_lights::Snapshot) -> methods::PcDevices {
    methods::PcDevices {
        devices: s
            .devices
            .iter()
            .map(|r| methods::PcDevice {
                device: r.key.clone(),
                name: r.name.clone(),
                writable: writable(r),
                match_keyboard: r.match_keyboard,
                zones: r
                    .zones
                    .iter()
                    .zip(&r.zone_ids)
                    .map(|((name, c), id)| methods::PcZone {
                        id: id.clone(),
                        name: name.clone(),
                        color: c.map(super::pc_lights::hex),
                    })
                    .collect(),
            })
            .collect(),
        status: s.status.clone(),
        conflicts: s
            .conflicts
            .iter()
            .map(|c| format!("{} on {}", c.app, c.device))
            .collect(),
    }
}

fn invalid(message: String) -> Error {
    Error {
        code: code::INVALID_PARAMS,
        message,
        data: None,
    }
}

/// The device (by key, or a name only one device has), the zone (by id or name) and the
/// colour (`#rrggbb` or `rrggbb`) a `pc_lighting.set_zone` names.
fn find_zone(
    s: &super::pc_lights::Snapshot,
    p: &methods::SetZoneParams,
) -> Result<(String, usize, [u8; 3]), Error> {
    let want = p.device.to_lowercase();
    let by_name: Vec<_> = s
        .devices
        .iter()
        .filter(|r| r.name.to_lowercase().contains(&want))
        .collect();
    let by_key = s
        .devices
        .iter()
        .find(|r| r.key.eq_ignore_ascii_case(&p.device));
    let row = match (by_key, by_name.as_slice()) {
        (Some(r), _) => r,
        (None, [r]) => *r,
        (None, []) => {
            return Err(Error {
                code: code::DISCONNECTED,
                message: format!("no PC lighting device {:?}", p.device),
                data: None,
            })
        }
        (None, _) => {
            return Err(invalid(format!(
                "{:?} names more than one device; use its key",
                p.device
            )))
        }
    };
    if !writable(row) {
        return Err(Error {
            code: code::UNSUPPORTED,
            message: format!("Foxglove can't set {}'s colours yet", row.name),
            data: None,
        });
    }
    let index = row
        .zone_ids
        .iter()
        .position(|id| id == &p.zone)
        .or_else(|| {
            row.zones
                .iter()
                .position(|(name, _)| name.eq_ignore_ascii_case(&p.zone))
        })
        .ok_or_else(|| {
            let zones: Vec<_> = row
                .zone_ids
                .iter()
                .zip(&row.zones)
                .map(|(id, (name, _))| format!("{id} ({name})"))
                .collect();
            invalid(format!(
                "{} has no zone {:?}; its zones: {}",
                row.name,
                p.zone,
                zones.join(", ")
            ))
        })?;
    let hex = p.color.trim();
    let hex = if hex.starts_with('#') {
        hex.to_string()
    } else {
        format!("#{hex}")
    };
    let color = super::pc_lights::parse_hex(&hex)
        .ok_or_else(|| invalid(format!("{:?} isn't a #rrggbb colour", p.color)))?;
    Ok((row.key.clone(), index, color))
}

/// The answer after a zone write: the devices, if the zone now shows the colour; else the
/// worker's status as the error.
fn zone_readback(
    s: &super::pc_lights::Snapshot,
    key: &str,
    index: usize,
    color: [u8; 3],
) -> Result<methods::PcDevices, Error> {
    let shown = s
        .devices
        .iter()
        .find(|r| r.key == key)
        .and_then(|r| r.zones.get(index))
        .and_then(|(_, c)| *c);
    if shown == Some(color) {
        return Ok(pc_devices(s));
    }
    Err(Error {
        code: code::BACKEND,
        message: s
            .status
            .clone()
            .unwrap_or_else(|| "the zone didn't take the colour".into()),
        data: None,
    })
}

/// `screens.list` (API 1.4, #560): each screen by id, role, name and size.
fn screens_list(screens: &[super::screen_roles::Screen]) -> methods::Screens {
    methods::Screens {
        screens: screens
            .iter()
            .map(|s| methods::ScreenInfo {
                id: s.id,
                role: s.role.api_name().into(),
                name: s.name.clone(),
                width: s.width,
                height: s.height,
                format: s.format.clone(),
                rotation: s.rotation,
            })
            .collect(),
    }
}

/// Whether a plugin may draw on screen `id`: the first info screen, the one Foxglove draws.
fn check_panel_screen(screens: &[super::screen_roles::Screen], id: u8) -> Result<(), String> {
    use super::screen_roles::Role;
    match super::screen_roles::first(screens, Role::Info) {
        Some(s) if s.id == id => Ok(()),
        _ => match screens.iter().find(|s| s.id == id) {
            Some(s) => Err(format!("{} isn't an info screen Foxglove draws", s.the())),
            None => Err(format!("no screen {id}")),
        },
    }
}

#[cfg(test)]
mod screens_tests {
    use super::*;
    use crate::foxglove::fixtures::{second_desk, users_desk};
    use crate::foxglove::screen_roles::of_board;

    /// #560: `screens.list` on the second desk's three screens, and which one plugins draw on.
    #[test]
    fn screens_list_names_each_screen_by_role_and_size() {
        let plank = of_board(&second_desk::board());
        let list = screens_list(&plank);
        let got: Vec<_> = list
            .screens
            .iter()
            .map(|s| (s.id, s.role.as_str(), s.name.as_str(), s.width, s.height))
            .collect();
        assert_eq!(
            got,
            [
                (0, "info", "Info screen", 128, 32),
                (1, "art", "Art screen", 160, 80),
                (2, "display", "Display", 240, 135),
            ]
        );
        assert_eq!(check_panel_screen(&plank, 0), Ok(()));
        assert_eq!(
            check_panel_screen(&plank, 1),
            Err("the art screen isn't an info screen Foxglove draws".into())
        );
        assert_eq!(check_panel_screen(&plank, 9), Err("no screen 9".into()));
        // The Corne: its info screen 64x52, its art screen stored turned.
        let corne = screens_list(&of_board(&users_desk::board()));
        assert_eq!(
            (
                corne.screens[0].width,
                corne.screens[0].height,
                corne.screens[1].rotation
            ),
            (64, 52, 1)
        );
        // An old plugin's call (no screen id) still parses.
        let old: methods::PanelOverrideParams = serde_json::from_value(json!({
            "device": "d", "width": 64, "height": 52, "bitmap": "", "ttl_ms": 0
        }))
        .unwrap();
        assert_eq!(old.screen, None);
    }
}

#[cfg(test)]
mod parity_tests {
    use super::*;
    use crate::foxglove::fixtures::{second_desk, users_desk};
    use crate::foxglove::keyboard_settings::RgbSync;
    use crate::foxglove::pc_lights::Snapshot;

    fn desk() -> Snapshot {
        let mut omen = users_desk::catalog_row((0x103c, 0x84fd));
        omen.zones[0].1 = Some([0xa5, 0x75, 0x00]);
        Snapshot {
            devices: vec![
                omen,
                second_desk::case_row(),
                users_desk::mouse_row(),
                users_desk::catalog_row((0x03f0, 0x7397)),
            ],
            status: Some("HP OMEN 45L case lights: colours set".into()),
            conflicts: vec![crate::foxglove::pc_lights::Conflicting {
                app: "OMEN Light Studio".into(),
                device: "HP OMEN 45L case lights".into(),
                key: "103c:84fd".into(),
                hint: None,
            }],
            ..Snapshot::default()
        }
    }

    fn zone(device: &str, zone: &str, color: &str) -> methods::SetZoneParams {
        methods::SetZoneParams {
            device: device.into(),
            zone: zone.into(),
            color: color.into(),
        }
    }

    /// #617: `pc_lighting.list` gives each device's zones by id and name with `#rrggbb`, and
    /// whether Foxglove can write it.
    #[test]
    fn pc_lighting_list_reads_back_each_zone() {
        let list = pc_devices(&desk());
        let omen = &list.devices[0];
        assert_eq!((omen.device.as_str(), omen.writable), ("103c:84fd", true));
        assert_eq!(omen.zones[0].color.as_deref(), Some("#a57500"));
        assert_eq!(omen.zones[0].id, "1");
        assert_eq!(omen.zones[1].color, None);
        assert!(
            list.devices[1].writable,
            "the second desk's case (any driver)"
        );
        assert!(!list.devices[2].writable, "a LampArray is Windows'");
        assert!(
            !list.devices[3].writable,
            "the cooler isn't controllable yet"
        );
        assert_eq!(
            list.status.as_deref(),
            Some("HP OMEN 45L case lights: colours set")
        );
        assert_eq!(
            list.conflicts,
            ["OMEN Light Studio on HP OMEN 45L case lights"],
            "a vendor tool running is detected"
        );
    }

    /// The device by key or by a unique part of its name, the zone by id or name, the colour
    /// with or without `#`; everything else is refused with a reason.
    #[test]
    fn set_zone_resolves_device_zone_and_colour() {
        let s = desk();
        let first = s.devices[0].zones[0].0.clone();
        assert_eq!(
            find_zone(&s, &zone("103c:84fd", "1", "#ff0000")).unwrap(),
            ("103c:84fd".into(), 0, [0xff, 0, 0])
        );
        assert_eq!(
            find_zone(&s, &zone("45l", &first.to_uppercase(), "A57500")).unwrap(),
            ("103c:84fd".into(), 0, [0xa5, 0x75, 0])
        );
        let err = |p| find_zone(&s, &p).unwrap_err();
        let e = err(zone("103c:84fd", "99", "#ff0000"));
        assert_eq!(e.code, code::INVALID_PARAMS);
        assert!(e.message.contains("its zones: 1 ("), "{}", e.message);
        assert_eq!(
            err(zone("103c:84fd", "1", "red")).code,
            code::INVALID_PARAMS
        );
        assert_eq!(
            err(zone("nothing", "1", "#ff0000")).code,
            code::DISCONNECTED
        );
        assert_eq!(
            err(zone("046d:c332", "1", "#ff0000")).code,
            code::UNSUPPORTED
        );
        assert_eq!(
            err(zone("omen", "1", "#ff0000")).code,
            code::INVALID_PARAMS,
            "two OMEN devices"
        );
    }

    /// The write's answer is the readback: the zone shows the colour, or the worker's status
    /// comes back as the error.
    #[test]
    fn set_zone_answers_with_the_readback() {
        let s = desk();
        assert!(zone_readback(&s, "103c:84fd", 0, [0xa5, 0x75, 0]).is_ok());
        let e = zone_readback(&s, "103c:84fd", 0, [1, 2, 3]).unwrap_err();
        assert_eq!(
            (e.code, e.message.as_str()),
            (code::BACKEND, "HP OMEN 45L case lights: colours set")
        );
    }

    #[test]
    fn sync_answers_from_the_keyboard_readback() {
        assert_eq!(
            sync_result("k", RgbSync::Known { on: false }).unwrap(),
            methods::SyncState {
                device: "k".into(),
                on: false
            }
        );
        assert_eq!(
            sync_result("k", RgbSync::NotSupported).unwrap_err().code,
            code::UNSUPPORTED
        );
        assert_eq!(
            sync_result("k", RgbSync::NoKeyboard).unwrap_err().code,
            code::DISCONNECTED
        );
    }

    /// The new calls use the existing permissions: reads need `devices.read`, writes
    /// `lighting.write`.
    #[test]
    fn the_new_calls_use_the_existing_permissions() {
        let need = |m: &str| methods::find(m).and_then(|m| m.permission);
        assert_eq!(need("pc_lighting.list"), Some("devices.read"));
        assert_eq!(need("lighting.sync"), Some("devices.read"));
        assert_eq!(need("pc_lighting.set_zone"), Some("lighting.write"));
        assert_eq!(need("lighting.set_sync"), Some("lighting.write"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tools_never_get_sensitive_permissions_unasked() {
        let asked = [
            Permission::DevicesRead,
            Permission::KeyEvents,
            Permission::HidRaw,
        ];
        let client = ClientInfo {
            name: "t".into(),
            version: "1".into(),
        };
        assert_eq!(approve(&client, &asked), vec![Permission::DevicesRead]);
        assert_eq!(
            caps(Caps::LAYERS | Caps::SCREENS),
            vec!["layers", "screens"]
        );
    }
}
