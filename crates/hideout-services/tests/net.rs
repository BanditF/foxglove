//! The network broker through a real hub, window-free, against loopback servers (contract
//! §8.10): grants enforced with their error ids, the SDK's own client code (the first-party
//! plugins' path: `connect_tcp`, `listen_tcp` with a WebSocket handshake, `listen_udp`), every
//! stream closed when its connection ends, and the caps.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream, UdpSocket};
use std::path::Path;
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use foxglove_plugin::{NetEvent, Plugin};
use hideout_contract::capability::Holder;
use hideout_router::server::Hub;
use hideout_router::{ModuleEntry, Origin, Out, Router, Settings};
use hideout_services::net::{Broker, Caps};
use hideout_services::HostServices;
use serde_json::{json, Value};

const WAIT: Duration = Duration::from_secs(10);

fn entry(id: &str, granted: &[String]) -> ModuleEntry {
    ModuleEntry {
        id: id.into(),
        name: id.into(),
        holder: Holder::FirstParty,
        granted: granted.to_vec(),
        catalog_provide: Some(vec![]),
        channel_risk: BTreeMap::new(),
        activate_on_call: false,
        panels: None,
        settings_dir: String::new(),
        data_dir: String::new(),
    }
}

/// A hub serving the broker, with these modules and their grants.
fn hub(modules: &[(&str, Vec<String>)], caps: Caps) -> (Hub, Broker, Receiver<Out>) {
    let mut router = Router::new(Settings::default(), Box::new(|_, asked| asked.to_vec()));
    for (id, granted) in modules {
        router.add_module(entry(id, granted));
    }
    let (events, rx) = mpsc::channel();
    let hub = Hub::new(router, events);
    let broker = Broker::new(caps);
    hub.set_services(Arc::new(HostServices {
        net: Some(broker.clone()),
        mic: None,
    }));
    (hub, broker, rx)
}

/// A free loopback port (bound and let go).
fn free_port() -> u16 {
    TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// An echo server; `ended` gets one message per connection that reached its end of stream.
fn echo_server() -> (u16, Receiver<()>) {
    let l = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = l.local_addr().unwrap().port();
    let (tx, ended) = mpsc::channel();
    std::thread::spawn(move || {
        for s in l.incoming() {
            let Ok(mut s) = s else { continue };
            let tx = tx.clone();
            std::thread::spawn(move || {
                let mut buf = [0u8; 4096];
                loop {
                    match s.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            if s.write_all(&buf[..n]).is_err() {
                                break;
                            }
                        }
                    }
                }
                let _ = tx.send(());
            });
        }
    });
    (port, ended)
}

/// A raw JSON-RPC client on the hub (a bridged module), reading on a thread of its own.
struct Raw {
    w: Option<std::io::PipeWriter>,
    rx: Receiver<Value>,
    events: Vec<Value>,
    /// While true the reader stops taking lines (the client "isn't reading").
    paused: Arc<(Mutex<bool>, Condvar)>,
}

impl Raw {
    fn attach(hub: &Hub, id: &str) -> Raw {
        let (from_hub, to_party) = std::io::pipe().unwrap();
        let (from_party, to_hub) = std::io::pipe().unwrap();
        hub.attach(Origin::Bridged(id.into()), from_party, to_party);
        let (tx, rx) = mpsc::channel();
        let paused = Arc::new((Mutex::new(false), Condvar::new()));
        let p = paused.clone();
        std::thread::spawn(move || {
            let mut r = BufReader::new(from_hub);
            loop {
                {
                    let (lock, cv) = &*p;
                    let mut held = lock.lock().unwrap();
                    while *held {
                        held = cv.wait(held).unwrap();
                    }
                }
                let mut line = String::new();
                match r.read_line(&mut line) {
                    Ok(0) | Err(_) => return,
                    Ok(_) => {
                        if tx.send(serde_json::from_str(&line).unwrap()).is_err() {
                            return;
                        }
                    }
                }
            }
        });
        let mut raw = Raw {
            w: Some(to_hub),
            rx,
            events: Vec::new(),
            paused,
        };
        let init = raw.call(
            0,
            "initialize",
            json!({"api": "1.3", "client": {"name": id, "version": "1"}}),
        );
        assert!(init.get("result").is_some(), "{init}");
        raw
    }

