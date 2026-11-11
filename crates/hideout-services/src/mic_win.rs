// SPDX-License-Identifier: MIT
//! Core Audio for the microphone (contract §8.11): `IMMDeviceEnumerator` for the default
//! communications capture endpoint, `IAudioEndpointVolume` for its mute, and two callbacks that
//! put [`Event`]s on the microphone's channel: `IMMNotificationClient` (default-device and
//! device-state changes) and `IAudioEndpointVolumeCallback` (`OnNotify`). Never polled.
//!
//! The same calls `crates/info-screen`'s microphone watch makes (MIT, this repository).

use std::sync::mpsc::Sender;

use windows::core::{implement, Result, GUID, PCWSTR};
use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use windows::Win32::Foundation::PROPERTYKEY;
use windows::Win32::Media::Audio::Endpoints::*;
use windows::Win32::Media::Audio::*;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_MULTITHREADED, STGM_READ,
};

use super::mic::{Backend, Endpoint, Event, MuteError, OWN_CONTEXT};

const OWN: GUID = GUID::from_u128(OWN_CONTEXT);

#[implement(IAudioEndpointVolumeCallback)]
struct Volume(Sender<Event>);

impl IAudioEndpointVolumeCallback_Impl for Volume_Impl {
    fn OnNotify(&self, data: *mut AUDIO_VOLUME_NOTIFICATION_DATA) -> Result<()> {
        // SAFETY: Core Audio passes a valid notification for the length of the call.
        if let Some(d) = unsafe { data.as_ref() } {
            let _ = self.0.send(Event::Volume {
                muted: d.bMuted.as_bool(),
                own: d.guidEventContext == OWN,
            });
        }
        Ok(())
    }
}

#[implement(IMMNotificationClient)]
struct Devices(Sender<Event>);

impl IMMNotificationClient_Impl for Devices_Impl {
    fn OnDeviceStateChanged(&self, _: &PCWSTR, _: DEVICE_STATE) -> Result<()> {
        let _ = self.0.send(Event::Devices);
        Ok(())
    }
    fn OnDeviceAdded(&self, _: &PCWSTR) -> Result<()> {
        let _ = self.0.send(Event::Devices);
        Ok(())
    }
    fn OnDeviceRemoved(&self, _: &PCWSTR) -> Result<()> {
        let _ = self.0.send(Event::Devices);
        Ok(())
    }
    fn OnDefaultDeviceChanged(&self, flow: EDataFlow, role: ERole, _: &PCWSTR) -> Result<()> {
        if flow == eCapture && role == eCommunications {
            let _ = self.0.send(Event::Devices);
        }
        Ok(())
    }
    fn OnPropertyValueChanged(&self, _: &PCWSTR, _: &PROPERTYKEY) -> Result<()> {
        Ok(())
    }
}

/// Core Audio, on the microphone's thread (a multithreaded apartment).
pub struct CoreAudio {
    devices: IMMDeviceEnumerator,
    client: IMMNotificationClient,
    volume_events: IAudioEndpointVolumeCallback,
    bound: Option<IAudioEndpointVolume>,
}

impl CoreAudio {
    /// Joins the MTA, makes the enumerator and registers for device events.
    pub fn new(events: Sender<Event>) -> std::result::Result<CoreAudio, String> {
        // SAFETY: plain COM calls on this thread; the objects live as long as `CoreAudio`.
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            let devices: IMMDeviceEnumerator =
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
                    .map_err(|e| format!("Core Audio: {e}"))?;
            let client: IMMNotificationClient = Devices(events.clone()).into();
            devices
                .RegisterEndpointNotificationCallback(&client)
                .map_err(|e| format!("device events: {e}"))?;
            Ok(CoreAudio {
                devices,
                client,
                volume_events: Volume(events).into(),
                bound: None,
            })
        }
    }

    fn unbind(&mut self) {
        if let Some(v) = self.bound.take() {
            // SAFETY: the callback was registered on this endpoint by `bind_default`.
            unsafe {
                let _ = v.UnregisterControlChangeNotify(&self.volume_events);
            }
        }
    }
}

impl Drop for CoreAudio {
    fn drop(&mut self) {
        self.unbind();
        // SAFETY: registered in `new`.
        unsafe {
            let _ = self
                .devices
                .UnregisterEndpointNotificationCallback(&self.client);
        }
    }
}

fn friendly_name(device: &IMMDevice) -> String {
    // SAFETY: reading one property of a live device.
    unsafe {
        device
            .OpenPropertyStore(STGM_READ)
            .and_then(|p| p.GetValue(&PKEY_Device_FriendlyName))
            .map(|v| v.to_string())
            .unwrap_or_default()
    }
}

/// `E_NOTFOUND` (HRESULT_FROM_WIN32(ERROR_NOT_FOUND)): no endpoint for that role.
const E_NOTFOUND: i32 = 0x8007_0490_u32 as i32;

impl Backend for CoreAudio {
    fn bind_default(&mut self) -> std::result::Result<Option<Endpoint>, String> {
        self.unbind();
        // SAFETY: COM calls on this thread; the volume callback is unregistered in `unbind`.
        unsafe {
            let device = match self
                .devices
                .GetDefaultAudioEndpoint(eCapture, eCommunications)
            {
                Ok(d) => d,
                Err(e) if e.code().0 == E_NOTFOUND => return Ok(None),
                Err(e) => return Err(format!("default microphone: {e}")),
            };
            let id = device
                .GetId()
                .ok()
                .and_then(|p| {
                    let s = p.to_string().ok();
                    windows::Win32::System::Com::CoTaskMemFree(Some(p.0 as _));
                    s
                })
                .unwrap_or_default();
            let volume: IAudioEndpointVolume = device
                .Activate(CLSCTX_ALL, None)
                .map_err(|e| format!("endpoint volume: {e}"))?;
            volume
                .RegisterControlChangeNotify(&self.volume_events)
                .map_err(|e| format!("mute events: {e}"))?;
            self.bound = Some(volume);
            Ok(Some(Endpoint {
                id,
                name: friendly_name(&device),
            }))
        }
    }

    fn muted(&mut self) -> std::result::Result<bool, String> {
        let v = self.bound.as_ref().ok_or("no microphone bound")?;
        // SAFETY: a live endpoint volume.
        unsafe {
            v.GetMute()
                .map(|m| m.as_bool())
                .map_err(|e| format!("mute: {e}"))
        }
    }

    fn set_mute(&mut self, muted: bool) -> std::result::Result<(), MuteError> {
        let v = self.bound.as_ref().ok_or(MuteError::Unavailable)?;
        // SAFETY: a live endpoint volume; the context GUID outlives the call.
        unsafe {
            v.SetMute(muted, &OWN)
                .map_err(|e| MuteError::Backend(e.to_string()))
        }
    }
}
