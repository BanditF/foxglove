// SPDX-License-Identifier: MIT
//! The Windows watch: one thread that sleeps in `MsgWaitForMultipleObjects` on the watched
//! processes' handles and its message queue. A process handle signals when that process ends;
//! the queue gets the `EVENT_OBJECT_CREATE` hook's calls (out of context, so they run on this
//! thread) when any top-level window is created. Nothing wakes it on a timer.

use std::cell::RefCell;
use std::path::PathBuf;

use windows_sys::Win32::Foundation::{
    CloseHandle, HANDLE, HWND, INVALID_HANDLE_VALUE, WAIT_OBJECT_0,
};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
    PROCESS_SYNCHRONIZE,
};
use windows_sys::Win32::UI::Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetAncestor, GetWindowThreadProcessId, MsgWaitForMultipleObjects,
    PeekMessageW, EVENT_OBJECT_CREATE, GA_ROOT, MSG, OBJID_WINDOW, PM_REMOVE, QS_ALLINPUT,
    WINEVENT_OUTOFCONTEXT, WM_QUIT,
};

use crate::{write_seen, Sink, Watch};

/// `WaitForMultipleObjects` takes at most 64 handles; the queue is the 64th.
const MAX_PROCESSES: usize = 63;

thread_local! {
    /// Processes whose top-level window was just created, from the hook.
    static NEW_WINDOWS: RefCell<Vec<u32>> = const { RefCell::new(Vec::new()) };
}

unsafe extern "system" fn on_event(
    _hook: HWINEVENTHOOK,
    _event: u32,
    hwnd: HWND,
    object: i32,
    child: i32,
    _thread: u32,
    _time: u32,
) {
    if object != OBJID_WINDOW || child != 0 || hwnd.is_null() {
        return;
    }
    // Top-level windows only: a control inside a window is no new app.
    if unsafe { GetAncestor(hwnd, GA_ROOT) } != hwnd {
        return;
    }
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
    if pid != 0 {
        NEW_WINDOWS.with(|n| n.borrow_mut().push(pid));
    }
}

/// The full exe path of process `pid`, when it can be read.
pub(crate) fn exe_of(pid: u32) -> Option<String> {
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            return None;
        }
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut len);
        CloseHandle(h);
        (ok != 0).then(|| String::from_utf16_lossy(&buf[..len as usize]))
    }
}

/// Every running process as (pid, exe file name).
fn processes() -> Vec<(u32, String)> {
    let mut out = Vec::new();
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap == INVALID_HANDLE_VALUE {
            return out;
        }
        let mut e: PROCESSENTRY32W = std::mem::zeroed();
        e.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut more = Process32FirstW(snap, &mut e) != 0;
        while more {
            let len = e.szExeFile.iter().position(|&c| c == 0).unwrap_or(0);
            out.push((
                e.th32ProcessID,
                String::from_utf16_lossy(&e.szExeFile[..len]),
            ));
            more = Process32NextW(snap, &mut e) != 0;
        }
        CloseHandle(snap);
    }
    out
}

struct Watcher {
    watch: Watch,
    file: PathBuf,
    sink: Sink,
    /// The handle of each watched process, by pid, in wait order.
    handles: Vec<(u32, HANDLE)>,
}

impl Watcher {
    /// Process `pid` (exe `exe`) is new to the watch: waited on when it is a watched app.
    fn consider(&mut self, pid: u32, exe: &str) -> bool {
        if self.watch.is_running(pid) {
            return false;
        }
        let Some(label) = self.watch.watched(exe) else {
            return false;
        };
        if self.handles.len() >= MAX_PROCESSES {
            log::warn!("appwatch: more than {MAX_PROCESSES} watched processes; {label} ({pid}) not waited on");
            return false;
        }
        let h = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
        if h.is_null() {
            log::info!("appwatch: {label} ({pid}) can't be waited on");
            return false;
        }
        self.handles.push((pid, h));
        if self.watch.started(pid, &label) {
            write_seen(&self.file, self.watch.seen());
        }
        log::info!("appwatch: {label} runs ({pid})");
        true
    }

    fn publish(&mut self) {
        let changes = self.watch.changes();
        if !changes.is_empty() {
            (self.sink)(changes);
        }
    }