    fn send(&mut self, v: Value) {
        writeln!(self.w.as_mut().unwrap(), "{v}").unwrap();
    }

    /// Calls and returns the response; notifications on the way are kept.
    fn call(&mut self, id: u64, method: &str, params: Value) -> Value {
        self.send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        loop {
            let v = self.rx.recv_timeout(WAIT).expect("an answer");
            if v.get("method").is_some() {
                self.events.push(v);
            } else if v["id"] == json!(id) {
                return v;
            }
        }
    }

    /// The next notification named `method`.
    fn event(&mut self, method: &str) -> Value {
        if let Some(i) = self.events.iter().position(|e| e["method"] == method) {
            return self.events.remove(i);
        }
        loop {
            let v = self
                .rx
                .recv_timeout(WAIT)
                .unwrap_or_else(|_| panic!("no {method}"));
            if v["method"] == method {
                return v;
            }
            self.events.push(v);
        }
    }

    fn pause(&self, on: bool) {
        let (lock, cv) = &*self.paused;
        *lock.lock().unwrap() = on;
        cv.notify_all();
    }

    /// Ends the connection (the module stopped, or crashed).
    fn hang_up(&mut self) {
        self.w = None;
    }
}

fn data_id(v: &Value) -> (i64, &str) {
    (
        v["error"]["code"].as_i64().unwrap_or_default(),
        v["error"]["data"]["id"].as_str().unwrap_or_default(),
    )
}

fn open(raw: &mut Raw, id: u64, kind: &str, host: &str, port: u16) -> Value {
    raw.call(
        id,
        "net.open",
        json!({"kind": kind, "host": host, "port": port}),
    )
}

#[test]
fn grants_are_enforced_and_refusals_carry_their_ids() {
    let (port, _) = echo_server();
    let (hub, broker, _rx) = hub(
        &[
            (
                "example.status",
                vec![format!("net.connect:127.0.0.1:{port}")],
            ),
            ("example.any", vec!["net".into()]),
            ("example.none", vec![]),
        ],
        Caps::default(),
    );
    let mut status = Raw::attach(&hub, "example.status");
    let ok = open(&mut status, 1, "tcp", "127.0.0.1", port);
    assert_eq!(ok["result"], json!({"stream": 1}), "{ok}");
    // Another port, another spelling of the host, a listen: not granted.
    let other = if port == u16::MAX { port - 1 } else { port + 1 };
    for (i, (kind, host, p)) in [
        ("tcp", "127.0.0.1", other),
        ("tcp", "localhost", port),
        ("tcp-listen", "127.0.0.1", port),
    ]
    .into_iter()
    .enumerate()
    {
        let r = open(&mut status, 10 + i as u64, kind, host, p);
        assert_eq!(data_id(&r), (5, "net.denied"), "{kind} {host}:{p}: {r}");
        assert!(
            r["error"]["data"]["reason"]
                .as_str()
                .unwrap()
                .contains("needs permission"),
            "{r}"
        );
    }
    assert_eq!(
        data_id(&open(&mut status, 20, "udp", "0.0.0.0", 5568)),
        (-32602, "net.argument")
    );
    assert_eq!(
        data_id(&open(&mut status, 21, "tcp", "127.0.0.1", 0)),
        (-32602, "net.argument")
    );
    let bad = status.call(
        22,
        "net.open",
        json!({"kind": "icmp", "host": "127.0.0.1", "port": 1}),
    );
    assert_eq!(data_id(&bad), (-32602, "net.argument"), "{bad}");
    // Nothing is open but the one granted stream.
    assert_eq!(broker.open_streams(), 1);

    // `net` covers any of them; no grant covers none.
    let mut any = Raw::attach(&hub, "example.any");
    assert_eq!(
        open(&mut any, 1, "tcp", "127.0.0.1", port)["result"],
        json!({"stream": 1})
    );
    let mut none = Raw::attach(&hub, "example.none");
    assert_eq!(
        data_id(&open(&mut none, 1, "tcp", "127.0.0.1", port)),
        (5, "net.denied")
    );

    // A refused connect is the network's no: error 6 net.io.
    let closed = free_port();
    let mut any2 = Raw::attach(&hub, "example.any");
    assert_eq!(
        data_id(&open(&mut any2, 1, "tcp", "127.0.0.1", closed)),
        (6, "net.io")
    );

    // Streams are per connection: example.any's stream 1 is not example.status's.
    let s = status.call(30, "net.send", json!({"stream": 2, "data": "aGk="}));
    assert_eq!(data_id(&s), (-32602, "net.stream"));
}

