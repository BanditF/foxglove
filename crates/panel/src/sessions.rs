//! Apps' audio sessions (#719), Windows' own model, the Volume Mixer's: the sessions on the
//! default output device (IAudioSessionManager2), their names and icons, and their levels
//! (ISimpleAudioVolume). The knob points at one of them; nothing here is kept on disk.
//!
//! - [`start`]: one thread owns the COM objects. It learns of sessions from Windows' events:
//!   IAudioSessionNotification (a session created), IAudioSessionEvents (a session's state, its
//!   disconnect) and IMMNotificationClient (another default device). Polling: none, and no peak
//!   metering: the thread sleeps on its channel between events and requests.
//! - [`Sessions`]: what the knob asks of it: the sessions in the Active state, in Windows' order,
//!   a level step and a mute.
//! - [`step_level`], [`display_name`]: the pure parts, tested everywhere.

/// One app's session as the Volume Mixer shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Session {
    /// The session instance id (unique per session, for its lifetime).
    pub id: String,
    /// Its display name, else the program's name (as the Volume Mixer falls back).
    pub name: String,
    /// Its icon: the session's IconPath, else the program's path (the icon is the exe's).
    pub icon: Option<String>,
    pub pid: u32,
}

/// What changed, for the knob: it re-reads what it needs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Change {
    /// A session started, became active or inactive.
    Changed,
    /// A session expired or was disconnected (its app closed, its device went): `id`.
    Gone(String),
    /// Master's level or mute changed (the volume keys, the knob on master; #719's tab).
    Master,
}

/// What the knob asks of Windows' sessions. Implemented by [`start`]'s handle and by tests.
pub trait Sessions: Send + Sync {
    /// The sessions in the Active state, in Windows' order.
    fn active(&self) -> Result<Vec<Session>, String>;
    /// Moves session `id`'s level by `steps` of [`STEP`]; the new level in percent.
    fn step(&self, id: &str, steps: i32) -> Result<u8, String>;
    /// Mutes or unmutes session `id`; whether it is muted now.
    fn toggle_mute(&self, id: &str) -> Result<bool, String>;
    /// Session `id`'s level in percent, and whether it is muted.
    fn level(&self, id: &str) -> Result<(u8, bool), String>;
    /// Master's (the default output device's) level in percent, and whether it is muted.
    fn master(&self) -> Result<(u8, bool), String>;
}

/// One detent: 2 %, the step Windows' own volume keys take.
pub const STEP: i32 = 2;

/// `level` (0.0-1.0) moved by `steps` detents, on Windows' 2 % grid, within 0-100 %.
pub fn step_level(level: f32, steps: i32) -> f32 {
    let percent = (level * 100.0).round() as i32;
    // A level off the grid keeps its offset (41 % goes to 43 % or 39 %).
    let moved = percent + steps * STEP;
    moved.clamp(0, 100) as f32 / 100.0
}

/// The name the Volume Mixer shows: the session's display name, else the program's file name
/// without ".exe", else "System sounds" for that session, else the process id.
pub fn display_name(display: &str, exe_path: Option<&str>, system: bool, pid: u32) -> String {
    let display = display.trim();
    // A resource reference ("@%SystemRoot%\...,-123") is the system sounds' own name.
    if system && (display.is_empty() || display.starts_with('@')) {
        return "System sounds".into();
    }
    if !display.is_empty() && !display.starts_with('@') {
        return display.to_string();
    }
    if let Some(stem) = exe_path
        .and_then(|p| p.rsplit(['\\', '/']).next())
        .map(|f| {
            f.strip_suffix(".exe")
                .or_else(|| f.strip_suffix(".EXE"))
                .unwrap_or(f)
        })
        .filter(|s| !s.is_empty())
    {
        return stem.to_string();
    }
    format!("App {pid}")
}

#[cfg(windows)]
mod imp {
    use super::{display_name, step_level, Change, Session, Sessions};
    use std::sync::mpsc::{channel, sync_channel, Receiver, Sender, SyncSender};
    use std::sync::Arc;
    use std::time::Duration;

