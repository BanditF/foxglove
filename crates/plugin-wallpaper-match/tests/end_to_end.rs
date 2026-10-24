//! Wallpaper Match end to end: the runner starts it sandboxed as Foxglove does, a stand-in
//! wallpaper connects over a WebSocket on a free port, and a mock keyboard records the leases.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use foxglove_devices::facts::{Facts, Value as Fact};
use foxglove_plugin_api::permission::Permission;
use foxglove_plugin_api::rpc::Error;
use foxglove_plugin_host::runner::{Config, PluginSpec, Runner};
use foxglove_plugin_host::{Client, Context, Devices, Limits};
use serde_json::{json, Value};

const TOKEN: &str = "s3cret-pairing-token-123";

#[derive(Default)]
struct Keyboard {
    calls: Mutex<Vec<(String, Value)>>,
}

impl Devices for Keyboard {
    fn call(&self, method: &str, params: Value) -> Result<Value, Error> {
        let n = {
            let mut calls = self.calls.lock().unwrap();
            calls.push((method.to_string(), params));
            calls.len()
        };
        Ok(match method {
            "devices.list" => json!({"devices": [{"id": "kb", "name": "Test board", "kind": "keyboard",
                "backend": "mock", "connected": true, "caps": ["lighting"]}]}),
            "lighting.lease" => json!({"lease": n}),
            _ => json!({}),
        })
    }
}

impl Keyboard {
    fn count(&self, method: &str) -> usize {
        self.calls.lock().unwrap().iter().filter(|(m, _)| m == method).count()
    }

    fn last(&self, method: &str) -> Value {
        self.calls.lock().unwrap().iter().rev().find(|(m, _)| m == method).map(|(_, p)| p.clone()).unwrap_or_default()
    }

    fn wait(&self, what: &str, f: impl Fn(&Keyboard) -> bool) {
        let until = Instant::now() + Duration::from_secs(30);
        while !f(self) {
            assert!(Instant::now() < until, "never saw {what}: {:?}", self.calls.lock().unwrap());
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

/// A stand-in wallpaper: the WebSocket upgrade, then masked text frames.
fn connect(port: u16, token: &str) -> (TcpStream, String) {
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
    write!(s, "GET /?token={token} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nOrigin: null\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n").unwrap();
    let mut head = Vec::new();
    let mut b = [0u8];
    while !head.ends_with(b"\r\n\r\n") && s.read(&mut b).unwrap_or(0) == 1 {
        head.push(b[0]);
    }
    (s, String::from_utf8_lossy(&head).into_owned())
}

fn say(s: &mut TcpStream, msg: Value) {
    let text = msg.to_string();
    let mask = [7u8, 1, 2, 3];
    let mut f = vec![0x81, 0x80 | text.len() as u8];
    f.extend_from_slice(&mask);
    f.extend(text.bytes().enumerate().map(|(i, b)| b ^ mask[i % 4]));
    s.write_all(&f).unwrap();
}

fn wallpaper(id: &str, accent: &str, paused: bool) -> Value {
    json!({"v": 1, "wallpaperId": id, "theme": "dark", "accent": accent, "palette": [accent], "paused": paused})
}

#[test]
fn a_wallpapers_accent_holds_a_low_lease_until_it_pauses_closes_or_the_keyboard_goes() {
    foxglove_plugin_host::set_settings_source(|_| {
        let mut m = serde_json::Map::new();
        m.insert("pairing-token".into(), json!(TOKEN));
        m.insert("origins".into(), json!(""));
        m
    });
    let exe = PathBuf::from(env!("CARGO_BIN_EXE_wallpaper-match"));
    let dir = std::env::temp_dir().join(format!("wallpaper-match-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::copy(&exe, dir.join(exe.file_name().unwrap())).unwrap();
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let spec = PluginSpec {
        id: "test.wallpaper-match".into(),
        dir: dir.clone(),
        run: vec![exe.file_name().unwrap().to_string_lossy().into(), "--port".into(), port.to_string()],
        sandbox: cfg!(windows),
    };
    let keyboard = Arc::new(Keyboard::default());
    let facts = Facts::default();
    let (k, f) = (keyboard.clone(), facts.clone());
    let mut perms: Vec<Permission> = ["facts.read:keyboard.connected", "devices.read", "lighting.write"].map(|p| Permission::try_from(p.to_string()).unwrap()).to_vec();
    perms.push(Permission::NetListenTcp(port));
    let context = move || Context { facts: f.clone(), devices: k.clone(), client: Client::Plugin(perms.clone()), app_version: "test".into(), limits: Limits::default() };
    let runner = Runner::start(spec, Config::default(), context, |_| {});

    // Wait for the listener, then: a wrong token is refused.
    let until = Instant::now() + Duration::from_secs(30);
    while TcpStream::connect(("127.0.0.1", port)).is_err() {
        assert!(Instant::now() < until, "the plugin never listened");
        std::thread::sleep(Duration::from_millis(50));
    }
    let (_, refused) = connect(port, "wrong-token-wrong-token");
    assert!(refused.starts_with("HTTP/1.1 403"), "{refused}");

    // The wallpaper's accent: one low-priority lease.
    let (mut wp, reply) = connect(port, TOKEN);
    assert!(reply.starts_with("HTTP/1.1 101"), "{reply}");
    say(&mut wp, wallpaper("fury-print", "#ff0000", false));
    keyboard.wait("a lease", |k| k.count("lighting.lease") == 1);
    let lease = keyboard.last("lighting.lease");
    assert_eq!((lease["priority"].clone(), lease["color"]["hue"].clone()), (json!("low"), json!(0)));
    // A new accent changes it in place.
    say(&mut wp, wallpaper("fury-print", "#00ff00", false));
    keyboard.wait("an update", |k| k.count("lighting.update") == 1);
    assert_eq!(keyboard.last("lighting.update")["color"]["hue"], json!(120));
    // Another wallpaper: given back, then taken afresh.
    say(&mut wp, wallpaper("courier", "#0000ff", false));
    keyboard.wait("a release and a new lease", |k| k.count("lighting.release") == 1 && k.count("lighting.lease") == 2);
    // Paused: given back; unpaused: taken again.
    say(&mut wp, wallpaper("courier", "#0000ff", true));
    keyboard.wait("the release on pause", |k| k.count("lighting.release") == 2);
    say(&mut wp, wallpaper("courier", "#0000ff", false));
    keyboard.wait("a lease after the pause", |k| k.count("lighting.lease") == 3);
    // The keyboard goes: given back; it comes back: taken again.
    facts.set("keyboard.connected", Fact::Bool(false));
    keyboard.wait("the release on disconnect", |k| k.count("lighting.release") == 3);
    facts.set("keyboard.connected", Fact::Bool(true));
    keyboard.wait("a lease after reconnecting", |k| k.count("lighting.lease") == 4);
    // The wallpaper closes: given back.
    drop(wp);
    keyboard.wait("the release on close", |k| k.count("lighting.release") == 4);

    drop(runner);
    #[cfg(windows)]
    foxglove_plugin_host::delete_container("test.wallpaper-match");
    let _ = std::fs::remove_dir_all(dir);
}