#[test]
fn bytes_flow_both_ways_and_the_far_end_closing_is_said_once() {
    let l = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = l.local_addr().unwrap().port();
    let (hub, broker, _rx) = hub(
        &[(
            "example.status",
            vec![format!("net.connect:127.0.0.1:{port}")],
        )],
        Caps::default(),
    );
    let mut m = Raw::attach(&hub, "example.status");
    let r = open(&mut m, 1, "tcp", "127.0.0.1", port);
    assert_eq!(r["result"]["stream"], 1, "{r}");
    let (mut far, _) = l.accept().unwrap();
    // Module to far end.
    let sent = m.call(2, "net.send", json!({"stream": 1, "data": "aGVsbG8="}));
    assert_eq!(sent["result"], json!({}));
    let mut got = [0u8; 5];
    far.read_exact(&mut got).unwrap();
    assert_eq!(&got, b"hello");
    // Far end to module, then it closes.
    far.write_all(b"ping").unwrap();
    let d = m.event("net.data");
    assert_eq!(d["params"], json!({"stream": 1, "data": "cGluZw=="}));
    drop(far);
    let c = m.event("net.closed");
    assert_eq!(c["params"]["stream"], 1);
    assert_eq!(c["params"]["reason"], "closed by the other side");
    assert_eq!(broker.open_streams(), 0);
    // A send to the ended stream is refused; closing it again is fine.
    let s = m.call(3, "net.send", json!({"stream": 1, "data": "aGk="}));
    assert_eq!(data_id(&s), (-32602, "net.stream"));
    assert_eq!(
        m.call(4, "net.close", json!({"stream": 1}))["result"],
        json!({})
    );
    assert!(
        m.events.iter().all(|e| e["method"] != "net.closed"),
        "net.closed once"
    );
}

#[test]
fn every_stream_closes_when_the_module_stops() {
    let (port, ended) = echo_server();
    let listen = free_port();
    let udp = free_port();
    let (hub, broker, _rx) = hub(
        &[(
            "example.status",
            vec![
                format!("net.connect:127.0.0.1:{port}"),
                format!("net.listen:tcp:127.0.0.1:{listen}"),
                format!("net.listen:udp:127.0.0.1:{udp}"),
            ],
        )],
        Caps::default(),
    );
    let mut m = Raw::attach(&hub, "example.status");
    for (i, (kind, p)) in [
        ("tcp", port),
        ("tcp", port),
        ("tcp-listen", listen),
        ("udp", udp),
    ]
    .into_iter()
    .enumerate()
    {
        let r = open(&mut m, i as u64 + 1, kind, "127.0.0.1", p);
        assert!(r.get("result").is_some(), "{kind}: {r}");
    }
    // One accepted connection too.
    let _client = TcpStream::connect((Ipv4Addr::LOCALHOST, listen)).unwrap();
    m.event("net.accepted");
    assert_eq!(broker.open_streams(), 5);

    // The module goes (exit, crash or stop all end its connection).
    m.hang_up();
    for _ in 0..2 {
        ended
            .recv_timeout(WAIT)
            .expect("the echo server saw its connection end");
    }
    assert_eq!(broker.open_streams(), 0);
}

