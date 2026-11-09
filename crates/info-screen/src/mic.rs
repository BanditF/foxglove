//! The microphone's mute state, for the host's audio broker (#237): the default communications
//! microphone (Core Audio `eCapture`/`eCommunications`), watched through Core Audio's callbacks
//! (mute changes, default-device changes), never polled.
//!
//! Endpoint mute is what Windows' own mute toggles; it isn't a privacy guarantee (apps can mute
//! themselves, exclusive-mode streams bypass it).

/// What the default communications microphone is doing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MicState {
    Muted,
    Unmuted,
    /// No capture device.
    Unavailable,
    /// It couldn't be read; never shown as muted.
    Unknown(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mic {
    pub state: MicState,
    /// Core Audio's endpoint id, empty when unavailable.
    pub id: String,
    pub name: String,
}

#[cfg(windows)]
mod imp {
    use std::sync::mpsc::{self, Sender};

    use windows::core::{implement, Result, GUID, PCWSTR};
    use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
    use windows::Win32::Foundation::PROPERTYKEY;
    use windows::Win32::Media::Audio::Endpoints::*;
    use windows::Win32::Media::Audio::*;
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_MULTITHREADED, STGM_READ,
    };

    use super::{Mic, MicState};

    /// Marks Foxglove's own mute changes in the callbacks it receives.
    pub const OWN_CHANGES: GUID = GUID::from_u128(0x6f78_676c_6f76_6500_8000_0000_0000_0237);

    #[implement(IAudioEndpointVolumeCallback)]
    struct Volume(Sender<()>);
    impl IAudioEndpointVolumeCallback_Impl for Volume_Impl {
        fn OnNotify(&self, _: *mut AUDIO_VOLUME_NOTIFICATION_DATA) -> Result<()> {
            let _ = self.0.send(());
            Ok(())
        }
    }

    #[implement(IMMNotificationClient)]
    struct Devices(Sender<()>);
    impl IMMNotificationClient_Impl for Devices_Impl {
        fn OnDeviceStateChanged(&self, _: &PCWSTR, _: DEVICE_STATE) -> Result<()> {
            let _ = self.0.send(());
            Ok(())
        }
        fn OnDeviceAdded(&self, _: &PCWSTR) -> Result<()> {
            Ok(())
        }
        fn OnDeviceRemoved(&self, _: &PCWSTR) -> Result<()> {
            let _ = self.0.send(());
            Ok(())
        }
        fn OnDefaultDeviceChanged(&self, flow: EDataFlow, role: ERole, _: &PCWSTR) -> Result<()> {
            if flow == eCapture && role == eCommunications {
                let _ = self.0.send(());
            }
            Ok(())
        }
        fn OnPropertyValueChanged(&self, _: &PCWSTR, _: &PROPERTYKEY) -> Result<()> {
            Ok(())
        }
    }

    fn enumerator() -> std::result::Result<IMMDeviceEnumerator, String> {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
                .map_err(|e| format!("Core Audio: {e}"))
        }
    }

    fn name(device: &IMMDevice) -> String {
        unsafe {
            device
                .OpenPropertyStore(STGM_READ)
                .and_then(|p| p.GetValue(&PKEY_Device_FriendlyName))
                .map(|v| v.to_string())
                .unwrap_or_default()
        }
    }

    /// The default communications microphone and its volume control, if there is one.
    fn current(
        devices: &IMMDeviceEnumerator,
    ) -> std::result::Result<Option<(IMMDevice, IAudioEndpointVolume)>, String> {
        unsafe {
            let Ok(device) = devices.GetDefaultAudioEndpoint(eCapture, eCommunications) else {
                return Ok(None);
            };
            let volume: IAudioEndpointVolume = device
                .Activate(CLSCTX_ALL, None)
                .map_err(|e| format!("endpoint: {e}"))?;
            Ok(Some((device, volume)))
        }
    }

    fn read(found: &Option<(IMMDevice, IAudioEndpointVolume)>) -> Mic {
        let Some((device, volume)) = found else {
            return Mic {
                state: MicState::Unavailable,
                id: String::new(),
                name: String::new(),
            };
        };
        let id = unsafe {
            device
                .GetId()
                .ok()
                .and_then(|p| p.to_string().ok())
                .unwrap_or_default()
        };
        let state = match unsafe { volume.GetMute() } {
            Ok(m) if m.as_bool() => MicState::Muted,
            Ok(_) => MicState::Unmuted,
            Err(e) => MicState::Unknown(e.to_string()),
        };
        Mic {
            state,
            id,
            name: name(device),
        }
    }

    /// Calls `on_change` with the microphone's state now and after every change, from a thread
    /// of its own that sleeps between callbacks.
    pub fn watch(on_change: impl Fn(Mic) + Send + 'static) {
        let _ = std::thread::Builder::new()
            .name("mic-watch".into())
            .spawn(move || {
                let devices = match enumerator() {
                    Ok(d) => d,
                    Err(e) => {
                        return on_change(Mic {
                            state: MicState::Unknown(e),
                            id: String::new(),
                            name: String::new(),
                        })
                    }
                };
                let (tx, rx) = mpsc::channel();
                let device_events: IMMNotificationClient = Devices(tx.clone()).into();
                let volume_events: IAudioEndpointVolumeCallback = Volume(tx).into();
                unsafe {
                    let _ = devices.RegisterEndpointNotificationCallback(&device_events);
                }
                let mut last: Option<Mic> = None;
                loop {
                    // Re-bind on every wake: the default microphone may have changed.
                    let found = current(&devices).unwrap_or(None);
                    if let Some((_, v)) = &found {
                        unsafe {
                            let _ = v.RegisterControlChangeNotify(&volume_events);
                        }
                    }
                    let mic = read(&found);
                    if last.as_ref() != Some(&mic) {
                        on_change(mic.clone());
                        last = Some(mic);
                    }
                    if rx.recv().is_err() {
                        break;
                    }
                    while rx.try_recv().is_ok() {}
                    if let Some((_, v)) = &found {
                        unsafe {
                            let _ = v.UnregisterControlChangeNotify(&volume_events);
                        }
                    }
                }
            });
    }

    /// Mutes or unmutes the default communications microphone, now.
    pub fn set_mute(muted: bool) -> std::result::Result<(), String> {
        let devices = enumerator()?;
        let Some((_, volume)) = current(&devices)? else {
            return Err("no microphone".into());
        };
        unsafe {
            volume
                .SetMute(muted, &OWN_CHANGES)
                .map_err(|e| format!("mute: {e}"))
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use super::{Mic, MicState};

    pub fn watch(on_change: impl Fn(Mic) + Send + 'static) {
        on_change(Mic {
            state: MicState::Unavailable,
            id: String::new(),
            name: String::new(),
        });
    }

    pub fn set_mute(_: bool) -> Result<(), String> {
        Err("no microphone control on this system".into())
    }
}

pub use imp::{set_mute, watch};

#[cfg(test)]
mod tests {
    use super::*;

    /// Reads this PC's microphone once (no changes made): `cargo test -p foxglove-panel -- --ignored`.
    #[test]
    #[ignore]
    fn reads_the_default_microphone() {
        let (tx, rx) = std::sync::mpsc::channel();
        watch(move |m| {
            let _ = tx.send(m);
        });
        let mic = rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("a first state");
        eprintln!("{mic:?}");
        assert!(!matches!(mic.state, MicState::Unknown(_)), "{mic:?}");
    }
}
