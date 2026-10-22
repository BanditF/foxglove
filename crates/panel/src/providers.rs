//! What the panel shows, read from Windows: volume (Core Audio), media (the system media
//! controls, as on the lock screen) and the foreground program (for per-game hints).

#[cfg(windows)]
mod imp {
    use windows::core::Result;
    use windows::Media::Control::{
        GlobalSystemMediaTransportControlsSessionManager as SessionManager,
        GlobalSystemMediaTransportControlsSessionPlaybackStatus as PlaybackStatus,
    };
    use windows::Win32::Foundation::{CloseHandle, MAX_PATH};
    use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
    use windows::Win32::Media::Audio::{eConsole, eRender, IMMDeviceEnumerator, MMDeviceEnumerator};
    use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_MULTITHREADED};
    use windows::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};

    pub fn init_thread() {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        }
    }

    /// Default output device: (percent, muted). Asks again every call, so it follows device
    /// switches.
    pub fn volume() -> Option<(u8, bool)> {
        let get = || -> Result<(u8, bool)> {
            unsafe {
                let en: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
                let dev = en.GetDefaultAudioEndpoint(eRender, eConsole)?;
                let vol: IAudioEndpointVolume = dev.Activate(CLSCTX_ALL, None)?;
                let level = vol.GetMasterVolumeLevelScalar()?;
                let muted = vol.GetMute()?.as_bool();
                Ok(((level * 100.0).round() as u8, muted))
            }
        };
        get().ok()
    }

    pub struct Media {
        manager: Option<SessionManager>,
    }

    /// (title, artist, playing) of the session Windows considers current.
    impl Media {
        pub fn new() -> Self {
            let manager = SessionManager::RequestAsync().and_then(|op| op.join()).ok();
            Media { manager }
        }

        pub fn now_playing(&mut self) -> Option<(String, String, bool)> {
            if self.manager.is_none() {
                *self = Media::new();
            }
            let get = || -> Result<(String, String, bool)> {
                let session = self.manager.as_ref().unwrap().GetCurrentSession()?;
                let props = session.TryGetMediaPropertiesAsync()?.join()?;
                let playing = session.GetPlaybackInfo()?.PlaybackStatus()? == PlaybackStatus::Playing;
                Ok((props.Title()?.to_string(), props.Artist()?.to_string(), playing))
            };
            get().ok().filter(|(title, _, _)| !title.is_empty())
        }
    }

    /// File name of the program owning the foreground window, e.g. "valorant.exe".
    pub fn foreground_exe() -> Option<String> {
        unsafe {
            let hwnd = GetForegroundWindow();
            let mut pid = 0u32;
            GetWindowThreadProcessId(hwnd, Some(&mut pid));
            if pid == 0 {
                return None;
            }
            let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
            let mut buf = [0u16; MAX_PATH as usize];
            let mut len = buf.len() as u32;
            let ok = QueryFullProcessImageNameW(
                process,
                PROCESS_NAME_WIN32,
                windows::core::PWSTR(buf.as_mut_ptr()),
                &mut len,
            );
            let _ = CloseHandle(process);
            ok.ok()?;
            let path = String::from_utf16_lossy(&buf[..len as usize]);
            path.rsplit(['\\', '/']).next().map(|s| s.to_string())
        }
    }
}

#[cfg(not(windows))]
mod imp {
    pub fn init_thread() {}
    pub fn volume() -> Option<(u8, bool)> {
        None
    }
    pub struct Media;
    impl Media {
        pub fn new() -> Self {
            Media
        }
        pub fn now_playing(&mut self) -> Option<(String, String, bool)> {
            None
        }
    }
    pub fn foreground_exe() -> Option<String> {
        None
    }
}

pub use imp::*;

impl Default for Media {
    fn default() -> Self {
        Self::new()
    }
}
