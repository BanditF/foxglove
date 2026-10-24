//! The local API on the named pipe (#69, docs/local-api.md): outside tools and foxglove-cli
//! reach the app's facts and devices. The listener sleeps in the kernel until a client
//! connects; each client gets its own thread.

use std::sync::Arc;

use den::{Client, Context, Devices, Limits};
use foxglove_kit_api::methods::{self, ClientInfo};
use foxglove_kit_api::permission::Permission;
use foxglove_kit_api::rpc::{code, Error};
use palm::{Caps, Device as _};
use serde_json::{json, Value};

/// Starts the pipe listener (Windows; `FOXGLOVE_DISABLE=api` leaves it out).
pub fn start() {
    if super::repaint_trace::disabled("api") {
        return;
    }
    #[cfg(windows)]
    super::services::spawn_named("api-pipe", || {
        let result = den::pipe::listen(|| Context {
            facts: super::palm_host::bus().facts().clone(),
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

/// Until the Den asks the user (next PR), tools get what they ask for except the sensitive
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
    serde_json::from_value(p).map_err(|e| Error { code: code::INVALID_PARAMS, message: e.to_string(), data: None })
}

fn unsupported() -> Error {
    Error { code: code::UNSUPPORTED, message: "this device has no lighting".into(), data: None }
}

fn palm_error(e: palm::PalmError) -> Error {
    let code = match e {
        palm::PalmError::Unsupported => code::UNSUPPORTED,
        palm::PalmError::Disconnected => code::DISCONNECTED,
        palm::PalmError::Busy(_) => code::BUSY,
        palm::PalmError::Timeout => code::TIMEOUT,
        palm::PalmError::Denied(_) => code::DENIED,
        palm::PalmError::Backend(_) => code::BACKEND,
    };
    Error { code, message: super::palm_host::describe(&e), data: None }
}

/// Runs `f` on the device a call names: the host-link keyboard or an open VIA keyboard.
fn with_device<R>(id: &str, f: impl FnOnce(&dyn palm::Device) -> Result<R, Error>) -> Result<R, Error> {
    if let Some(d) = super::palm_host::cached().filter(|d| d.info().id.to_string() == id) {
        return f(d.as_ref());
    }
    super::via_host::with_device(id, f)
        .unwrap_or_else(|| Err(Error { code: code::DISCONNECTED, message: format!("no device {id}"), data: None }))
}

impl Devices for AppDevices {
    fn call(&self, method: &str, _params: Value) -> Result<Value, Error> {
        match method {
            "devices.list" => {
                let devices = super::palm_host::cached()
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
                    .chain(super::via_host::devices().into_iter().map(|(info, c)| methods::Device {
                        id: info.id.to_string(),
                        name: info.name.clone(),
                        kind: format!("{:?}", info.kind).to_lowercase(),
                        backend: info.backend.to_string(),
                        connected: true,
                        caps: caps(c),
                    }))
                    .collect();
                Ok(json!(methods::Devices { devices }))
            }
            "lighting.state" => {
                let p: methods::DeviceParams = params(_params)?;
                let st = with_device(&p.device, |d| d.lighting().ok_or_else(unsupported)?.state().map_err(palm_error))?;
                Ok(json!(methods::LightingState {
                    on: st.on,
                    hue: st.hue,
                    saturation: st.sat,
                    brightness: st.brightness,
                    effect: st.effect.id,
                    speed: st.speed,
                }))
            }
            "lighting.set" => {
                let p: methods::SetLightingParams = params(_params)?;
                with_device(&p.device, |d| {
                    let lighting = d.lighting().ok_or_else(unsupported)?;
                    if let Some(on) = p.on {
                        lighting.set_on(on).map_err(palm_error)?;
                    }
                    if let Some(c) = p.color {
                        lighting.set_color(c.hue, c.saturation, c.brightness).map_err(palm_error)?;
                    }
                    if let Some(effect) = p.effect {
                        lighting.set_effect(palm::Effect { id: effect, params: vec![] }).map_err(palm_error)?;
                    }
                    Ok(json!({}))
                })
            }
            "profiles.list" => {
                let (names, active) = super::store::profiles();
                Ok(json!(methods::Profiles { names, active }))
            }
            "profiles.activate" => {
                let p: methods::ActivateProfileParams = serde_json::from_value(_params).map_err(|e| Error {
                    code: code::INVALID_PARAMS,
                    message: e.to_string(),
                    data: None,
                })?;
                let (names, _) = super::store::profiles();
                if p.name.as_ref().is_some_and(|n| !names.contains(n)) {
                    return Err(Error { code: code::INVALID_PARAMS, message: "no such profile".into(), data: None });
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
        let asked = [Permission::DevicesRead, Permission::KeyEvents, Permission::HidRaw];
        let client = ClientInfo { name: "t".into(), version: "1".into() };
        assert_eq!(approve(&client, &asked), vec![Permission::DevicesRead]);
        assert_eq!(caps(Caps::LAYERS | Caps::SCREENS), vec!["layers", "screens"]);
    }
}
