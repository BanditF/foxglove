//! Apps' audio sessions (#719), Windows' own model, the Volume Mixer's: the sessions on the
//! default output device (IAudioSessionManager2), their names and icons, and their levels
//! (ISimpleAudioVolume). The knob points at one app (one row: [`AppGroup`]); nothing here is
//! kept on disk.
//!
//! - [`start`]: one thread owns the COM objects. It learns of sessions from Windows' events:
//!   IAudioSessionNotification (a session created), IAudioSessionEvents (a session's state, its
//!   level and mute, its disconnect) and IMMNotificationClient (another default device). Polling:
//!   none: the thread sleeps on its channel between events and requests. Peaks
//!   (IAudioMeterInformation) are read only when the knob asks ([`Sessions::peaks`]): one read per
//!   active session, plus one of the device's own meter (#1060).
//! - [`Sessions`]: what the knob asks of it: the sessions in the Active state, in Windows' order,
//!   their peaks now, a level step and a mute; and master's (#855: the default output device's
//!   IAudioEndpointVolume, while the knob holds master volume), a step and a mute.
//! - [`group`] (#1061): one row per app. Sessions of one package, else one program path, are one
//!   row; a windowless child of another program joins the windowed program that started it.
//! - [`Heard`] (#854, #1060): the sessions heard during this run, the only ones the knob offers.
//!   An Active session that never sends sound (a background helper holding a silent stream)
//!   isn't, and neither is a mirror: a session whose meter only follows the device's mix (lighting
//!   or a wallpaper that listens to the speakers).
//! - [`step_level`], [`display_name`], [`group`], [`Heard`]: the pure parts, tested everywhere.

use std::collections::HashMap;

/// One app's session as the Volume Mixer shows it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Session {
    /// The session instance id (unique per session, for its lifetime).
    pub id: String,
    /// Its display name, else the program's description, else its name ([`display_name`]).
    pub name: String,
    /// Its icon: the session's IconPath, else the program's path (the icon is the exe's).
    pub icon: Option<String>,
    pub pid: u32,
    /// The program's full path, when Windows tells it (#952: the knob's hidden list matches it),
    /// else the path in the session's identifier (a protected process, #1061).
    pub exe: Option<String>,
    /// Whether the program has a window (hidden and tool windows count; [`has_window`]), `None`
    /// when that can't be told. Filled in by [`Sessions::active`].
    pub windowed: Option<bool>,
    /// The app it belongs to (#1061): its package ([`package_key`]), else its program's path
    /// ([`exe_key`]); `None` when neither is known (the system sounds).
    pub key: Option<String>,
    /// Whether the program has an app window: one the taskbar and Alt-Tab show (visible, not
    /// owned, not a tool window, not cloaked, with a title). `None` when it can't be told.
    pub app_window: Option<bool>,
    /// The process that started it, when that's another program with an app window (not the
    /// shell): the app a windowless helper belongs to (#1061, a WebView2 host).
    pub owner: Option<Owner>,
    /// The parent process id (0: unknown), for the row's top process.
    pub parent: u32,
}

/// The program a windowless helper's session belongs to (#1061).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Owner {
    pub pid: u32,
    /// Its app key ([`package_key`] or [`exe_key`]).
    pub key: String,
    /// Its name, as [`display_name`] gives it.
    pub name: String,
    pub exe: Option<String>,
}

/// One session's meter at one read (#1060), with its level and mute as last told by Windows.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Meter {
    pub id: String,
    /// Its peak (0.0-1.0), measured before its own level and mute.
    pub peak: f32,
    /// Its level (0.0-1.0).
    pub level: f32,
    pub muted: bool,
}

impl Meter {
    /// What it adds to the device's mix: its peak at its level, nothing when muted.
    pub fn contribution(&self) -> f32 {
        if self.muted {
            0.0
        } else {
            self.peak * self.level
        }
    }
}

/// One read of the meters (#1060): the device's own peak (the mix, before its volume; `None`
/// when it can't be read) and each Active session's.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Peaks {
    pub endpoint: Option<f32>,
    pub sessions: Vec<Meter>,
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
    /// On a device Foxglove sets the volume of (#1064, `master`): Windows' flyout or the volume
    /// keys changed master, and Foxglove took it as master's new level (its card shows it).
    Adopted,
}

/// What the knob asks of Windows' sessions. Implemented by [`start`]'s handle and by tests.
pub trait Sessions: Send + Sync {
    /// The sessions in the Active state, in Windows' order.
    fn active(&self) -> Result<Vec<Session>, String>;
    /// Each Active session's peak now (one meter read each), and the device's (one read).
    fn peaks(&self) -> Result<Peaks, String>;
    /// Moves session `id`'s level by `steps` of [`STEP`]; the new level in percent.
    fn step(&self, id: &str, steps: i32) -> Result<u8, String>;
    /// Mutes or unmutes session `id`; whether it is muted now.
    fn set_mute(&self, id: &str, muted: bool) -> Result<bool, String>;
    /// Session `id`'s level in percent, and whether it is muted.
    fn level(&self, id: &str) -> Result<(u8, bool), String>;
    /// Master's (the default output device's) level in percent, and whether it is muted.
    fn master(&self) -> Result<(u8, bool), String>;
    /// Moves master's level by `steps` of [`STEP`] and unmutes it, as Windows' own volume keys
    /// do (#855); the new level in percent.
    fn step_master(&self, steps: i32) -> Result<u8, String>;
    /// Mutes or unmutes master (#855); whether it is muted now.
    fn toggle_master_mute(&self) -> Result<bool, String>;
    /// On a device Foxglove sets the volume of (#1064): what session `id`'s own level, Windows'
    /// level for it and master are, or that the app sets its own volume. `None` elsewhere.
    fn app_volume(&self, _id: &str) -> Option<crate::master::AppVolume> {
        None
    }
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

/// How close a session's peak must be to the device's to follow it (#1060): 10 % of it.
pub const MIRROR_TOLERANCE: f32 = 0.10;
/// Reads that match the mix before a session is called a mirror, and reads that don't before a
/// mirror counts as played after all (it started playing sound of its own).
pub const MIRROR_READS: u8 = 3;

/// An app key from a package family name (#1061).
pub fn package_key(family: &str) -> String {
    format!("pkg:{}", family.to_lowercase())
}

/// An app key from a program's full path, in any case (#1061).
pub fn exe_key(path: &str) -> String {
    format!("exe:{}", path.to_lowercase())
}

/// The program path in a session identifier (`{…}|\Device\HarddiskVolume3\…\app.exe%b{…}`):
/// the text after the first `|`, before `%b` (#1061: a protected process's path, which
/// `QueryFullProcessImageNameW` can't read).
pub fn id_path(identifier: &str) -> Option<&str> {
    let (_, rest) = identifier.split_once('|')?;
    let path = rest.split_once("%b").map_or(rest, |(p, _)| p);
    let path = path.split('|').next().unwrap_or(path).trim();
    (!path.is_empty() && path.contains('\\')).then_some(path)
}

/// `path` (an NT device path, `\Device\HarddiskVolume3\…`) with its device replaced by its drive
/// letter, from `drives` (drive, device) pairs; unchanged when no drive matches.
pub fn dos_path(path: &str, drives: &[(String, String)]) -> String {
    for (drive, device) in drives {
        if let Some(rest) = path
            .get(..device.len())
            .filter(|p| p.eq_ignore_ascii_case(device))
            .and_then(|_| path.get(device.len()..))
            .filter(|r| r.starts_with('\\'))
        {
            return format!("{drive}{rest}");
        }
    }
    path.to_string()
}

/// One app's sessions (#1061), the knob's row: what the cycle offers and a turn moves.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AppGroup {
    /// The app key ([`package_key`], [`exe_key`], else `id:<session id>`).
    pub key: String,
    /// The name and icon of the app's windowed (else top) process, never a helper's.
    pub name: String,
    pub icon: Option<String>,
    /// The windowed (else top) program's path: what "Hide from knob" stores.
    pub exe: Option<String>,
    /// Whether the app has a window ([`has_window`] over its sessions; `None`: can't tell).
    pub windowed: Option<bool>,
    /// Whether the app has an app window (the taskbar's; a source for [`Heard`]'s mirror test).
    pub app_window: bool,
    /// Its sessions, in Windows' order.
    pub members: Vec<Member>,
}

/// One session of a row.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Member {
    pub id: String,
    pub pid: u32,
    pub exe: Option<String>,
}

impl AppGroup {
    pub fn has(&self, id: &str) -> bool {
        self.members.iter().any(|m| m.id == id)
    }
}

/// A session's row key: its owner's when it is a windowless helper of another program with an
/// app window, else its own, else its id.
fn row_key(s: &Session) -> String {
    match (&s.owner, s.app_window) {
        (Some(o), Some(false)) if s.key.as_deref() != Some(o.key.as_str()) => o.key.clone(),
        _ => s.key.clone().unwrap_or_else(|| format!("id:{}", s.id)),
    }
}

