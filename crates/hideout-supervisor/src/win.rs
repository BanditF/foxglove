// SPDX-License-Identifier: MIT
//! Windows pieces (§7.7): a named Job Object per instance with memory and CPU caps and no
//! breakaway, starting a process inside it before it runs a single instruction, opening a
//! process again by pid after a hub restart, its image path and start time, and the
//! one-supervisor-per-folder lock.

use std::ffi::OsString;
use std::io;
use std::os::windows::ffi::OsStringExt;
use std::os::windows::io::AsRawHandle;
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, Command};

use hideout_contract::module::ResourceLimits;
use windows_sys::Win32::Foundation::{
    CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, FILETIME, HANDLE, INVALID_HANDLE_VALUE,
    WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD, THREADENTRY32,
};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, IsProcessInJob, JobObjectCpuRateControlInformation,
    JobObjectExtendedLimitInformation, OpenJobObjectW, QueryInformationJobObject,
    SetInformationJobObject, TerminateJobObject, JOBOBJECT_CPU_RATE_CONTROL_INFORMATION,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_CPU_RATE_CONTROL_ENABLE,
    JOB_OBJECT_CPU_RATE_CONTROL_HARD_CAP, JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION,
    JOB_OBJECT_LIMIT_JOB_MEMORY,
};
use windows_sys::Win32::System::Threading::{
    CreateMutexW, GetExitCodeProcess, GetProcessTimes, OpenProcess, OpenThread,
    QueryFullProcessImageNameW, ResumeThread, TerminateProcess, WaitForSingleObject,
    CREATE_NO_WINDOW, CREATE_SUSPENDED, INFINITE, PROCESS_NAME_WIN32,
    PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SET_QUOTA, PROCESS_SYNCHRONIZE, PROCESS_TERMINATE,
    THREAD_SUSPEND_RESUME,
};

/// An owned kernel handle.
pub struct Handle(pub(crate) HANDLE);

// SAFETY: kernel handles may be used from any thread.
unsafe impl Send for Handle {}
unsafe impl Sync for Handle {}

impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: the handle is owned and closed once.
        unsafe { CloseHandle(self.0) };
    }
}

/// JOB_OBJECT_ASSIGN_PROCESS | SET_ATTRIBUTES | QUERY | TERMINATE (winnt.h).
const JOB_ACCESS: u32 = 0x1 | 0x2 | 0x4 | 0x8;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn check(h: HANDLE) -> io::Result<Handle> {
    if h.is_null() || h == INVALID_HANDLE_VALUE {
        Err(io::Error::last_os_error())
    } else {
        Ok(Handle(h))
    }
}

/// The caps each instance's Job Object gets (§7.7).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Caps {
    /// All the job's processes together, in bytes.
    pub memory: usize,
    /// Hard CPU cap in 1/100 of a percent of the whole machine (5000 = half).
    pub cpu_rate: u32,
}

impl Caps {
    /// The hub's maximum (§7.7, v1.10): what a module with no `[limits]` gets, and the most any
    /// module gets. 4 GiB for all its processes, half the machine's CPU.
    pub const HUB_MAX: Caps = Caps {
        memory: 4 << 30,
        cpu_rate: 5000,
    };

    /// A module's caps from its manifest's `[limits]` (§7.7, v1.10): each value clamped to
    /// `max`; an absent value (or no `[limits]`) is `max`'s.
    pub fn for_module(limits: Option<&ResourceLimits>, max: Caps) -> Caps {
        let l = limits.copied().unwrap_or_default();
        Caps {
            memory: l
                .memory_mb
                .map_or(max.memory, |m| ((m as usize) << 20).min(max.memory)),
            cpu_rate: l
                .cpu_percent
                .map_or(max.cpu_rate, |c| (c * 100).min(max.cpu_rate)),
        }
    }
}

impl Default for Caps {
    fn default() -> Self {
        Caps::HUB_MAX
    }
}

