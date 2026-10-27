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
    #[cfg(windows)]
    super::services::spawn_named("api-pipe", || {
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
    let events = super::devices::bus().subscribe(false);
    super::services::spawn_named("lease-restore", move || {
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
fn keyboard() -> Option<std::sync::Arc<digitalis::devices::DigitalisDevice>> {
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