/// The sessions as rows, one per app (#1061), in Windows' order of each app's first session:
///
/// - one key per app: its package, else its program's full path (any case), else the path from
///   its session identifier (the worker fills `exe` and `key` from it);
/// - a session of another program with no app window of its own joins the row of the program
///   that started it, when that one has an app window (`owner`): a WebView2 host. A child with
///   its own window (a game under its launcher) stays its own row;
/// - the name, icon and program come from the row's top process (no other session's process is
///   its parent), the windowed one first, else the owner when only its helper plays;
/// - Windows' grouping parameter isn't used: every session has its own.
pub fn group(sessions: Vec<Session>) -> Vec<AppGroup> {
    let mut keys: Vec<String> = Vec::new();
    let mut parts: HashMap<String, Vec<Session>> = HashMap::new();
    for s in sessions {
        let key = row_key(&s);
        if !parts.contains_key(&key) {
            keys.push(key.clone());
        }
        parts.entry(key).or_default().push(s);
    }
    keys.into_iter()
        .map(|key| {
            let list = parts.remove(&key).unwrap_or_default();
            let own = |s: &&Session| s.key.as_deref() == Some(key.as_str());
            let pids: Vec<u32> = list.iter().map(|s| s.pid).collect();
            let owner = list
                .iter()
                .filter_map(|s| s.owner.as_ref())
                .find(|o| o.key == key);
            // `app_window` is the program's (any of its processes), so the top process decides
            // between a browser and its audio child of the same program.
            let top = |s: &&Session| s.parent == 0 || !pids.contains(&s.parent);
            let lead = list
                .iter()
                .filter(own)
                .filter(top)
                .find(|s| s.app_window == Some(true))
                .or_else(|| list.iter().filter(own).find(top))
                .or_else(|| list.iter().find(own));
            let (name, icon, exe) = match (lead, owner) {
                (Some(l), _) => (l.name.clone(), l.icon.clone(), l.exe.clone()),
                (None, Some(o)) => (o.name.clone(), o.exe.clone(), o.exe.clone()),
                (None, None) => {
                    let first = &list[0];
                    (first.name.clone(), first.icon.clone(), first.exe.clone())
                }
            };
            let windowed = if owner.is_some() || list.iter().any(|s| s.windowed == Some(true)) {
                Some(true)
            } else if list.iter().all(|s| s.windowed == Some(false)) {
                Some(false)
            } else {
                None
            };
            AppGroup {
                app_window: owner.is_some() || list.iter().any(|s| s.app_window == Some(true)),
                members: list
                    .iter()
                    .map(|s| Member {
                        id: s.id.clone(),
                        pid: s.pid,
                        exe: s.exe.clone(),
                    })
                    .collect(),
                key,
                name,
                icon,
                exe,
                windowed,
            }
        })
        .collect()
}

/// Two rows with one name (two installs of one app) each add their program's folder:
/// "<app> · <folder>" (UX addendum, #1061). The folder is the one nearest the program where the
/// paths differ, so Chromium-style installs (`…\<Product>\Application\<exe>`) read by product.
pub fn tell_apart(rows: &mut [AppGroup]) {
    let names: Vec<String> = rows.iter().map(|r| r.name.to_lowercase()).collect();
    // Each row's folders, nearest the program first (no file name, no drive).
    let folders: Vec<Vec<String>> = rows
        .iter()
        .map(|r| {
            r.exe
                .as_deref()
                .map(|p| {
                    p.rsplit(['\\', '/'])
                        .skip(1)
                        .filter(|f| !f.is_empty() && !f.ends_with(':'))
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default()
        })
        .collect();
    let differs = |a: &[String], b: &[String]| {
        (0..a.len()).find(|&d| b.get(d).is_none_or(|f| !f.eq_ignore_ascii_case(&a[d])))
    };
    for (i, row) in rows.iter_mut().enumerate() {
        let depth = (0..names.len())
            .filter(|&j| j != i && names[j] == names[i])
            .filter_map(|j| differs(&folders[i], &folders[j]))
            .min();
        if let Some(folder) = depth.map(|d| &folders[i][d]) {
            row.name = format!("{} · {folder}", row.name);
        }
    }
}

/// What one read says of one session (#1060).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Read {
    /// Its peak is the mix, and other apps' sound accounts for the mix.
    Mirror,
    /// Close to the mix but not within the tolerance: no verdict either way.
    Unsure,
    /// It plays sound of its own.
    Source,
}

/// A session's place in the mirror test.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Track {
    /// Its row key and whether its row has an app window, as last learnt ([`Heard::learn`]).
    key: Option<String>,
    app_window: bool,
    /// Reads that matched the mix, and own-sound reads in a row since it was called a mirror.
    matches: u8,
    misses: u8,
}

/// The sessions heard during this run, in the order first heard (#854). The knob offers only
/// these: an Active session whose peak never rose above [`AUDIBLE`] (a background helper holding
/// a silent stream open) isn't offered. Once heard a session stays offered (a pause or a mute
/// doesn't drop it) until it ends, and its place never moves: a new one goes last.
///
/// A mirror never counts as heard (#1060): a session whose peak follows the device's own peak
/// (within [`MIRROR_TOLERANCE`]) while other apps' sound (peak x level, nothing when muted)
/// accounts for that peak. Those other apps must be sources beyond doubt: their peak isn't the
/// mix, or their app has an app window. After [`MIRROR_READS`] matching reads a session is a
/// mirror; it counts as heard only after as many reads of its own sound in a row. A tray player
/// alone with a mirror can't be told apart from it: both count.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Heard {
    order: Vec<String>,
    tracks: HashMap<String, Track>,
}

impl Heard {
    /// Learns which row each session is in (call before [`Heard::note`] with the rows now).
    pub fn learn(&mut self, rows: &[AppGroup]) {
        for row in rows {
            for m in &row.members {
                let t = self.tracks.entry(m.id.clone()).or_default();
                t.key = Some(row.key.clone());
                t.app_window = row.app_window;
            }
        }
    }

    /// Notes the sessions audible in `peaks`; whether any was new.
    pub fn note(&mut self, peaks: &Peaks) -> bool {
        let mix = peaks.endpoint.filter(|e| *e > AUDIBLE);
        let mut new = false;
        for m in peaks.sessions.iter().filter(|m| m.peak > AUDIBLE) {
            let read = mix.map_or(Read::Source, |e| self.read(m, &peaks.sessions, e));
            let t = self.tracks.entry(m.id.clone()).or_default();
            let heard = match read {
                Read::Mirror => {
                    t.matches = t.matches.saturating_add(1);
                    t.misses = 0;
                    false
                }
                Read::Unsure => false,
                Read::Source if t.matches >= MIRROR_READS => {
                    t.misses = t.misses.saturating_add(1);
                    t.misses >= MIRROR_READS
                }
                Read::Source => true,
            };
            if heard && !self.has(&m.id) {
                self.order.push(m.id.clone());
                new = true;
            }
        }
        new
    }

    /// One session's read against the mix `e`.
    fn read(&self, m: &Meter, all: &[Meter], e: f32) -> Read {
        let tol = (MIRROR_TOLERANCE * e).max(AUDIBLE);
        let off = (m.peak - e).abs();
        if off > 2.0 * tol {
            return Read::Source;
        }
        let key = |id: &str| {
            self.tracks
                .get(id)
                .and_then(|t| t.key.clone())
                .unwrap_or_else(|| format!("id:{id}"))
        };
        let own = key(&m.id);
        let sources: Vec<f32> = all
            .iter()
            .filter(|n| n.peak > AUDIBLE && key(&n.id) != own)
            .filter(|n| {
                (n.peak - e).abs() > tol || self.tracks.get(&n.id).is_some_and(|t| t.app_window)
            })
            .map(Meter::contribution)
            .collect();
        let sum: f32 = sources.iter().sum();
        let max = sources.iter().copied().fold(0.0, f32::max);
        let explained = sum > AUDIBLE && sum >= e - tol && max <= e + tol;
        match (explained, off <= tol) {
            (false, _) => Read::Source,
            (true, true) => Read::Mirror,
            (true, false) => Read::Unsure,
        }
    }

    pub fn has(&self, id: &str) -> bool {
        self.order.iter().any(|h| h == id)
    }

    /// Whether session `id` is called a mirror now (for the log and tests).
    pub fn is_mirror(&self, id: &str) -> bool {
        !self.has(id)
            && self
                .tracks
                .get(id)
                .is_some_and(|t| t.matches >= MIRROR_READS)
    }

    /// Session `id` ended: it is forgotten (its id never comes back).
    pub fn forget(&mut self, id: &str) {
        self.order.retain(|h| h != id);
        self.tracks.remove(id);
    }

    /// Of `rows`, the ones with a heard session, in the order first heard.
    pub fn offer(&self, mut rows: Vec<AppGroup>) -> Vec<AppGroup> {
        let first = |r: &AppGroup| {
            r.members
                .iter()
                .filter_map(|m| self.order.iter().position(|h| *h == m.id))
                .min()
        };
        rows.retain(|r| first(r).is_some());
        rows.sort_by_key(first);
        rows
    }
}

/// Whether `pid` or another process of program `exe` is in `owners`.
fn owns(pid: u32, exe: Option<&str>, owners: &[(u32, Option<String>)]) -> bool {
    let same = |o: &Option<String>| match (exe, o) {
        (Some(a), Some(b)) => a.eq_ignore_ascii_case(b),
        _ => false,
    };
    owners.iter().any(|(p, o)| *p == pid || same(o))
}

/// Whether a session's program has a window (#952), given the processes that own a top-level
/// window now (`owners`: pid and program path; hidden and tool windows count, so a player
/// minimised to the tray still has one). A process counts when it owns one, or another process
/// of the same program does (a browser's or a chat app's sound comes from a helper process of
/// the program that has the window). `None` when it can't be told: no pid (the system sounds),
/// or no windows listed at all. A wrong skip is worse than one extra app.
pub fn has_window(pid: u32, exe: Option<&str>, owners: &[(u32, Option<String>)]) -> Option<bool> {
    if pid == 0 || owners.is_empty() {
        return None;
    }
    Some(owns(pid, exe, owners))
}

