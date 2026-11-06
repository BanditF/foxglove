//! Apps' audio sessions (#719), Windows' own model, the Volume Mixer's: the sessions on the
//! default output device (IAudioSessionManager2), their names and icons, and their levels
//! (ISimpleAudioVolume). The knob points at one of them; nothing here is kept on disk.
//!
//! - [`start`]: one thread owns the COM objects. It learns of sessions from Windows' events:
//!   IAudioSessionNotification (a session created), IAudioSessionEvents (a session's state, its
//!   disconnect) and IMMNotificationClient (another default device). Polling: none: the thread
//!   sleeps on its channel between events and requests. Peaks (IAudioMeterInformation) are read
//!   only when the knob asks ([`Sessions::peaks`]), one read per active session.
//! - [`Sessions`]: what the knob asks of it: the sessions in the Active state, in Windows' order,
//!   their peaks now, a level step and a mute.
//! - [`Heard`] (#854): the sessions heard during this run, the only ones the knob offers. An
//!   Active session that never sends sound (a background helper holding a silent stream) isn't.
//! - [`step_level`], [`display_name`], [`Heard`]: the pure parts, tested everywhere.

/// One app's session as the Volume Mixer shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Session {
    /// The session instance id (unique per session, for its lifetime).
    pub id: String,
    /// Its display name, else the program's description, else its name ([`display_name`]).
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
    /// Each Active session's peak now (0.0-1.0; one meter read each), by session id.
    fn peaks(&self) -> Result<Vec<(String, f32)>, String>;
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

/// A peak at or under this (-80 dB) is silence: a stream that is open but sends nothing.
pub const AUDIBLE: f32 = 1e-4;

/// The sessions heard during this run, in the order first heard (#854). The knob offers only
/// these: an Active session whose peak never rose above [`AUDIBLE`] (a background helper holding
/// a silent stream open) isn't offered. Once heard a session stays offered (a pause or a mute
/// doesn't drop it) until it ends, and its place never moves: a new one goes last.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Heard(Vec<String>);

impl Heard {
    /// Notes the sessions audible in `peaks`; whether any was new.
    pub fn note(&mut self, peaks: &[(String, f32)]) -> bool {
        let mut new = false;
        for (id, peak) in peaks {
            if *peak > AUDIBLE && !self.has(id) {
                self.0.push(id.clone());
                new = true;
            }
        }
        new
    }

    pub fn has(&self, id: &str) -> bool {
        self.0.iter().any(|h| h == id)
    }

    /// Session `id` ended: it is forgotten (its id never comes back).
    pub fn forget(&mut self, id: &str) {
        self.0.retain(|h| h != id);
    }

    /// Of `active`, the heard sessions, in the order first heard.
    pub fn offer(&self, mut active: Vec<Session>) -> Vec<Session> {
        active.retain(|s| self.has(&s.id));
        active.sort_by_key(|s| self.0.iter().position(|h| *h == s.id));
        active
    }
}

