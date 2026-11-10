// SPDX-License-Identifier: MIT
//! A direct channel end to end (§8.4, #977), window-free: two sample modules on a real router
//! pipe. The provider (`foxglove.example-art`) offers `previews`; the consumer
//! (`foxglove.example-viewer`) holds `channel.open:foxglove.example-art/previews`. The router
//! brokers the grant and steps out: while hundreds of panel frames stream over the provider's
//! one-time pipe, none of the router's threads runs a single CPU cycle (the window-free half of
//! the H4.3 CPU-trace lab case). The pipe takes one consumer only, and a consumer without the
//! grant is refused before the provider hears of it.
#![cfg(windows)]

use std::collections::BTreeMap;
use std::io::Write;
use std::sync::mpsc;
use std::time::Duration;

use foxglove_plugin::channel::{self, read_frame, ChannelConn};
use foxglove_plugin::hideout::{
    open_pipe, ChannelCreate, Handler, InitializeModule, ModuleRegister, RpcError, Session,
    SessionError,
};
use hideout_contract::capability::Holder;
use hideout_contract::frame::{decode_payload, encode, Frame};
use hideout_router::server::Hub;
use hideout_router::{ModuleEntry, Router, Settings};
use serde_json::{json, Value};
use windows_sys::Win32::Foundation::{CloseHandle, LocalFree, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD, THREADENTRY32,
};
use windows_sys::Win32::System::Threading::{
    GetThreadDescription, OpenThread, THREAD_QUERY_LIMITED_INFORMATION,
};
use windows_sys::Win32::System::WindowsProgramming::QueryThreadCycleTime;

const ART: &str = "foxglove.example-art";
const VIEWER: &str = "foxglove.example-viewer";
const FRAMES: u32 = 600;
const SHAPES: usize = 64;

fn entry(id: &str, name: &str, granted: &[&str], channels: &[&str]) -> ModuleEntry {
    ModuleEntry {
        id: id.into(),
        name: name.into(),
        holder: Holder::FirstParty,
        granted: granted.iter().map(|s| s.to_string()).collect(),
        catalog_provide: None,
        channel_risk: channels
            .iter()
            .map(|c| (c.to_string(), "normal".to_string()))
            .collect::<BTreeMap<_, _>>(),
        activate_on_call: false,
        panels: None,
        settings_dir: String::new(),
        data_dir: String::new(),
    }
}

/// The provider: answers `channel.create` with a one-time pipe and streams `FRAMES` canvas
/// frames (§8.4 kind 2) to whoever presents the nonce, then closes.
struct Art {
    done: mpsc::Sender<()>,
}

impl Handler for Art {
    fn channel_create(&mut self, c: ChannelCreate) -> Result<Value, RpcError> {
        assert_eq!(c.consumer, format!("module:{VIEWER}"));
        let done = self.done.clone();
        let pipe = channel::serve(ART, &c.channel, &c.nonce, move |mut conn: ChannelConn| {
            for seq in 0..FRAMES {
                let colours = (0..SHAPES)
                    .map(|i| [seq as u8, i as u8, 0x40, 0xff])
                    .collect();
                let wire = encode(&Frame::Canvas { seq, colours });
                if conn.write_all(&wire).is_err() {
                    break;
                }
            }
            let _ = done.send(());
        })
        .map_err(|e| RpcError::new(6, e.to_string()))?;
        Ok(json!({ "pipe": pipe }))
    }
}

/// The consumer has no hub calls to answer.
struct Viewer;
impl Handler for Viewer {}

type PipeSession = Session<std::io::BufReader<std::fs::File>, std::fs::File>;

