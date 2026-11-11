//! `apps.watch` (contract §8.12, v1.13, #1108) on the hub: the SDK's `watch_app` and
//! `app_windows`, over the legacy stdio bridge, against the router with the real app watch
//! (`hideout-appwatch`) publishing `apps.running.<name>` as the hub wires it
//! (crates/hideout/src/router_start.rs). The app is a stand-in: a copy of this test binary
//! under a unique name that makes one hidden top-level window (never shown) and waits. Started
//! and ended by the test; nothing appears on the desktop.
#![cfg(windows)]

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::os::windows::process::CommandExt;
use std::sync::{mpsc, Mutex};
use std::time::Duration;

use foxglove_plugin::{AppWindowEvent, Plugin, PluginError};
use hideout_contract::capability::{Holder, Source};
use hideout_router::server::Hub;
use hideout_router::{ModuleEntry, Origin, Out, Router, Settings};
use serde_json::{json, Value};

const WATCHER: &str = "foxglove.example-watcher";
const OTHER: &str = "foxglove.example-other";

/// `CREATE_NO_WINDOW`: the stand-in gets no console window.
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Every wait below ends on its event; this is only the fallback.
const WAIT: Duration = Duration::from_secs(20);

fn entry(id: &str, name: &str, granted: Vec<String>) -> ModuleEntry {
    ModuleEntry {
        id: id.into(),
        name: name.into(),
        holder: Holder::ThirdParty(Source::Catalog),
        granted,
        catalog_provide: Some(vec![]),
        channel_risk: BTreeMap::new(),
        activate_on_call: false,
        panels: None,
        settings_dir: String::new(),
        data_dir: String::new(),
    }
}

/// A bridged plugin's stdin/stdout on the hub: (what the hub writes, what the plugin writes).
fn open(hub: &Hub, id: &str) -> (std::io::PipeReader, std::io::PipeWriter) {
    let (from_hub, to_party) = std::io::pipe().unwrap();
    let (from_party, to_hub) = std::io::pipe().unwrap();
    hub.attach(Origin::Bridged(id.into()), from_party, to_party);
    (from_hub, to_hub)
}

/// The stand-in app's body: one hidden top-level window, then a wait until the test ends it
/// (at most 60 s).
#[test]
#[ignore = "the stand-in app of the_sdk_hears_a_stand_in_app_open_and_close"]
fn stand_in_holds_a_hidden_window() {
    use windows_sys::Win32::UI::WindowsAndMessaging::CreateWindowExW;
    let class: Vec<u16> = "STATIC\0".encode_utf16().collect();
    // No WS_VISIBLE: created, never shown.
    let hwnd = unsafe {
        CreateWindowExW(
            0,
            class.as_ptr(),
            std::ptr::null(),
            0,
            0,
            0,
            0,
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null(),
        )
    };
    assert!(!hwnd.is_null());
    std::thread::sleep(Duration::from_secs(60));
}

