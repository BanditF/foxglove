//! Golden transcripts: lines in, lines out, compared as JSON.

use super::*;
use std::io::Cursor;
use std::time::Duration;

#[derive(Clone, Default)]
struct Out(Arc<Mutex<Vec<u8>>>);

impl Write for Out {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Out {
    fn lines(&self) -> Vec<Value> {
        String::from_utf8(self.0.lock().unwrap().clone())
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }
}

fn plugin(perms: &[&str]) -> Context {
    Context {
        facts: Facts::default(),
        devices: Arc::new(NoDevices),
        client: Client::Plugin(
            perms
                .iter()
                .map(|p| Permission::try_from(p.to_string()).unwrap())
                .collect(),
        ),
        app_version: "0.3.0".into(),
        limits: Limits::default(),
    }
}

fn run(ctx: Context, input: &[Value]) -> Vec<Value> {
    let text: String = input.iter().map(|v| v.to_string() + "\n").collect();
    let out = Out::default();
    serve(Cursor::new(text), out.clone(), ctx).unwrap();
    out.lines()
}

fn init() -> Value {
    json!({"jsonrpc":"2.0","id":0,"method":"initialize","params":{"api":"1.0","client":{"name":"t","version":"1"}}})
}

fn call(id: i64, method: &str, params: Value) -> Value {
    json!({"jsonrpc":"2.0","id":id,"method":method,"params":params})
}

fn err(id: i64, code: i32) -> impl Fn(&Value) -> bool {
    move |v: &Value| v["id"] == json!(id) && v["error"]["code"] == json!(code)
}

#[test]
fn initialize_comes_first_and_reports_the_grant() {
    let out = run(
        plugin(&["facts.read:clock.*"]),
        &[
            call(1, "ping", json!({})),
            init(),
            call(2, "ping", json!({})),
        ],
    );
    assert!(err(1, code::INVALID_REQUEST)(&out[0]));
    assert_eq!(
        out[1],
        json!({"jsonrpc":"2.0","id":0,"result":{"api":"1.4","app":"0.3.0","granted":["facts.read:clock.*"]}})
    );
    assert_eq!(out[2], json!({"jsonrpc":"2.0","id":2,"result":{}}));
}

#[test]
fn newer_major_is_refused() {
    let mut i = init();
    i["params"]["api"] = json!("2.0");
    let out = run(plugin(&[]), &[i]);
    assert!(err(0, code::VERSION)(&out[0]));
}

#[test]
fn facts_follow_the_grant() {
    let ctx = plugin(&["facts.read:weather.*", "facts.write:weather.*"]);
    let facts = ctx.facts.clone();
    facts.set("clock.time", DeviceValue::Text("12:00".into()));
    let out = run(
        ctx,
        &[
            init(),
            call(1, "facts.set", json!({"key":"weather.temp","value":14})),
            call(2, "facts.get", json!({"key":"weather.temp"})),
            call(3, "facts.get", json!({"key":"clock.time"})),
            call(4, "facts.set", json!({"key":"device.x.layers","value":1})),
            call(
                5,
                "facts.set",
                json!({"key":"weather.icon","value":{"base64":"AAE="}}),
            ),
        ],
    );
    assert_eq!(out[1]["result"], json!({}));
    assert_eq!(out[2]["result"], json!({"value":14}));
    assert!(err(3, code::DENIED)(&out[3]));
    assert!(
        err(4, code::DENIED)(&out[4]),
        "device facts are never writable"
    );
    assert_eq!(
        facts.get("weather.icon"),
        Some(DeviceValue::Bytes(vec![0, 1]))
    );
}

#[test]
fn device_methods_check_permissions_then_reach_the_app() {
    struct Lights;
    impl Devices for Lights {
        fn call(&self, method: &str, _: Value) -> Result<Value, Error> {
            assert_eq!(method, "lighting.set");
            Ok(json!({}))
        }
    }
    let mut ctx = plugin(&["lighting.write"]);
    ctx.devices = Arc::new(Lights);
    let out = run(
        ctx,
        &[
            init(),
            call(1, "lighting.set", json!({"device":"d","on":true})),
            call(
                2,
                "behaviors.invoke",
                json!({"device":"d","id":1,"param1":0,"param2":0}),
            ),
            call(3, "no.such", json!({})),
            call(4, "facts.changed", json!({})),
        ],
    );
    assert_eq!(out[1]["result"], json!({}));
    assert!(err(2, code::DENIED)(&out[2]));
    assert!(err(3, code::METHOD_NOT_FOUND)(&out[3]));
    assert!(
        err(4, code::METHOD_NOT_FOUND)(&out[4]),
        "events can't be called"
    );
}

#[test]
fn tools_get_only_what_the_user_approves() {
    let mut ctx = plugin(&[]);
    ctx.client = Client::Tool(Arc::new(|client: &ClientInfo, asked: &[Permission]| {
        assert_eq!(client.name, "deck");
        asked.iter().filter(|p| !p.sensitive()).cloned().collect()
    }));
    let mut i = init();
    i["params"]["client"]["name"] = json!("deck");
    i["params"]["permissions"] = json!(["devices.read", "key.events"]);
    let out = run(ctx, &[i]);
    assert_eq!(out[0]["result"]["granted"], json!(["devices.read"]));
}

#[test]
fn tools_are_never_granted_secrets() {
    let mut ctx = plugin(&[]);
    ctx.client = Client::Tool(Arc::new(|_: &ClientInfo, asked: &[Permission]| {
        assert!(
            !asked.contains(&Permission::Secrets),
            "not asked about secrets"
        );
        let mut all = asked.to_vec();
        all.push(Permission::Secrets);
        all
    }));
    let mut i = init();
    i["params"]["permissions"] = json!(["devices.read", "secrets"]);
    let out = run(ctx, &[i, call(2, "secrets.get", json!({"name": "token"}))]);
    assert_eq!(out[0]["result"]["granted"], json!(["devices.read"]));
    assert!(err(2, code::DENIED)(&out[1]));
}

#[test]
fn notifications_get_no_reply_and_bad_lines_get_parse_errors() {
    let out = run(
        plugin(&[]),
        &[
            init(),
            json!({"jsonrpc":"2.0","method":"ping"}),
            json!("not a request"),
            call(9, "ping", json!(null)),
        ],
    );
    assert_eq!(out.len(), 3);
    assert!(err(0, code::PARSE_ERROR)(&out[1]));
    assert_eq!(out[2]["id"], json!(9));
}

#[test]
fn floods_are_rate_limited() {
    let mut ctx = plugin(&[]);
    ctx.limits = Limits {
        per_second: 0.0,
        burst: 3.0,
    };
    let out = run(
        ctx,
        &[
            init(),
            call(1, "ping", json!({})),
            call(2, "ping", json!({})),
            call(3, "ping", json!({})),
        ],
    );
    assert_eq!(out[2]["result"], json!({}));
    assert!(err(3, code::RATE_LIMITED)(&out[3]));
}

#[test]
fn subscriptions_deliver_matching_changes() {
    let ctx = plugin(&["facts.read:media.*", "facts.write:media.*"]);
    let facts = ctx.facts.clone();
    let (reader, feed) = pipe();
    let out = Out::default();
    let out2 = out.clone();
    let server = std::thread::spawn(move || serve(reader, out2, ctx));
    for line in [
        init(),
        call(1, "facts.subscribe", json!({"pattern":"media.*"})),
        call(2, "facts.subscribe", json!({"pattern":"clock.*"})),
    ] {
        feed.send(line.to_string() + "\n").unwrap();
    }
    wait(|| out.lines().len() >= 3);
    facts.set("clock.time", DeviceValue::Text("x".into()));
    facts.set("media.title", DeviceValue::Text("Song".into()));
    wait(|| out.lines().len() >= 4);
    let lines = out.lines();
    assert!(err(2, code::DENIED)(&lines[2]));
    assert_eq!(
        lines[3],
        json!({"jsonrpc":"2.0","method":"facts.changed","params":{"subscription":1,"key":"media.title","value":"Song"}})
    );
    drop(feed);
    server.join().unwrap().unwrap();
}

/// A reader fed line by line from the test.
fn pipe() -> (impl BufRead + Send + 'static, Sender<String>) {
    struct Feed(mpsc::Receiver<String>, Vec<u8>);
    impl std::io::Read for Feed {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if self.1.is_empty() {
                match self.0.recv() {
                    Ok(s) => self.1 = s.into_bytes(),
                    Err(_) => return Ok(0),
                }
            }
            let n = buf.len().min(self.1.len());
            buf[..n].copy_from_slice(&self.1[..n]);
            self.1.drain(..n);
            Ok(n)
        }
    }
    let (tx, rx) = mpsc::channel();
    (std::io::BufReader::new(Feed(rx, Vec::new())), tx)
}

