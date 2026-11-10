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

pub(crate) fn spawn(
    watch: Watch,
    file: PathBuf,
    sink: Sink,
) -> Option<std::thread::JoinHandle<()>> {
    std::thread::Builder::new()
        .name("hideout-appwatch".into())
        .spawn(move || {
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
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_process_exe_is_read_by_pid() {
        let me = exe_of(std::process::id()).unwrap();
        assert!(me.to_ascii_lowercase().ends_with(".exe"), "{me}");
    }
}