#[test]
fn the_caps_hold() {
    let (port, _) = echo_server();
    let listen = free_port();
    let caps = Caps {
        per_connection: 2,
        per_hub: 3,
        ..Caps::default()
    };
    let grant = vec![
        format!("net.connect:127.0.0.1:{port}"),
        format!("net.listen:tcp:127.0.0.1:{listen}"),
    ];
    let (hub, broker, _rx) = hub(&[("example.a", grant.clone()), ("example.b", grant)], caps);
    let mut a = Raw::attach(&hub, "example.a");
    let mut b = Raw::attach(&hub, "example.b");
    assert!(open(&mut a, 1, "tcp", "127.0.0.1", port)
        .get("result")
        .is_some());
    assert!(open(&mut a, 2, "tcp", "127.0.0.1", port)
        .get("result")
        .is_some());
    assert_eq!(
        data_id(&open(&mut a, 3, "tcp", "127.0.0.1", port)),
        (8, "net.limit")
    );
    assert!(open(&mut b, 1, "tcp", "127.0.0.1", port)
        .get("result")
        .is_some());
    // The hub is full: 3.
    assert_eq!(
        data_id(&open(&mut b, 2, "tcp", "127.0.0.1", port)),
        (8, "net.limit")
    );
    assert_eq!(broker.open_streams(), 3);
    // One closed frees one; ids are never reused on a connection.
    assert_eq!(
        a.call(4, "net.close", json!({"stream": 1}))["result"],
        json!({})
    );
    assert_eq!(
        open(&mut b, 3, "tcp-listen", "127.0.0.1", listen)["result"],
        json!({"stream": 2})
    );
    // b holds 2 (its cap): a connection its listener accepts is closed at once, unannounced.
    let mut c = TcpStream::connect((Ipv4Addr::LOCALHOST, listen)).unwrap();
    c.set_read_timeout(Some(WAIT)).unwrap();
    let mut buf = [0u8; 1];
    assert!(
        matches!(c.read(&mut buf), Ok(0) | Err(_)),
        "closed by the hub"
    );
    assert!(b.events.iter().all(|e| e["method"] != "net.accepted"));
    assert_eq!(broker.open_streams(), 3);
}

#[test]
fn a_module_that_isnt_reading_loses_the_stream_not_the_hub() {
    let l = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = l.local_addr().unwrap().port();
    let caps = Caps {
        backlog_bytes: 64 * 1024,
        ..Caps::default()
    };
    let (hub, broker, _rx) = hub(
        &[(
            "example.slow",
            vec![format!("net.connect:127.0.0.1:{port}")],
        )],
        caps,
    );
    let mut m = Raw::attach(&hub, "example.slow");
    assert_eq!(
        open(&mut m, 1, "tcp", "127.0.0.1", port)["result"]["stream"],
        1
    );
    let (mut far, _) = l.accept().unwrap();
    m.pause(true);
    // 4 MiB toward a module that reads nothing: the hub stops queueing past the cap and ends
    // the stream; the far end's writes fail once the hub closed it.
    let writer = std::thread::spawn(move || {
        let chunk = vec![7u8; 64 * 1024];
        for _ in 0..64 {
            if far.write_all(&chunk).is_err() {
                return;
            }
        }
    });
    writer.join().unwrap();
    m.pause(false);
    let c = m.event("net.closed");
    assert_eq!(c["params"]["reason"], "the module isn't reading");
    assert_eq!(broker.open_streams(), 0);
}

