//! Provider notifications. Callbacks only wake the panel worker; reads stay on that worker.
use std::sync::Arc;

pub type Wake = Arc<dyn Fn() + Send + Sync>;

#[cfg(windows)]
mod imp {
    use super::Wake;
    use windows::core::{implement, Result, PCWSTR};
    use windows::Foundation::TypedEventHandler;
    use windows::Media::Control::{
        GlobalSystemMediaTransportControlsSession as Session,
        GlobalSystemMediaTransportControlsSessionManager as Manager,
    };
    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, PROPERTYKEY, WPARAM};
    use windows::Win32::Media::Audio::Endpoints::*;
    use windows::Win32::Media::Audio::*;
    use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_ALL};
    use windows::Win32::UI::Accessibility::*;
    use windows::Win32::UI::WindowsAndMessaging::*;

    #[implement(IAudioEndpointVolumeCallback)]
    struct Volume(Wake);
    impl IAudioEndpointVolumeCallback_Impl for Volume_Impl {
        fn OnNotify(&self, _: *mut AUDIO_VOLUME_NOTIFICATION_DATA) -> Result<()> {
            (self.0)();
            Ok(())
        }
    }

    #[implement(IMMNotificationClient)]
    struct Devices(Wake);
    impl IMMNotificationClient_Impl for Devices_Impl {
        fn OnDeviceStateChanged(&self, _: &PCWSTR, _: DEVICE_STATE) -> Result<()> {
            (self.0)();
            Ok(())
        }
        fn OnDeviceAdded(&self, _: &PCWSTR) -> Result<()> {
            (self.0)();
            Ok(())
        }
        fn OnDeviceRemoved(&self, _: &PCWSTR) -> Result<()> {
            (self.0)();
            Ok(())
        }
        fn OnDefaultDeviceChanged(&self, _: EDataFlow, _: ERole, _: &PCWSTR) -> Result<()> {
            (self.0)();
            Ok(())
        }
        fn OnPropertyValueChanged(&self, _: &PCWSTR, _: &PROPERTYKEY) -> Result<()> {
            Ok(())
        }
    }

    thread_local! { static FOREGROUND: std::cell::RefCell<Option<Wake>> = const { std::cell::RefCell::new(None) }; }

    fn wake_foreground_thread() {
        FOREGROUND.with(|w| {
            if let Some(w) = w.borrow().as_ref() {
                w();
            }
        });
    }

    /// The hidden window's procedure: region settings ("intl", #445) and clock changes wake the
    /// panel, which re-reads the formats. Everything else goes to Windows.
    unsafe extern "system" fn settings_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        let intl = msg == WM_SETTINGCHANGE
            && lparam.0 != 0
            // SAFETY: for WM_SETTINGCHANGE a non-zero lParam is a NUL-terminated string that
            // lives for the duration of the message.
            && unsafe { PCWSTR(lparam.0 as *const u16).to_string() }.is_ok_and(|s| s == "intl");
        if intl || msg == WM_TIMECHANGE {
            crate::clock::invalidate();
            wake_foreground_thread();
        }
        // SAFETY: the default procedure, with the arguments Windows gave us.
        unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
    }

    /// A hidden top-level window on this thread, so WM_SETTINGCHANGE broadcasts reach it
    /// (message-only windows don't get broadcasts). Destroyed with the thread.
    unsafe fn settings_window() -> Option<HWND> {
        let class = windows::core::w!("FoxglovePanelSettings");
        // SAFETY: plain Win32 calls on this thread; the class name is a static string and the
        // procedure lives for the program. A second registration (another Changes) fails with
        // "class exists", which is fine.
        unsafe {
            let instance = windows::Win32::System::LibraryLoader::GetModuleHandleW(None).ok()?;
            let wc = WNDCLASSW {
                lpfnWndProc: Some(settings_proc),
                hInstance: instance.into(),
                lpszClassName: class,
                ..Default::default()
            };
            RegisterClassW(&wc);
            CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                class,
                windows::core::w!(""),
                WINDOW_STYLE::default(),
                0,
                0,
                0,
                0,
                None,
                None,
                Some(instance.into()),
                None,
            )
            .ok()
        }
    }
    unsafe extern "system" fn foreground(
        _: HWINEVENTHOOK,
        _: u32,
        _: HWND,
        _: i32,
        _: i32,
        _: u32,
        _: u32,
    ) {
        wake_foreground_thread();
    }

    pub struct Changes {
        wake: Wake,
        enumerator: Option<IMMDeviceEnumerator>,
        devices: IMMNotificationClient,
        volume: IAudioEndpointVolumeCallback,
        endpoint: Option<IAudioEndpointVolume>,
        manager: Option<Manager>,
        manager_token: Option<i64>,
        session: Option<Session>,
        media_token: Option<i64>,
        playback_token: Option<i64>,
        foreground_thread: Option<u32>,
    }

    impl Changes {
        pub fn new(wake: Wake) -> Self {
            super::super::providers::init_thread();
            let enumerator: Option<IMMDeviceEnumerator> =
                unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok() };
            let devices: IMMNotificationClient = Devices(wake.clone()).into();
            if let Some(en) = &enumerator {
                let _ = unsafe { en.RegisterEndpointNotificationCallback(&devices) };
            }
            let manager = Manager::RequestAsync().and_then(|op| op.join()).ok();
            let notify = wake.clone();
            let manager_token = manager.as_ref().and_then(|m| {
                m.CurrentSessionChanged(&TypedEventHandler::new(move |_, _| {
                    notify();
                    Ok(())
                }))
                .ok()
            });
            let (tx, rx) = std::sync::mpsc::channel();
            let notify = wake.clone();
            let _ = std::thread::Builder::new()
                .name("panel-foreground".into())
                .spawn(move || unsafe {
                    let mut msg = MSG::default();
                    let _ = PeekMessageW(&mut msg, None, 0, 0, PM_NOREMOVE);
                    let id = windows::Win32::System::Threading::GetCurrentThreadId();
                    FOREGROUND.with(|w| *w.borrow_mut() = Some(notify));
                    let hook = SetWinEventHook(
                        EVENT_SYSTEM_FOREGROUND,
                        EVENT_SYSTEM_FOREGROUND,
                        None,
                        Some(foreground),
                        0,
                        0,
                        WINEVENT_OUTOFCONTEXT,
                    );
                    let settings = settings_window();
                    if tx.send(id).is_ok() {
                        while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
                            let _ = DispatchMessageW(&msg);
                        }
                    }
                    if let Some(w) = settings {
                        let _ = DestroyWindow(w);
                    }
                    let _ = UnhookWinEvent(hook);
                    FOREGROUND.with(|w| *w.borrow_mut() = None);
                });
            let foreground_thread = rx.recv().ok();
            let mut changes = Self {
                wake: wake.clone(),
                enumerator,
                devices,
                volume: Volume(wake).into(),
                endpoint: None,
                manager,
                manager_token,
                session: None,
                media_token: None,
                playback_token: None,
                foreground_thread,
            };
            changes.refresh();
            changes
        }

        /// Rebind after a default audio device or current media session change.
        pub fn refresh(&mut self) {
            unsafe {
                if let Some(old) = self.endpoint.take() {
                    let _ = old.UnregisterControlChangeNotify(&self.volume);
                }
                self.endpoint = self
                    .enumerator
                    .as_ref()
                    .and_then(|en| en.GetDefaultAudioEndpoint(eRender, eConsole).ok())
                    .and_then(|d| d.Activate::<IAudioEndpointVolume>(CLSCTX_ALL, None).ok());
                if let Some(endpoint) = &self.endpoint {
                    let _ = endpoint.RegisterControlChangeNotify(&self.volume);
                }
            }
            let session = self
                .manager
                .as_ref()
                .and_then(|m| m.GetCurrentSession().ok());
            if session == self.session {
                return;
            }
            self.clear_session();
            if let Some(s) = &session {
                let notify = self.wake.clone();
                self.media_token = s
                    .MediaPropertiesChanged(&TypedEventHandler::new(move |_, _| {
                        notify();
                        Ok(())
                    }))
                    .ok();
                let notify = self.wake.clone();
                self.playback_token = s
                    .PlaybackInfoChanged(&TypedEventHandler::new(move |_, _| {
                        notify();
                        Ok(())
                    }))
                    .ok();
            }
            self.session = session;
        }

        fn clear_session(&mut self) {
            if let Some(s) = self.session.take() {
                if let Some(t) = self.media_token.take() {
                    let _ = s.RemoveMediaPropertiesChanged(t);
                }
                if let Some(t) = self.playback_token.take() {
                    let _ = s.RemovePlaybackInfoChanged(t);
                }
            }
        }
    }
    impl Drop for Changes {
        fn drop(&mut self) {
            self.clear_session();
            if let (Some(m), Some(t)) = (&self.manager, self.manager_token) {
                let _ = m.RemoveCurrentSessionChanged(t);
            }
            unsafe {
                if let Some(en) = &self.enumerator {
                    let _ = en.UnregisterEndpointNotificationCallback(&self.devices);
                }
                if let Some(ep) = &self.endpoint {
                    let _ = ep.UnregisterControlChangeNotify(&self.volume);
                }
                if let Some(id) = self.foreground_thread {
                    let _ = PostThreadMessageW(id, WM_QUIT, Default::default(), Default::default());
                }
            }
        }
    }
}

#[cfg(not(windows))]
mod imp {
    pub struct Changes;
    impl Changes {
        pub fn new(_: super::Wake) -> Self {
            Self
        }
        pub fn refresh(&mut self) {}
    }
}
pub use imp::Changes;
