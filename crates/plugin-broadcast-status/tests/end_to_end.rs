//! Broadcast Status end to end: the runner starts it sandboxed as Foxglove does, against a mock
//! keyboard and a mock OBS (an obs-websocket v5 server on a free port), with the password in
//! a test secret store. OBS starts after the plugin: it waits for Foxglove to say so.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use foxglove_devices::facts::{Facts, Value as DeviceValue};
use foxglove_plugin_api::permission::Permission;
use foxglove_plugin_api::rpc::Error;
use foxglove_plugin_host::runner::{Config, PluginSpec, Runner};
use foxglove_plugin_host::{apps, secrets, Client, Context, Devices, Limits};
use serde_json::{json, Value};

/// Records what the plugin asks for.
#[derive(Default)]
struct Mock {
    calls: Mutex<Vec<(String, Value)>>,
}

impl Devices for Mock {
    fn call_from(&self, _conn: u64, method: &str, params: Value) -> Result<Value, Error> {
        self.calls
            .lock()
            .unwrap()
            .push((method.to_string(), params));
        Ok(match method {
            "devices.list" => {
                json!({"devices": [{"id": "kb", "name": "Test board", "kind": "keyboard",
                "backend": "mock", "connected": true, "caps": ["lighting", "screens"]}]})
            }
            "lighting.lease" => json!({"lease": self.calls.lock().unwrap().len()}),
            _ => json!({}),
        })
    }
}

