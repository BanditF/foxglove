// SPDX-License-Identifier: MIT
//! The rebuild watch (R2 of `review/ux-plan-zero-click-rebuild.md`, §4): notices that the
//! program file or manifest of a dev-folder app was rebuilt, waits until the file has settled and
//! reports what it found. It carries nothing and restarts nothing: the report (`Outcome`) is
//! for R3, and the carry and the launch check stay where they are (`keep_followed`,
//! `void_if_changed`).
//!
//! No polling. The only waits are the OS change notification (no timeout while idle) and a timer
//! that a change event arms: 1.5 s of quiet after the last event, then at most 3 readiness tries
//! (now, +5 s, +30 s). A new event during the tries starts over; with no event nothing runs.
//!
//! The caller must start a watch only for an app that is enrolled under a dev folder with
//! developer mode on (`Registry::dev_folder_of` is `Some`), and stop it when that stops holding.
//! The watch is not recursive, follows two file names and opens the folder with full share flags
//! so it never blocks the build. The file is read with read-only sharing: a writer still holding
//! it gives a sharing violation, which counts as "not ready".
//!
//! The hash reported here is a readiness fact, not a check: the bytes that get started must be
//! checked again by the launch check (the TOCTOU point of the plan, §7.2, belongs to R3/S7).

use std::path::{Path, PathBuf};
use std::time::Duration;

/// The two files a rebuild changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Which {
    Exe,
    Manifest,
}

/// What to watch: the app's id (for the log), its program file and its manifest.
#[derive(Clone, Debug)]
pub struct Target {
    pub id: String,
    pub exe: PathBuf,
    pub manifest: PathBuf,
}

/// Quiet time and retry delays. [`Timing::DEFAULT`] is the plan's.
#[derive(Clone, Copy, Debug)]
pub struct Timing {
    pub quiet: Duration,
    /// Waits before try 2 and try 3 (so there are `retries.len() + 1` tries).
    pub retries: [Duration; 2],
}

impl Timing {
    pub const DEFAULT: Timing = Timing {
        quiet: Duration::from_millis(1500),
        retries: [Duration::from_secs(5), Duration::from_secs(30)],
    };
}

/// One readiness check of one file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Probed {
    Ready {
        size: u64,
        sha256: String,
    },
    /// A writer still holds the file (sharing violation).
    Busy,
    BadHeader,
    /// The size changed while it was read.
    SizeChanged,
    Unreadable(String),
}

/// Reads and hashes a file on one handle.
pub trait Probe {
    fn check(&mut self, path: &Path, which: Which) -> Probed;
}

/// Why a rebuild was not taken up (rows 10 and 11).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GaveUp {
    /// Busy, bad header or a size that kept changing: "looks unfinished".
    Unfinished,
    /// Could not be read at all.
    Unreadable,
}

/// What the watch reports.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The new files are readable and complete. Hashes are hex SHA-256.
    Settled {
        exe_sha256: String,
        manifest_sha256: String,
        size: u64,
    },
    GaveUp(GaveUp),
}

#[derive(Debug)]
enum State {
    Idle,
    /// Waiting for the quiet time after the last event.
    Quiet {
        at: Duration,
    },
    /// Try `tries` failed; the next one is at `at`.
    Retry {
        at: Duration,
        tries: usize,
    },
}

/// The debounce and readiness rules on a clock the caller supplies (`now` is time since any
/// fixed start). No I/O of its own.
pub struct Machine {
    id: String,
    timing: Timing,
    state: State,
    first_event: Option<Duration>,
    last: Option<(String, String)>,
}

impl Machine {
    pub fn new(id: &str, timing: Timing) -> Machine {
        Machine {
            id: id.to_string(),
            timing,
            state: State::Idle,
            first_event: None,
            last: None,
        }
    }

    /// When the next check is due; `None` while idle (nothing is waited for).
    pub fn deadline(&self) -> Option<Duration> {
        match self.state {
            State::Idle => None,
            State::Quiet { at } | State::Retry { at, .. } => Some(at),
        }
    }

    /// A change in one of the watched files: starts, or restarts, the quiet time and the tries.
    pub fn on_change(&mut self, now: Duration, which: Which) {
        if matches!(self.state, State::Idle) {
            self.first_event = Some(now);
            log::info!(
                "rebuild {}: change seen {}",
                self.id,
                match which {
                    Which::Exe => "exe",
                    Which::Manifest => "manifest",
                }
            );
        }
        self.state = State::Quiet {
            at: now + self.timing.quiet,
        };
    }