/// Whether a session's program has an app window (#1061), given every window owner (`owners`)
/// and the owners of app windows (`apps`, a part of them); `None` as for [`has_window`].
pub fn has_app_window(
    pid: u32,
    exe: Option<&str>,
    owners: &[(u32, Option<String>)],
    apps: &[(u32, Option<String>)],
) -> Option<bool> {
    has_window(pid, exe, owners).map(|_| owns(pid, exe, apps))
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
    use super::{
        display_name, dos_path, exe_key, has_app_window, has_window, id_path, package_key,
        step_level, Change, Meter, Owner, Peaks, Session, Sessions,
    };
    use crate::master::{self, AppVolume, Audio, Device, Live, Soft};
    use std::collections::HashMap;
    use std::sync::mpsc::{channel, sync_channel, Receiver, RecvTimeoutError, Sender, SyncSender};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use windows::core::{implement, Interface, Ref, Result, BOOL, GUID, PCWSTR, PWSTR};
    use windows::Win32::Foundation::{
        CloseHandle, ERROR_SUCCESS, FILETIME, HANDLE, MAX_PATH, PROPERTYKEY, S_OK,
    };
    use windows::Win32::Media::Audio::Endpoints::{
        IAudioEndpointVolume, IAudioEndpointVolumeCallback, IAudioEndpointVolumeCallback_Impl,
        IAudioMeterInformation,
    };
    use windows::Win32::Media::Audio::*;
    use windows::Win32::Storage::FileSystem::{
        GetFileVersionInfoSizeW, GetFileVersionInfoW, QueryDosDeviceW, VerQueryValueW,
    };
    use windows::Win32::Storage::Packaging::Appx::GetPackageFamilyName;
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoTaskMemFree, CLSCTX_ALL, COINIT_MULTITHREADED,
    };
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };
    use windows::Win32::System::Threading::{
        GetProcessTimes, OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
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
        Peaks(SyncSender<std::result::Result<Peaks, String>>),
        Step(String, i32, SyncSender<std::result::Result<u8, String>>),
        Mute(String, bool, SyncSender<std::result::Result<bool, String>>),
        Level(String, SyncSender<std::result::Result<(u8, bool), String>>),
        MasterLevel(SyncSender<std::result::Result<(u8, bool), String>>),
        MasterStep(i32, SyncSender<std::result::Result<u8, String>>),
        MasterMute(SyncSender<std::result::Result<bool, String>>),
        /// The device's level and mute changed, and whether Foxglove changed them (#1064: key
        /// steps are counted from the level Windows last reported, Foxglove's own included).
        Master(f32, bool, bool),
        Created(Control),
        State(String, AudioSessionState),
        /// A session's level and mute, from its own event (#1060: the mirror test's input), and
        /// whether Foxglove made the change.
        Volume(String, f32, bool, bool),
        Disconnected(String),
        Device,
        /// What the tray row says of an app on a device Foxglove sets the volume of (#1064).
        AppVolume(String, SyncSender<Option<AppVolume>>),
        /// The per-device setting changed, or the Knob card asks for the device's facts again.
        Recheck,
        /// Foxglove quits: the device and the apps back to Windows' own levels.
        Stop(SyncSender<()>),
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
        fn OnSimpleVolumeChanged(
            &self,
            level: f32,
            muted: BOOL,
            context: *const GUID,
        ) -> Result<()> {
            // SAFETY: Windows passes the change's context, valid for this call (or null).
            let own = unsafe { context.as_ref() }.is_some_and(|c| *c == OWN_CHANGES);
            let _ = self
                .tx
                .send(Req::Volume(self.id.clone(), level, muted.as_bool(), own));
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
        fn OnNotify(&self, data: *mut AUDIO_VOLUME_NOTIFICATION_DATA) -> Result<()> {
            // SAFETY: Windows passes the notification's data, valid for this call (or null).
            let Some(d) = (unsafe { data.as_ref() }) else {
                return Ok(());
            };
            // Foxglove's own master steps (#855) show its own card: no tab over it; its own holds
            // of a device it sets the volume of (#1064) are only noted there.
            let own = d.guidEventContext == OWN_CHANGES;
            let _ = self
                .0
                .send(Req::Master(d.fMasterVolume, d.bMuted.as_bool(), own));
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

    /// The processes that own a top-level window now, hidden and tool windows included (#952),
    /// with their program paths and whether one of their windows is an app window (#1061: the
    /// taskbar's, as Alt-Tab picks them: visible, not owned, not a tool window, not cloaked,
    /// with a title): one EnumWindows pass, one path read per process.
    fn window_owners() -> Vec<(u32, Option<String>, bool)> {
        use windows::Win32::Foundation::{HWND, LPARAM};
        use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED};
        use windows::Win32::UI::WindowsAndMessaging::{
            EnumWindows, GetWindow, GetWindowLongPtrW, GetWindowTextLengthW,
            GetWindowThreadProcessId, IsWindowVisible, GWL_EXSTYLE, GW_OWNER, WS_EX_TOOLWINDOW,
        };

        /// Whether `hwnd` is an app window.
        ///
        /// SAFETY: `hwnd` is a window handle Windows just passed to the EnumWindows callback.
        unsafe fn app_window(hwnd: HWND) -> bool {
            unsafe {
                if !IsWindowVisible(hwnd).as_bool() {
                    return false;
                }
                if GetWindow(hwnd, GW_OWNER).is_ok_and(|o| !o.is_invalid()) {
                    return false;
                }
                if GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32 & WS_EX_TOOLWINDOW.0 != 0 {
                    return false;
                }
                let mut cloaked = 0u32;
                let read = DwmGetWindowAttribute(
                    hwnd,
                    DWMWA_CLOAKED,
                    (&mut cloaked as *mut u32).cast(),
                    std::mem::size_of::<u32>() as u32,
                );
                if read.is_ok() && cloaked != 0 {
                    return false;
                }
                GetWindowTextLengthW(hwnd) > 0
            }
        }

        unsafe extern "system" fn each(hwnd: HWND, data: LPARAM) -> BOOL {
            // SAFETY: `data` is the Vec below, alive for the whole EnumWindows call.
            let pids = unsafe { &mut *(data.0 as *mut Vec<(u32, bool)>) };
            let mut pid = 0u32;
            // SAFETY: a window handle Windows just passed us.
            unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
            if pid != 0 {
                // SAFETY: as above.
                let app = unsafe { app_window(hwnd) };
                match pids.iter_mut().find(|(p, _)| *p == pid) {
                    Some((_, a)) => *a |= app,
                    None => pids.push((pid, app)),
                }
            }
            BOOL(1)
        }
        let mut pids: Vec<(u32, bool)> = Vec::new();
        // SAFETY: the callback only touches `pids`, which outlives the call.
        let ok = unsafe {
            EnumWindows(
                Some(each),
                LPARAM(&mut pids as *mut Vec<(u32, bool)> as isize),
            )
        };
        if ok.is_err() {
            return Vec::new();
        }
        pids.into_iter()
            .map(|(p, app)| (p, exe_path(p), app))
            .collect()
    }

    /// The process that owns the shell's desktop window (Explorer): never an app's owner.
    fn shell_pid() -> u32 {
        use windows::Win32::UI::WindowsAndMessaging::{GetShellWindow, GetWindowThreadProcessId};
        let mut pid = 0u32;
        // SAFETY: plain Win32 calls; a null window gives pid 0.
        unsafe {
            let shell = GetShellWindow();
            if !shell.is_invalid() {
                GetWindowThreadProcessId(shell, Some(&mut pid));
            }
        }
        pid
    }

    /// Every process's parent, from one process snapshot.
    fn parents() -> HashMap<u32, u32> {
        let mut out = HashMap::new();
        // SAFETY: plain Win32 calls on a snapshot handle closed before returning.
        unsafe {
            let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else {
                return out;
            };
            let mut entry = PROCESSENTRY32W {
                dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
                ..Default::default()
            };
            let mut more = Process32FirstW(snap, &mut entry).is_ok();
            while more {
                out.insert(entry.th32ProcessID, entry.th32ParentProcessID);
                more = Process32NextW(snap, &mut entry).is_ok();
            }
            let _ = CloseHandle(snap);
        }
        out
    }

    /// Runs `f` with a query handle for `pid` (closed after); `None` when it can't be opened.
    fn with_process<T>(pid: u32, f: impl FnOnce(HANDLE) -> Option<T>) -> Option<T> {
        if pid == 0 {
            return None;
        }
        // SAFETY: plain Win32 calls; the handle is closed before returning.
        unsafe {
            let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
            let out = f(process);
            let _ = CloseHandle(process);
            out
        }
    }

    fn exe_path(pid: u32) -> Option<String> {
        with_process(pid, |process| {
            let mut buf = [0u16; MAX_PATH as usize];
            let mut len = buf.len() as u32;
            // SAFETY: a valid handle and a buffer of `len` u16s.
            let ok = unsafe {
                QueryFullProcessImageNameW(
                    process,
                    PROCESS_NAME_WIN32,
                    PWSTR(buf.as_mut_ptr()),
                    &mut len,
                )
            };
            ok.ok()?;
            Some(String::from_utf16_lossy(&buf[..len as usize]))
        })
    }

    /// The process's package family name, when it is a packaged (Store) app.
    fn package_family(pid: u32) -> Option<String> {
        with_process(pid, |process| {
            let mut buf = [0u16; 256];
            let mut len = buf.len() as u32;
            // SAFETY: a valid handle and a buffer of `len` u16s.
            let r =
                unsafe { GetPackageFamilyName(process, &mut len, Some(PWSTR(buf.as_mut_ptr()))) };
            if r != ERROR_SUCCESS || len == 0 {
                return None;
            }
            let end = buf[..len as usize]
                .iter()
                .position(|&c| c == 0)
                .unwrap_or(len as usize);
            Some(String::from_utf16_lossy(&buf[..end])).filter(|s| !s.is_empty())
        })
    }

    /// When the process started (100 ns ticks), to tell a parent from a later process that got
    /// its id.
    fn started(pid: u32) -> Option<u64> {
        with_process(pid, |process| {
            let (mut c, mut e, mut k, mut u) = (
                FILETIME::default(),
                FILETIME::default(),
                FILETIME::default(),
                FILETIME::default(),
            );
            // SAFETY: a valid handle and four FILETIMEs.
            unsafe { GetProcessTimes(process, &mut c, &mut e, &mut k, &mut u) }.ok()?;
            Some(u64::from(c.dwHighDateTime) << 32 | u64::from(c.dwLowDateTime))
        })
    }

    /// The drives' NT device names (`C:`, `\Device\HarddiskVolume3`), for [`dos_path`].
    fn drives() -> Vec<(String, String)> {
        let mut out = Vec::new();
        for letter in b'A'..=b'Z' {
            let drive = format!("{}:", letter as char);
            let wide: Vec<u16> = drive.encode_utf16().chain(std::iter::once(0)).collect();
            let mut buf = [0u16; 512];
            // SAFETY: a NUL-terminated name and a buffer Windows fills (its length is passed).
            let n = unsafe { QueryDosDeviceW(PCWSTR(wide.as_ptr()), Some(&mut buf)) } as usize;
            if n == 0 {
                continue;
            }
            // The first of the NUL-separated names.
            let end = buf[..n].iter().position(|&c| c == 0).unwrap_or(n);
            let device = String::from_utf16_lossy(&buf[..end]);
            if !device.is_empty() {
                out.push((drive, device));
            }
        }
        out
    }

    /// A process's app key and program path (#1061).
    fn program(pid: u32) -> (Option<String>, Option<String>) {
        let exe = exe_path(pid);
        let key = package_family(pid)
            .map(|f| package_key(&f))
            .or_else(|| exe.as_deref().map(exe_key));
        (key, exe)
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

    /// Master moved by `steps` detents and unmuted, as Windows' volume keys do (#855); the new
    /// level in percent. Marked as Foxglove's own change.
    fn master_step(e: &IAudioEndpointVolume, steps: i32) -> Result<u8> {
        // SAFETY: COM calls on the sessions' MTA thread.
        unsafe {
            let new = step_level(e.GetMasterVolumeLevelScalar()?, steps);
            e.SetMasterVolumeLevelScalar(new, &OWN_CHANGES)?;
            if e.GetMute()?.as_bool() {
                e.SetMute(false, &OWN_CHANGES)?;
            }
            Ok((new * 100.0).round() as u8)
        }
    }

    /// Master muted or unmuted (#855); whether it is muted now. Marked as Foxglove's own change.
    fn master_mute(e: &IAudioEndpointVolume) -> Result<bool> {
        // SAFETY: COM calls on the sessions' MTA thread.
        unsafe {
            let muted = !e.GetMute()?.as_bool();
            e.SetMute(muted, &OWN_CHANGES)?;
            Ok(muted)
        }
    }

    struct Tracked {
        id: String,
        /// The session identifier: one per app and device, the same after the app restarts
        /// (#1064: its own level is kept by it).
        sid: String,
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
        /// Its app key (#1061), read once with the path.
        key: Option<String>,
        /// Its level and mute: read once, then from its own events (#1060).
        level: f32,
        muted: bool,
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
                exe: self.exe.clone(),
                key: self.key.clone(),
                ..Session::default()
            }
        }

        fn meter(&self) -> Meter {
            Meter {
                id: self.id.clone(),
                peak: self.peak(),
                level: self.level,
                muted: self.muted,
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

    /// Windows' device and sessions for the software master (#1064). Every write is marked as
    /// Foxglove's own, so its notification is ignored.
    struct Com<'a> {
        endpoint: Option<&'a IAudioEndpointVolume>,
        sessions: &'a mut Vec<Tracked>,
    }

    impl Audio for Com<'_> {
        fn endpoint(&self) -> std::result::Result<(f32, bool), String> {
            let e = self.endpoint.ok_or("no output device")?;
            // SAFETY: COM calls on the sessions' MTA thread.
            unsafe {
                Ok((
                    e.GetMasterVolumeLevelScalar().map_err(|e| e.to_string())?,
                    e.GetMute().map_err(|e| e.to_string())?.as_bool(),
                ))
            }
        }
        fn set_endpoint(&mut self, level: f32) -> std::result::Result<(), String> {
            let e = self.endpoint.ok_or("no output device")?;
            // SAFETY: as above.
            unsafe { e.SetMasterVolumeLevelScalar(level, &OWN_CHANGES) }.map_err(|e| e.to_string())
        }
        fn set_endpoint_mute(&mut self, muted: bool) -> std::result::Result<(), String> {
            let e = self.endpoint.ok_or("no output device")?;
            // SAFETY: as above.
            unsafe { e.SetMute(muted, &OWN_CHANGES) }.map_err(|e| e.to_string())
        }
        fn sessions(&self) -> Vec<Live> {
            self.sessions
                .iter()
                .map(|t| Live {
                    id: t.id.clone(),
                    sid: t.sid.clone(),
                    level: t.level,
                    muted: t.muted,
                })
                .collect()
        }
        fn set_level(&mut self, id: &str, level: f32) -> std::result::Result<(), String> {
            let t = self
                .sessions
                .iter_mut()
                .find(|t| t.id == id)
                .ok_or("the app's sound ended")?;
            let v = t.volume()?;
            // SAFETY: as above.
            unsafe { v.SetMasterVolume(level, &OWN_CHANGES) }.map_err(|e| e.to_string())?;
            t.level = level;
            Ok(())
        }
        fn set_mute(&mut self, id: &str, muted: bool) -> std::result::Result<(), String> {
            let t = self
                .sessions
                .iter_mut()
                .find(|t| t.id == id)
                .ok_or("the app's sound ended")?;
            let v = t.volume()?;
            // SAFETY: as above.
            unsafe { v.SetMute(muted, &OWN_CHANGES) }.map_err(|e| e.to_string())?;
            t.muted = muted;
            Ok(())
        }
    }

    /// Windows' "Allow applications to take exclusive control of this device", in the
    /// endpoint's property store ({b3f8fa53-0004-438e-9003-51a46e139bfc},3).
    const EXCLUSIVE: PROPERTYKEY = PROPERTYKEY {
        fmtid: GUID::from_u128(0xb3f8fa53_0004_438e_9003_51a46e139bfc),
        pid: 3,
    };

    /// The device instance ids up the endpoint's device tree (its audio function's, the USB
    /// device's ...) and their hardware ids, for the data list (#1064).
    fn hardware_ids(endpoint_id: &str) -> Vec<String> {
        use windows::Win32::Devices::DeviceAndDriverInstallation::{
            CM_Get_DevNode_Registry_PropertyW, CM_Get_Device_IDW, CM_Get_Parent,
            CM_Locate_DevNodeW, CM_DRP_HARDWAREID, CM_LOCATE_DEVNODE_NORMAL, CR_SUCCESS,
        };
        let mut out = Vec::new();
        let instance: Vec<u16> = format!("SWD\\MMDEVAPI\\{endpoint_id}")
            .encode_utf16()
            .chain(Some(0))
            .collect();
        let mut node = 0u32;
        // SAFETY: a NUL-terminated id and an out value.
        let found = unsafe {
            CM_Locate_DevNodeW(
                &mut node,
                PCWSTR(instance.as_ptr()),
                CM_LOCATE_DEVNODE_NORMAL,
            )
        };
        if found != CR_SUCCESS {
            return out;
        }
        for _ in 0..4 {
            let mut parent = 0u32;
            // SAFETY: a devnode Windows gave us, and an out value.
            if unsafe { CM_Get_Parent(&mut parent, node, 0) } != CR_SUCCESS {
                break;
            }
            node = parent;
            let mut id = [0u16; 512];
            // SAFETY: a devnode and a buffer of its length.
            if unsafe { CM_Get_Device_IDW(node, &mut id, 0) } == CR_SUCCESS {
                let end = id.iter().position(|u| *u == 0).unwrap_or(id.len());
                out.push(String::from_utf16_lossy(&id[..end]));
            }
            let mut buf = [0u16; 2048];
            let mut len = (buf.len() * 2) as u32;
            // SAFETY: a devnode and a buffer of `len` bytes.
            let r = unsafe {
                CM_Get_DevNode_Registry_PropertyW(
                    node,
                    CM_DRP_HARDWAREID,
                    None,
                    Some(buf.as_mut_ptr().cast()),
                    &mut len,
                    0,
                )
            };
            if r == CR_SUCCESS {
                let units = &buf[..(len as usize / 2).min(buf.len())];
                out.extend(
                    units
                        .split(|u| *u == 0)
                        .filter(|w| !w.is_empty())
                        .map(String::from_utf16_lossy),
                );
            }
        }
        out
    }

    /// The default output device's facts for the software master (#1064).
    fn device_facts(d: &IMMDevice, e: &IAudioEndpointVolume) -> Option<Device> {
        use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
        use windows::Win32::System::Com::STGM_READ;
        // SAFETY: COM calls on the sessions' MTA thread.
        unsafe {
            let endpoint = take(d.GetId());
            if endpoint.is_empty() {
                return None;
            }
            let props = d.OpenPropertyStore(STGM_READ).ok();
            let name = props
                .as_ref()
                .and_then(|p| p.GetValue(&PKEY_Device_FriendlyName).ok())
                .map(|v| v.to_string())
                .unwrap_or_default();
            let exclusive = props
                .as_ref()
                .and_then(|p| p.GetValue(&EXCLUSIVE).ok())
                .and_then(|v| {
                    u32::try_from(&v)
                        .map(|n| n != 0)
                        .or_else(|_| bool::try_from(&v))
                        .ok()
                });
            let support = e.QueryHardwareSupport().unwrap_or(0);
            let (mut step, mut count) = (0u32, 0u32);
            let _ = e.GetVolumeStepInfo(&mut step, &mut count);
            Some(Device {
                ids: hardware_ids(&endpoint),
                endpoint,
                name,
                eligible: master::eligible(support),
                exclusive,
                step: master::step_for(count),
                ..Device::default()
            })
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
        /// The device's own meter (#1060): the mix, read once per peak read.
        endpoint_meter: Option<IAudioMeterInformation>,
        master_changed: IAudioEndpointVolumeCallback,
        sessions: Vec<Tracked>,
        /// The drives' device names, read once (for a protected process's path, #1061).
        drives: Option<Vec<(String, String)>>,
        /// Owners' descriptions by path, read once each.
        descriptions: HashMap<String, Option<String>>,
        /// The software master (#1064): `None` without a host (no settings folder).
        soft: Option<Soft>,
        /// The default output device's facts (#1064).
        device: Option<Device>,
        /// Foxglove is quitting: the device was put back, and nothing starts it again.
        stopped: bool,
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
                        self.device = device_facts(d, &e);
                        self.endpoint = Some(e);
                    }
                    self.endpoint_meter =
                        d.Activate::<IAudioMeterInformation>(CLSCTX_ALL, None).ok();
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
            self.evaluate();
        }

        /// Whether Foxglove sets the default device's volume now (#1064): on, it starts (or
        /// recovers from the last run's journal); off, it stops, or puts back what a crash left.
        fn evaluate(&mut self) {
            if self.stopped {
                return;
            }
            let (Some(soft), Some(dev)) = (self.soft.as_mut(), self.device.as_mut()) else {
                master::publish_device(self.device.clone());
                return;
            };
            dev.listed = soft.host().listed(&dev.ids);
            dev.choice = soft.host().choice(&dev.endpoint);
            dev.on = master::turned_on(dev.eligible, dev.listed, dev.choice);
            let mut audio = Com {
                endpoint: self.endpoint.as_ref(),
                sessions: &mut self.sessions,
            };
            if soft.engaged().is_some_and(|e| e != dev.endpoint) {
                soft.abandon();
            }
            dev.paused = false;
            if dev.on {
                if let Err(e) = soft.engage(&mut audio, &dev.endpoint, &dev.name, dev.step) {
                    eprintln!("Foxglove: master volume: {e}");
                }
                // Blocked by a journal that can't be read: the setting says so (#1070).
                dev.paused = soft.engaged().is_none() && soft.journal_unread();
            } else if soft.engaged().is_some() {
                if let Err(e) = soft.disengage(&mut audio) {
                    eprintln!("Foxglove: master volume: {e}");
                }
            } else {
                soft.recover_off(&mut audio, &dev.endpoint);
                // Apps a muted master left muted when it stopped, if the device isn't muted now.
                soft.unmute_left(&mut audio);
            }
            master::publish_device(self.device.clone());
        }

        /// Stops setting the device's volume (it changes, or Foxglove quits): the device and the
        /// apps back to their own levels, with the objects of the device that was default.
        fn release(&mut self) {
            let Some(soft) = self.soft.as_mut() else {
                return;
            };
            let mut audio = Com {
                endpoint: self.endpoint.as_ref(),
                sessions: &mut self.sessions,
            };
            if let Err(e) = soft.disengage(&mut audio) {
                eprintln!("Foxglove: master volume: {e}; kept in the journal");
                soft.abandon();
            }
        }

        fn close(&mut self) {
            self.endpoint_meter = None;
            self.device = None;
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
                let sid = take(c2.GetSessionIdentifier());
                let system = c2.IsSystemSoundsSession() == S_OK;
                let (mut key, mut exe) = if system { (None, None) } else { program(pid) };
                if exe.is_none() && !system {
                    // A protected process (#1061): its path is in the session's identifier.
                    if let Some(nt) = id_path(&sid) {
                        let drives = self.drives.get_or_insert_with(drives);
                        let path = dos_path(nt, drives);
                        key = key.or_else(|| Some(exe_key(&path)));
                        exe = Some(path);
                    }
                }
                let (level, muted) = control
                    .cast::<ISimpleAudioVolume>()
                    .ok()
                    .map(|v| {
                        (
                            v.GetMasterVolume().unwrap_or(1.0),
                            v.GetMute().map(|m| m.as_bool()).unwrap_or(false),
                        )
                    })
                    .unwrap_or((1.0, false));
                self.sessions.push(Tracked {
                    id,
                    sid,
                    meter: control.cast::<IAudioMeterInformation>().ok(),
                    system,
                    description: exe.as_deref().and_then(file_description),
                    exe,
                    key,
                    level,
                    muted,
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
                    let windows = window_owners();
                    let owners: Vec<(u32, Option<String>)> =
                        windows.iter().map(|(p, e, _)| (*p, e.clone())).collect();
                    let apps: Vec<(u32, Option<String>)> = windows
                        .iter()
                        .filter(|(_, _, app)| *app)
                        .map(|(p, e, _)| (*p, e.clone()))
                        .collect();
                    let tree = parents();
                    let shell = shell_pid();
                    let active: Vec<Session> = self
                        .sessions
                        .iter()
                        .filter(|t| t.state == AudioSessionStateActive)
                        .map(|t| {
                            let app_window =
                                has_app_window(t.pid, t.exe.as_deref(), &owners, &apps);
                            Session {
                                windowed: has_window(t.pid, t.exe.as_deref(), &owners),
                                app_window,
                                parent: tree.get(&t.pid).copied().unwrap_or(0),
                                ..t.session()
                            }
                        })
                        .collect();
                    let list = active
                        .into_iter()
                        .map(|s| {
                            let owner = (s.app_window == Some(false))
                                .then(|| self.owner(&s, &tree, &owners, &apps, shell))
                                .flatten();
                            Session { owner, ..s }
                        })
                        .collect();
                    let _ = reply.send(Ok(list));
                }
                Req::Peaks(reply) => {
                    // SAFETY: COM call on this MTA thread.
                    let endpoint = self
                        .endpoint_meter
                        .as_ref()
                        .and_then(|m| unsafe { m.GetPeakValue() }.ok());
                    let sessions = self
                        .sessions
                        .iter()
                        .filter(|t| t.state == AudioSessionStateActive)
                        .map(Tracked::meter)
                        .collect();
                    let _ = reply.send(Ok(Peaks { endpoint, sessions }));
                }
                Req::Step(id, steps, reply) => {
                    if let Some(soft) = self.soft.as_mut() {
                        let mut audio = Com {
                            endpoint: self.endpoint.as_ref(),
                            sessions: &mut self.sessions,
                        };
                        if let Some(r) = soft.step_app(&mut audio, &id, steps, Instant::now()) {
                            let _ = reply.send(r);
                            return;
                        }
                    }
                    let r = self.find(&id).and_then(|t| t.volume()).and_then(|v| {
                        // SAFETY: COM calls on this MTA thread.
                        unsafe {
                            let level = v.GetMasterVolume().map_err(|e| e.to_string())?;
                            let new = step_level(level, steps);
                            v.SetMasterVolume(new, &OWN_CHANGES)
                                .map_err(|e| e.to_string())?;
                            Ok(new)
                        }
                    });
                    if let (Ok(new), Some(t)) = (&r, self.sessions.iter_mut().find(|t| t.id == id))
                    {
                        t.level = *new;
                    }
                    let _ = reply.send(r.map(|new| (new * 100.0).round() as u8));
                }
                Req::Mute(id, muted, reply) => {
                    let r = self.find(&id).and_then(|t| t.volume()).and_then(|v| {
                        // SAFETY: COM call on this MTA thread.
                        unsafe { v.SetMute(muted, &OWN_CHANGES) }.map_err(|e| e.to_string())?;
                        Ok(muted)
                    });
                    if let (Ok(_), Some(t)) = (&r, self.sessions.iter_mut().find(|t| t.id == id)) {
                        t.muted = muted;
                    }
                    if r.is_ok() {
                        if let Some(soft) = self.soft.as_mut() {
                            soft.app_muted(&id, muted);
                        }
                    }
                    let _ = reply.send(r);
                }
                Req::Volume(id, level, muted, own) => {
                    if let Some(t) = self.sessions.iter_mut().find(|t| t.id == id) {
                        t.level = level;
                        t.muted = muted;
                    }
                    if let (false, Some(soft)) = (own, self.soft.as_mut()) {
                        let mut audio = Com {
                            endpoint: self.endpoint.as_ref(),
                            sessions: &mut self.sessions,
                        };
                        soft.volume_event(&mut audio, &id, level, muted, Instant::now());
                    }
                }
                Req::AppVolume(id, reply) => {
                    let _ = reply.send(self.soft.as_ref().and_then(|s| s.app_volume(&id)));
                }
                Req::Recheck => {
                    if let Some(soft) = self.soft.as_mut() {
                        soft.set_today(today());
                    }
                    // The facts again (the exclusive-control setting may have changed).
                    if let (Some(e), Some(devices)) = (&self.endpoint, &self.devices) {
                        // SAFETY: COM call on this MTA thread.
                        if let Ok(d) = unsafe { devices.GetDefaultAudioEndpoint(eRender, eConsole) }
                        {
                            if let Some(facts) = device_facts(&d, e) {
                                if self
                                    .device
                                    .as_ref()
                                    .is_some_and(|o| o.endpoint == facts.endpoint)
                                {
                                    self.device = Some(facts);
                                }
                            }
                        }
                    }
                    self.evaluate();
                }
                Req::Stop(reply) => {
                    self.release();
                    self.stopped = true;
                    let _ = reply.send(());
                }
                Req::Level(id, reply) => {
                    // An app's own level on a device Foxglove sets the volume of (#1064).
                    let own = self.soft.as_ref().and_then(|s| s.own(&id));
                    let r = self.find(&id).and_then(|t| t.volume()).and_then(|v| {
                        // SAFETY: COM calls on this MTA thread.
                        unsafe {
                            let level = v.GetMasterVolume().map_err(|e| e.to_string())?;
                            let muted = v.GetMute().map_err(|e| e.to_string())?.as_bool();
                            Ok(((own.unwrap_or(level) * 100.0).round() as u8, muted))
                        }
                    });
                    let _ = reply.send(r);
                }
                Req::MasterLevel(reply) => {
                    if let Some(m) = self.soft.as_ref().and_then(Soft::master) {
                        let _ = reply.send(Ok(m));
                        return;
                    }
                    let r = match &self.endpoint {
                        None => Err("no output device".to_string()),
                        Some(e) => master_level(e).map_err(|e| e.to_string()),
                    };
                    let _ = reply.send(r);
                }
                Req::MasterStep(steps, reply) => {
                    if let Some(soft) = self.soft.as_mut().filter(|s| s.engaged().is_some()) {
                        let mut audio = Com {
                            endpoint: self.endpoint.as_ref(),
                            sessions: &mut self.sessions,
                        };
                        let _ = reply.send(soft.step_master(&mut audio, steps));
                        return;
                    }
                    // Not set by Foxglove: the device's own level; an unmute also frees the apps a
                    // muted master left muted (its writes are Foxglove's own, so no outside
                    // notification does it, #1072).
                    if let (Some(soft), Some(_)) = (self.soft.as_mut(), &self.endpoint) {
                        let mut audio = Com {
                            endpoint: self.endpoint.as_ref(),
                            sessions: &mut self.sessions,
                        };
                        let _ = reply.send(soft.plain_step(&mut audio, steps));
                        return;
                    }
                    let r = match &self.endpoint {
                        None => Err("no output device".to_string()),
                        Some(e) => master_step(e, steps).map_err(|e| e.to_string()),
                    };
                    let _ = reply.send(r);
                }
                Req::MasterMute(reply) => {
                    if let Some(soft) = self.soft.as_mut().filter(|s| s.engaged().is_some()) {
                        let mut audio = Com {
                            endpoint: self.endpoint.as_ref(),
                            sessions: &mut self.sessions,
                        };
                        let _ = reply.send(soft.toggle_mute(&mut audio));
                        return;
                    }
                    if let (Some(soft), Some(_)) = (self.soft.as_mut(), &self.endpoint) {
                        let mut audio = Com {
                            endpoint: self.endpoint.as_ref(),
                            sessions: &mut self.sessions,
                        };
                        let _ = reply.send(soft.plain_mute(&mut audio));
                        return;
                    }
                    let r = match &self.endpoint {
                        None => Err("no output device".to_string()),
                        Some(e) => master_mute(e).map_err(|e| e.to_string()),
                    };
                    let _ = reply.send(r);
                }
                Req::Master(level, muted, own) => {
                    // On a device Foxglove sets the volume of (#1064): master's new level, and
                    // the device held again, here in this notification's handling.
                    if let Some(soft) = self.soft.as_mut().filter(|s| s.engaged().is_some()) {
                        let mut audio = Com {
                            endpoint: self.endpoint.as_ref(),
                            sessions: &mut self.sessions,
                        };
                        if soft.endpoint_event(&mut audio, level, muted, own, Instant::now()) {
                            (self.notify)(Change::Adopted);
                        }
                        return;
                    }
                    // Paused by a journal that couldn't be read: the device's next change tries
                    // the start again (no polling, #1070).
                    if self.device.as_ref().is_some_and(|d| d.paused) {
                        self.evaluate();
                    }
                    if !own {
                        // Unmuted while Foxglove doesn't set its volume: the apps a muted master
                        // left muted when it stopped are unmuted too (#1070).
                        if !muted {
                            if let Some(soft) = self.soft.as_mut() {
                                let mut audio = Com {
                                    endpoint: self.endpoint.as_ref(),
                                    sessions: &mut self.sessions,
                                };
                                soft.unmute_left(&mut audio);
                            }
                        }
                        (self.notify)(Change::Master)
                    }
                }
                Req::Created(Control(c)) => {
                    let before = self.sessions.len();
                    self.track(c);
                    if self.sessions.len() > before {
                        if let Some(soft) = self.soft.as_mut() {
                            let mut audio = Com {
                                endpoint: self.endpoint.as_ref(),
                                sessions: &mut self.sessions,
                            };
                            if let Some(new) = audio.sessions().pop() {
                                soft.created(&mut audio, &new);
                            }
                        }
                    }
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
                Req::Device => {
                    self.release();
                    self.open();
                }
            }
        }

        /// Re-applies an app's coalesced level when its deadline comes (#1064).
        fn run_due(&mut self) {
            if let Some(soft) = self.soft.as_mut() {
                let mut audio = Com {
                    endpoint: self.endpoint.as_ref(),
                    sessions: &mut self.sessions,
                };
                if soft.run_due(&mut audio, Instant::now()) {
                    (self.notify)(Change::Adopted);
                }
            }
        }

        /// The app a windowless session belongs to (#1061): up its process tree past processes of
        /// its own program (a WebView2 host's utility process under its browser process), the
        /// first process of another program, when that one has an app window and isn't the shell.
        /// A parent that started after its child is another process that got the id: none.
        fn owner(
            &mut self,
            s: &Session,
            tree: &HashMap<u32, u32>,
            owners: &[(u32, Option<String>)],
            apps: &[(u32, Option<String>)],
            shell: u32,
        ) -> Option<Owner> {
            let own = s.key.as_deref()?;
            let mut child = s.pid;
            for _ in 0..8 {
                let parent = *tree.get(&child)?;
                if parent == 0 || parent == child || parent == shell {
                    return None;
                }
                if started(parent)? > started(child)? {
                    return None;
                }
                let (key, exe) = program(parent);
                let key = key?;
                if key == own {
                    child = parent;
                    continue;
                }
                if has_app_window(parent, exe.as_deref(), owners, apps) != Some(true) {
                    return None;
                }
                let description = match &exe {
                    Some(path) => self
                        .descriptions
                        .entry(path.clone())
                        .or_insert_with(|| file_description(path))
                        .clone(),
                    None => None,
                };
                return Some(Owner {
                    pid: parent,
                    name: display_name("", exe.as_deref(), description.as_deref(), false, parent),
                    key,
                    exe,
                });
            }
            None
        }

        fn drop_session(&mut self, id: &str) {
            if let Some(soft) = self.soft.as_mut() {
                soft.gone(id);
            }
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
        fn peaks(&self) -> std::result::Result<Peaks, String> {
            self.ask(Req::Peaks)
        }
        fn step(&self, id: &str, steps: i32) -> std::result::Result<u8, String> {
            self.ask(|r| Req::Step(id.to_string(), steps, r))
        }
        fn set_mute(&self, id: &str, muted: bool) -> std::result::Result<bool, String> {
            self.ask(|r| Req::Mute(id.to_string(), muted, r))
        }
        fn level(&self, id: &str) -> std::result::Result<(u8, bool), String> {
            self.ask(|r| Req::Level(id.to_string(), r))
        }
        fn master(&self) -> std::result::Result<(u8, bool), String> {
            self.ask(Req::MasterLevel)
        }
        fn step_master(&self, steps: i32) -> std::result::Result<u8, String> {
            self.ask(|r| Req::MasterStep(steps, r))
        }
        fn toggle_master_mute(&self) -> std::result::Result<bool, String> {
            self.ask(Req::MasterMute)
        }
        fn app_volume(&self, id: &str) -> Option<AppVolume> {
            let (reply, answer) = sync_channel(1);
            self.0.send(Req::AppVolume(id.to_string(), reply)).ok()?;
            answer.recv_timeout(REPLY).ok().flatten()
        }
    }

    /// The sessions' thread, for [`recheck`] and [`stop`].
    static CONTROL: Mutex<Option<Sender<Req>>> = Mutex::new(None);

    /// Days since 1970 (the journal's records' age).
    fn today() -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| (d.as_secs() / 86_400) as i64)
    }

    /// The per-device setting changed, or the Knob card is drawn: the device's facts are read
    /// again and the software master follows the setting (#1064).
    pub fn recheck() {
        if let Some(tx) = CONTROL.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
            let _ = tx.send(Req::Recheck);
        }
    }

    /// Foxglove quits: the device and every app back to Windows' own levels (#1064), waiting at
    /// most `within` for the sessions' thread.
    pub fn stop(within: Duration) {
        let tx = CONTROL.lock().unwrap_or_else(|e| e.into_inner()).clone();
        if let Some(tx) = tx {
            let (reply, answer) = sync_channel(1);
            if tx.send(Req::Stop(reply)).is_ok() {
                let _ = answer.recv_timeout(within);
            }
        }
    }

    /// Starts following Windows' sessions; `notify` hears each change (on the sessions' thread:
    /// only enqueue). `None` when the thread can't start.
    pub fn start(
        notify: impl Fn(Change) + Send + Sync + 'static,
        host: Option<Box<dyn master::Host>>,
    ) -> Option<Box<dyn Sessions>> {
        let (tx, rx): (Sender<Req>, Receiver<Req>) = channel();
        let worker_tx = tx.clone();
        *CONTROL.lock().unwrap_or_else(|e| e.into_inner()) = Some(tx.clone());
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
                    endpoint_meter: None,
                    master_changed: MasterChanged(worker_tx).into(),
                    sessions: Vec::new(),
                    drives: None,
                    descriptions: HashMap::new(),
                    soft: host.map(|h| Soft::new(h, today())),
                    device: None,
                    stopped: false,
                };
                w.open();
                loop {
                    master::publish_master(w.soft.as_ref().and_then(Soft::master));
                    // The only wait besides the channel: an app's coalesced re-apply (#1064).
                    let due = w.soft.as_ref().and_then(Soft::next_due);
                    let req = match due {
                        Some(at) => {
                            match rx.recv_timeout(at.saturating_duration_since(Instant::now())) {
                                Ok(req) => req,
                                Err(RecvTimeoutError::Timeout) => {
                                    w.run_due();
                                    continue;
                                }
                                Err(RecvTimeoutError::Disconnected) => break,
                            }
                        }
                        None => match rx.recv() {
                            Ok(req) => req,
                            Err(_) => break,
                        },
                    };
                    w.handle(req);
                }
                let _ = &w.devices_client;
            })
            .ok()?;
        Some(Box::new(Handle(tx)))
    }
}