/// Connects a module to the router pipe and brings it to ready.
fn module(
    router_pipe: &str,
    id: &str,
    token: &str,
    channels: &[&str],
    h: &mut dyn Handler,
) -> PipeSession {
    let pipe = open_pipe(router_pipe).expect("router pipe");
    let reader = std::io::BufReader::new(pipe.try_clone().unwrap());
    let mut s = Session::new(reader, pipe);
    let init = InitializeModule {
        id: id.into(),
        version: "0.1.0".into(),
        instance: format!("inst-{id}"),
        token: token.into(),
        hub: "1.0".into(),
        api: Default::default(),
    };
    s.call_initialize(&init, h).expect("initialize");
    let reg = ModuleRegister {
        channels: channels.iter().map(|c| c.to_string()).collect(),
        ..Default::default()
    };
    let r = s.call_register(&reg, h).expect("register");
    assert!(r.rejected.is_empty(), "{:?}", r.rejected);
    s.ready(&format!("inst-{id}")).unwrap();
    s
}

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
fn frames_flow_between_two_modules_while_the_router_stays_idle() {
    let mut router = Router::new(Settings::default(), Box::new(|_, _| Vec::new()));
    router.add_module(entry(
        ART,
        "Example Art",
        &["channel.offer:previews"],
        &["previews"],
    ));
    router.add_module(entry(
        VIEWER,
        "Example Viewer",
        &["channel.open:foxglove.example-art/previews"],
        &[],
    ));
    router.set_token(ART, "tok-art");
    router.set_token(VIEWER, "tok-viewer");
    let (events, _rx) = mpsc::channel();
    let hub = Hub::new(router, events);
    let name = format!(r"\\.\pipe\hideout-channels-test-{}", std::process::id());
    hub.serve_pipe(&name).expect("serve the test pipe");

    // The provider answers hub calls on its own thread.
    let (done_tx, done) = mpsc::channel();
    let art_pipe = name.clone();
    let (art_ready, art_is_ready) = mpsc::channel();
    std::thread::spawn(move || {
        let mut art = Art { done: done_tx };
        let mut s = module(&art_pipe, ART, "tok-art", &["previews"], &mut art);
        art_ready.send(()).unwrap();
        let _ = s.run(&mut art);
    });
    art_is_ready.recv_timeout(Duration::from_secs(10)).unwrap();

    // The grant: channel.open through the router.
    let mut viewer = Viewer;
    let mut s = module(&name, VIEWER, "tok-viewer", &[], &mut viewer);
    let opened = s
        .call(
            "channel.open",
            json!({"module": ART, "channel": "previews"}),
            &mut viewer,
        )
        .expect("channel.open");
    let pipe = opened["pipe"].as_str().unwrap().to_string();
    let nonce = opened["nonce"].as_str().unwrap().to_string();
    assert!(channel::is_channel_pipe(&pipe), "{pipe}");
    assert_eq!(nonce.len(), 32, "a 128-bit nonce");
    assert!(hub.read(|r| r.next_deadline()).is_none(), "nothing pending");

    std::thread::sleep(Duration::from_millis(200)); // router threads reach their waits
    let watched = threads("hideout-router");
    assert!(
        watched.len() >= 4,
        "timer, accept and two connections' threads"
    );
    let before: Vec<u64> = watched.iter().map(|(_, h)| cycles(*h)).collect();

    // The stream: straight from the provider's pipe.
    let mut conn = channel::connect(&pipe, &nonce).expect("connect");
    assert!(
        channel::connect(&pipe, &nonce).is_err(),
        "a second consumer is refused"
    );
    let mut got = 0u32;
    while let Some(payload) = read_frame(&mut conn).unwrap() {
        match decode_payload(&payload).expect("a whole canvas frame") {
            Frame::Canvas { seq, colours } => {
                assert_eq!(seq, got);
                assert_eq!(colours.len(), SHAPES);
                assert_eq!(colours[3], [seq as u8, 3, 0x40, 0xff]);
            }
            other => panic!("unexpected frame {other:?}"),
        }
        got += 1;
    }
    assert_eq!(got, FRAMES);
    done.recv_timeout(Duration::from_secs(10)).unwrap();

    for ((n, h), b) in watched.iter().zip(before) {
        assert_eq!(cycles(*h), b, "{n} ran while frames streamed");
        unsafe { CloseHandle(*h) };
    }
    assert!(
        channel::connect(&pipe, &nonce).is_err(),
        "the spent pipe is gone"
    );

    // Without the grant, the provider never hears of it (error 5 from the router).
    let tool_pipe = open_pipe(&name).unwrap();
    let mut tool = Session::new(
        std::io::BufReader::new(tool_pipe.try_clone().unwrap()),
        tool_pipe,
    );
    tool.call(
        "initialize",
        json!({"client": {"name": "example-cli", "version": "0.1.0"}, "hub": "1.0",
               "api": {"foxglove": "1.13"}, "permissions": []}),
        &mut Viewer,
    )
    .expect("tool initialize");
    let err = tool
        .call(
            "channel.open",
            json!({"module": ART, "channel": "previews"}),
            &mut Viewer,
        )
        .unwrap_err();
    assert!(
        matches!(&err, SessionError::Rpc(e) if e.code == 5),
        "{err:?}"
    );
}