    /// The timer ran out: one readiness try. Returns what is now known, if anything.
    pub fn on_deadline(
        &mut self,
        now: Duration,
        probe: &mut dyn Probe,
        t: &Target,
    ) -> Option<Outcome> {
        let tries = match self.state {
            State::Idle => return None,
            State::Quiet { at } | State::Retry { at, .. } if now < at => return None,
            State::Quiet { .. } => 1,
            State::Retry { tries, .. } => tries + 1,
        };
        let max = self.timing.retries.len() + 1;
        let exe = probe.check(&t.exe, Which::Exe);
        let manifest = if t.manifest == t.exe {
            exe.clone()
        } else {
            probe.check(&t.manifest, Which::Manifest)
        };
        match (exe, manifest) {
            (Probed::Ready { size, sha256: e }, Probed::Ready { sha256: m, .. }) => {
                let ms = self
                    .first_event
                    .map_or(0, |f| now.saturating_sub(f).as_millis());
                self.state = State::Idle;
                if self.last.as_ref() == Some(&(e.clone(), m.clone())) {
                    log::info!(
                        "rebuild {}: settled after {ms} ms, files unchanged",
                        self.id
                    );
                    return None;
                }
                let old = self.last.as_ref().map_or("-", |l| &l.0[..8.min(l.0.len())]);
                log::info!(
                    "rebuild {}: settled after {ms} ms, size {size}, sha256 {old}\u{2192}{}",
                    self.id,
                    &e[..8.min(e.len())]
                );
                self.last = Some((e.clone(), m.clone()));
                Some(Outcome::Settled {
                    exe_sha256: e,
                    manifest_sha256: m,
                    size,
                })
            }
            (a, b) => {
                let bad = if matches!(a, Probed::Ready { .. }) {
                    b
                } else {
                    a
                };
                let (why, kind) = match &bad {
                    Probed::Busy => ("sharing violation".to_string(), GaveUp::Unfinished),
                    Probed::BadHeader => ("bad header".to_string(), GaveUp::Unfinished),
                    Probed::SizeChanged => ("size changed".to_string(), GaveUp::Unfinished),
                    Probed::Unreadable(e) => (format!("unreadable: {e}"), GaveUp::Unreadable),
                    Probed::Ready { .. } => unreachable!("one side is not ready"),
                };
                log::info!(
                    "rebuild {}: not ready: {why}, try {tries} of {max}",
                    self.id
                );
                if tries < max {
                    self.state = State::Retry {
                        at: now + self.timing.retries[tries - 1],
                        tries,
                    };
                    None
                } else {
                    log::info!("rebuild {}: gave up", self.id);
                    self.state = State::Idle;
                    Some(Outcome::GaveUp(kind))
                }
            }
        }
    }
}

/// What ended a wait on the event source.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Wake {
    Changed(Vec<Which>),
    Timeout,
    Stop,
}

/// Where change events come from (the OS in production, a script in tests) and the clock.
pub trait EventSource {
    /// Time since any fixed start.
    fn now(&self) -> Duration;
    /// Blocks until a change, the timeout (`None` = no timeout) or a stop.
    fn wait(&mut self, timeout: Option<Duration>) -> std::io::Result<Wake>;
}

/// Runs the watch until the source says stop (or fails). `on_outcome` gets every outcome.
pub fn run(
    t: &Target,
    timing: Timing,
    src: &mut dyn EventSource,
    probe: &mut dyn Probe,
    on_outcome: &mut dyn FnMut(Outcome),
) {
    let mut m = Machine::new(&t.id, timing);
    loop {
        let timeout = m.deadline().map(|d| d.saturating_sub(src.now()));
        match src.wait(timeout) {
            Ok(Wake::Changed(files)) => {
                for f in files {
                    m.on_change(src.now(), f);
                }
            }
            Ok(Wake::Timeout) => {
                if let Some(o) = m.on_deadline(src.now(), probe, t) {
                    on_outcome(o);
                }
            }
            Ok(Wake::Stop) => return,
            Err(e) => {
                log::warn!("rebuild {}: watch failed: {e}", t.id);
                return;
            }
        }
    }
}

/// Whether `bytes` start like a Windows program: `MZ`, and `PE\0\0` where the header points.
pub fn looks_like_pe(bytes: &[u8]) -> bool {
    if bytes.len() < 0x40 || &bytes[..2] != b"MZ" {
        return false;
    }
    let off = u32::from_le_bytes([bytes[0x3c], bytes[0x3d], bytes[0x3e], bytes[0x3f]]) as usize;
    bytes.get(off..off + 4) == Some(b"PE\0\0")
}