#[test]
fn the_plugins_own_client_code_works_through_the_hub() {
    let (port, _) = echo_server();
    let listen = free_port();
    let udp = free_port();
    let (hub, _broker, _rx) = hub(
        &[(
            "example.status",
            vec![
                format!("net.connect:127.0.0.1:{port}"),
                format!("net.listen:tcp:127.0.0.1:{listen}"),
                format!("net.listen:udp:127.0.0.1:{udp}"),
            ],
        )],
        Caps::default(),
    );
    let (hub_reads, plugin_writes) = std::io::pipe().unwrap();
    let (plugin_reads, hub_writes) = std::io::pipe().unwrap();
    hub.attach(
        Origin::Bridged("example.status".into()),
        hub_reads,
        hub_writes,
    );
    let plugin = Plugin::connect_over(plugin_reads, plugin_writes, "example.status", "1").unwrap();

    // connect_tcp, as a status module reaches its app.
    let mut s = plugin.connect_tcp("127.0.0.1", port).unwrap();
    s.write_all(b"hello through the hub").unwrap();
    let mut got = vec![0u8; 21];
    s.read_exact(&mut got).unwrap();
    assert_eq!(&got, b"hello through the hub");

    // listen_tcp with a WebSocket handshake answered by the SDK's ws code.
    let listener = plugin.listen_tcp(listen).unwrap();
    let client = std::thread::spawn(move || {
        let mut c = TcpStream::connect((Ipv4Addr::LOCALHOST, listen)).unwrap();
        c.set_read_timeout(Some(WAIT)).unwrap();
        let key = "dGhlIHNhbXBsZSBub25jZQ==";
        write!(c, "GET /feed HTTP/1.1\r\nHost: 127.0.0.1\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n").unwrap();
        let mut head = Vec::new();
        let mut b = [0u8; 1];
        while !head.ends_with(b"\r\n\r\n") {
            assert_eq!(c.read(&mut b).unwrap(), 1);
            head.push(b[0]);
        }
        let mut frame = [0u8; 4];
        c.read_exact(&mut frame).unwrap();
        (String::from_utf8(head).unwrap(), frame)
    });
    let (mut ws, _from) = listener.accept().unwrap();
    let h = foxglove_plugin::ws::read_handshake(&mut ws).unwrap();
    assert_eq!(h.path, "/feed");
    foxglove_plugin::ws::accept(&mut ws, &h, None).unwrap();
    foxglove_plugin::ws::write_text(&mut ws, "hi").unwrap();
    let (head, frame) = client.join().unwrap();
    assert!(head.starts_with("HTTP/1.1 101"), "{head}");
    assert!(head.contains(&format!(
        "Sec-WebSocket-Accept: {}",
        foxglove_plugin::ws::accept_key("dGhlIHNhbXBsZSBub25jZQ==")
    )));
    assert_eq!(frame, [0x81, 2, b'h', b'i']);

    // listen_udp, as a lighting module receives its datagrams.
    let rx = plugin.listen_udp(udp).unwrap();
    let sender = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    sender
        .send_to(b"frame", (Ipv4Addr::LOCALHOST, udp))
        .unwrap();
    match rx.recv_timeout(WAIT).unwrap() {
        NetEvent::Data(bytes, from) => {
            assert_eq!(bytes, b"frame");
            assert_eq!(from, Some(sender.local_addr().unwrap().to_string()));
        }
        other => panic!("{other:?}"),
    }
}

/// `transcripts/net/stream-refused.json` (§16): the broker's answers, replayed.
#[test]
fn the_stream_refused_vector_passes() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/hideout-vectors/transcripts/net/stream-refused.json");
    let doc: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let udp = free_port();
    let (hub, _broker, _rx) = hub(
        &[(
            "foxglove.example-art",
            vec![format!("net.listen:udp:127.0.0.1:{udp}")],
        )],
        Caps::default(),
    );
    let mut m = Raw::attach(&hub, "foxglove.example-art");
    let mut pending: Option<Value> = None;
    let mut checked = 0;
    for step in doc["steps"].as_array().unwrap() {
        if step.get("event").is_some() {
            let r = open(&mut m, 100, "udp", "127.0.0.1", udp);
            assert_eq!(r["result"], json!({"stream": 1}), "{r}");
            continue;
        }
        let msg = &step["msg"];
        if step["to"] == "hub" {
            pending = Some(msg.clone());
            continue;
        }
        let call = pending.take().unwrap();
        let got = m.call(
            call["id"].as_u64().unwrap(),
            call["method"].as_str().unwrap(),
            call["params"].clone(),
        );
        match msg.get("error") {
            Some(want) => {
                assert_eq!(got["error"]["code"], want["code"], "{got}");
                assert_eq!(got["error"]["data"]["id"], want["data"]["id"], "{got}");
            }
            None => assert_eq!(got["result"], msg["result"], "{got}"),
        }
        checked += 1;
    }
    assert_eq!(checked, 5);
}