fn soon(what: &str, f: impl Fn() -> bool) {
    let until = Instant::now() + Duration::from_secs(30);
    while !f() {
        assert!(Instant::now() < until, "never saw {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

impl Mock {
    fn wait(&self, what: &str, f: impl Fn(&str, &Value) -> bool) -> Value {
        soon(what, || {
            self.calls.lock().unwrap().iter().any(|(m, p)| f(m, p))
        });
        self.calls
            .lock()
            .unwrap()
            .iter()
            .find(|(m, p)| f(m, p))
            .unwrap()
            .1
            .clone()
    }

    fn clear(&self) {
        self.calls.lock().unwrap().clear();
    }
}

/// The OBS side of a WebSocket: unmasked frames out, masked frames in.
struct Obs(TcpStream);

impl Obs {
    fn accept(listener: &TcpListener) -> Obs {
        let (mut s, _) = listener.accept().unwrap();
        s.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
        let mut head = vec![];
        while !head.ends_with(b"\r\n\r\n") {
            let mut b = [0u8];
            s.read_exact(&mut b).unwrap();
            head.push(b[0]);
        }
        let head = String::from_utf8(head).unwrap();
        assert!(
            head.starts_with("GET / HTTP/1.1")
                && head.contains("Sec-WebSocket-Protocol: obswebsocket.json"),
            "{head}"
        );
        s.write_all(b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\r\n").unwrap();
        Obs(s)
    }

    fn send(&mut self, msg: Value) {
        let text = msg.to_string();
        let mut f = vec![0x81];
        if text.len() < 126 {
            f.push(text.len() as u8);
        } else {
            f.push(126);
            f.extend_from_slice(&(text.len() as u16).to_be_bytes());
        }
        f.extend_from_slice(text.as_bytes());
        self.0.write_all(&f).unwrap();
    }

    fn recv(&mut self) -> Value {
        let mut h = [0u8; 2];
        self.0.read_exact(&mut h).unwrap();
        assert_eq!(
            (h[0], h[1] & 0x80),
            (0x81, 0x80),
            "a final, masked text frame"
        );
        let len = match h[1] & 0x7F {
            126 => {
                let mut l = [0u8; 2];
                self.0.read_exact(&mut l).unwrap();
                u16::from_be_bytes(l) as usize
            }
            n => n as usize,
        };
        let mut mask = [0u8; 4];
        self.0.read_exact(&mut mask).unwrap();
        let mut body = vec![0u8; len];
        self.0.read_exact(&mut body).unwrap();
        body.iter_mut()
            .enumerate()
            .for_each(|(i, b)| *b ^= mask[i % 4]);
        serde_json::from_slice(&body).unwrap()
    }
}

#[test]
fn streaming_is_red_and_live_and_a_lost_obs_lets_go() {
    let exe = PathBuf::from(env!("CARGO_BIN_EXE_broadcast-status"));
    let dir = std::env::temp_dir().join(format!("broadcast-status-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::copy(&exe, dir.join(exe.file_name().unwrap())).unwrap();
    let id = "test.broadcast-status";
    secrets::use_store(Box::<secrets::Memory>::default());
    secrets::set(id, "obs-password", Some("hunter2")).unwrap();

    // A mock OBS on a free port, so tests and a real OBS on the machine don't meet. It isn't
    // running yet when the plugin starts.
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let spec = PluginSpec {
        id: id.into(),
        dir: dir.clone(),
        run: vec![
            exe.file_name().unwrap().to_string_lossy().into(),
            "--port".into(),
            port.to_string(),
        ],
        sandbox: cfg!(windows),
    };
    let mock = Arc::new(Mock::default());
    let facts = Facts::default();
    let (m, f) = (mock.clone(), facts.clone());
    let mut perms: Vec<Permission> = [
        "secrets",
        "facts.write:broadcast.*",
        "devices.read",
        "lighting.write",
        "screens.write",
    ]
    .map(|p| Permission::try_from(p.to_string()).unwrap())
    .to_vec();
    perms.push(Permission::NetConnect {
        host: "127.0.0.1".into(),
        port: Some(port),
    });
    perms.push(Permission::AppsWatch("obs64.exe".into()));
    let context = move || Context {
        facts: f.clone(),
        devices: m.clone(),
        client: Client::Plugin(perms.clone()),
        app_version: "test".into(),
        limits: Limits::default(),
    };
    let runner = Runner::start(spec, Config::default(), context, |_| {});

    // No OBS window is open, so it doesn't try; it waits for one.
    soon("the plugin waiting for OBS", apps::watching);
    let listener = TcpListener::bind(("127.0.0.1", port)).unwrap();
    listener.set_nonblocking(true).unwrap();
    std::thread::sleep(Duration::from_secs(3));
    assert!(listener.accept().is_err(), "no connection before OBS opens");
    listener.set_nonblocking(false).unwrap();
    assert_eq!(apps::shown(1, "OBS64.exe"), 1);

    // Hello with a challenge; the plugin identifies with the password's response.
    let mut obs = Obs::accept(&listener);
    obs.send(json!({"op": 0, "d": {"obsWebSocketVersion": "5.5.0", "rpcVersion": 1, "authentication": {"challenge": "ch", "salt": "sa"}}}));
    let identify = obs.recv();
    assert_eq!(identify["op"], 1);
    let expected = {
        use base64::Engine;
        use sha2::{Digest, Sha256};
        let b64 = |b: &[u8]| base64::engine::general_purpose::STANDARD.encode(b);
        let secret = b64(&Sha256::digest(b"hunter2sa"));
        b64(&Sha256::digest(format!("{secret}ch")))
    };
    assert_eq!(identify["d"]["authentication"], json!(expected));
    obs.send(json!({"op": 2, "d": {"negotiatedRpcVersion": 1}}));

    // The snapshot: streaming, not recording, on "Gameplay". Only Get requests are asked.
    for _ in 0..3 {
        let ask = obs.recv();
        assert_eq!(ask["op"], 6);
        let t = ask["d"]["requestType"].as_str().unwrap().to_string();
        assert!(t.starts_with("Get"), "{t}");
        let data = match t.as_str() {
            "GetStreamStatus" => json!({"outputActive": true}),
            "GetRecordStatus" => json!({"outputActive": false, "outputPaused": false}),
            _ => json!({"currentProgramSceneName": "Gameplay"}),
        };
        obs.send(json!({"op": 7, "d": {"requestType": t, "requestId": ask["d"]["requestId"], "requestStatus": {"result": true, "code": 100}, "responseData": data}}));
    }
    let lease = mock.wait("a red lease", |m, p| {
        m == "lighting.lease" && p["color"]["hue"] == 0 && p["color"]["saturation"] == 100
    });
    assert_eq!(lease["priority"], "high");
    let live = mock.wait("the LIVE screen", |m, _| m == "screens.panel_override");
    assert_eq!(
        (live["width"].clone(), live["ttl_ms"].clone()),
        (json!(64), json!(0)),
        "held, not timed"
    );
    soon("the facts", || {
        facts.get("broadcast.streaming") == Some(DeviceValue::Bool(true))
            && facts.get("broadcast.scene") == Some(DeviceValue::Text("Gameplay".into()))
    });

    // The stream stops: the red goes and READY shows briefly.
    mock.clear();
    obs.send(json!({"op": 5, "d": {"eventType": "StreamStateChanged", "eventIntent": 64, "eventData": {"outputActive": false, "outputState": "OBS_WEBSOCKET_OUTPUT_STOPPED"}}}));
    mock.wait("the red given back", |m, _| m == "lighting.release");
    let ready = mock.wait("the READY screen", |m, _| m == "screens.panel_override");
    assert_eq!(ready["ttl_ms"], json!(3000));
    soon("streaming off", || {
        facts.get("broadcast.streaming") == Some(DeviceValue::Bool(false))
    });

    // OBS quits: NOT CONNECTED briefly, and a burst of tries reconnects.
    mock.clear();
    drop(obs);
    let gone = mock.wait("the NOT CONNECTED screen", |m, _| {
        m == "screens.panel_override"
    });
    assert_eq!(gone["ttl_ms"], json!(3000));
    soon("connected off", || {
        facts.get("broadcast.connected") == Some(DeviceValue::Bool(false))
    });
    let again = Obs::accept(&listener);

    // OBS quits for good: its window closes, so after the connection drops it doesn't retry.
    assert_eq!(apps::destroyed(1), 1);
    drop(again);
    listener.set_nonblocking(true).unwrap();
    std::thread::sleep(Duration::from_secs(5));
    assert!(listener.accept().is_err(), "no retries after OBS quit");

    drop(runner);
    #[cfg(windows)]
    foxglove_plugin_host::delete_container(id);
    let _ = std::fs::remove_dir_all(dir);
}
