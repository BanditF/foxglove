//! Probes one Windows API and exits with what happened, for the sandbox tests (#215):
//! `smtc` (the media sessions, as the panel's media provider reads them) and `mic` (the default
//! communications microphone's mute state, through Core Audio).
//!
//! Exit codes: 0 worked, 3 refused or failed (the error goes to stderr), 4 nothing to probe
//! (no microphone), 2 usage.

fn main() {
    let mode = std::env::args().nth(1).unwrap_or_default();
    let result = match mode.as_str() {
        "smtc" => smtc(),
        "mic" => mic(),
        _ => {
            eprintln!("sandbox-probe smtc|mic");
            std::process::exit(2);
        }
    };
    match result {
        Ok(true) => std::process::exit(0),
        Ok(false) => std::process::exit(4),
        Err(e) => {
            eprintln!("sandbox-probe {mode}: {e}");
            std::process::exit(3);
        }
    }
}

#[cfg(windows)]
fn smtc() -> Result<bool, String> {
    use windows::Media::Control::GlobalSystemMediaTransportControlsSessionManager as Manager;
    let manager = Manager::RequestAsync()
        .map_err(|e| e.to_string())?
        .join()
        .map_err(|e| e.to_string())?;
    // Reading the current session (or none) is what the media provider does.
    let _ = manager.GetCurrentSession();
    Ok(true)
}

#[cfg(windows)]
fn mic() -> Result<bool, String> {
    use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
    use windows::Win32::Media::Audio::{
        eCapture, eCommunications, IMMDeviceEnumerator, MMDeviceEnumerator,
    };
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_MULTITHREADED,
    };
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let devices: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
            .map_err(|e| format!("enumerator: {e}"))?;
        let Ok(mic) = devices.GetDefaultAudioEndpoint(eCapture, eCommunications) else {
            return Ok(false);
        };
        let volume: IAudioEndpointVolume = mic
            .Activate(CLSCTX_ALL, None)
            .map_err(|e| format!("activate: {e}"))?;
        let _muted = volume.GetMute().map_err(|e| format!("mute state: {e}"))?;
        Ok(true)
    }
}

#[cfg(not(windows))]
fn smtc() -> Result<bool, String> {
    Ok(false)
}

#[cfg(not(windows))]
fn mic() -> Result<bool, String> {
    Ok(false)
}