    fn run(mut self) {
        for (pid, exe) in processes() {
            self.consider(pid, &exe);
        }
        self.publish();
        let hook = unsafe {
            SetWinEventHook(
                EVENT_OBJECT_CREATE,
                EVENT_OBJECT_CREATE,
                std::ptr::null_mut(),
                Some(on_event),
                0,
                0,
                WINEVENT_OUTOFCONTEXT,
            )
        };
        if hook.is_null() {
            log::warn!("appwatch: no window hook; app starts after this one aren't seen");
        }
        loop {
            let handles: Vec<HANDLE> = self.handles.iter().map(|(_, h)| *h).collect();
            let n = handles.len() as u32;
            let r =
                unsafe { MsgWaitForMultipleObjects(n, handles.as_ptr(), 0, u32::MAX, QS_ALLINPUT) };
            let mut changed = false;
            if (WAIT_OBJECT_0..WAIT_OBJECT_0 + n).contains(&r) {
                let (pid, h) = self.handles.remove((r - WAIT_OBJECT_0) as usize);
                unsafe { CloseHandle(h) };
                self.watch.ended(pid);
                log::info!("appwatch: process {pid} ended");
                changed = true;
            } else if r == WAIT_OBJECT_0 + n {
                let mut msg: MSG = unsafe { std::mem::zeroed() };
                while unsafe { PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) } != 0
                {
                    if msg.message == WM_QUIT {
                        if !hook.is_null() {
                            unsafe { UnhookWinEvent(hook) };
                        }
                        return;
                    }
                    if msg.message == WM_FORGET {
                        // About's Forget seen apps (#991): the list and the file go; an app
                        // that runs now has run, so it stays.
                        self.watch.forget_seen();
                        write_seen(&self.file, self.watch.seen());
                        log::info!("appwatch: seen apps forgotten");
                        changed = true;
                        continue;
                    }
                    unsafe { DispatchMessageW(&msg) };
                }
                let pids = NEW_WINDOWS.with(|w| std::mem::take(&mut *w.borrow_mut()));
                for pid in pids {
                    if self.watch.is_running(pid) {
                        continue;
                    }
                    if let Some(exe) = exe_of(pid) {
                        changed |= self.consider(pid, &exe);
                    }
                }
            } else {
                log::warn!("appwatch: wait failed ({r}); the watch stops");
                return;
            }
            if changed {
                self.publish();
            }
        }
    }
}

/// The running watches' threads with their seen files, for [`post_forget`].
static WATCHES: std::sync::Mutex<Vec<(u32, PathBuf)>> = std::sync::Mutex::new(Vec::new());

/// The message that tells the watch to forget the apps seen (`crate::forget_seen`).
const WM_FORGET: u32 = windows_sys::Win32::UI::WindowsAndMessaging::WM_APP + 1;

/// Asks the watch that keeps `file` to forget the apps seen. Returns false when none runs.
pub(crate) fn post_forget(file: &std::path::Path) -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::PostThreadMessageW;
    let watches = WATCHES.lock().unwrap_or_else(|e| e.into_inner());
    watches
        .iter()
        .filter(|(_, f)| f == file)
        .any(|(t, _)| unsafe { PostThreadMessageW(*t, WM_FORGET, 0, 0) } != 0)
}

/// The exe path of a running process named `<label>.exe`, when one runs and can be read.
pub(crate) fn running_exe(label: &str) -> Option<String> {
    let want = format!("{label}.exe");
    processes()
        .into_iter()
        .filter(|(_, exe)| exe.eq_ignore_ascii_case(&want))
        .find_map(|(pid, _)| exe_of(pid))
}

/// The exe path from the app's App Paths entry (`<label>.exe`'s default value), the current
/// user's first, then the machine's. Quotes around the path go.
pub(crate) fn app_path(label: &str) -> Option<String> {
    use windows_sys::Win32::System::Registry::{
        RegGetValueW, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ,
    };
    let key: Vec<u16> = format!(r"Software\Microsoft\Windows\CurrentVersion\App Paths\{label}.exe")
        .encode_utf16()
        .chain([0])
        .collect();
    for root in [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE] {
        let mut buf = [0u16; 1024];
        let mut len = std::mem::size_of_val(&buf) as u32;
        let r = unsafe {
            RegGetValueW(
                root,
                key.as_ptr(),
                std::ptr::null(),
                RRF_RT_REG_SZ,
                std::ptr::null_mut(),
                buf.as_mut_ptr().cast(),
                &mut len,
            )
        };
        if r == 0 {
            let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
            let path = String::from_utf16_lossy(&buf[..n]);
            let path = path.trim().trim_matches('"').trim().to_string();
            if !path.is_empty() {
                return Some(path);
            }
        }
    }
    None
}