fn wait(mut done: impl FnMut() -> bool) {
    let end = Instant::now() + Duration::from_secs(2);
    while !done() && Instant::now() < end {
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// A live connection for the network tests: lines go in while `serve` runs.
struct Live {
    tx: Option<mpsc::Sender<Vec<u8>>>,
    out: Out,
    server: Option<std::thread::JoinHandle<()>>,
}

struct ChanReader {
    rx: mpsc::Receiver<Vec<u8>>,
    buf: Vec<u8>,
}

impl std::io::Read for ChanReader {
    fn read(&mut self, b: &mut [u8]) -> std::io::Result<usize> {
        if self.buf.is_empty() {
            match self.rx.recv() {
                Ok(v) => self.buf = v,
                Err(_) => return Ok(0),
            }
        }
        let n = b.len().min(self.buf.len());
        b[..n].copy_from_slice(&self.buf[..n]);
        self.buf.drain(..n);
        Ok(n)
    }
}

impl Live {
    fn start(ctx: Context) -> Live {
        let (tx, rx) = mpsc::channel();
        let out = Out::default();
        let writer = out.clone();
        let server = std::thread::spawn(move || {
            serve(
                std::io::BufReader::new(ChanReader { rx, buf: vec![] }),
                writer,
                ctx,
            )
            .unwrap();
        });
        let live = Live {
            tx: Some(tx),
            out,
            server: Some(server),
        };
        live.send(init());
        live
    }

    fn send(&self, v: Value) {
        self.tx
            .as_ref()
            .unwrap()
            .send((v.to_string() + "\n").into_bytes())
            .unwrap();
    }

    /// Waits (up to 10 s) for a line matching `f`.
    fn wait(&self, f: impl Fn(&Value) -> bool) -> Value {
        let until = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(v) = self.out.lines().into_iter().find(|v| f(v)) {
                return v;
            }
            assert!(
                Instant::now() < until,
                "timed out; got {:?}",
                self.out.lines()
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn stop(mut self) {
        drop(self.tx.take());
        self.server.take().unwrap().join().unwrap();
    }
}

fn reply(id: i64) -> impl Fn(&Value) -> bool {
    move |v: &Value| v["id"] == json!(id)
}

#[test]
fn net_udp_listen_is_brokered_and_scoped() {
    // A free port for the grant.
    let port = std::net::UdpSocket::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let live = Live::start(plugin(&[&format!("net.listen:udp:127.0.0.1:{port}")]));
    live.send(call(
        1,
        "net.open",
        json!({"kind":"udp","host":"127.0.0.1","port":port.wrapping_add(1)}),
    ));
    assert!(
        err(1, code::DENIED)(&live.wait(reply(1))),
        "another port is refused"
    );
    live.send(call(
        2,
        "net.open",
        json!({"kind":"udp","host":"0.0.0.0","port":port}),
    ));
    assert!(err(2, code::DENIED)(&live.wait(reply(2))), "loopback only");
    live.send(call(
        3,
        "net.open",
        json!({"kind":"udp","host":"127.0.0.1","port":port}),
    ));
    let stream = live.wait(reply(3))["result"]["stream"].as_u64().unwrap();
    std::net::UdpSocket::bind("127.0.0.1:0")
        .unwrap()
        .send_to(b"hello", ("127.0.0.1", port))
        .unwrap();
    let data = live.wait(|v| v["method"] == "net.data");
    assert_eq!(data["params"]["stream"], json!(stream));
    assert_eq!(data["params"]["data"], json!(STANDARD.encode(b"hello")));
    live.stop();
    // The connection ended, so the port is free again.
    assert!(
        std::net::UdpSocket::bind(("127.0.0.1", port)).is_ok() || {
            std::thread::sleep(Duration::from_millis(500));
            std::net::UdpSocket::bind(("127.0.0.1", port)).is_ok()
        }
    );
}

#[test]
fn net_tcp_listen_accepts_on_loopback_and_stops_when_closed() {
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let live = Live::start(plugin(&[&format!("net.listen:tcp:127.0.0.1:{port}")]));
    live.send(call(
        1,
        "net.open",
        json!({"kind":"tcp-listen","host":"127.0.0.1","port":port + 1}),
    ));
    assert!(
        err(1, code::DENIED)(&live.wait(reply(1))),
        "only the granted port"
    );
    live.send(call(
        2,
        "net.open",
        json!({"kind":"tcp-listen","host":"127.0.0.1","port":port}),
    ));
    let listener = live.wait(reply(2))["result"]["stream"].as_u64().unwrap();
    // A local page connects: the plugin is told, with a stream of its own.
    let mut page = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    let accepted = live.wait(|v| v["method"] == "net.accepted");
    assert_eq!(accepted["params"]["listener"], json!(listener));
    let stream = accepted["params"]["stream"].as_u64().unwrap();
    assert!(accepted["params"]["from"]
        .as_str()
        .unwrap()
        .starts_with("127.0.0.1:"));
    page.write_all(b"hello").unwrap();
    let data = live.wait(|v| v["method"] == "net.data" && v["params"]["stream"] == json!(stream));
    assert_eq!(data["params"]["data"], json!(STANDARD.encode(b"hello")));
    live.send(call(
        3,
        "net.send",
        json!({"stream":stream,"data":STANDARD.encode(b"hi")}),
    ));
    let mut got = [0u8; 2];
    std::io::Read::read_exact(&mut page, &mut got).unwrap();
    assert_eq!(&got, b"hi");
    live.send(call(
        4,
        "net.send",
        json!({"stream":listener,"data":STANDARD.encode(b"x")}),
    ));
    assert!(
        err(4, code::INVALID_PARAMS)(&live.wait(reply(4))),
        "nothing is sent to a listener"
    );
    // Closing the listener stops accepting; the open connection stays until it ends.
    live.send(call(5, "net.close", json!({"stream":listener})));
    live.wait(reply(5));
    let until = Instant::now() + Duration::from_secs(10);
    while std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
        assert!(Instant::now() < until, "still accepting after close");
        std::thread::sleep(Duration::from_millis(20));
    }
    drop(page);
    live.wait(|v| v["method"] == "net.closed" && v["params"]["stream"] == json!(stream));
    live.stop();
}

#[test]
fn net_tcp_connect_both_ways() {
    let server = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = server.local_addr().unwrap().port();
    let live = Live::start(plugin(&[&format!("net.connect:127.0.0.1:{port}")]));
    live.send(call(
        1,
        "net.open",
        json!({"kind":"tcp","host":"localhost","port":port}),
    ));
    assert!(
        err(1, code::DENIED)(&live.wait(reply(1))),
        "the grant names 127.0.0.1, not localhost"
    );
    live.send(call(
        2,
        "net.open",
        json!({"kind":"tcp","host":"127.0.0.1","port":port}),
    ));
    let (mut peer, _) = server.accept().unwrap();
    let stream = live.wait(reply(2))["result"]["stream"].as_u64().unwrap();
    live.send(call(
        3,
        "net.send",
        json!({"stream":stream,"data":STANDARD.encode(b"ping")}),
    ));
    let mut got = [0u8; 4];
    std::io::Read::read_exact(&mut peer, &mut got).unwrap();
    assert_eq!(&got, b"ping");
    peer.write_all(b"pong").unwrap();
    let data = live.wait(|v| v["method"] == "net.data");
    assert_eq!(data["params"]["data"], json!(STANDARD.encode(b"pong")));
    drop(peer);
    let closed = live.wait(|v| v["method"] == "net.closed");
    assert_eq!(closed["params"]["stream"], json!(stream));
    live.stop();
}

#[test]
fn notify_sends_declared_events_to_one_running_plugin() {
    let (tx, rx) = mpsc::channel();
    register("test.notify", Some(tx));
    assert!(notify(
        "test.notify",
        "action.invoked",
        json!({"action": "toggle"})
    ));
    let sent: Value = serde_json::from_str(&rx.try_recv().unwrap()).unwrap();
    assert_eq!(
        sent,
        json!({"jsonrpc":"2.0","method":"action.invoked","params":{"action":"toggle"}})
    );
    assert!(
        !notify("test.notify", "facts.get", json!({})),
        "only events"
    );
    assert!(
        !notify("test.other", "action.invoked", json!({"action": "toggle"})),
        "only that plugin"
    );
    register("test.notify", None);
    assert!(
        !notify("test.notify", "action.invoked", json!({"action": "toggle"})),
        "not after it stops"
    );
}

#[test]
fn secrets_belong_to_one_plugin_and_need_the_permission() {
    secrets::use_store(Box::new(secrets::Memory::default()));
    let as_plugin = |id: Option<&str>, perms: &[&str], input: &[Value]| {
        let text: String = input.iter().map(|v| v.to_string() + "\n").collect();
        let out = Out::default();
        serve_with(
            Cursor::new(text),
            out.clone(),
            plugin(perms),
            id.map(String::from),
            |_| {},
        )
        .unwrap();
        out.lines()
    };
    let set = call(
        1,
        "secrets.set",
        json!({"name": "obs-password", "value": "hunter2"}),
    );
    let get = call(2, "secrets.get", json!({"name": "obs-password"}));
    let out = as_plugin(
        Some("org.a"),
        &["secrets"],
        &[init(), set.clone(), get.clone()],
    );
    assert_eq!(out[2]["result"], json!({"value": "hunter2"}));
    // Another plugin sees only its own.
    let out = as_plugin(Some("org.b"), &["secrets"], &[init(), get.clone()]);
    assert_eq!(out[1]["result"], json!({"value": null}));
    // Without the permission, or from an outside tool, nothing.
    let out = as_plugin(Some("org.a"), &[], &[init(), get.clone()]);
    assert!(err(2, code::DENIED)(&out[1]));
    let out = as_plugin(None, &["secrets"], &[init(), get.clone()]);
    assert!(err(2, code::DENIED)(&out[1]));
    // Removing it, and the uninstall clean-up.
    as_plugin(
        Some("org.a"),
        &["secrets"],
        &[
            init(),
            call(3, "secrets.set", json!({"name": "t", "value": "x"})),
        ],
    );
    secrets::delete_all("org.a").unwrap();
    let out = as_plugin(Some("org.a"), &["secrets"], &[init(), get]);
    assert_eq!(out[1]["result"], json!({"value": null}));
}

/// Records which connection made each device call, and which connections closed.
#[derive(Default)]
struct Conns {
    calls: Mutex<Vec<u64>>,
    closed: Mutex<Vec<u64>>,
}

impl Devices for Conns {
    fn call_from(&self, conn: u64, _method: &str, _params: Value) -> Result<Value, Error> {
        self.calls.lock().unwrap().push(conn);
        Ok(json!({}))
    }
    fn closed(&self, conn: u64) {
        self.closed.lock().unwrap().push(conn);
    }
}

#[test]
fn device_calls_know_their_connection_and_hear_it_close() {
    let conns = Arc::new(Conns::default());
    let ctx = |c: Arc<Conns>| Context {
        devices: c,
        ..plugin(&["lighting.write"])
    };
    let lease = call(
        1,
        "lighting.lease",
        json!({"device": "kb", "color": {"hue": 0, "saturation": 100, "brightness": 50}}),
    );
    run(ctx(conns.clone()), &[init(), lease.clone()]);
    run(ctx(conns.clone()), &[init(), lease]);
    let calls = conns.calls.lock().unwrap().clone();
    assert_eq!(calls.len(), 2);
    assert_ne!(calls[0], calls[1], "each connection has its own number");
    assert_eq!(
        *conns.closed.lock().unwrap(),
        calls,
        "and each one's end is reported"
    );
}

#[test]
fn a_plugin_watches_only_the_apps_it_declares() {
    let (tx, rx) = mpsc::channel();
    let out = Out::default();
    let writer = out.clone();
    // Its own app name, so the apps module's own test doesn't meet it.
    let ctx = plugin(&["apps.watch:host-test.exe"]);
    let server = std::thread::spawn(move || {
        serve_with(
            std::io::BufReader::new(ChanReader { rx, buf: vec![] }),
            writer,
            ctx,
            Some("test.watch".into()),
            |o| register("test.watch", Some(o)),
        )
        .unwrap();
        register("test.watch", None);
    });
    let send = |v: Value| {
        tx.send(
            (v.to_string()
                + "
")
            .into_bytes(),
        )
        .unwrap()
    };
    let wait = |f: &dyn Fn(&Value) -> bool| {
        let until = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(v) = out.lines().into_iter().find(|v| f(v)) {
                return v;
            }
            assert!(Instant::now() < until, "{:?}", out.lines());
            std::thread::sleep(Duration::from_millis(10));
        }
    };
    send(init());
    send(call(1, "apps.watch", json!({"exe": "notepad.exe"})));
    assert!(
        err(1, code::DENIED)(&wait(&|v| v["id"] == 1)),
        "only the granted app"
    );
    send(call(2, "apps.watch", json!({"exe": "HOST-TEST.exe"})));
    assert_eq!(
        wait(&|v| v["id"] == 2)["result"],
        json!({"exe": "host-test.exe", "open": 0})
    );
    assert_eq!(apps::shown(-5, "host-test.exe"), 1);
    assert_eq!(
        wait(&|v| v["method"] == "app.window.appeared")["params"],
        json!({"exe": "host-test.exe", "open": 1})
    );
    assert_eq!(apps::destroyed(-5), 1);
    assert_eq!(
        wait(&|v| v["method"] == "app.window.closed")["params"],
        json!({"exe": "host-test.exe", "open": 0})
    );
    // The watch ends with the connection.
    drop(tx);
    server.join().unwrap();
    assert_eq!(apps::shown(-6, "host-test.exe"), 0);
}

#[test]
fn tools_cant_watch_apps() {
    let out = run(
        plugin(&["apps.watch:obs64.exe"]),
        &[init(), call(1, "apps.watch", json!({"exe": "obs64.exe"}))],
    );
    assert!(err(1, code::DENIED)(&out[1]));
}

#[test]
fn a_plugin_reads_its_own_settings_and_tools_none() {
    set_settings_source(|plugin| {
        let mut m = serde_json::Map::new();
        m.insert("plugin".into(), json!(plugin));
        m
    });
    let get = call(1, "settings.get", json!({}));
    let out = Out::default();
    let text: String = [init(), get.clone()]
        .iter()
        .map(|v| v.to_string() + "\n")
        .collect();
    serve_with(
        Cursor::new(text),
        out.clone(),
        plugin(&[]),
        Some("org.a".into()),
        |_| {},
    )
    .unwrap();
    assert_eq!(
        out.lines()[1]["result"],
        json!({"values": {"plugin": "org.a"}})
    );
    let out = run(plugin(&[]), &[init(), get]);
    assert!(
        err(1, code::DENIED)(&out[1]),
        "an outside tool has no settings"
    );
}