#[cfg(windows)]
pub use imp::{recheck, start, stop};

/// Off Windows there are no sessions.
#[cfg(not(windows))]
pub fn start(
    _notify: impl Fn(Change) + Send + Sync + 'static,
    _host: Option<Box<dyn crate::master::Host>>,
) -> Option<Box<dyn Sessions>> {
    None
}

#[cfg(not(windows))]
pub fn recheck() {}

#[cfg(not(windows))]
pub fn stop(_within: std::time::Duration) {}

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
            pid: 1,
            key: Some(exe_key(&format!(r"C:\Apps\{id}.exe"))),
            app_window: Some(true),
            ..Session::default()
        }
    }

    fn ids(list: &[AppGroup]) -> Vec<&str> {
        list.iter().map(|s| s.name.as_str()).collect()
    }

    /// One read with no device meter (an old driver): every audible session counts.
    fn peaks(p: &[(&str, f32)]) -> Peaks {
        Peaks {
            endpoint: None,
            sessions: p
                .iter()
                .map(|(id, v)| Meter {
                    id: id.to_string(),
                    peak: *v,
                    level: 1.0,
                    muted: false,
                })
                .collect(),
        }
    }

    #[test]
    fn a_program_with_a_window_anywhere_counts_as_windowed() {
        let owners = vec![
            (10, Some(r"C:\Apps\Player\player.exe".to_string())),
            (20, Some(r"C:\Apps\Chat\chat.exe".to_string())),
            (30, None),
        ];
        // Its own window (hidden ones count: the list is every top-level window).
        assert_eq!(has_window(10, None, &owners), Some(true));
        // A helper process of a program whose main process has the window.
        assert_eq!(
            has_window(21, Some(r"C:\APPS\chat\CHAT.exe"), &owners),
            Some(true)
        );
        // A background helper of its own: no window.
        assert_eq!(
            has_window(40, Some(r"C:\Vendor\helper.exe"), &owners),
            Some(false)
        );
        // Can't tell: no pid (system sounds), or no windows read at all.
        assert_eq!(has_window(0, None, &owners), None);
        assert_eq!(has_window(40, Some("x.exe"), &[]), None);
        // App windows are a part of them: the chat app has one, the player only a tray window.
        let apps = vec![owners[1].clone()];
        assert_eq!(
            has_app_window(21, Some(r"C:\Apps\Chat\chat.exe"), &owners, &apps),
            Some(true)
        );
        assert_eq!(has_app_window(10, None, &owners, &apps), Some(false));
        assert_eq!(has_app_window(0, None, &owners, &apps), None);
    }

    fn rows(list: &[&str]) -> Vec<AppGroup> {
        group(list.iter().map(|id| session(id)).collect())
    }

    #[test]
    fn only_heard_sessions_are_offered_and_they_stay() {
        let mut h = Heard::default();
        let active = || rows(&["game", "helper", "music"]);
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
        assert_eq!(ids(&h.offer(rows(&["game"]))), ["game"]);
        assert_eq!(ids(&h.offer(active())), ["music", "game"]);
        // Ended: forgotten.
        h.forget("music");
        assert!(!h.has("music"));
        assert_eq!(ids(&h.offer(active())), ["game"]);
    }

    // The 11/10 capture (#1060/#1061 research: session list, meters and logs on the user's PC).

    const VIVALDI: &str = r"C:\Users\u\AppData\Local\Vivaldi\Application\vivaldi.exe";
    const LIGHTING: &str =
        r"C:\Program Files\WindowsApps\Vendor.LightStudio_1.0_x64__abc\LightStudio-background.exe";
    const WALLPAPER: &str =
        r"C:\Program Files (x86)\Steam\steamapps\common\wallpaper_engine\wallpaper32.exe";
    const GAME: &str = r"D:\Games\EFT\EscapeFromTarkov.exe";

    /// The Active sessions as the worker lists them: Vivaldi's only session in its audio-service
    /// child (no window of its own; the browser process has two app windows), an audio-reactive
    /// lighting app and wallpaper holding loopback sessions (hidden windows only), and a game
    /// whose path came from its session identifier (protected process).
    fn captured() -> Vec<Session> {
        vec![
            Session {
                id: "vivaldi-audio".into(),
                name: "Vivaldi".into(),
                pid: 43484,
                parent: 18132,
                exe: Some(VIVALDI.into()),
                key: Some(exe_key(VIVALDI)),
                windowed: Some(true),
                app_window: Some(true),
                ..Session::default()
            },
            Session {
                id: "lighting".into(),
                name: "LightStudio-background".into(),
                pid: 5532,
                parent: 3100,
                exe: Some(LIGHTING.into()),
                key: Some(package_key("Vendor.LightStudio_abc")),
                windowed: Some(true),
                app_window: Some(false),
                ..Session::default()
            },
            Session {
                id: "wallpaper".into(),
                name: "wallpaper32".into(),
                pid: 19848,
                parent: 3100,
                exe: Some(WALLPAPER.into()),
                key: Some(exe_key(WALLPAPER)),
                windowed: Some(true),
                app_window: Some(false),
                ..Session::default()
            },
            Session {
                id: "game".into(),
                name: "EscapeFromTarkov".into(),
                pid: 38880,
                parent: 42608,
                exe: Some(GAME.into()),
                key: Some(exe_key(GAME)),
                windowed: Some(true),
                app_window: Some(true),
                ..Session::default()
            },
        ]
    }

    fn meter(id: &str, peak: f32, level: f32) -> Meter {
        Meter {
            id: id.into(),
            peak,
            level,
            muted: false,
        }
    }

    /// The captured reads: the device's peak, Vivaldi's raw peak at its 8 % level, and the
    /// lighting and wallpaper sessions' (the mix again); later the game plays as well.
    fn captured_reads() -> Vec<Peaks> {
        let read = |e: f32, vivaldi: f32, game: f32, mirror: f32| Peaks {
            endpoint: Some(e),
            sessions: vec![
                meter("vivaldi-audio", vivaldi, 0.08),
                meter("lighting", mirror, 1.0),
                meter("wallpaper", mirror, 1.0),
                meter("game", game, 1.0),
            ],
        };
        vec![
            read(0.0044, 0.0548, 0.0, 0.0043),
            read(0.0082, 0.1029, 0.0, 0.0082),
            read(0.0061, 0.0760, 0.0, 0.0061),
            read(0.0044, 0.0548, 0.0, 0.0044),
            // The game plays too: the mix is between the louder source and the sum.
            read(0.0070, 0.0548, 0.0038, 0.0070),
            read(0.0079, 0.0548, 0.0038, 0.0079),
        ]
    }

    #[test]
    fn the_captured_mirrors_are_never_heard_and_the_cycle_is_vivaldi_then_the_game() {
        let rows = group(captured());
        let mut h = Heard::default();
        h.learn(&rows);
        for read in captured_reads() {
            h.note(&read);
        }
        assert!(h.has("vivaldi-audio") && h.has("game"));
        assert!(!h.has("lighting") && !h.has("wallpaper"));
        assert!(h.is_mirror("lighting") && h.is_mirror("wallpaper"));
        assert_eq!(ids(&h.offer(rows)), ["Vivaldi", "EscapeFromTarkov"]);
    }

    #[test]
    fn a_mirror_isnt_heard_even_on_its_first_read() {
        // The first click reads once: the lighting app's first match already keeps it out.
        let rows = group(captured());
        let mut h = Heard::default();
        h.learn(&rows);
        h.note(&captured_reads()[0]);
        assert_eq!(ids(&h.offer(rows)), ["Vivaldi"]);
        assert!(
            !h.is_mirror("lighting"),
            "one match: not called a mirror yet"
        );
    }

    #[test]
    fn a_source_at_full_level_is_heard_and_its_mirrors_arent() {
        // Vivaldi at 100 %: its peak is the mix too, but its app has an app window, so it is the
        // source beyond doubt; the lighting app's peak is explained by it.
        let rows = group(captured());
        let mut h = Heard::default();
        h.learn(&rows);
        for e in [0.05, 0.08, 0.03] {
            h.note(&Peaks {
                endpoint: Some(e),
                sessions: vec![
                    meter("vivaldi-audio", e, 1.0),
                    meter("lighting", e, 1.0),
                    meter("wallpaper", e * 1.02, 1.0),
                ],
            });
        }
        assert_eq!(ids(&h.offer(rows)), ["Vivaldi"]);
    }

    #[test]
    fn a_tray_player_alone_with_a_mirror_is_a_tie_and_both_are_offered() {
        // A player minimised to the tray has no app window: it and the lighting app look alike.
        let mut list = captured();
        list.retain(|s| s.id == "lighting");
        list.push(Session {
            id: "player".into(),
            name: "Player".into(),
            pid: 700,
            exe: Some(r"C:\Apps\Player\Player.exe".into()),
            key: Some(exe_key(r"C:\Apps\Player\Player.exe")),
            windowed: Some(true),
            app_window: Some(false),
            ..Session::default()
        });
        let rows = group(list);
        let mut h = Heard::default();
        h.learn(&rows);
        for e in [0.2, 0.3, 0.25, 0.1] {
            h.note(&Peaks {
                endpoint: Some(e),
                sessions: vec![meter("lighting", e, 1.0), meter("player", e, 1.0)],
            });
        }
        assert_eq!(ids(&h.offer(rows)), ["LightStudio-background", "Player"]);
    }

    #[test]
    fn a_muted_or_silent_level_app_that_plays_is_heard() {
        // Its own meter is before its level and mute, so it isn't the mix (the game makes it).
        let rows = group(captured());
        let mut h = Heard::default();
        h.learn(&rows);
        for e in [0.02, 0.04, 0.03] {
            h.note(&Peaks {
                endpoint: Some(e),
                sessions: vec![
                    Meter {
                        muted: true,
                        ..meter("vivaldi-audio", 0.05, 0.5)
                    },
                    meter("game", e, 1.0),
                    meter("lighting", e, 1.0),
                ],
            });
        }
        let mut zero = Heard::default();
        zero.learn(&rows);
        zero.note(&Peaks {
            endpoint: Some(0.02),
            sessions: vec![meter("vivaldi-audio", 0.05, 0.0), meter("game", 0.02, 1.0)],
        });
        assert_eq!(ids(&h.offer(rows.clone())), ["Vivaldi", "EscapeFromTarkov"]);
        assert!(zero.has("vivaldi-audio"), "at 0 %");
        assert!(!h.has("lighting"));
    }

    #[test]
    fn a_mirror_that_starts_playing_its_own_sound_joins_last() {
        let rows = group(captured());
        let mut h = Heard::default();
        h.learn(&rows);
        for read in captured_reads() {
            h.note(&read);
        }
        // Its peak stops following the mix: one read isn't enough (a mirror called so), as
        // many as it took to call it one are.
        let own = |h: &mut Heard| {
            h.note(&Peaks {
                endpoint: Some(0.004),
                sessions: vec![
                    meter("vivaldi-audio", 0.05, 0.08),
                    meter("lighting", 0.3, 1.0),
                ],
            })
        };
        assert!(!own(&mut h));
        assert!(!own(&mut h));
        assert!(own(&mut h));
        assert_eq!(
            ids(&h.offer(rows)),
            ["Vivaldi", "EscapeFromTarkov", "LightStudio-background"]
        );
    }

    #[test]
    fn a_read_near_the_mix_gives_no_verdict() {
        // 15 % off the mix while the mix is explained: neither a match nor its own sound.
        let rows = group(captured());
        let mut h = Heard::default();
        h.learn(&rows);
        h.note(&Peaks {
            endpoint: Some(0.01),
            sessions: vec![
                meter("vivaldi-audio", 0.125, 0.08),
                meter("lighting", 0.0115, 1.0),
            ],
        });
        assert!(!h.has("lighting") && !h.is_mirror("lighting"));
    }

    #[test]
    fn without_the_devices_meter_every_audible_session_counts() {
        let rows = group(captured());
        let mut h = Heard::default();
        h.learn(&rows);
        let mut read = captured_reads().remove(0);
        read.endpoint = None;
        h.note(&read);
        assert_eq!(
            ids(&h.offer(rows)),
            ["Vivaldi", "LightStudio-background", "wallpaper32"]
        );
    }

    #[test]
    fn a_parent_with_a_silent_session_and_a_playing_child_are_one_row() {
        // Chromium: the browser process (app windows) holds a silent session, its audio-service
        // child (same program) the playing one.
        let browser = Session {
            id: "vivaldi-main".into(),
            name: "Vivaldi".into(),
            pid: 18132,
            parent: 900,
            exe: Some(VIVALDI.into()),
            key: Some(exe_key(VIVALDI)),
            windowed: Some(true),
            app_window: Some(true),
            ..Session::default()
        };
        let child = Session {
            name: "vivaldi".into(),
            ..captured().remove(0)
        };
        let rows = group(vec![child, browser]);
        assert_eq!(rows.len(), 1);
        let row = &rows[0];
        assert_eq!(row.name, "Vivaldi", "the windowed process's name");
        assert_eq!(row.exe.as_deref(), Some(VIVALDI));
        let members: Vec<&str> = row.members.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(members, ["vivaldi-audio", "vivaldi-main"]);
        // Only the child plays: the row is heard.
        let mut h = Heard::default();
        h.learn(&rows);
        h.note(&peaks(&[("vivaldi-main", 0.0), ("vivaldi-audio", 0.1)]));
        assert_eq!(h.offer(rows).len(), 1);
    }

    #[test]
    fn a_windowless_helper_joins_its_windowed_parent_but_a_game_keeps_its_own_row() {
        const APP: &str = r"C:\Apps\Notes\Notes.exe";
        const VIEW: &str = r"C:\Program Files (x86)\Microsoft\EdgeWebView\msedgewebview2.exe";
        const LAUNCHER: &str = r"C:\Program Files (x86)\Steam\steam.exe";
        let notes = Owner {
            pid: 50,
            key: exe_key(APP),
            name: "Notes".into(),
            exe: Some(APP.into()),
        };
        let webview = Session {
            id: "webview".into(),
            name: "Microsoft Edge WebView2".into(),
            pid: 52,
            parent: 51,
            exe: Some(VIEW.into()),
            key: Some(exe_key(VIEW)),
            windowed: Some(true),
            app_window: Some(false),
            owner: Some(notes.clone()),
            ..Session::default()
        };
        let game = Session {
            owner: Some(Owner {
                pid: 17112,
                key: exe_key(LAUNCHER),
                name: "Steam".into(),
                exe: Some(LAUNCHER.into()),
            }),
            ..captured().remove(3)
        };
        let rows = group(vec![webview.clone(), game]);
        assert_eq!(ids(&rows), ["Notes", "EscapeFromTarkov"]);
        assert_eq!(rows[0].key, exe_key(APP));
        assert_eq!(
            rows[0].exe.as_deref(),
            Some(APP),
            "Hide stores the app, not the host"
        );
        assert_eq!(rows[0].icon.as_deref(), Some(APP));
        assert_eq!(rows[0].windowed, Some(true));
        assert_eq!(rows[0].members[0].exe.as_deref(), Some(VIEW));
        assert_eq!(rows[1].exe.as_deref(), Some(GAME));
        // With the app's own session too: one row, named and keyed by the app.
        let own = Session {
            id: "notes".into(),
            name: "Notes".into(),
            pid: 50,
            exe: Some(APP.into()),
            key: Some(exe_key(APP)),
            windowed: Some(true),
            app_window: Some(true),
            ..Session::default()
        };
        let rows = group(vec![webview.clone(), own]);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].members.len(), 2);
        // A helper whose program has an app window of its own isn't joined to anything.
        let windowed = Session {
            app_window: Some(true),
            ..webview
        };
        assert_eq!(group(vec![windowed])[0].key, exe_key(VIEW));
    }

    #[test]
    fn a_protected_process_is_keyed_by_the_path_in_its_identifier() {
        let ident = r"{0.0.0.00000000}.{3f2a}|\Device\HarddiskVolume5\Games\EFT\EscapeFromTarkov.exe%b{00000000-0000-0000-0000-000000000000}";
        let nt = id_path(ident).unwrap();
        assert_eq!(
            nt,
            r"\Device\HarddiskVolume5\Games\EFT\EscapeFromTarkov.exe"
        );
        let drives = vec![
            ("C:".to_string(), r"\Device\HarddiskVolume3".to_string()),
            ("D:".to_string(), r"\Device\HarddiskVolume5".to_string()),
        ];
        assert_eq!(dos_path(nt, &drives), GAME);
        // The key matches the one a readable path gives.
        assert_eq!(exe_key(&dos_path(nt, &drives)), exe_key(GAME));
        // Not a prefix of another volume's name; unknown devices stay as they are.
        assert_eq!(
            dos_path(r"\Device\HarddiskVolume50\x.exe", &drives),
            r"\Device\HarddiskVolume50\x.exe"
        );
        assert_eq!(id_path("no bar here"), None);
        assert_eq!(id_path("{x}|%b{y}"), None);
        // The system sounds' identifier has no program path.
        assert_eq!(id_path(r"{0.0.0.00000000}.{x}|#%b{y}"), None);
        // An instance identifier carries a trailing part after another bar.
        assert_eq!(
            id_path(r"{a}|\Device\HarddiskVolume3\a.exe%b{b}|1%b38880"),
            Some(r"\Device\HarddiskVolume3\a.exe")
        );
    }

    #[test]
    fn the_system_sounds_and_unknown_programs_are_rows_of_their_own() {
        let system = Session {
            id: "sys".into(),
            name: "System sounds".into(),
            ..Session::default()
        };
        let other = Session {
            id: "other".into(),
            name: "System sounds".into(),
            ..Session::default()
        };
        let rows = group(vec![system, other]);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].key, "id:sys");
        assert_eq!(rows[0].windowed, None);
    }

    #[test]
    fn two_rows_with_one_name_add_their_folder() {
        let s = |id: &str, exe: &str| Session {
            id: id.into(),
            name: "Vivaldi".into(),
            pid: 1,
            exe: Some(exe.into()),
            key: Some(exe_key(exe)),
            ..Session::default()
        };
        let mut rows = group(vec![
            s("a", r"C:\Apps\Vivaldi\vivaldi.exe"),
            s("b", r"C:\Apps\Vivaldi Snapshot\vivaldi.exe"),
            Session {
                name: "Player".into(),
                ..s("c", r"C:\Apps\Player\player.exe")
            },
        ]);
        tell_apart(&mut rows);
        assert_eq!(
            ids(&rows),
            ["Vivaldi · Vivaldi", "Vivaldi · Vivaldi Snapshot", "Player"]
        );
        for r in &rows {
            let l = r.name.to_lowercase();
            assert!(!l.contains("session") && !l.contains("(2"));
        }
        // Chromium-style installs: the direct folder is "Application" for both, so the folder
        // where the paths differ names them.
        let mut rows = group(vec![
            s(
                "a",
                r"C:\Users\u\AppData\Local\Vivaldi\Application\vivaldi.exe",
            ),
            s(
                "b",
                r"C:\Users\u\AppData\Local\Vivaldi Snapshot\Application\vivaldi.exe",
            ),
        ]);
        tell_apart(&mut rows);
        assert_eq!(
            ids(&rows),
            ["Vivaldi · Vivaldi", "Vivaldi · Vivaldi Snapshot"]
        );
        // The same path twice (can't be told apart): no folder added.
        let mut same = vec![rows[0].clone(), rows[0].clone()];
        same[0].name = "Vivaldi".into();
        same[1].name = "Vivaldi".into();
        tell_apart(&mut same);
        assert_eq!(ids(&same), ["Vivaldi", "Vivaldi"]);
    }

    #[test]
    fn a_contribution_is_the_peak_at_its_level_and_nothing_muted() {
        assert!((meter("a", 0.5, 0.08).contribution() - 0.04).abs() < 1e-6);
        assert_eq!(
            Meter {
                muted: true,
                ..meter("a", 0.5, 1.0)
            }
            .contribution(),
            0.0
        );
    }
}
