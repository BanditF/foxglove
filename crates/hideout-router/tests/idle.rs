// SPDX-License-Identifier: MIT
//! The idle router does no periodic work (#935): with no calls pending, its timer thread waits
//! with no timeout and the pipe's accept thread waits in the kernel, so neither runs at all.
//! Checked window-free by the threads' CPU cycle counters over a quiet second.
#![cfg(windows)]

use std::sync::mpsc;
use std::time::Duration;

use hideout_router::server::Hub;
use hideout_router::{Router, Settings};
use windows_sys::Win32::Foundation::{CloseHandle, LocalFree, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD, THREADENTRY32,
};
use windows_sys::Win32::System::Threading::{
    GetThreadDescription, OpenThread, THREAD_QUERY_LIMITED_INFORMATION,
};
use windows_sys::Win32::System::WindowsProgramming::QueryThreadCycleTime;

/// This process's threads whose name starts with `prefix`, as (name, handle).
fn threads(prefix: &str) -> Vec<(String, HANDLE)> {
    let pid = std::process::id();
    let mut out = Vec::new();
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
        assert_ne!(snap, INVALID_HANDLE_VALUE);
        let mut e: THREADENTRY32 = std::mem::zeroed();
        e.dwSize = std::mem::size_of::<THREADENTRY32>() as u32;
        let mut ok = Thread32First(snap, &mut e) != 0;
        while ok {
            if e.th32OwnerProcessID == pid {
                let h = OpenThread(THREAD_QUERY_LIMITED_INFORMATION, 0, e.th32ThreadID);
                if !h.is_null() {
                    let mut p = std::ptr::null_mut();
                    let mut name = String::new();
                    if GetThreadDescription(h, &mut p) >= 0 && !p.is_null() {
                        let len = (0..).take_while(|&i| *p.add(i) != 0).count();
                        name = String::from_utf16_lossy(std::slice::from_raw_parts(p, len));
                        LocalFree(p as _);
                    }
                    if name.starts_with(prefix) {
                        out.push((name, h));
                    } else {
                        CloseHandle(h);
                    }
                }
            }
            ok = Thread32Next(snap, &mut e) != 0;
        }
        CloseHandle(snap);
    }
    out
}

fn cycles(h: HANDLE) -> u64 {
    let mut c = 0u64;
    assert_ne!(unsafe { QueryThreadCycleTime(h, &mut c) }, 0);
    c
}

#[test]
fn idle_router_threads_never_run() {
    let router = Router::new(Settings::default(), Box::new(|_, _| Vec::new()));
    let (events, _rx) = mpsc::channel();
    let hub = Hub::new(router, events);
    let name = format!(r"\\.\pipe\hideout-idle-test-{}", std::process::id());
    hub.serve_pipe(&name).expect("serve the test pipe");
    assert!(hub.read(|r| r.next_deadline()).is_none());
    std::thread::sleep(Duration::from_millis(300)); // threads reach their waits
    let ts = threads("hideout-router");
    let names: Vec<_> = ts.iter().map(|(n, _)| n.as_str()).collect();
    assert!(names.contains(&"hideout-router-timer"), "{names:?}");
    assert!(names.contains(&"hideout-router-accept"), "{names:?}");
    let before: Vec<u64> = ts.iter().map(|(_, h)| cycles(*h)).collect();
    std::thread::sleep(Duration::from_secs(1));
    for ((n, h), b) in ts.iter().zip(before) {
        assert_eq!(cycles(*h), b, "{n} ran while the router was idle");
        unsafe { CloseHandle(*h) };
    }
}