#[test]
fn the_sdk_hears_a_stand_in_app_open_and_close() {
    let label = format!("hideout-apps-watch-{}", std::process::id());
    let exe = format!("{label}.exe");
    let dir = std::env::temp_dir().join(format!("hideout-apps-watch-t{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let stand_in = dir.join(&exe);
    std::fs::copy(std::env::current_exe().unwrap(), &stand_in).unwrap();

    let mut router = Router::new(Settings::default(), Box::new(|_, asked| asked.to_vec()));
    router.add_module(entry(
        WATCHER,
        "Example Watcher",
        vec![format!("apps.watch:{exe}")],
    ));
    router.add_module(entry(OTHER, "Example Other", vec!["devices.read".into()]));
    let (events, _events) = mpsc::channel();
    let hub = Hub::new(router, events);

    // The app watch publishes through the router, as the hub wires it; the test hears each
    // batch after the router has it.
    let (tx, published) = mpsc::channel();
    let tx = Mutex::new(tx);
    let h = hub.clone();
    hideout_appwatch::start(
        [label.clone()].into(),
        &dir,
        Box::new(move |changes| {
            h.act(|r| {
                changes
                    .iter()
                    .flat_map(|(k, v)| r.publish(k, v.clone()))
                    .collect()
            });
            let _ = tx.lock().unwrap().send(changes);
        }),
    )
    .expect("the watch starts");
    assert_eq!(
        published.recv_timeout(WAIT).unwrap(),
        vec![(format!("apps.running.{label}"), json!(false))]
    );

    // The watching plugin, through the SDK: not running yet.
    let (r, w) = open(&hub, WATCHER);
    let plugin = Plugin::connect_over(r, w, "watcher", "1.0.0").unwrap();
    let windows = plugin.app_windows().unwrap();
    assert_eq!(plugin.watch_app(&exe.to_ascii_uppercase()).unwrap(), 0);

    // The app starts: `app.window.appeared`, with the exe as the hub spells it.
    let mut child = std::process::Command::new(&stand_in)
        .args(["--ignored", "--exact", "stand_in_holds_a_hidden_window"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .unwrap();
    assert_eq!(
        windows.recv_timeout(WAIT).unwrap(),
        AppWindowEvent::Appeared {
            exe: exe.clone(),
            open: 1
        }
    );
    // A watch asked now answers 1.
    assert_eq!(plugin.watch_app(&exe).unwrap(), 1);

    // The app quits: `app.window.closed` with `open` 0, which the SDK's callers read as quit.
    child.kill().unwrap();
    child.wait().unwrap();
    assert_eq!(
        windows.recv_timeout(WAIT).unwrap(),
        AppWindowEvent::Closed {
            exe: exe.clone(),
            open: 0
        }
    );

    // Without `apps.watch:<exe>`: refused, error 5 `apps.denied`, and no watch starts.
    let (r, w) = open(&hub, OTHER);
    let other = Plugin::connect_over(r, w, "other", "1.0.0").unwrap();
    match other.watch_app(&exe) {
        Err(PluginError::Api(e)) => {
            assert_eq!(e.code, 5, "{e:?}");
            assert_eq!(e.data.unwrap()["id"], "apps.denied");
        }
        r => panic!("{r:?}"),
    }
    assert_eq!(hub.read(|r| r.app_watchers(&label)), 1);
    assert!(windows.try_recv().is_err(), "one event per change");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_watch_ends_with_the_modules_connection() {
    let mut router = Router::new(Settings::default(), Box::new(|_, asked| asked.to_vec()));
    router.add_module(entry(
        WATCHER,
        "Example Watcher",
        vec!["apps.watch:example-app.exe".into()],
    ));
    let (events, gone) = mpsc::channel();
    let hub = Hub::new(router, events);
    hub.act(|r| r.publish("apps.running.example-app", json!(false)));

    let (r, mut w) = open(&hub, WATCHER);
    let mut r = BufReader::new(r);
    let mut call = |v: Value| {
        writeln!(w, "{v}").unwrap();
        let mut line = String::new();
        r.read_line(&mut line).unwrap();
        serde_json::from_str::<Value>(&line).unwrap()
    };
    let init = call(
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
        "api":"1.3","client":{"name":"watcher","version":"1.0.0"}}}),
    );
    assert!(init.get("result").is_some(), "{init}");
    let watch = call(
        json!({"jsonrpc":"2.0","id":2,"method":"apps.watch","params":{"exe":"example-app.exe"}}),
    );
    assert_eq!(
        watch["result"],
        json!({"exe": "example-app.exe", "open": 0}),
        "{watch}"
    );
    // A path or a name without `.exe` is no program name: −32602 `apps.argument`.
    let bad = call(
        json!({"jsonrpc":"2.0","id":3,"method":"apps.watch","params":{"exe":"C:\\x\\example-app.exe"}}),
    );
    assert_eq!(bad["error"]["code"], -32602, "{bad}");
    assert_eq!(bad["error"]["data"]["id"], "apps.argument");
    assert_eq!(hub.read(|r| r.app_watchers("example-app")), 1);

    // The module goes: its watch ends with the connection.
    drop(call);
    drop((r, w));
    loop {
        match gone.recv_timeout(WAIT).expect("the module's disconnect") {
            Out::ModuleGone(id) if id == WATCHER => break,
            _ => {}
        }
    }
    assert_eq!(hub.read(|r| r.app_watchers("example-app")), 0);
    // A later start reaches no one.
    hub.act(|r| {
        let out = r.publish("apps.running.example-app", json!(true));
        assert!(out.iter().all(|o| !matches!(o, Out::Send(..))), "{out:?}");
        out
    });
}