    use windows::core::{implement, Interface, Ref, Result, BOOL, GUID, PCWSTR, PWSTR};
    use windows::Win32::Foundation::{CloseHandle, MAX_PATH, PROPERTYKEY, S_OK};
    use windows::Win32::Media::Audio::Endpoints::{
        IAudioEndpointVolume, IAudioEndpointVolumeCallback, IAudioEndpointVolumeCallback_Impl,
    };
    use windows::Win32::Media::Audio::*;
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoTaskMemFree, CLSCTX_ALL, COINIT_MULTITHREADED,
    };
    use windows::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };

    /// Foxglove's own level changes, so a reader of the session's events can tell them apart.
    pub const OWN_CHANGES: GUID = GUID::from_u128(0x6f78_676c_6f76_6500_8000_0000_0000_0719);

    /// How long a request waits for the sessions' thread: a fallback only (it answers at once).
    const REPLY: Duration = Duration::from_secs(2);

    /// A session control handed from Windows' callback thread to ours. Both are in the MTA,
    /// where Core Audio's objects may be used from any thread.
    struct Control(IAudioSessionControl);
    // SAFETY: see above: MTA objects, used only from MTA threads.
    unsafe impl Send for Control {}

    enum Req {
        Active(SyncSender<std::result::Result<Vec<Session>, String>>),
        Step(String, i32, SyncSender<std::result::Result<u8, String>>),
        Mute(String, SyncSender<std::result::Result<bool, String>>),
        Level(String, SyncSender<std::result::Result<(u8, bool), String>>),
        MasterLevel(SyncSender<std::result::Result<(u8, bool), String>>),
        Master,
        Created(Control),
        State(String, AudioSessionState),
        Disconnected(String),
        Device,
    }

    #[implement(IAudioSessionNotification)]
    struct Created(Sender<Req>);
    impl IAudioSessionNotification_Impl for Created_Impl {
        fn OnSessionCreated(&self, newsession: Ref<IAudioSessionControl>) -> Result<()> {
            if let Ok(s) = newsession.ok() {
                let _ = self.0.send(Req::Created(Control(s.clone())));
            }
            Ok(())
        }
    }

    #[implement(IAudioSessionEvents)]
    struct Events {
        id: String,
        tx: Sender<Req>,
    }
    impl IAudioSessionEvents_Impl for Events_Impl {
        fn OnDisplayNameChanged(&self, _: &PCWSTR, _: *const GUID) -> Result<()> {
            Ok(())
        }
        fn OnIconPathChanged(&self, _: &PCWSTR, _: *const GUID) -> Result<()> {
            Ok(())
        }
        fn OnSimpleVolumeChanged(&self, _: f32, _: BOOL, _: *const GUID) -> Result<()> {
            Ok(())
        }
        fn OnChannelVolumeChanged(
            &self,
            _: u32,
            _: *const f32,
            _: u32,
            _: *const GUID,
        ) -> Result<()> {
            Ok(())
        }
        fn OnGroupingParamChanged(&self, _: *const GUID, _: *const GUID) -> Result<()> {
            Ok(())
        }
        fn OnStateChanged(&self, newstate: AudioSessionState) -> Result<()> {
            let _ = self.tx.send(Req::State(self.id.clone(), newstate));
            Ok(())
        }
        fn OnSessionDisconnected(&self, _: AudioSessionDisconnectReason) -> Result<()> {
            let _ = self.tx.send(Req::Disconnected(self.id.clone()));
            Ok(())
        }
    }

    #[implement(IAudioEndpointVolumeCallback)]
    struct MasterChanged(Sender<Req>);
    impl IAudioEndpointVolumeCallback_Impl for MasterChanged_Impl {
        fn OnNotify(&self, _: *mut AUDIO_VOLUME_NOTIFICATION_DATA) -> Result<()> {
            let _ = self.0.send(Req::Master);
            Ok(())
        }
    }

    #[implement(IMMNotificationClient)]
    struct Devices(Sender<Req>);
    impl IMMNotificationClient_Impl for Devices_Impl {
        fn OnDeviceStateChanged(&self, _: &PCWSTR, _: DEVICE_STATE) -> Result<()> {
            Ok(())
        }
        fn OnDeviceAdded(&self, _: &PCWSTR) -> Result<()> {
            Ok(())
        }
        fn OnDeviceRemoved(&self, _: &PCWSTR) -> Result<()> {
            Ok(())
        }
        fn OnDefaultDeviceChanged(&self, flow: EDataFlow, role: ERole, _: &PCWSTR) -> Result<()> {
            if flow == eRender && role == eConsole {
                let _ = self.0.send(Req::Device);
            }
            Ok(())
        }
        fn OnPropertyValueChanged(&self, _: &PCWSTR, _: &PROPERTYKEY) -> Result<()> {
            Ok(())
        }
    }

    /// Takes a string Windows allocated (CoTaskMemAlloc), and frees it.
    fn take(p: Result<PWSTR>) -> String {
        match p {
            Ok(p) if !p.is_null() => {
                // SAFETY: a NUL-terminated string Windows allocated for us; freed once, here.
                unsafe {
                    let s = p.to_string().unwrap_or_default();
                    CoTaskMemFree(Some(p.0 as *const _));
                    s
                }
            }
            _ => String::new(),
        }
    }

    fn exe_path(pid: u32) -> Option<String> {
        if pid == 0 {
            return None;
        }
        // SAFETY: plain Win32 calls; the handle is closed before returning.
        unsafe {
            let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
            let mut buf = [0u16; MAX_PATH as usize];
            let mut len = buf.len() as u32;
            let ok = QueryFullProcessImageNameW(
                process,
                PROCESS_NAME_WIN32,
                PWSTR(buf.as_mut_ptr()),
                &mut len,
            );
            let _ = CloseHandle(process);
            ok.ok()?;
            Some(String::from_utf16_lossy(&buf[..len as usize]))
        }
    }

    /// Master's level in percent and its mute.
    fn master_level(e: &IAudioEndpointVolume) -> Result<(u8, bool)> {
        // SAFETY: COM calls on the sessions' MTA thread.
        unsafe {
            let level = e.GetMasterVolumeLevelScalar()?;
            Ok(((level * 100.0).round() as u8, e.GetMute()?.as_bool()))
        }
    }

    struct Tracked {
        id: String,
        control: IAudioSessionControl,
        events: IAudioSessionEvents,
        state: AudioSessionState,
    }

    impl Tracked {
        fn session(&self) -> Session {
            // SAFETY: COM calls on an MTA object from an MTA thread.
            unsafe {
                let c2 = self.control.cast::<IAudioSessionControl2>().ok();
                let pid = c2.as_ref().and_then(|c| c.GetProcessId().ok()).unwrap_or(0);
                let system = c2
                    .as_ref()
                    .is_some_and(|c| c.IsSystemSoundsSession() == S_OK);
                let display = take(self.control.GetDisplayName());
                let icon = take(self.control.GetIconPath());
                let exe = exe_path(pid);
                Session {
                    id: self.id.clone(),
                    name: display_name(&display, exe.as_deref(), system, pid),
                    icon: Some(icon).filter(|i| !i.is_empty()).or(exe),
                    pid,
                }
            }
        }

        fn volume(&self) -> std::result::Result<ISimpleAudioVolume, String> {
            self.control
                .cast::<ISimpleAudioVolume>()
                .map_err(|e| format!("no volume for this session: {e}"))
        }
    }

    struct Worker {
        tx: Sender<Req>,
        notify: Arc<dyn Fn(Change) + Send + Sync>,
        devices: Option<IMMDeviceEnumerator>,
        devices_client: IMMNotificationClient,
        manager: Option<IAudioSessionManager2>,
        created: IAudioSessionNotification,
        endpoint: Option<IAudioEndpointVolume>,
        master_changed: IAudioEndpointVolumeCallback,
        sessions: Vec<Tracked>,
    }

    impl Worker {
        /// The default output device's session manager, and its sessions (the enumerator is
        /// asked first: Windows sends session notifications only after that).
        fn open(&mut self) {
            self.close();
            let Some(devices) = &self.devices else {
                return;
            };
            // SAFETY: COM calls on this MTA thread.
            let device = unsafe { devices.GetDefaultAudioEndpoint(eRender, eConsole) };
            // Master's level: its changes come as events (the knob's tab on master).
            if let Ok(d) = &device {
                // SAFETY: as above; unregistered in `close`.
                unsafe {
                    if let Ok(e) = d.Activate::<IAudioEndpointVolume>(CLSCTX_ALL, None) {
                        let _ = e.RegisterControlChangeNotify(&self.master_changed);
                        self.endpoint = Some(e);
                    }
                }
            }
            // SAFETY: as above.
            let manager = device
                .and_then(|d| unsafe { d.Activate::<IAudioSessionManager2>(CLSCTX_ALL, None) });
            let manager = match manager {
                Ok(m) => m,
                Err(e) => {
                    eprintln!("Foxglove: audio sessions: no output device: {e}");
                    return;
                }
            };
            // SAFETY: as above.
            unsafe {
                if let Ok(list) = manager.GetSessionEnumerator() {
                    let n = list.GetCount().unwrap_or(0);
                    for i in 0..n {
                        if let Ok(c) = list.GetSession(i) {
                            self.track(c);
                        }
                    }
                }
                if let Err(e) = manager.RegisterSessionNotification(&self.created) {
                    eprintln!("Foxglove: audio sessions: no session events: {e}");
                }
            }
            self.manager = Some(manager);
        }

        fn close(&mut self) {
            if let Some(e) = self.endpoint.take() {
                // SAFETY: COM call on this MTA thread, with the object registered in `open`.
                unsafe {
                    let _ = e.UnregisterControlChangeNotify(&self.master_changed);
                }
            }
            if let Some(m) = self.manager.take() {
                // SAFETY: COM call on this MTA thread, with the object registered above.
                unsafe {
                    let _ = m.UnregisterSessionNotification(&self.created);
                }
            }
            for t in std::mem::take(&mut self.sessions) {
                self.forget(&t);
                (self.notify)(Change::Gone(t.id));
            }
        }

        fn forget(&self, t: &Tracked) {
            // SAFETY: COM call on this MTA thread, with the object registered in `track`.
            unsafe {
                let _ = t.control.UnregisterAudioSessionNotification(&t.events);
            }
        }

        /// Follows a session (once): its state and disconnect come as events.
        fn track(&mut self, control: IAudioSessionControl) {
            // SAFETY: COM calls on this MTA thread.
            unsafe {
                let Ok(c2) = control.cast::<IAudioSessionControl2>() else {
                    return;
                };
                let id = take(c2.GetSessionInstanceIdentifier());
                if id.is_empty() || self.sessions.iter().any(|t| t.id == id) {
                    return;
                }
                let state = control.GetState().unwrap_or(AudioSessionStateInactive);
                if state == AudioSessionStateExpired {
                    return;
                }
                let events: IAudioSessionEvents = Events {
                    id: id.clone(),
                    tx: self.tx.clone(),
                }
                .into();
                if let Err(e) = control.RegisterAudioSessionNotification(&events) {
                    eprintln!("Foxglove: audio sessions: no events for a session: {e}");
                }
                self.sessions.push(Tracked {
                    id,
                    control,
                    events,
                    state,
                });
            }
        }

        fn find(&self, id: &str) -> std::result::Result<&Tracked, String> {
            self.sessions
                .iter()
                .find(|t| t.id == id)
                .ok_or_else(|| "the app's sound ended".to_string())
        }

        fn handle(&mut self, req: Req) {
            match req {
                Req::Active(reply) => {
                    let list = self
                        .sessions
                        .iter()
                        .filter(|t| t.state == AudioSessionStateActive)
                        .map(Tracked::session)
                        .collect();
                    let _ = reply.send(Ok(list));
                }
                Req::Step(id, steps, reply) => {
                    let r = self.find(&id).and_then(|t| t.volume()).and_then(|v| {
                        // SAFETY: COM calls on this MTA thread.
                        unsafe {
                            let level = v.GetMasterVolume().map_err(|e| e.to_string())?;
                            let new = step_level(level, steps);
                            v.SetMasterVolume(new, &OWN_CHANGES)
                                .map_err(|e| e.to_string())?;
                            Ok((new * 100.0).round() as u8)
                        }
                    });
                    let _ = reply.send(r);
                }
                Req::Mute(id, reply) => {
                    let r = self.find(&id).and_then(|t| t.volume()).and_then(|v| {
                        // SAFETY: COM calls on this MTA thread.
                        unsafe {
                            let muted = v.GetMute().map_err(|e| e.to_string())?.as_bool();
                            v.SetMute(!muted, &OWN_CHANGES).map_err(|e| e.to_string())?;
                            Ok(!muted)
                        }
                    });
                    let _ = reply.send(r);
                }
                Req::Level(id, reply) => {
                    let r = self.find(&id).and_then(|t| t.volume()).and_then(|v| {
                        // SAFETY: COM calls on this MTA thread.
                        unsafe {
                            let level = v.GetMasterVolume().map_err(|e| e.to_string())?;
                            let muted = v.GetMute().map_err(|e| e.to_string())?.as_bool();
                            Ok(((level * 100.0).round() as u8, muted))
                        }
                    });
                    let _ = reply.send(r);
                }
                Req::MasterLevel(reply) => {
                    let r = match &self.endpoint {
                        None => Err("no output device".to_string()),
                        Some(e) => master_level(e).map_err(|e| e.to_string()),
                    };
                    let _ = reply.send(r);
                }
                Req::Master => (self.notify)(Change::Master),
                Req::Created(Control(c)) => {
                    self.track(c);
                    (self.notify)(Change::Changed);
                }
                Req::State(id, state) => {
                    if state == AudioSessionStateExpired {
                        self.drop_session(&id);
                    } else if let Some(t) = self.sessions.iter_mut().find(|t| t.id == id) {
                        t.state = state;
                        (self.notify)(Change::Changed);
                    }
                }
                Req::Disconnected(id) => self.drop_session(&id),
                Req::Device => self.open(),
            }
        }

        fn drop_session(&mut self, id: &str) {
            if let Some(i) = self.sessions.iter().position(|t| t.id == id) {
                let t = self.sessions.remove(i);
                self.forget(&t);
                (self.notify)(Change::Gone(t.id));
            }
        }
    }

    /// The handle the knob uses: each call asks the sessions' thread and waits for its answer.
    pub struct Handle(Sender<Req>);

    impl Handle {
        fn ask<T>(
            &self,
            make: impl FnOnce(SyncSender<std::result::Result<T, String>>) -> Req,
        ) -> std::result::Result<T, String> {
            let (reply, answer) = sync_channel(1);
            self.0
                .send(make(reply))
                .map_err(|_| "the audio sessions' thread is gone".to_string())?;
            answer
                .recv_timeout(REPLY)
                .map_err(|_| "Windows' audio sessions didn't answer".to_string())?
        }
    }

    impl Sessions for Handle {
        fn active(&self) -> std::result::Result<Vec<Session>, String> {
            self.ask(Req::Active)
        }
        fn step(&self, id: &str, steps: i32) -> std::result::Result<u8, String> {
            self.ask(|r| Req::Step(id.to_string(), steps, r))
        }
        fn toggle_mute(&self, id: &str) -> std::result::Result<bool, String> {
            self.ask(|r| Req::Mute(id.to_string(), r))
        }
        fn level(&self, id: &str) -> std::result::Result<(u8, bool), String> {
            self.ask(|r| Req::Level(id.to_string(), r))
        }
        fn master(&self) -> std::result::Result<(u8, bool), String> {
            self.ask(Req::MasterLevel)
        }
    }

    /// Starts following Windows' sessions; `notify` hears each change (on the sessions' thread:
    /// only enqueue). `None` when the thread can't start.
    pub fn start(notify: impl Fn(Change) + Send + Sync + 'static) -> Option<Box<dyn Sessions>> {
        let (tx, rx): (Sender<Req>, Receiver<Req>) = channel();
        let worker_tx = tx.clone();
        let notify: Arc<dyn Fn(Change) + Send + Sync> = Arc::new(notify);
        std::thread::Builder::new()
            .name("audio-sessions".into())
            .spawn(move || {
                // SAFETY: COM for this thread, in the MTA (Core Audio's callbacks come there).
                let devices: Option<IMMDeviceEnumerator> = unsafe {
                    let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
                    CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok()
                };
                let devices_client: IMMNotificationClient = Devices(worker_tx.clone()).into();
                if let Some(d) = &devices {
                    // SAFETY: COM call on this MTA thread; unregistered never (the thread lives
                    // as long as the app).
                    unsafe {
                        let _ = d.RegisterEndpointNotificationCallback(&devices_client);
                    }
                }
                let mut w = Worker {
                    tx: worker_tx.clone(),
                    notify,
                    devices,
                    devices_client,
                    manager: None,
                    created: Created(worker_tx.clone()).into(),
                    endpoint: None,
                    master_changed: MasterChanged(worker_tx).into(),
                    sessions: Vec::new(),
                };
                w.open();
                for req in rx {
                    w.handle(req);
                }
                let _ = &w.devices_client;
            })
            .ok()?;
        Some(Box::new(Handle(tx)))
    }
}

#[cfg(windows)]
pub use imp::start;

/// Off Windows there are no sessions.
#[cfg(not(windows))]
pub fn start(_notify: impl Fn(Change) + Send + Sync + 'static) -> Option<Box<dyn Sessions>> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_detent_is_windows_2_percent_and_stays_in_range() {
        assert_eq!(step_level(0.40, 1), 0.42);
        assert_eq!(step_level(0.40, -3), 0.34);
        assert_eq!(step_level(0.41, 1), 0.43);
        assert_eq!(step_level(0.99, 2), 1.0);
        assert_eq!(step_level(0.01, -1), 0.0);
        assert_eq!(step_level(0.0, -5), 0.0);
    }

    #[test]
    fn names_fall_back_as_the_volume_mixer_does() {
        assert_eq!(display_name("Spotify", None, false, 4), "Spotify");
        assert_eq!(
            display_name(
                "",
                Some(r"C:\Program Files\Mozilla Firefox\firefox.exe"),
                false,
                9
            ),
            "firefox"
        );
        assert_eq!(
            display_name("@%SystemRoot%\\System32\\AudioSrv.Dll,-202", None, true, 0),
            "System sounds"
        );
        assert_eq!(display_name("  ", None, false, 77), "App 77");
    }
}