/// `FileDescription` and `ProductName` from the version info of the file at `path`, in its
/// first listed language, else US English (Unicode, then Western).
pub(crate) fn version_names(path: &str) -> (Option<String>, Option<String>) {
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW,
    };
    let wide: Vec<u16> = path.encode_utf16().chain([0]).collect();
    let size = unsafe { GetFileVersionInfoSizeW(wide.as_ptr(), std::ptr::null_mut()) };
    if size == 0 {
        return (None, None);
    }
    let mut data = vec![0u8; size as usize];
    if unsafe { GetFileVersionInfoW(wide.as_ptr(), 0, size, data.as_mut_ptr().cast()) } == 0 {
        return (None, None);
    }
    let query = |sub: &str| -> Option<(*const u8, u32)> {
        let w: Vec<u16> = sub.encode_utf16().chain([0]).collect();
        let mut p: *mut std::ffi::c_void = std::ptr::null_mut();
        let mut len = 0u32;
        let ok = unsafe { VerQueryValueW(data.as_ptr().cast(), w.as_ptr(), &mut p, &mut len) };
        (ok != 0 && !p.is_null() && len > 0).then_some((p as *const u8, len))
    };
    let mut langs = Vec::new();
    if let Some((p, len)) = query(r"\VarFileInfo\Translation") {
        // Pairs of (language, code page), each a u16.
        for i in 0..(len as usize / 4) {
            let pair = unsafe { std::ptr::read_unaligned(p.cast::<[u16; 2]>().add(i)) };
            langs.push(format!("{:04x}{:04x}", pair[0], pair[1]));
        }
    }
    langs.extend(["040904b0".to_string(), "040904e4".to_string()]);
    let text = |name: &str| -> Option<String> {
        langs.iter().find_map(|l| {
            let (p, len) = query(&format!(r"\StringFileInfo\{l}\{name}"))?;
            // `len` counts UTF-16 units, with the closing zero.
            let units = unsafe { std::slice::from_raw_parts(p.cast::<u16>(), len as usize) };
            let n = units.iter().position(|&c| c == 0).unwrap_or(units.len());
            let s = String::from_utf16_lossy(&units[..n]).trim().to_string();
            (!s.is_empty()).then_some(s)
        })
    };
    (text("FileDescription"), text("ProductName"))
}

pub(crate) fn spawn(
    watch: Watch,
    file: PathBuf,
    sink: Sink,
) -> Option<std::thread::JoinHandle<()>> {
    std::thread::Builder::new()
        .name("hideout-appwatch".into())
        .spawn(move || {
            let me = unsafe { windows_sys::Win32::System::Threading::GetCurrentThreadId() };
            WATCHES
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push((me, file.clone()));
            Watcher {
                watch,
                file,
                sink,
                handles: Vec::new(),
            }
            .run()
        })
        .map_err(|e| log::warn!("appwatch: no thread: {e}"))
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::BTreeSet;
    use std::sync::mpsc;
    use std::time::Duration;

    /// The child half of the test below: waits until it is ended (at most 60 s).
    #[test]
    #[ignore = "run by the_watch_sees_a_running_app_and_its_exit"]
    fn probe_child_waits() {
        std::thread::sleep(Duration::from_secs(60));
    }

    /// A console process with no window (a copy of this test binary under a unique name) runs
    /// before the watch starts: the snapshot says it runs, and its exit is seen from its
    /// handle, with no timer. Window-free: the child is started with CREATE_NO_WINDOW.
    #[test]
    fn the_watch_sees_a_running_app_and_its_exit() {
        use std::os::windows::process::CommandExt;
        let dir = std::env::temp_dir().join(format!("hideout-appwatch-t{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let label = format!("hideout-appwatch-probe-{}", std::process::id());
        let exe = dir.join(format!("{label}.exe"));
        std::fs::copy(std::env::current_exe().unwrap(), &exe).unwrap();
        let mut child = std::process::Command::new(&exe)
            .args(["--ignored", "--exact", "win::tests::probe_child_waits"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .creation_flags(0x0800_0000)
            .spawn()
            .unwrap();
        let (tx, rx) = mpsc::channel();
        let tx = std::sync::Mutex::new(tx);
        let names: BTreeSet<String> = [label.clone()].into();
        crate::start(
            names,
            &dir,
            Box::new(move |c| {
                let _ = tx.lock().unwrap().send(c);
            }),
        )
        .expect("the watch starts");
        let first = rx.recv_timeout(Duration::from_secs(10)).unwrap();
        assert_eq!(
            first,
            vec![
                (format!("apps.running.{label}"), json!(true)),
                (format!("apps.seen.{label}"), json!(true)),
            ]
        );
        child.kill().unwrap();
        child.wait().unwrap();
        let next = rx.recv_timeout(Duration::from_secs(10)).unwrap();
        assert_eq!(next, vec![(format!("apps.running.{label}"), json!(false))]);
        assert!(std::fs::read_to_string(dir.join(crate::SEEN_FILE))
            .unwrap()
            .contains(&label));
        // Forget seen apps reaches the running watch by its message queue (#991).
        crate::forget_seen(&dir);
        let forgot = rx.recv_timeout(Duration::from_secs(10)).unwrap();
        assert_eq!(forgot, vec![(format!("apps.seen.{label}"), json!(false))]);
        assert!(!std::fs::read_to_string(dir.join(crate::SEEN_FILE))
            .unwrap_or_default()
            .contains(&label));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_process_exe_is_read_by_pid() {
        let me = exe_of(std::process::id()).unwrap();
        assert!(me.to_ascii_lowercase().ends_with(".exe"), "{me}");
    }
}
