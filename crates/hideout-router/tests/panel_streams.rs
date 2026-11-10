// SPDX-License-Identifier: MIT
//! The hub's panel host reads a live stream end to end (§5.4, §8.4; H2.9), window-free: the
//! sample room map (`examples/hideout-room-map`) on a real router pipe streams its canvas at
//! 25 frames a second; the panel host opens the channel through the router only while the page
//! is visible, draws 20 to 30 frames a second, and the router's threads run no CPU cycle while
//! frames flow (#988's thread-cycle check). Minimising closes the channel: the panel host's
//! reader and the module's writer both end. The window-free half of the H2.9 CPU-trace lab case.
#![cfg(windows)]

use std::collections::BTreeMap;
use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use foxglove_plugin::hideout::{open_pipe, Handler, InitializeModule, Session};
use hideout_contract::capability::Holder;
use hideout_panels::stream_io::StreamIo;
use hideout_panels::streams::{StreamCommand, StreamKey};
use hideout_panels::{PanelHost, PanelKey};
use hideout_room_map::{panels, registration, RoomMap, CHANNEL, ID};
use hideout_router::server::Hub;
use hideout_router::{ModuleEntry, Out, Router, Settings};
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

fn count(prefix: &str) -> usize {
    let ts = threads(prefix);
    for (_, h) in &ts {
        unsafe { CloseHandle(*h) };
    }
    ts.len()
}

fn entry() -> ModuleEntry {
    ModuleEntry {
        id: ID.into(),
        name: "Room map".into(),
        holder: Holder::FirstParty,
        granted: vec![format!("channel.offer:{CHANNEL}")],
        catalog_provide: None,
        channel_risk: BTreeMap::from([(CHANNEL.to_string(), "normal".to_string())]),
        activate_on_call: false,
        panels: None,
        settings_dir: String::new(),
        data_dir: String::new(),
    }
}

/// Runs the panel host's side until `until`: wakes only when a reader has a frame or a frame
/// slot is due (no polling), pumps, and draws. Returns the frames drawn.
fn run_host(
    host: &mut PanelHost,
    io: &mut StreamIo,
    wake: &mpsc::Receiver<()>,
    until: Instant,
) -> u64 {
    let drawn = |h: &PanelHost| h.streams.iter().map(|(_, s)| s.drawn).sum::<u64>();
    let start = drawn(host);
    loop {
        let now = Instant::now();
        if now >= until {
            return drawn(host) - start;
        }
        // Reads what arrived and draws what is due (`present`, at most 30 a second).
        let (_, opens) = io.pump(&mut host.streams, now);
        assert!(
            opens.is_empty(),
            "no reopen while the stream runs: {opens:?}"
        );
        let next = host.streams.next_wake().map_or(until, |t| t.min(until));
        let _ = wake.recv_timeout(next.saturating_duration_since(Instant::now()));
    }
}