#[cfg(windows)]
#[allow(unused_imports)]
pub use win::{spawn, spawn_with, FileProbe, Handle};

#[cfg(windows)]
mod win {
    use super::*;
    use std::io::Read;
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::fs::OpenOptionsExt;
    use std::sync::Arc;
    use std::time::Instant;
    use windows_sys::Win32::Foundation::{
        CloseHandle, HANDLE, INVALID_HANDLE_VALUE, WAIT_OBJECT_0, WAIT_TIMEOUT,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, ReadDirectoryChangesW, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OVERLAPPED,
        FILE_LIST_DIRECTORY, FILE_NOTIFY_CHANGE_CREATION, FILE_NOTIFY_CHANGE_FILE_NAME,
        FILE_NOTIFY_CHANGE_LAST_WRITE, FILE_NOTIFY_CHANGE_SIZE, FILE_SHARE_DELETE, FILE_SHARE_READ,
        FILE_SHARE_WRITE, OPEN_EXISTING,
    };
    use windows_sys::Win32::System::Threading::{
        CreateEventW, SetEvent, WaitForMultipleObjects, INFINITE,
    };
    use windows_sys::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};

    const ERROR_SHARING_VIOLATION: i32 = 32;
    const ERROR_LOCK_VIOLATION: i32 = 33;

    /// Reads with read-only sharing, so a writer that still holds the file shows as busy.
    pub struct FileProbe;

    /// The largest program and manifest the probe reads (bigger is "unreadable", never allocated).
    const MAX_EXE: u64 = 1 << 30;
    const MAX_MANIFEST: u64 = 1 << 20;

    impl Probe for FileProbe {
        fn check(&mut self, path: &Path, which: Which) -> Probed {
            let max = if which == Which::Exe {
                MAX_EXE
            } else {
                MAX_MANIFEST
            };
            check_capped(path, which, max)
        }
    }

    /// Reads in chunks on one handle (no buffer sized from the file), hashing as it goes.
    pub(super) fn check_capped(path: &Path, which: Which, max: u64) -> Probed {
        use sha2::{Digest, Sha256};
        let busy = |e: &std::io::Error| {
            matches!(
                e.raw_os_error(),
                Some(ERROR_SHARING_VIOLATION | ERROR_LOCK_VIOLATION)
            )
        };
        let mut f = match std::fs::OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ)
            .open(path)
        {
            Ok(f) => f,
            Err(e) if busy(&e) => return Probed::Busy,
            Err(e) => return Probed::Unreadable(e.to_string()),
        };
        let len = match f.metadata() {
            Ok(m) => m.len(),
            Err(e) => return Probed::Unreadable(e.to_string()),
        };
        if len > max {
            return Probed::Unreadable("too large".into());
        }
        let mut hash = Sha256::new();
        let mut chunk = vec![0u8; 64 * 1024];
        let mut total = 0u64;
        let mut head: Vec<u8> = Vec::new();
        loop {
            let n = match f.read(&mut chunk) {
                Ok(n) => n,
                Err(e) if busy(&e) => return Probed::Busy,
                Err(e) => return Probed::Unreadable(e.to_string()),
            };
            if n == 0 {
                break;
            }
            total += n as u64;
            if total > max {
                return Probed::Unreadable("too large".into());
            }
            hash.update(&chunk[..n]);
            if head.len() < 0x1000 {
                head.extend_from_slice(&chunk[..n.min(0x1000)]);
            }
        }
        if total != len {
            return Probed::SizeChanged;
        }
        if which == Which::Exe && !looks_like_pe(&head) {
            return Probed::BadHeader;
        }
        Probed::Ready {
            size: len,
            sha256: hash.finalize().iter().map(|b| format!("{b:02x}")).collect(),
        }
    }

    struct Ev(HANDLE);
    // An event handle is safe to signal and wait on from any thread.
    unsafe impl Send for Ev {}
    unsafe impl Sync for Ev {}
    impl Ev {
        fn new() -> std::io::Result<Ev> {
            let h = unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) };
            if h.is_null() {
                return Err(std::io::Error::last_os_error());
            }
            Ok(Ev(h))
        }
    }
    impl Drop for Ev {
        fn drop(&mut self) {
            unsafe { CloseHandle(self.0) };
        }
    }

    /// One watched folder with a pending `ReadDirectoryChangesW` (never recursive).
    struct Dir {
        h: HANDLE,
        ev: Ev,
        ov: Box<OVERLAPPED>,
        /// DWORD-aligned.
        buf: Box<[u32; 4096]>,
        names: Vec<(Vec<u16>, Which)>,
    }

    impl Dir {
        fn open(dir: &Path, names: Vec<(Vec<u16>, Which)>) -> std::io::Result<Dir> {
            let wide: Vec<u16> = dir.as_os_str().encode_wide().chain([0]).collect();
            let h = unsafe {
                CreateFileW(
                    wide.as_ptr(),
                    FILE_LIST_DIRECTORY,
                    FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                    std::ptr::null(),
                    OPEN_EXISTING,
                    FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OVERLAPPED,
                    std::ptr::null_mut(),
                )
            };
            if h == INVALID_HANDLE_VALUE {
                let e = std::io::Error::last_os_error();
                return Err(std::io::Error::other(format!("{}: {e}", dir.display())));
            }
            // Made before `d`, so the folder handle is closed if the event can't be.
            let ev = match Ev::new() {
                Ok(ev) => ev,
                Err(e) => {
                    unsafe { CloseHandle(h) };
                    return Err(e);
                }
            };
            let mut d = Dir {
                h,
                ev,
                ov: Box::new(unsafe { std::mem::zeroed() }),
                buf: Box::new([0; 4096]),
                names,
            };
            d.arm()?;
            Ok(d)
        }

        fn arm(&mut self) -> std::io::Result<()> {
            *self.ov = unsafe { std::mem::zeroed() };
            self.ov.hEvent = self.ev.0;
            let ok = unsafe {
                ReadDirectoryChangesW(
                    self.h,
                    self.buf.as_mut_ptr().cast(),
                    (self.buf.len() * 4) as u32,
                    0,
                    FILE_NOTIFY_CHANGE_FILE_NAME
                        | FILE_NOTIFY_CHANGE_LAST_WRITE
                        | FILE_NOTIFY_CHANGE_SIZE
                        | FILE_NOTIFY_CHANGE_CREATION,
                    std::ptr::null_mut(),
                    &mut *self.ov,
                    None,
                )
            };
            if ok == 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        }

        /// The finished request's files, re-armed before the buffer is read so no later change
        /// is missed. An overflow (0 bytes) counts as both files changed.
        fn complete(&mut self) -> std::io::Result<Vec<Which>> {
            let mut n = 0u32;
            if unsafe { GetOverlappedResult(self.h, &*self.ov, &mut n, 0) } == 0 {
                return Err(std::io::Error::last_os_error());
            }
            let bytes: Vec<u8> = self
                .buf
                .iter()
                .flat_map(|w| w.to_le_bytes())
                .take(n as usize)
                .collect();
            self.arm()?;
            if n == 0 {
                return Ok(vec![Which::Exe, Which::Manifest]);
            }
            let mut out = Vec::new();
            let mut at = 0usize;
            loop {
                let rd = |o: usize| {
                    bytes
                        .get(o..o + 4)
                        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize)
                };
                let (Some(next), Some(len)) = (rd(at), rd(at + 8)) else {
                    break;
                };
                let Some(raw) = bytes.get(at + 12..at + 12 + len) else {
                    break;
                };
                let name: Vec<u16> = raw
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|c| u16::from_le_bytes(*c))
                    .collect();
                for (n, w) in &self.names {
                    if eq_ignore_case(n, &name) && !out.contains(w) {
                        out.push(*w);
                    }
                }
                if next == 0 {
                    break;
                }
                at += next;
            }
            Ok(out)
        }
    }

    fn eq_ignore_case(a: &[u16], b: &[u16]) -> bool {
        let s = |x: &[u16]| String::from_utf16_lossy(x).to_lowercase();
        s(a) == s(b)
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            // The kernel still owns the buffer until the request is over.
            unsafe {
                CancelIoEx(self.h, &*self.ov);
                let mut n = 0u32;
                GetOverlappedResult(self.h, &*self.ov, &mut n, 1);
                CloseHandle(self.h);
            }
        }
    }

    /// The OS event source for one target: one pending request per folder plus a stop event.
    struct OsSource {
        dirs: Vec<Dir>,
        stop: Arc<Ev>,
        start: Instant,
    }

    // The handles and buffers are used by the watch thread only, after it is moved there.
    unsafe impl Send for OsSource {}

    impl OsSource {
        fn new(t: &Target, stop: Arc<Ev>) -> std::io::Result<OsSource> {
            let name = |p: &Path| -> std::io::Result<Vec<u16>> {
                Ok(p.file_name()
                    .ok_or_else(|| std::io::Error::other("no file name"))?
                    .encode_wide()
                    .collect())
            };
            let folder = |p: &Path| -> std::io::Result<PathBuf> {
                p.parent()
                    .map(Path::to_path_buf)
                    .ok_or_else(|| std::io::Error::other("no folder"))
            };
            let (ef, mf) = (folder(&t.exe)?, folder(&t.manifest)?);
            // Before any open: opening a UNC path would send the user's credentials to that host.
            for d in [&ef, &mf] {
                let shown = d.display().to_string();
                let probe = format!("{}\\x", shown.trim_end_matches('\\'));
                if !hideout_registry::permission::on_local_fixed_drive(&probe) {
                    return Err(std::io::Error::other(format!(
                        "not a local folder: {shown}"
                    )));
                }
            }
            let dirs = if ef == mf {
                vec![Dir::open(
                    &ef,
                    vec![
                        (name(&t.exe)?, Which::Exe),
                        (name(&t.manifest)?, Which::Manifest),
                    ],
                )?]
            } else {
                vec![
                    Dir::open(&ef, vec![(name(&t.exe)?, Which::Exe)])?,
                    Dir::open(&mf, vec![(name(&t.manifest)?, Which::Manifest)])?,
                ]
            };
            for d in [&ef, &mf].into_iter().take(dirs.len()) {
                log::info!("rebuild {}: watch added {}", t.id, d.display());
            }
            Ok(OsSource {
                dirs,
                stop,
                start: Instant::now(),
            })
        }
    }

    impl EventSource for OsSource {
        fn now(&self) -> Duration {
            self.start.elapsed()
        }

        fn wait(&mut self, timeout: Option<Duration>) -> std::io::Result<Wake> {
            let end = timeout.map(|t| Instant::now() + t);
            loop {
                // Stop first: the lowest signalled index wins, so a flood of events cannot starve it.
                let mut hs: Vec<HANDLE> = vec![self.stop.0];
                hs.extend(self.dirs.iter().map(|d| d.ev.0));
                let ms = end.map_or(INFINITE, |e| {
                    e.saturating_duration_since(Instant::now())
                        .as_millis()
                        .min(u32::MAX as u128 - 1) as u32
                });
                let r = unsafe { WaitForMultipleObjects(hs.len() as u32, hs.as_ptr(), 0, ms) };
                if r == WAIT_TIMEOUT {
                    return Ok(Wake::Timeout);
                }
                let i = r.wrapping_sub(WAIT_OBJECT_0) as usize;
                if i >= hs.len() {
                    return Err(std::io::Error::last_os_error());
                }
                if i == 0 {
                    return Ok(Wake::Stop);
                }
                let files = self.dirs[i - 1].complete()?;
                if !files.is_empty() {
                    return Ok(Wake::Changed(files));
                }
            }
        }
    }

    /// A running watch; dropping it stops the thread and closes the folder handles.
    pub struct Handle {
        stop: Arc<Ev>,
        thread: Option<std::thread::JoinHandle<()>>,
    }

    impl Drop for Handle {
        fn drop(&mut self) {
            unsafe { SetEvent(self.stop.0) };
            if let Some(t) = self.thread.take() {
                let _ = t.join();
            }
        }
    }

    /// Starts the watch with the plan's timing.
    pub fn spawn(t: Target, on_outcome: Box<dyn FnMut(Outcome) + Send>) -> std::io::Result<Handle> {
        spawn_with(t, Timing::DEFAULT, on_outcome)
    }

    pub fn spawn_with(
        t: Target,
        timing: Timing,
        mut on_outcome: Box<dyn FnMut(Outcome) + Send>,
    ) -> std::io::Result<Handle> {
        let stop = Arc::new(Ev::new()?);
        let mut src = match OsSource::new(&t, stop.clone()) {
            Ok(s) => s,
            Err(e) => {
                log::warn!("rebuild {}: watch failed: {e}", t.id);
                return Err(e);
            }
        };
        let thread = std::thread::Builder::new()
            .name("hideout-rebuild-watch".into())
            .spawn(move || {
                run(&t, timing, &mut src, &mut FileProbe, &mut *on_outcome);
                log::info!("rebuild {}: watch removed", t.id);
            })?;
        Ok(Handle {
            stop,
            thread: Some(thread),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    fn target() -> Target {
        Target {
            id: "app".into(),
            exe: PathBuf::from("C:\\dev\\app.exe"),
            manifest: PathBuf::from("C:\\dev\\module.toml"),
        }
    }

    fn ready(sha: &str) -> Probed {
        Probed::Ready {
            size: 10,
            sha256: format!("{sha}{sha}{sha}{sha}{sha}{sha}{sha}{sha}"),
        }
    }

    /// Scripted answers per file, in order; the last one repeats.
    struct FakeProbe {
        exe: VecDeque<Probed>,
        manifest: VecDeque<Probed>,
        calls: usize,
    }
    impl FakeProbe {
        fn new(exe: Vec<Probed>) -> FakeProbe {
            FakeProbe {
                exe: exe.into(),
                manifest: vec![ready("m")].into(),
                calls: 0,
            }
        }
    }
    impl Probe for FakeProbe {
        fn check(&mut self, _: &Path, which: Which) -> Probed {
            self.calls += 1;
            let q = if which == Which::Exe {
                &mut self.exe
            } else {
                &mut self.manifest
            };
            if q.len() > 1 {
                q.pop_front().unwrap()
            } else {
                q.front().unwrap().clone()
            }
        }
    }

    /// Events at fixed times; between them the fake clock moves only as far as the timeout asks.
    struct FakeSource {
        now: Duration,
        events: VecDeque<(Duration, Which)>,
        timeouts: Vec<Option<Duration>>,
    }
    impl FakeSource {
        fn new(events: &[(u64, Which)]) -> FakeSource {
            FakeSource {
                now: Duration::ZERO,
                events: events.iter().map(|(t, w)| (ms(*t), *w)).collect(),
                timeouts: vec![],
            }
        }
    }
    impl EventSource for FakeSource {
        fn now(&self) -> Duration {
            self.now
        }
        fn wait(&mut self, timeout: Option<Duration>) -> std::io::Result<Wake> {
            self.timeouts.push(timeout);
            let due = timeout.map(|t| self.now + t);
            match (self.events.front().copied(), due) {
                (Some((at, w)), d) if d.is_none_or(|d| at <= d) => {
                    self.events.pop_front();
                    self.now = self.now.max(at);
                    Ok(Wake::Changed(vec![w]))
                }
                (_, Some(d)) => {
                    self.now = d;
                    Ok(Wake::Timeout)
                }
                (None, None) => Ok(Wake::Stop),
                (Some(_), None) => unreachable!(),
            }
        }
    }

    fn drive(events: &[(u64, Which)], exe: Vec<Probed>) -> (Vec<Outcome>, FakeSource, FakeProbe) {
        let mut src = FakeSource::new(events);
        let mut probe = FakeProbe::new(exe);
        let mut out = vec![];
        run(&target(), Timing::DEFAULT, &mut src, &mut probe, &mut |o| {
            out.push(o)
        });
        (out, src, probe)
    }

    fn settled(sha: &str) -> Outcome {
        let Probed::Ready { sha256, size } = ready(sha) else {
            unreachable!()
        };
        Outcome::Settled {
            exe_sha256: sha256,
            manifest_sha256: match ready("m") {
                Probed::Ready { sha256, .. } => sha256,
                _ => unreachable!(),
            },
            size,
        }
    }

    #[test]
    fn a_burst_of_events_is_one_try_after_the_quiet_time() {
        let ev = [
            (0, Which::Exe),
            (400, Which::Exe),
            (900, Which::Exe),
            (1000, Which::Manifest),
        ];
        let (out, src, probe) = drive(&ev, vec![ready("a")]);
        assert_eq!(out, vec![settled("a")]);
        assert_eq!(probe.calls, 2, "exe and manifest once");
        // Quiet ran from the last event (1000), so the try was at 2500; idle waits had no timeout.
        assert_eq!(src.now, ms(2500));
        assert_eq!(src.timeouts.last(), Some(&None), "idle: no timer");
        assert_eq!(
            src.timeouts.first(),
            Some(&None),
            "idle at the start: no timer"
        );
    }

    #[test]
    fn a_busy_file_is_retried_at_5_s_then_taken() {
        let (out, src, _) = drive(&[(0, Which::Exe)], vec![Probed::Busy, ready("a")]);
        assert_eq!(out, vec![settled("a")]);
        assert_eq!(src.now, ms(1500 + 5000));
    }

    #[test]
    fn a_bad_header_gives_up_after_three_tries() {
        let (out, src, probe) = drive(&[(0, Which::Exe)], vec![Probed::BadHeader]);
        assert_eq!(out, vec![Outcome::GaveUp(GaveUp::Unfinished)]);
        assert_eq!(src.now, ms(1500 + 5000 + 30_000));
        assert_eq!(probe.calls, 6, "both files each try");
    }

    #[test]
    fn unreadable_is_its_own_outcome() {
        let (out, _, _) = drive(
            &[(0, Which::Exe)],
            vec![Probed::Unreadable("denied".into())],
        );
        assert_eq!(out, vec![Outcome::GaveUp(GaveUp::Unreadable)]);
    }

    #[test]
    fn a_new_event_while_waiting_to_retry_starts_over() {
        // Busy at 1500; the next retry would be 6500. A rename-aside style event at 3000 restarts
        // the quiet time (4500) and the tries.
        let (out, src, _) = drive(
            &[(0, Which::Exe), (3000, Which::Exe)],
            vec![Probed::Busy, ready("b")],
        );
        assert_eq!(out, vec![settled("b")]);
        assert_eq!(src.now, ms(4500));
    }

    #[test]
    fn size_changing_counts_as_unfinished() {
        let (out, _, _) = drive(&[(0, Which::Exe)], vec![Probed::SizeChanged]);
        assert_eq!(out, vec![Outcome::GaveUp(GaveUp::Unfinished)]);
    }

    #[test]
    fn the_same_files_again_report_nothing_and_a_manifest_change_reports() {
        let mut src = FakeSource::new(&[
            (0, Which::Exe),
            (10_000, Which::Exe),
            (20_000, Which::Manifest),
        ]);
        let mut probe = FakeProbe::new(vec![ready("a")]);
        let mut out = vec![];
        let mut m = Machine::new("app", Timing::DEFAULT);
        let t = target();
        let step = |m: &mut Machine,
                    src: &mut FakeSource,
                    probe: &mut FakeProbe,
                    out: &mut Vec<Outcome>| {
            if let Ok(Wake::Changed(f)) =
                src.wait(m.deadline().map(|d| d.saturating_sub(src.now())))
            {
                m.on_change(src.now, f[0]);
            }
            src.now = m.deadline().unwrap();
            out.extend(m.on_deadline(src.now, probe, &t));
        };
        step(&mut m, &mut src, &mut probe, &mut out);
        step(&mut m, &mut src, &mut probe, &mut out);
        assert_eq!(out, vec![settled("a")], "second identical settle is silent");
        probe.manifest = vec![ready("n")].into();
        step(&mut m, &mut src, &mut probe, &mut out);
        assert_eq!(out.len(), 2, "a changed manifest is reported");
    }

    #[test]
    fn a_deadline_before_its_time_does_nothing() {
        let mut m = Machine::new("app", Timing::DEFAULT);
        m.on_change(ms(0), Which::Exe);
        let mut probe = FakeProbe::new(vec![ready("a")]);
        assert_eq!(m.on_deadline(ms(100), &mut probe, &target()), None);
        assert_eq!(probe.calls, 0);
        assert_eq!(m.deadline(), Some(ms(1500)));
    }

    #[test]
    fn pe_header_check() {
        let mut b = vec![0u8; 0x100];
        b[..2].copy_from_slice(b"MZ");
        b[0x3c] = 0x80;
        b[0x80..0x84].copy_from_slice(b"PE\0\0");
        assert!(looks_like_pe(&b));
        b[0x80] = b'X';
        assert!(!looks_like_pe(&b));
        assert!(!looks_like_pe(b"MZ"));
        assert!(!looks_like_pe(&[]));
    }

    #[cfg(windows)]
    mod os {
        use super::*;
        use std::sync::mpsc;

        fn pe() -> Vec<u8> {
            let mut b = vec![0u8; 0x100];
            b[..2].copy_from_slice(b"MZ");
            b[0x3c] = 0x80;
            b[0x80..0x84].copy_from_slice(b"PE\0\0");
            b
        }

        /// Window-free: a real change notification on a temp folder. A rebuild written in two
        /// steps (half a file, then the whole) is reported once, as the finished file; a
        /// half-written file with a held write handle is "busy" until it is closed.
        #[test]
        fn a_real_rebuild_is_reported_once_when_it_is_finished() {
            let dir = std::env::temp_dir().join(format!("hideout-rebuild-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            let t = Target {
                id: "app".into(),
                exe: dir.join("app.exe"),
                manifest: dir.join("module.toml"),
            };
            std::fs::write(&t.exe, pe()).unwrap();
            std::fs::write(&t.manifest, "x").unwrap();
            let (tx, rx) = mpsc::channel();
            let timing = Timing {
                quiet: ms(200),
                retries: [ms(300), ms(300)],
            };
            let h = spawn_with(
                t.clone(),
                timing,
                Box::new(move |o| {
                    let _ = tx.send(o);
                }),
            )
            .unwrap();
            // Rename the old one aside and write the new one, as the copy-in does.
            std::fs::rename(&t.exe, dir.join("app.exe.old")).unwrap();
            let mut new = pe();
            new.extend_from_slice(b"new build");
            {
                use std::io::Write;
                let mut f = std::fs::File::create(&t.exe).unwrap();
                f.write_all(&new[..0x90]).unwrap();
                std::thread::sleep(ms(50));
                f.write_all(&new[0x90..]).unwrap();
            }
            let o = rx
                .recv_timeout(Duration::from_secs(10))
                .expect("no outcome");
            let Outcome::Settled {
                exe_sha256, size, ..
            } = o
            else {
                panic!("{o:?}")
            };
            assert_eq!(size, new.len() as u64);
            assert_eq!(exe_sha256, foxglove_plugin_trust::sha256_hex(&new));
            assert!(rx.recv_timeout(ms(800)).is_err(), "reported twice");
            // Unrelated files in the folder do not wake it.
            std::fs::write(dir.join("other.txt"), "x").unwrap();
            assert!(rx.recv_timeout(ms(600)).is_err());
            drop(h);
            let _ = std::fs::remove_dir_all(&dir);
        }

        #[test]
        fn network_and_non_local_paths_are_refused_before_any_open() {
            for p in [
                r"\\localhost\c$\dev",
                r"\\?\UNC\host\share\dev",
                r"relative\dir",
                r"\\.\C:\dev",
            ] {
                let d = PathBuf::from(p);
                let t = Target {
                    id: "x".into(),
                    exe: d.join("a.exe"),
                    manifest: d.join("module.toml"),
                };
                let e = spawn_with(t, Timing::DEFAULT, Box::new(|_| {}))
                    .err()
                    .expect(p);
                assert!(e.to_string().contains("not a local folder"), "{p}: {e}");
            }
        }

        #[test]
        fn stop_is_not_delayed_by_an_event_flood() {
            let dir =
                std::env::temp_dir().join(format!("hideout-rebuild-flood-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            let t = Target {
                id: "app".into(),
                exe: dir.join("app.exe"),
                manifest: dir.join("module.toml"),
            };
            let h = spawn_with(t.clone(), Timing::DEFAULT, Box::new(|_| {})).unwrap();
            let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
            let flood = {
                let (stop, exe) = (stop.clone(), t.exe.clone());
                std::thread::spawn(move || {
                    while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                        let _ = std::fs::write(&exe, b"x");
                    }
                })
            };
            std::thread::sleep(ms(200));
            let t0 = std::time::Instant::now();
            drop(h);
            let took = t0.elapsed();
            stop.store(true, std::sync::atomic::Ordering::Relaxed);
            flood.join().unwrap();
            assert!(took < Duration::from_secs(3), "stop took {took:?}");
            let _ = std::fs::remove_dir_all(&dir);
        }

        #[test]
        fn a_huge_file_is_unreadable_and_a_big_one_hashes_in_chunks() {
            let dir =
                std::env::temp_dir().join(format!("hideout-rebuild-big-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            let big = dir.join("module.toml");
            std::fs::File::create(&big)
                .unwrap()
                .set_len(2 << 20)
                .unwrap();
            assert_eq!(
                FileProbe.check(&big, Which::Manifest),
                Probed::Unreadable("too large".into())
            );
            let exe = dir.join("a.exe");
            let mut b = pe();
            b.extend((0..300_000u32).map(|i| (i % 251) as u8));
            std::fs::write(&exe, &b).unwrap();
            assert_eq!(
                FileProbe.check(&exe, Which::Exe),
                Probed::Ready {
                    size: b.len() as u64,
                    sha256: foxglove_plugin_trust::sha256_hex(&b)
                }
            );
            assert_eq!(
                super::super::win::check_capped(&exe, Which::Exe, 1000),
                Probed::Unreadable("too large".into())
            );
            let _ = std::fs::remove_dir_all(&dir);
        }

        #[test]
        fn a_writer_holding_the_file_is_busy() {
            let dir =
                std::env::temp_dir().join(format!("hideout-rebuild-busy-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            let p = dir.join("a.exe");
            let _w = std::fs::File::create(&p).unwrap();
            assert_eq!(FileProbe.check(&p, Which::Exe), Probed::Busy);
            drop(_w);
            std::fs::write(&p, pe()).unwrap();
            assert!(matches!(
                FileProbe.check(&p, Which::Exe),
                Probed::Ready { .. }
            ));
            std::fs::write(&p, b"not a program").unwrap();
            assert_eq!(FileProbe.check(&p, Which::Exe), Probed::BadHeader);
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
}
