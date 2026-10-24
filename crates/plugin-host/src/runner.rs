//! The plugin runner: starts an installed plugin, serves its stdin/stdout with the API host, keeps it
//! healthy and contained (docs/local-api.md, "Lifecycle").
//!
//! - **Containment (Windows):** each plugin starts suspended, goes into its own Job Object (killed
//!   with Foxglove or on stop, memory-capped, no breakaway), and only then runs. Plugins that
//!   declare neither `net` nor `files` also run in an AppContainer (see `sandbox`).
//! - **Health:** a plugin must show it's alive (any call; `ping` is enough) within the ping
//!   timeout, or it's restarted. Its stderr goes to `plugin.log` in its folder.
//! - **Supervision:** a plugin that exits or hangs is restarted with backoff (1 s doubling to
//!   60 s, reset after a minute of uptime); after `max_crashes` within `crash_window` it's
//!   disabled until the user turns it on again.

use std::collections::VecDeque;
use std::io::{BufReader, Read, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::Context;

/// What to run: from the plugin's manifest and folder.
#[derive(Clone, Debug)]
pub struct PluginSpec {
    pub id: String,
    pub dir: PathBuf,
    /// The manifest's `run`: the program (inside `dir`) and its arguments.
    pub run: Vec<String>,
    /// Run in an AppContainer (no network, no user files; Windows). For plugins that declare
    /// neither `net` nor `files`.
    pub sandbox: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct Config {
    pub ping_timeout: Duration,
    pub backoff_start: Duration,
    pub backoff_max: Duration,
    pub max_crashes: usize,
    pub crash_window: Duration,
    /// Per-process memory cap in the Job Object.
    pub memory_limit: usize,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            ping_timeout: crate::PING_TIMEOUT,
            backoff_start: Duration::from_secs(1),
            backoff_max: Duration::from_secs(60),
            max_crashes: 5,
            crash_window: Duration::from_secs(600),
            memory_limit: 512 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum PluginState {
    Running,
    /// Restarting after `reason`, in `wait`.
    Restarting { reason: String, wait: Duration },
    /// Too many crashes; stays off until started again.
    Disabled(String),
    Stopped,
}

enum Event {
    /// The I/O of run `n` ended (the plugin exited or closed its output).
    Exited(u64),
    Stop,
}

/// A running plugin's supervisor; dropping it stops the plugin.
pub struct Runner {
    tx: Sender<Event>,
}

impl Drop for Runner {
    fn drop(&mut self) {
        let _ = self.tx.send(Event::Stop);
    }
}

impl Runner {
    /// Starts supervising: `context` makes each connection's API context, `status` hears every
    /// state change (on the supervisor thread; keep it quick).
    pub fn start(
        spec: PluginSpec,
        config: Config,
        context: impl Fn() -> Context + Send + 'static,
        status: impl Fn(PluginState) + Send + 'static,
    ) -> Runner {
        let (tx, rx) = mpsc::channel();
        let events = tx.clone();
        let _ = std::thread::Builder::new()
            .name(format!("plugin-{}", spec.id))
            .spawn(move || supervise(spec, config, context, status, events, rx));
        Runner { tx }
    }

    pub fn stop(self) {}
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

/// Marks the plugin alive on every byte it writes.
struct Activity<R> {
    inner: R,
    last: Arc<AtomicU64>,
}

impl<R: Read> Read for Activity<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        if n > 0 {
            self.last.store(now_ms(), Ordering::Relaxed);
        }
        Ok(n)
    }
}

enum Proc {
    Std(Child),
    #[cfg(windows)]
    Contained(job::Process),
}

struct Running {
    proc: Proc,
    #[cfg(windows)]
    _job: job::Job,
}

impl Running {
    fn kill(&mut self) {
        #[cfg(windows)]
        self._job.terminate();
        match &mut self.proc {
            Proc::Std(child) => {
                let _ = child.kill();
                let _ = child.wait();
            }
            #[cfg(windows)]
            Proc::Contained(p) => p.kill(),
        }
    }

    fn exit_code(&mut self) -> Option<i32> {
        match &mut self.proc {
            Proc::Std(child) => child.wait().ok().and_then(|s| s.code()),
            #[cfg(windows)]
            Proc::Contained(p) => p.wait(),
        }
    }
}

type Io = (Running, Box<dyn Write + Send>, Box<dyn Read + Send>);

fn spawn(spec: &PluginSpec, config: &Config) -> std::io::Result<Io> {
    let program = spec.run.first().ok_or_else(|| std::io::Error::other("the manifest's run is empty"))?;
    let log = std::fs::OpenOptions::new().create(true).append(true).open(spec.dir.join("plugin.log"))?;
    #[cfg(windows)]
    if spec.sandbox {
        let c = crate::sandbox::spawn(&spec.id, &spec.dir, &spec.run, log)?;
        let process = job::Process(c.process);
        let job = job::Job::new(config.memory_limit)?;
        if let Err(e) = job.assign_handle(c.process) {
            process.kill();
            return Err(e);
        }
        job::resume_thread(c.thread);
        let running = Running { proc: Proc::Contained(process), _job: job };
        return Ok((running, Box::new(c.stdin), Box::new(c.stdout)));
    }
    let mut cmd = Command::new(spec.dir.join(program));
    cmd.args(&spec.run[1..])
        .current_dir(&spec.dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::from(log));
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        use windows_sys::Win32::System::Threading::{CREATE_NO_WINDOW, CREATE_SUSPENDED};
        cmd.creation_flags(CREATE_SUSPENDED | CREATE_NO_WINDOW);
    }
    let mut child = cmd.spawn()?;
    #[cfg(windows)]
    {
        // Into the job before its first instruction, so nothing it starts can escape.
        let job = match job::Job::new(config.memory_limit).and_then(|j| j.assign(&child).map(|()| j)) {
            Ok(job) => job,
            Err(e) => {
                let _ = child.kill();
                return Err(e);
            }
        };
        job::resume(child.id());
        let (stdin, stdout) = (child.stdin.take().expect("piped"), child.stdout.take().expect("piped"));
        Ok((Running { proc: Proc::Std(child), _job: job }, Box::new(stdin), Box::new(stdout)))
    }
    #[cfg(not(windows))]
    {
        let _ = config;
        let (stdin, stdout) = (child.stdin.take().expect("piped"), child.stdout.take().expect("piped"));
        Ok((Running { proc: Proc::Std(child) }, Box::new(stdin), Box::new(stdout)))
    }
}

fn supervise(
    spec: PluginSpec,
    config: Config,
    context: impl Fn() -> Context,
    status: impl Fn(PluginState),
    events: Sender<Event>,
    rx: mpsc::Receiver<Event>,
) {
    let mut crashes: VecDeque<Instant> = VecDeque::new();
    let mut backoff = config.backoff_start;
    let mut run: u64 = 0;
    loop {
        run += 1;
        let started = Instant::now();
        let reason = match spawn(&spec, &config) {
            Err(e) => format!("couldn't start: {e}"),
            Ok((mut running, stdin, stdout)) => {
                status(PluginState::Running);
                let last = Arc::new(AtomicU64::new(now_ms()));
                let ctx = context();
                let reader = BufReader::new(Activity { inner: stdout, last: last.clone() });
                let exited = events.clone();
                let this_run = run;
                let _ = std::thread::Builder::new().name(format!("plugin-{}-io", spec.id)).spawn(move || {
                    let _ = crate::serve(reader, stdin, ctx);
                    let _ = exited.send(Event::Exited(this_run));
                });
                // Wait for an exit, a stop, or silence longer than the ping timeout.
                let reason = loop {
                    match rx.recv_timeout(config.ping_timeout) {
                        // A late notice from an earlier run that was killed.
                        Ok(Event::Exited(n)) if n != run => {}
                        Ok(Event::Exited(_)) => {
                            let code = running.exit_code();
                            break format!("exited ({})", code.map_or("no code".into(), |c| c.to_string()));
                        }
                        Ok(Event::Stop) | Err(RecvTimeoutError::Disconnected) => {
                            running.kill();
                            status(PluginState::Stopped);
                            return;
                        }
                        Err(RecvTimeoutError::Timeout) => {
                            let silent = Duration::from_millis(now_ms().saturating_sub(last.load(Ordering::Relaxed)));
                            if silent >= config.ping_timeout {
                                running.kill();
                                break format!("no sign of life for {} s", silent.as_secs());
                            }
                        }
                    }
                };
                reason
            }
        };
        if started.elapsed() >= Duration::from_secs(60) {
            backoff = config.backoff_start;
        }
        let now = Instant::now();
        crashes.push_back(now);
        while crashes.front().is_some_and(|t| now.duration_since(*t) > config.crash_window) {
            crashes.pop_front();
        }
        if crashes.len() >= config.max_crashes {
            status(PluginState::Disabled(format!("{reason}; {} failures in a row", crashes.len())));
            // Wait for the stop that comes with dropping the runner.
            while let Ok(Event::Exited(_)) = rx.recv() {}
            return;
        }
        status(PluginState::Restarting { reason, wait: backoff });
        let until = Instant::now() + backoff;
        loop {
            match rx.recv_timeout(until.saturating_duration_since(Instant::now())) {
                Ok(Event::Stop) | Err(RecvTimeoutError::Disconnected) => {
                    status(PluginState::Stopped);
                    return;
                }
                Ok(Event::Exited(_)) => continue,
                Err(RecvTimeoutError::Timeout) => break,
            }
        }
        backoff = (backoff * 2).min(config.backoff_max);
    }
}

#[cfg(windows)]
mod job {
    use std::os::windows::io::AsRawHandle;
    use std::process::Child;

    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD, THREADENTRY32,
    };
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject,
        TerminateJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOB_OBJECT_LIMIT_PROCESS_MEMORY,
    };
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, OpenThread, ResumeThread, TerminateProcess, WaitForSingleObject, THREAD_SUSPEND_RESUME,
    };

    /// A Job Object holding one plugin and everything it starts; closing it kills them all.
    pub struct Job(HANDLE);
    unsafe impl Send for Job {}

    impl Job {
        pub fn new(memory_limit: usize) -> std::io::Result<Job> {
            let h = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
            if h.is_null() {
                return Err(std::io::Error::last_os_error());
            }
            let job = Job(h);
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
                | JOB_OBJECT_LIMIT_PROCESS_MEMORY
                | JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION;
            info.ProcessMemoryLimit = memory_limit;
            let ok = unsafe {
                SetInformationJobObject(
                    job.0,
                    JobObjectExtendedLimitInformation,
                    &info as *const _ as *const _,
                    std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                )
            };
            if ok == 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(job)
        }

        pub fn assign(&self, child: &Child) -> std::io::Result<()> {
            self.assign_handle(child.as_raw_handle() as HANDLE)
        }

        pub fn assign_handle(&self, process: HANDLE) -> std::io::Result<()> {
            if unsafe { AssignProcessToJobObject(self.0, process) } == 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        }

        pub fn terminate(&self) {
            unsafe { TerminateJobObject(self.0, 1) };
        }
    }

    impl Drop for Job {
        fn drop(&mut self) {
            unsafe { CloseHandle(self.0) };
        }
    }

    /// A process started without `std::process` (the AppContainer path).
    pub struct Process(pub HANDLE);
    unsafe impl Send for Process {}

    impl Process {
        pub fn kill(&self) {
            unsafe {
                TerminateProcess(self.0, 1);
                WaitForSingleObject(self.0, 5000);
            }
        }

        pub fn wait(&self) -> Option<i32> {
            let mut code = 0u32;
            unsafe {
                WaitForSingleObject(self.0, 5000);
                (GetExitCodeProcess(self.0, &mut code) != 0).then_some(code as i32)
            }
        }
    }

    impl Drop for Process {
        fn drop(&mut self) {
            unsafe { CloseHandle(self.0) };
        }
    }

    /// Resumes one suspended thread and closes its handle.
    pub fn resume_thread(thread: HANDLE) {
        unsafe {
            ResumeThread(thread);
            CloseHandle(thread);
        }
    }

    /// Resumes the threads of a process started suspended.
    pub fn resume(pid: u32) {
        unsafe {
            let snap = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
            if snap == INVALID_HANDLE_VALUE {
                return;
            }
            let mut entry: THREADENTRY32 = std::mem::zeroed();
            entry.dwSize = std::mem::size_of::<THREADENTRY32>() as u32;
            let mut more = Thread32First(snap, &mut entry) != 0;
            while more {
                if entry.th32OwnerProcessID == pid {
                    let t = OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID);
                    if !t.is_null() {
                        ResumeThread(t);
                        CloseHandle(t);
                    }
                }
                more = Thread32Next(snap, &mut entry) != 0;
            }
            CloseHandle(snap);
        }
    }
}