/// The name to show: the session's display name, else "System sounds" for that session, else
/// the program's description (its version resource's FileDescription, as Task Manager shows),
/// else its file name without ".exe", else the process id.
pub fn display_name(
    display: &str,
    exe_path: Option<&str>,
    description: Option<&str>,
    system: bool,
    pid: u32,
) -> String {
    let display = display.trim();
    // A resource reference ("@%SystemRoot%\...,-123") is the system sounds' own name.
    if system && (display.is_empty() || display.starts_with('@')) {
        return "System sounds".into();
    }
    if !display.is_empty() && !display.starts_with('@') {
        return display.to_string();
    }
    if let Some(d) = description.map(str::trim).filter(|d| !d.is_empty()) {
        return d.to_string();
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
        IAudioMeterInformation,
    };
    use windows::Win32::Media::Audio::*;
    use windows::Win32::Storage::FileSystem::{
        GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW,
    };
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
        Peaks(SyncSender<std::result::Result<Vec<(String, f32)>, String>>),
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

    /// The program's FileDescription from its version resource (what Task Manager shows), in
    /// the file's own language first, else US English.
    fn file_description(path: &str) -> Option<String> {
        let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
        let file = PCWSTR(wide.as_ptr());
        // SAFETY: Win32 calls on a buffer of the size Windows asked for; every pointer
        // VerQueryValueW returns points into `data` and is read only while `data` lives.
        unsafe {
            let size = GetFileVersionInfoSizeW(file, None);
            if size == 0 {
                return None;
            }
            let mut data = vec![0u8; size as usize];
            GetFileVersionInfoW(file, None, size, data.as_mut_ptr().cast()).ok()?;
            let block = data.as_ptr().cast();
            // A value as u16s: `len` counts bytes for binary values and characters for strings.
            let query = |key: &str, bytes: bool| -> Option<Vec<u16>> {
                let key: Vec<u16> = key.encode_utf16().chain(std::iter::once(0)).collect();
                let mut ptr = std::ptr::null_mut();
                let mut len = 0u32;
                let found =
                    VerQueryValueW(block, PCWSTR(key.as_ptr()), &mut ptr, &mut len).as_bool();
                let len = if bytes { len / 2 } else { len } as usize;
                (found && !ptr.is_null() && len > 0)
                    .then(|| std::slice::from_raw_parts(ptr as *const u16, len).to_vec())
            };
            // The translations: (language, code page) pairs of u16s.
            let mut langs: Vec<(u16, u16)> = query("\\VarFileInfo\\Translation", true)
                .map(|t| t.as_chunks::<2>().0.iter().map(|&[l, c]| (l, c)).collect())
                .unwrap_or_default();
            langs.extend([(0x0409, 0x04B0), (0x0409, 0x04E4)]);
            langs.into_iter().find_map(|(lang, cp)| {
                let text = query(
                    &format!("\\StringFileInfo\\{lang:04x}{cp:04x}\\FileDescription"),
                    false,
                )?;
                let end = text.iter().position(|&c| c == 0).unwrap_or(text.len());
                let text = String::from_utf16_lossy(&text[..end]).trim().to_string();
                (!text.is_empty()).then_some(text)
            })
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
        /// Its meter (#854), read only when the knob asks for peaks.
        meter: Option<IAudioMeterInformation>,
        pid: u32,
        system: bool,
        /// The program's path and description, read once (a session's process never changes).
        exe: Option<String>,
        description: Option<String>,
    }

    impl Tracked {
        fn session(&self) -> Session {
            // SAFETY: COM calls on an MTA object from an MTA thread.
            let (display, icon) = unsafe {
                (
                    take(self.control.GetDisplayName()),
                    take(self.control.GetIconPath()),
                )
            };
            Session {
                id: self.id.clone(),
                name: display_name(
                    &display,
                    self.exe.as_deref(),
                    self.description.as_deref(),
                    self.system,
                    self.pid,
                ),
                icon: Some(icon).filter(|i| !i.is_empty()).or(self.exe.clone()),
                pid: self.pid,
            }
        }

        /// The session's peak now (0.0 when it has no meter).
        fn peak(&self) -> f32 {
            // SAFETY: COM call on an MTA object from an MTA thread.
            self.meter
                .as_ref()
                .and_then(|m| unsafe { m.GetPeakValue() }.ok())
                .unwrap_or(0.0)
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
                let pid = c2.GetProcessId().unwrap_or(0);
                let exe = exe_path(pid);
                self.sessions.push(Tracked {
                    id,
                    meter: control.cast::<IAudioMeterInformation>().ok(),
                    system: c2.IsSystemSoundsSession() == S_OK,
                    description: exe.as_deref().and_then(file_description),
                    exe,
                    pid,
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
                Req::Peaks(reply) => {
                    let peaks = self
                        .sessions
                        .iter()
                        .filter(|t| t.state == AudioSessionStateActive)
                        .map(|t| (t.id.clone(), t.peak()))
                        .collect();
                    let _ = reply.send(Ok(peaks));
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
        fn peaks(&self) -> std::result::Result<Vec<(String, f32)>, String> {
            self.ask(Req::Peaks)
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
    fn names_are_the_display_name_then_the_description_then_the_file() {
        let firefox = Some(r"C:\Program Files\Mozilla Firefox\firefox.exe");
        assert_eq!(
            display_name("Spotify", None, Some("Spotify Desktop"), false, 4),
            "Spotify"
        );
        assert_eq!(
            display_name("", firefox, Some("Firefox"), false, 9),
            "Firefox"
        );
        assert_eq!(
            display_name("@res.dll,-5", firefox, Some(" Mozilla Firefox "), false, 9),
            "Mozilla Firefox",
            "a resource reference isn't a name"
        );
        assert_eq!(display_name("", firefox, Some("  "), false, 9), "firefox");
        assert_eq!(display_name("", firefox, None, false, 9), "firefox");
        assert_eq!(
            display_name(
                "@%SystemRoot%\\System32\\AudioSrv.Dll,-202",
                None,
                Some("Host Process for Windows Services"),
                true,
                0
            ),
            "System sounds"
        );
        assert_eq!(display_name("  ", None, None, false, 77), "App 77");
    }

    fn session(id: &str) -> Session {
        Session {
            id: id.into(),
            name: id.into(),
            icon: None,
            pid: 1,
        }
    }

    fn ids(list: &[Session]) -> Vec<&str> {
        list.iter().map(|s| s.id.as_str()).collect()
    }

    fn peaks(p: &[(&str, f32)]) -> Vec<(String, f32)> {
        p.iter().map(|(id, v)| (id.to_string(), *v)).collect()
    }

    #[test]
    fn only_heard_sessions_are_offered_and_they_stay() {
        let mut h = Heard::default();
        let active = || vec![session("game"), session("helper"), session("music")];
        // A background helper's open stream is silent (or only dither): never offered.
        assert!(h.note(&peaks(&[
            ("game", 0.0),
            ("helper", 0.00005),
            ("music", 0.3)
        ])));
        assert_eq!(ids(&h.offer(active())), ["music"]);
        // The game plays later: it goes last, the music keeps its place.
        assert!(h.note(&peaks(&[("game", 0.6), ("helper", 0.0), ("music", 0.0)])));
        assert_eq!(ids(&h.offer(active())), ["music", "game"]);
        // Peaks again, in any order: nothing moves.
        assert!(!h.note(&peaks(&[("music", 0.9), ("game", 0.1)])));
        assert_eq!(ids(&h.offer(active())), ["music", "game"]);
        // Both silent now (paused, muted): still offered while active.
        assert!(!h.note(&peaks(&[("game", 0.0), ("music", 0.0)])));
        assert_eq!(ids(&h.offer(active())), ["music", "game"]);
        // Inactive: not offered; active again: back in its place.
        assert_eq!(ids(&h.offer(vec![session("game")])), ["game"]);
        assert_eq!(ids(&h.offer(active())), ["music", "game"]);
        // Ended: forgotten.
        h.forget("music");
        assert!(!h.has("music"));
        assert_eq!(ids(&h.offer(active())), ["game"]);
    }
}