#[test]
fn the_room_map_streams_to_the_panel_host_while_the_router_stays_idle() {
    let mut router = Router::new(Settings::default(), Box::new(|_, _| Vec::new()));
    router.add_module(entry());
    router.set_token(ID, "tok-room");
    let (events, outs) = mpsc::channel();
    let hub = Hub::new(router, events);
    let name = format!(
        r"\\.\pipe\hideout-panel-streams-test-{}",
        std::process::id()
    );
    hub.serve_pipe(&name).expect("serve the test pipe");

    // The sample module, in-process on its own thread, over the real router pipe.
    let (ready_tx, ready) = mpsc::channel();
    let pipe = name.clone();
    std::thread::spawn(move || {
        let mut map = RoomMap::default();
        let p = open_pipe(&pipe).expect("router pipe");
        let mut s = Session::new(std::io::BufReader::new(p.try_clone().unwrap()), p);
        let init = InitializeModule {
            id: ID.into(),
            version: "0.1.0".into(),
            instance: "inst-1".into(),
            token: "tok-room".into(),
            hub: "1.0".into(),
            api: Default::default(),
        };
        s.call_initialize(&init, &mut map).expect("initialize");
        let r = s
            .call_register(&registration(), &mut map)
            .expect("register");
        assert!(r.rejected.is_empty(), "{:?}", r.rejected);
        s.ready("inst-1").unwrap();
        ready_tx.send(()).unwrap();
        let _ = s.run(&mut map as &mut dyn Handler);
    });
    ready.recv_timeout(Duration::from_secs(10)).unwrap();

    // The panel host with the module's page.
    let mut host = PanelHost::default();
    assert!(host
        .register(ID, "Room map", None, &panels("inst-1"))
        .is_empty());
    let page = PanelKey::new(ID, "map");
    let rooms = StreamKey::new(page.clone(), "rooms");
    let (wake_tx, wake) = mpsc::channel();
    let wake_tx = std::sync::Mutex::new(wake_tx);
    let mut io = StreamIo::new(Arc::new(move || {
        let _ = wake_tx.lock().unwrap().send(());
    }));

    // Not visible: nothing opens.
    host.sync_streams(Instant::now());
    assert!(io.pump(&mut host.streams, Instant::now()).1.is_empty());

    // Visible: the open goes through the router; its answer connects the reader.
    host.set_visible(std::slice::from_ref(&page));
    host.take_visibility();
    host.sync_streams(Instant::now());
    let (_, opens) = io.pump(&mut host.streams, Instant::now());
    assert_eq!(opens.len(), 1);
    for o in opens {
        let StreamCommand::Open {
            module, channel, ..
        } = o
        else {
            unreachable!()
        };
        hub.act(|r| r.open_host_channel(&module, &channel, Instant::now()));
    }
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match outs.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(Out::HostChannel {
                module,
                channel,
                result,
            }) => {
                assert!(result.is_ok(), "{result:?}");
                io.granted(&mut host.streams, &module, &channel, result);
                break;
            }
            Ok(_) => {}
            Err(e) => panic!("no channel answer: {e}"),
        }
    }
    assert_eq!(io.open_count(), 1);
    assert!(hub.read(|r| r.next_deadline()).is_none(), "nothing pending");

    // Warm up, then measure one second of streaming.
    run_host(
        &mut host,
        &mut io,
        &wake,
        Instant::now() + Duration::from_millis(300),
    );
    let watched = threads("hideout-router");
    assert!(
        watched.len() >= 3,
        "timer, accept and the module's connection"
    );
    let before: Vec<u64> = watched.iter().map(|(_, h)| cycles(*h)).collect();
    let drawn = run_host(
        &mut host,
        &mut io,
        &wake,
        Instant::now() + Duration::from_secs(1),
    );
    for ((n, h), b) in watched.iter().zip(before) {
        assert_eq!(cycles(*h), b, "{n} ran while the panel stream ran");
        unsafe { CloseHandle(*h) };
    }
    assert!(
        (20..=30).contains(&drawn),
        "{drawn} frames drawn in a second"
    );
    let s = host.streams.get(&rooms).unwrap();
    assert_eq!(s.dropped, 0);
    assert_eq!(count("hideout-stream-rooms"), 1, "one reader");
    assert_eq!(
        count(&format!("hideout-channel-{CHANNEL}")),
        1,
        "one writer"
    );

    // Minimised: the channel closes, the reader and the module's writer both end.
    host.set_visible(&[]);
    host.take_visibility();
    host.sync_streams(Instant::now());
    let (_, opens) = io.pump(&mut host.streams, Instant::now());
    assert!(opens.is_empty());
    assert_eq!(io.open_count(), 0);
    std::thread::sleep(Duration::from_millis(500)); // one write interval and a margin
    assert_eq!(count("hideout-stream-rooms"), 0, "the reader ended");
    assert_eq!(
        count(&format!("hideout-channel-{CHANNEL}")),
        0,
        "the module stopped writing"
    );
    // Nothing is drawn while hidden; the last frame is kept for the return.
    while wake.try_recv().is_ok() {}
    let frozen = host.streams.get(&rooms).unwrap().picture_seq;
    let _ = wake.recv_timeout(Duration::from_millis(300));
    io.pump(&mut host.streams, Instant::now());
    let s = host.streams.get(&rooms).unwrap();
    assert_eq!(s.picture_seq, frozen);
    assert!(s.picture.is_some());
}