/// A named Job Object. Closing the handle does not end the processes (no kill-on-close): a
/// module keeps running for its grace period when the hub goes (§7.9). Only the hub holds a
/// handle, so the name goes with the hub: the process stays inside the (now nameless) job with
/// its caps, and a restarted hub nests it into a fresh job ([`Job::assign`]) instead of opening
/// the old one (#986).
pub struct Job {
    handle: Handle,
    pub name: String,
}

impl Job {
    /// `Local\hideout-job-<user>-<module>-<instance>`.
    pub fn name_for(user: &str, module: &str, instance: u64) -> String {
        format!(r"Local\hideout-job-{user}-{module}-{instance}")
    }

    pub fn create(name: &str, caps: Caps) -> io::Result<Job> {
        let w = wide(name);
        // SAFETY: a valid wide string; default security (the creator's).
        let handle = check(unsafe { CreateJobObjectW(std::ptr::null(), w.as_ptr()) })?;
        let mut ext = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        // No JOB_OBJECT_LIMIT_BREAKAWAY_OK / SILENT_BREAKAWAY_OK: children stay in the job.
        ext.BasicLimitInformation.LimitFlags =
            JOB_OBJECT_LIMIT_JOB_MEMORY | JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION;
        ext.JobMemoryLimit = caps.memory;
        // SAFETY: the struct and its size match the info class.
        let ok = unsafe {
            SetInformationJobObject(
                handle.0,
                JobObjectExtendedLimitInformation,
                &ext as *const _ as *const _,
                std::mem::size_of_val(&ext) as u32,
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut cpu = JOBOBJECT_CPU_RATE_CONTROL_INFORMATION {
            ControlFlags: JOB_OBJECT_CPU_RATE_CONTROL_ENABLE | JOB_OBJECT_CPU_RATE_CONTROL_HARD_CAP,
            ..Default::default()
        };
        cpu.Anonymous.CpuRate = caps.cpu_rate.clamp(1, 10000);
        // SAFETY: as above.
        let ok = unsafe {
            SetInformationJobObject(
                handle.0,
                JobObjectCpuRateControlInformation,
                &cpu as *const _ as *const _,
                std::mem::size_of_val(&cpu) as u32,
            )
        };
        if ok == 0 {
            // A CPU cap can be refused in a nested job (a lab runner's own job): the memory cap
            // and containment still hold.
            log::warn!(
                "supervisor: CPU cap on {name} refused: {}",
                io::Error::last_os_error()
            );
        }
        Ok(Job {
            handle,
            name: name.into(),
        })
    }

    /// The caps the job holds now, read back from Windows: the job memory limit (0 when none)
    /// and the hard CPU rate (0 when none).
    pub fn caps(&self) -> io::Result<Caps> {
        let mut ext = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        // SAFETY: the struct and its size match the info class; an owned job handle.
        let ok = unsafe {
            QueryInformationJobObject(
                self.handle.0,
                JobObjectExtendedLimitInformation,
                &mut ext as *mut _ as *mut _,
                std::mem::size_of_val(&ext) as u32,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut cpu = JOBOBJECT_CPU_RATE_CONTROL_INFORMATION::default();
        // SAFETY: as above.
        let ok = unsafe {
            QueryInformationJobObject(
                self.handle.0,
                JobObjectCpuRateControlInformation,
                &mut cpu as *mut _ as *mut _,
                std::mem::size_of_val(&cpu) as u32,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        let hard = JOB_OBJECT_CPU_RATE_CONTROL_ENABLE | JOB_OBJECT_CPU_RATE_CONTROL_HARD_CAP;
        let memory = if ext.BasicLimitInformation.LimitFlags & JOB_OBJECT_LIMIT_JOB_MEMORY != 0 {
            ext.JobMemoryLimit
        } else {
            0
        };
        // SAFETY: CpuRate is the union member for the hard-cap flags.
        let cpu_rate = if cpu.ControlFlags & hard == hard {
            unsafe { cpu.Anonymous.CpuRate }
        } else {
            0
        };
        Ok(Caps { memory, cpu_rate })
    }

    /// Whether process `p` runs inside this job (or a job nested in it).
    pub fn holds(&self, p: &Process) -> io::Result<bool> {
        let mut inside = 0;
        // SAFETY: owned process and job handles.
        let ok = unsafe { IsProcessInJob(p.0 .0, self.handle.0, &mut inside) };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(inside != 0)
    }

    /// Opens a job the hub made before it restarted (§7.9).
    pub fn open(name: &str) -> io::Result<Job> {
        let w = wide(name);
        // SAFETY: a valid wide string.
        let handle = check(unsafe { OpenJobObjectW(JOB_ACCESS, 0, w.as_ptr()) })?;
        Ok(Job {
            handle,
            name: name.into(),
        })
    }

    /// Puts a running process in this job (§7.9 take-back). A process already in a job is nested:
    /// this job must be new and empty, and it becomes a child of the old one, whose caps still
    /// hold. Processes the module started before are not moved.
    pub fn assign(&self, p: &Process) -> io::Result<()> {
        // SAFETY: owned job and process handles; the process has PROCESS_SET_QUOTA.
        if unsafe { AssignProcessToJobObject(self.handle.0, p.0 .0) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    /// The job's handle, for the AppContainer launch (`crate::sandbox`).
    pub(crate) fn raw(&self) -> HANDLE {
        self.handle.0
    }

    /// Ends every process in the job.
    pub fn kill(&self, code: u32) {
        // SAFETY: an owned job handle.
        unsafe { TerminateJobObject(self.handle.0, code) };
    }

    /// Starts `command` suspended, puts it in this job, then lets it run: the process and every
    /// child it makes are inside before its first instruction.
    pub fn spawn(&self, command: Command) -> io::Result<Child> {
        self.spawn_with(command, |mut c| c.spawn())
    }

    /// [`Job::spawn`] with the spawn step given (the legacy stdio bridge spawns for itself).
    pub fn spawn_with(
        &self,
        mut command: Command,
        spawn: impl FnOnce(Command) -> io::Result<Child>,
    ) -> io::Result<Child> {
        command.creation_flags(CREATE_SUSPENDED | CREATE_NO_WINDOW);
        let mut child = spawn(command)?;
        // SAFETY: the child's process handle is valid while `child` lives.
        let ok = unsafe { AssignProcessToJobObject(self.handle.0, child.as_raw_handle() as _) };
        if ok == 0 {
            let e = io::Error::last_os_error();
            let _ = child.kill();
            let _ = child.wait();
            return Err(e);
        }
        resume(child.id())?;
        Ok(child)
    }
}

/// Resumes every thread of a process started suspended (only its first thread exists).
fn resume(pid: u32) -> io::Result<()> {
    // SAFETY: a snapshot handle checked and owned below.
    let snap = check(unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) })?;
    let mut te = THREADENTRY32 {
        dwSize: std::mem::size_of::<THREADENTRY32>() as u32,
        ..Default::default()
    };
    let mut resumed = 0;
    // SAFETY: `te` is sized; the snapshot is valid.
    let mut more = unsafe { Thread32First(snap.0, &mut te) } != 0;
    while more {
        if te.th32OwnerProcessID == pid {
            // SAFETY: thread handle checked and closed by `Handle`.
            if let Ok(t) = check(unsafe { OpenThread(THREAD_SUSPEND_RESUME, 0, te.th32ThreadID) }) {
                unsafe { ResumeThread(t.0) };
                resumed += 1;
            }
        }
        // SAFETY: as above.
        more = unsafe { Thread32Next(snap.0, &mut te) } != 0;
    }
    if resumed == 0 {
        return Err(io::Error::other(format!("no thread of {pid} to resume")));
    }
    Ok(())
}

/// A process the hub didn't start in this run (taken back, §7.9), or any pid to inspect.
pub struct Process(Handle);

impl Process {
    /// Takes ownership of a process handle (one `CreateProcessW` returned).
    pub(crate) fn from_raw(h: HANDLE) -> Process {
        Process(Handle(h))
    }

    pub(crate) fn raw(&self) -> HANDLE {
        self.0 .0
    }

    /// Its process id.
    pub fn pid(&self) -> u32 {
        // SAFETY: an owned process handle.
        unsafe { windows_sys::Win32::System::Threading::GetProcessId(self.0 .0) }
    }

    pub fn open(pid: u32) -> io::Result<Process> {
        // SAFETY: plain call; the handle is checked.
        let h = unsafe {
            OpenProcess(
                PROCESS_SYNCHRONIZE
                    | PROCESS_QUERY_LIMITED_INFORMATION
                    | PROCESS_TERMINATE
                    | PROCESS_SET_QUOTA,
                0,
                pid,
            )
        };
        Ok(Process(check(h)?))
    }

    /// When it started: a FILETIME (100 ns since 1601). With the pid, this names one process;
    /// a reused pid has another start time.
    pub fn started(&self) -> io::Result<u64> {
        started_of(self.0 .0)
    }

    /// False once it has ended (a handle can outlive the process).
    pub fn running(&self) -> bool {
        // SAFETY: a process handle with SYNCHRONIZE.
        unsafe { WaitForSingleObject(self.0 .0, 0) == WAIT_TIMEOUT }
    }

    /// Ends this one process (never by name).
    pub fn kill(&self, code: u32) {
        // SAFETY: a process handle with PROCESS_TERMINATE.
        unsafe { TerminateProcess(self.0 .0, code) };
    }

    /// The full path of its exe.
    pub fn image(&self) -> io::Result<PathBuf> {
        image_of(self.0 .0)
    }

    /// Blocks until it ends; its exit code.
    pub fn wait(&self) -> io::Result<i64> {
        // SAFETY: a process handle with SYNCHRONIZE.
        if unsafe { WaitForSingleObject(self.0 .0, INFINITE) } != WAIT_OBJECT_0 {
            return Err(io::Error::last_os_error());
        }
        let mut code = 0u32;
        // SAFETY: as above, with query rights.
        if unsafe { GetExitCodeProcess(self.0 .0, &mut code) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(code as i32 as i64)
    }
}

fn image_of(h: HANDLE) -> io::Result<PathBuf> {
    let mut buf = vec![0u16; 32768];
    let mut len = buf.len() as u32;
    // SAFETY: `buf` holds `len` u16s.
    let ok =
        unsafe { QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut len) };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(PathBuf::from(OsString::from_wide(&buf[..len as usize])))
}

fn started_of(h: HANDLE) -> io::Result<u64> {
    let mut t = [FILETIME::default(); 4];
    let [c, e, k, u] = &mut t;
    // SAFETY: four valid FILETIME out-pointers.
    if unsafe { GetProcessTimes(h, c, e, k, u) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok((u64::from(t[0].dwHighDateTime) << 32) | u64::from(t[0].dwLowDateTime))
}

/// The start time of a process the hub just started (see [`Process::started`]).
pub fn child_started(child: &Child) -> io::Result<u64> {
    started_of(child.as_raw_handle() as HANDLE)
}

/// Takes the named mutex `name` for the life of the process. `None` when another process holds
/// it: one supervisor per hub folder, so a second hub never starts or takes back modules.
pub fn claim(name: &str) -> io::Result<Option<Handle>> {
    let w = wide(name);
    // SAFETY: a valid wide string; the handle is checked.
    let h = check(unsafe { CreateMutexW(std::ptr::null(), 0, w.as_ptr()) })?;
    // SAFETY: read right after the call that set it.
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        return Ok(None);
    }
    Ok(Some(h))
}

/// The image path of process `pid`.
pub fn image_of_pid(pid: u32) -> io::Result<PathBuf> {
    Process::open(pid)?.image()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(tag: &str) -> Job {
        let name = Job::name_for("test", tag, std::process::id() as u64);
        Job::create(&name, Caps::default()).unwrap()
    }

    #[test]
    fn spawned_process_runs_in_the_job_and_reports_its_code() {
        let j = job("code");
        let mut cmd = Command::new("cmd");
        cmd.args(["/c", "exit 3"]);
        let mut child = j.spawn(cmd).unwrap();
        assert_eq!(child.wait().unwrap().code(), Some(3));
    }

    #[test]
    fn kill_ends_the_job_and_reopen_finds_it() {
        let j = job("kill");
        let mut cmd = Command::new("ping");
        cmd.args(["-n", "30", "127.0.0.1"])
            .stdout(std::process::Stdio::null());
        let mut child = j.spawn(cmd).unwrap();
        let p = Process::open(child.id()).unwrap();
        assert!(p
            .image()
            .unwrap()
            .to_string_lossy()
            .to_lowercase()
            .ends_with("ping.exe"));
        let again = Job::open(&j.name).unwrap();
        again.kill(7);
        assert_eq!(p.wait().unwrap(), 7);
        assert_eq!(child.wait().unwrap().code(), Some(7));
    }

    #[test]
    fn a_process_outlives_its_job_name_and_nests_into_a_fresh_job() {
        let old = job("nest-old");
        let mut cmd = Command::new("ping");
        cmd.args(["-n", "30", "127.0.0.1"])
            .stdout(std::process::Stdio::null());
        let mut child = old.spawn(cmd).unwrap();
        let started = child_started(&child).unwrap();
        let name = old.name.clone();
        // The hub goes: its handle was the only one, so the name goes too (#986).
        drop(old);
        assert!(
            Job::open(&name).is_err(),
            "the job name outlived its last handle"
        );
        let p = Process::open(child.id()).unwrap();
        assert!(p.running());
        assert_eq!(p.started().unwrap(), started);
        let fresh = Job::create(&format!("{name}-again"), Caps::default()).unwrap();
        fresh.assign(&p).unwrap();
        fresh.kill(9);
        assert_eq!(p.wait().unwrap(), 9);
        assert!(!p.running());
        assert_eq!(child.wait().unwrap().code(), Some(9));
    }

    #[test]
    fn one_holder_per_lock_name() {
        let name = format!(r"Local\hideout-test-lock-{}", std::process::id());
        let first = claim(&name).unwrap();
        assert!(first.is_some());
        assert!(claim(&name).unwrap().is_none());
        drop(first);
        assert!(claim(&name).unwrap().is_some());
    }
}

/// A string or number under `HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion`.
fn current_version(name: &str) -> Option<String> {
    use windows_sys::Win32::System::Registry::{
        RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RRF_RT_REG_SZ,
    };
    let key = wide(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion");
    let value = wide(name);
    let mut buf = [0u16; 256];
    let mut size = std::mem::size_of_val(&buf) as u32;
    // SAFETY: the key and value names are NUL-terminated; the buffer and its size match.
    let rc = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            key.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_SZ | RRF_RT_REG_DWORD,
            std::ptr::null_mut(),
            buf.as_mut_ptr().cast(),
            &mut size,
        )
    };
    if rc != 0 {
        return None;
    }
    if name == "UBR" {
        return Some((buf[0] as u32 | (buf[1] as u32) << 16).to_string());
    }
    let n = (size as usize / 2).min(buf.len());
    let s = String::from_utf16_lossy(&buf[..n]);
    Some(s.trim_end_matches('\0').to_string())
}

/// `Windows 11 Pro 25H2 (build 26300.1234)` for Copy diagnostics; `None` when the registry
/// can't be read. Windows 11 still names itself "Windows 10" in `ProductName`: from build
/// 22000 it reads 11.
pub fn windows_version() -> Option<String> {
    let build = current_version("CurrentBuildNumber")?;
    let mut product = current_version("ProductName").unwrap_or_else(|| "Windows".into());
    if build.parse::<u32>().is_ok_and(|b| b >= 22000) {
        product = product.replace("Windows 10", "Windows 11");
    }
    let mut out = product;
    if let Some(v) = current_version("DisplayVersion") {
        out.push(' ');
        out.push_str(&v);
    }
    match current_version("UBR") {
        Some(u) => out.push_str(&format!(" (build {build}.{u})")),
        None => out.push_str(&format!(" (build {build})")),
    }
    Some(out)
}

#[cfg(test)]
mod version_tests {
    #[test]
    fn the_windows_line_names_a_build() {
        let v = super::windows_version().expect("registry");
        assert!(v.starts_with("Windows") && v.contains("(build "), "{v}");
    }
}
