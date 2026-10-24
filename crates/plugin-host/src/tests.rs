//! Golden transcripts: lines in, lines out, compared as JSON.

use super::*;
use std::io::Cursor;

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
        client: Client::Plugin(perms.iter().map(|p| Permission::try_from(p.to_string()).unwrap()).collect()),
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
    let out = run(plugin(&["facts.read:clock.*"]), &[call(1, "ping", json!({})), init(), call(2, "ping", json!({}))]);
    assert!(err(1, code::INVALID_REQUEST)(&out[0]));
    assert_eq!(
        out[1],
        json!({"jsonrpc":"2.0","id":0,"result":{"api":"1.2","app":"0.3.0","granted":["facts.read:clock.*"]}})
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
            call(5, "facts.set", json!({"key":"weather.icon","value":{"base64":"AAE="}})),
        ],
    );
    assert_eq!(out[1]["result"], json!({}));
    assert_eq!(out[2]["result"], json!({"value":14}));
    assert!(err(3, code::DENIED)(&out[3]));
    assert!(err(4, code::DENIED)(&out[4]), "device facts are never writable");
    assert_eq!(facts.get("weather.icon"), Some(DeviceValue::Bytes(vec![0, 1])));
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
            call(2, "behaviors.invoke", json!({"device":"d","id":1,"param1":0,"param2":0})),
            call(3, "no.such", json!({})),
            call(4, "facts.changed", json!({})),
        ],
    );
    assert_eq!(out[1]["result"], json!({}));
    assert!(err(2, code::DENIED)(&out[2]));
    assert!(err(3, code::METHOD_NOT_FOUND)(&out[3]));
    assert!(err(4, code::METHOD_NOT_FOUND)(&out[4]), "events can't be called");
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
fn notifications_get_no_reply_and_bad_lines_get_parse_errors() {
    let out = run(
        plugin(&[]),
        &[init(), json!({"jsonrpc":"2.0","method":"ping"}), json!("not a request"), call(9, "ping", json!(null))],
    );
    assert_eq!(out.len(), 3);
    assert!(err(0, code::PARSE_ERROR)(&out[1]));
    assert_eq!(out[2]["id"], json!(9));
}

#[test]
fn floods_are_rate_limited() {
    let mut ctx = plugin(&[]);
    ctx.limits = Limits { per_second: 0.0, burst: 3.0 };
    let out = run(ctx, &[init(), call(1, "ping", json!({})), call(2, "ping", json!({})), call(3, "ping", json!({}))]);
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
    for line in [init(), call(1, "facts.subscribe", json!({"pattern":"media.*"})), call(2, "facts.subscribe", json!({"pattern":"clock.*"}))] {
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
            serve(std::io::BufReader::new(ChanReader { rx, buf: vec![] }), writer, ctx).unwrap();
        });
        let live = Live { tx: Some(tx), out, server: Some(server) };
        live.send(init());
        live
    }

    fn send(&self, v: Value) {
        self.tx.as_ref().unwrap().send((v.to_string() + "\n").into_bytes()).unwrap();
    }

    /// Waits (up to 10 s) for a line matching `f`.
    fn wait(&self, f: impl Fn(&Value) -> bool) -> Value {
        let until = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(v) = self.out.lines().into_iter().find(|v| f(v)) {
                return v;
            }
            assert!(Instant::now() < until, "timed out; got {:?}", self.out.lines());
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
    let port = std::net::UdpSocket::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let live = Live::start(plugin(&[&format!("net.listen:udp:127.0.0.1:{port}")]));
    live.send(call(1, "net.open", json!({"kind":"udp","host":"127.0.0.1","port":port.wrapping_add(1)})));
    assert!(err(1, code::DENIED)(&live.wait(reply(1))), "another port is refused");
    live.send(call(2, "net.open", json!({"kind":"udp","host":"0.0.0.0","port":port})));
    assert!(err(2, code::DENIED)(&live.wait(reply(2))), "loopback only");
    live.send(call(3, "net.open", json!({"kind":"udp","host":"127.0.0.1","port":port})));
    let stream = live.wait(reply(3))["result"]["stream"].as_u64().unwrap();
    std::net::UdpSocket::bind("127.0.0.1:0").unwrap().send_to(b"hello", ("127.0.0.1", port)).unwrap();
    let data = live.wait(|v| v["method"] == "net.data");
    assert_eq!(data["params"]["stream"], json!(stream));
    assert_eq!(data["params"]["data"], json!(STANDARD.encode(b"hello")));
    live.stop();
    // The connection ended, so the port is free again.
    assert!(std::net::UdpSocket::bind(("127.0.0.1", port)).is_ok() || {
        std::thread::sleep(Duration::from_millis(500));
        std::net::UdpSocket::bind(("127.0.0.1", port)).is_ok()
    });
}

#[test]
fn net_tcp_connect_both_ways() {
    let server = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = server.local_addr().unwrap().port();
    let live = Live::start(plugin(&[&format!("net.connect:127.0.0.1:{port}")]));
    live.send(call(1, "net.open", json!({"kind":"tcp","host":"localhost","port":port})));
    assert!(err(1, code::DENIED)(&live.wait(reply(1))), "the grant names 127.0.0.1, not localhost");
    live.send(call(2, "net.open", json!({"kind":"tcp","host":"127.0.0.1","port":port})));
    let (mut peer, _) = server.accept().unwrap();
    let stream = live.wait(reply(2))["result"]["stream"].as_u64().unwrap();
    live.send(call(3, "net.send", json!({"stream":stream,"data":STANDARD.encode(b"ping")})));
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
