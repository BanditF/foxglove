//! No device enumeration or OS keyboard injection in these tests.
use anyhow::Result;
use digitalis::{
    protocol::{Hello, REPORT},
    Transport,
};
use foxglove_lab::{
    behaviors::Catalog,
    report::{Report, Status},
    runner::{Device, Runner},
    spec::{self, Case, Step},
};
use std::{
    cell::RefCell,
    collections::{BTreeMap, VecDeque},
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
    time::Duration,
};

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!(
            "foxglove-lab-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[derive(Default)]
struct Fake {
    requests: RefCell<Vec<Vec<u8>>>,
    replies: RefCell<VecDeque<[u8; REPORT]>>,
    layers: RefCell<VecDeque<u32>>,
    after_invoke: RefCell<VecDeque<u32>>,
}
impl Transport for Fake {
    fn send(&self, _: &[u8]) -> Result<()> {
        panic!("unexpected unacknowledged send")
    }
    fn request(&self, r: &[u8], p: u8, c: u8, t: Duration) -> Result<[u8; REPORT]> {
        assert!(t <= Duration::from_secs(12));
        self.requests.borrow_mut().push(r.to_vec());
        let reply = self
            .replies
            .borrow_mut()
            .pop_front()
            .ok_or_else(|| anyhow::anyhow!("scripted transport timeout"))?;
        assert_eq!(&reply[..2], &[p, c | 0x80]);
        if p == 0xd7 && c == 2 {
            self.layers
                .borrow_mut()
                .extend(self.after_invoke.borrow_mut().drain(..));
        }
        Ok(reply)
    }
}
impl Device for Fake {
    fn layer(&self, _: Duration) -> Result<Option<u32>> {
        Ok(self.layers.borrow_mut().pop_front())
    }
}
fn reply(bytes: &[u8]) -> [u8; REPORT] {
    let mut r = [0; REPORT];
    r[..bytes.len()].copy_from_slice(bytes);
    r
}
fn case(steps: &str) -> Case {
    toml::from_str(&format!(
        "name='test'\ndescription='test description'\nsteps={steps}"
    ))
    .unwrap()
}
fn catalog() -> Catalog {
    Catalog {
        ids: BTreeMap::from([
            ("mo".into(), 42),
            ("kp".into(), 43),
            ("bootloader".into(), 44),
        ]),
        source: "fixture".into(),
    }
}
fn run<D: foxglove_lab::runner::Device>(fake: &D, case: &Case) -> foxglove_lab::report::CaseResult {
    run_with(fake, case, 20)
}
/// HELLO bits: 5 position press, 6 screen readback, 7 INVOKE refusal.
const TEST_HOOKS: u32 = 20 | 1 << 5 | 1 << 6 | 1 << 7;
fn run_with<D: foxglove_lab::runner::Device>(
    fake: &D,
    case: &Case,
    capabilities: u32,
) -> foxglove_lab::report::CaseResult {
    let cat = catalog();
    let dir = Temp::new();
    let mut r = Runner::new(
        fake,
        &cat,
        Hello {
            version: 1,
            capabilities,
            ..Default::default()
        },
        Arc::new(AtomicBool::new(false)),
        true,
        &dir.0,
    );
    r.case(case, true)
}

#[test]
fn parses_seed_suites_and_all_step_kinds() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/hil");
    assert_eq!(spec::load(&root.join("smoke.toml")).unwrap().cases.len(), 6);
    assert_eq!(
        spec::load(&root.join("release.toml")).unwrap().cases.len(),
        19
    );
    for text in [
        "[{press_position={pos=12,state='down'}}]",
        "[{screen={half='left',golden='golden/a.png',mask=[{x=0,y=0,width=4,height=5}]}}]",
    ] {
        for s in case(text).steps {
            s.validate().unwrap();
        }
    }
}

#[test]
fn validates_bad_specs() {
    for text in [
        "[{invoke={behavior='mo',local_id=4}}]",
        "[{invoke={behavior='mo',hold_ms=10001}}]",
        "[{invoke={behavior='mo',expect_status='maybe'}}]",
        "[{expect_layer={top=32}}]",
        "[{expect_layer={top=0,within_ms=0}}]",
        "[{expect_conn={}}]",
        "[{screen={half='middle',golden='a.png'}}]",
        "[{screen={half='left',golden='a.png',mask=[{x=159,y=0,width=2,height=1}]}}]",
        "[{press_position={pos=48,state='down'}}]",
        "[{type_into_sink={text='a',expect='a',via=[{invoke={behavior='kp',hold_ms=1}}]}}]",
    ] {
        assert!(
            case(text).steps.iter().any(|s| s.validate().is_err()),
            "{text}"
        );
    }
    assert!(toml::from_str::<Case>(
        "name='a'\ndescription='b'\nsteps=[{invoke={behavior='mo',typo=1}}]"
    )
    .is_err());
    let dir = Temp::new();
    for text in [
        "schema=2",
        "schema=1\ninclude=['suite.toml']",
        "schema=1\ncases=[]",
    ] {
        let path = dir.0.join("suite.toml");
        std::fs::write(&path, text).unwrap();
        assert!(spec::load(&path).is_err());
    }
}

#[test]
fn invoke_and_layer_pass_with_report_buffered_during_request() {
    let fake = Fake::default();
    fake.replies
        .borrow_mut()
        .push_back(reply(&[0xd7, 0x82, 0, 42, 0]));
    fake.after_invoke.borrow_mut().extend([0b1001, 1]);
    let c=case("[{invoke={behavior='mo',param1=3}}, {expect_layer={active_contains=[3]}}, {expect_layer={top=0}}]");
    let result = run(&fake, &c);
    assert_eq!(result.status, Status::Pass);
    let requests = fake.requests.borrow();
    assert_eq!(&requests[0][..8], &[0xd7, 2, 42, 0, 3, 0, 0, 0]);
}

#[test]
fn invoke_status_failure_skips_later_steps_and_timeout_fails() {
    let fake = Fake::default();
    fake.replies
        .borrow_mut()
        .push_back(reply(&[0xd7, 0x82, 2, 42, 0]));
    let c = case("[{invoke={local_id=42}}, {expect_layer={top=0}}]");
    let result = run(&fake, &c);
    assert_eq!(result.status, Status::Fail);
    assert_eq!(result.steps[1].status, Status::Skipped);
    assert!(result.steps[0].actual.contains("busy"));
    let result = run(
        &Fake::default(),
        &case("[{expect_layer={top=0,within_ms=1}}]"),
    );
    assert_eq!(result.status, Status::Fail);
    assert!(result.message.contains("timeout"));
    assert_eq!(
        run(&Fake::default(), &case("[{invoke={behavior='mo'}}]")).status,
        Status::Fail
    );
}

#[test]
fn connection_and_rgb_match_scripted_replies() {
    let fake = Fake::default();
    let mut hello = reply(&[0xd5, 0x81, 1]);
    hello[7] = 2;
    let mut rgb = reply(&[0xd7, 0x83]);
    rgb[9] = 4;
    fake.replies
        .borrow_mut()
        .extend([reply(&[0xd7, 0x84, 0, 0, 0, 0, 1, 4]), hello, rgb]);
    let result = run(
        &fake,
        &case(
            "[{expect_conn={usb_or_ble=true,right_link=true}}, {expect_rgb={right_age_max_s=5}}]",
        ),
    );
    assert_eq!(result.status, Status::Pass);
}

#[test]
fn capabilities_and_refusal_gate_before_any_request() {
    for (requires, steps) in [
        (
            vec!["invoke_refusal"],
            "[{invoke={behavior='bootloader',expect_status='refused'}}]",
        ),
        (vec![], "[{invoke={local_id=44,expect_status='refused'}}]"),
        (vec!["host_link>=2"], "[{invoke={behavior='mo'}}]"),
        (
            vec![],
            "[{invoke={behavior='mo'}},{screen={half='left',golden='a.png'}}]",
        ),
        (vec![], "[{press_position={pos=12,state='down'}}]"),
        (vec![], "[{prompt={ask='Reset then y/n'}}]"),
    ] {
        let fake = Fake::default();
        let mut c = case(steps);
        c.requires = requires.into_iter().map(str::to_string).collect();
        let result = run(&fake, &c);
        assert_eq!(result.status, Status::Blocked);
        assert!(fake.requests.borrow().is_empty());
    }
    // A disruptive explicit id cannot bypass the gate by expecting success.
    let fake = Fake::default();
    assert_eq!(
        run(&fake, &case("[{invoke={local_id=44}}]")).status,
        Status::Fail
    );
    assert!(fake.requests.borrow().is_empty());
}

#[test]
fn cancellation_is_reported_without_injection() {
    let fake = Fake::default();
    let cat = catalog();
    let dir = Temp::new();
    let mut runner = Runner::new(
        &fake,
        &cat,
        Hello::default(),
        Arc::new(AtomicBool::new(true)),
        true,
        &dir.0,
    );
    assert_eq!(
        runner
            .case(&case("[{invoke={behavior='mo'}}]"), true)
            .status,
        Status::Fail
    );
    assert!(fake.requests.borrow().is_empty());
}

#[test]
fn reports_escape_markup_embed_images_and_apply_release_gate() {
    let dir = Temp::new();
    let mut c = case("[{prompt={ask='answer <y> & n'}}]");
    c.name = "<script>\"&".into();
    c.tags = vec!["release".into()];
    let mut blocked = run(&Fake::default(), &c);
    blocked.steps[0].attachments.push("screen.png".into());
    std::fs::write(
        dir.0.join("screen.png"),
        foxglove_lab::screen::png(&foxglove_lab::screen::portrait(&vec![0; 1360]).unwrap())
            .unwrap(),
    )
    .unwrap();
    let mut report = Report {
        schema: 1,
        environment: BTreeMap::from([("runner_version".into(), "test".into())]),
        firmware: serde_json::json!({"hello_version":1}),
        cases: vec![blocked],
    };
    assert!(!report.overall_pass());
    assert!(!report.exit_success());
    report.write(&dir.0).unwrap();
    let json: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.0.join("report.json")).unwrap()).unwrap();
    assert_eq!(json["cases"][0]["status"], "BLOCKED");
    let junit = std::fs::read_to_string(dir.0.join("junit.xml")).unwrap();
    assert!(junit.contains("<skipped message="));
    assert!(junit.contains("&lt;script&gt;&quot;&amp;"));
    let html = std::fs::read_to_string(dir.0.join("report.html")).unwrap();
    assert!(html.contains("data:image/png;base64,"));
    assert!(!html.contains("<script>"));
    assert!(html.find("<table>").unwrap() < html.find("<article>").unwrap());
    report.cases[0].tags.clear();
    assert!(report.overall_pass());
    report.cases[0].manual = false;
    assert!(!report.exit_success());
    report.cases[0].status = Status::Fail;
    assert!(!report.overall_pass());
    report.write(&dir.0).unwrap();
    assert!(std::fs::read_to_string(dir.0.join("junit.xml"))
        .unwrap()
        .contains("<failure message="));
}

#[test]
fn typing_validation_prevents_non_keypress_injection() {
    let fake = Fake::default();
    let result = run(
        &fake,
        &case("[{type_into_sink={text='a',expect='a',via=[{invoke={behavior='mo'}}]}}]"),
    );
    assert_eq!(result.status, Status::Fail);
    assert!(fake.requests.borrow().is_empty());
    let Step::TypeIntoSink(_) =
        &case("[{type_into_sink={text='a',expect='a',via=[{invoke={behavior='kp'}}]}}]").steps[0]
    else {
        panic!()
    };
}

#[test]
fn real_link_routes_scripted_layer_reports_and_rejects_reconnection() {
    use digitalis::link::{Connected, Link, RawHid};
    use foxglove_lab::runner::Hardware;
    use std::sync::Mutex;
    // Blocks like the real reader: waits for a scripted report or for `interrupt`.
    #[derive(Default)]
    struct Queue {
        reports: VecDeque<[u8; REPORT]>,
        stopped: bool,
    }
    struct Wire(Arc<(Mutex<Queue>, std::sync::Condvar)>);
    impl RawHid for Wire {
        fn read(&self, buf: &mut [u8; REPORT]) -> Result<usize> {
            let (lock, ready) = &*self.0;
            let mut q = lock.lock().unwrap();
            loop {
                if let Some(r) = q.reports.pop_front() {
                    *buf = r;
                    return Ok(REPORT);
                }
                if q.stopped {
                    return Ok(0);
                }
                q = ready.wait(q).unwrap();
            }
        }
        fn interrupt(&self) {
            let (lock, ready) = &*self.0;
            lock.lock().unwrap().stopped = true;
            ready.notify_all();
        }
        fn write(&self, r: &[u8]) -> Result<()> {
            assert_eq!(&r[..2], &[0xd7, 2]);
            let (lock, ready) = &*self.0;
            let mut q = lock.lock().unwrap();
            // Four-byte little-endian default and active masks. Layer 17 is active.
            q.reports
                .push_back(reply(&[0xff, 4, 1, 0, 0, 0, 0, 0, 2, 0]));
            q.reports.push_back(reply(&[0xd7, 0x82, 0, 42, 0]));
            ready.notify_all();
            Ok(())
        }
    }
    let inbox = Arc::new((Mutex::new(Queue::default()), std::sync::Condvar::new()));
    let source = inbox.clone();
    let link = Link::start(Box::new(move || {
        Ok((
            Box::new(Wire(source.clone())) as Box<dyn RawHid>,
            Connected {
                product: "scripted".into(),
                hello: Some(Hello {
                    version: 1,
                    ..Default::default()
                }),
            },
        ))
    }));
    link.wait_connected(Duration::from_secs(1)).unwrap();
    let hardware = Hardware::new(link);
    let result = run(
        &hardware,
        &case("[{invoke={behavior='mo',param1=17}},{expect_layer={active_contains=[17],top=17}}]"),
    );
    assert_eq!(result.status, Status::Pass);
    inbox.0.lock().unwrap().reports.push_back(reply(&[0xff, 0]));
    inbox.1.notify_all();
    assert!(hardware.layer(Duration::from_secs(1)).is_err());
    let generation = hardware.link.state().generation;
    hardware.link.reconnect();
    let end = std::time::Instant::now() + Duration::from_secs(1);
    while hardware.link.state().generation == generation && std::time::Instant::now() < end {
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(hardware.layer(Duration::ZERO).is_err());
}

#[test]
fn failed_behavior_and_busy_keep_prior_release_deadline() {
    let fake = Fake::default();
    fake.replies.borrow_mut().extend([
        reply(&[0xd7, 0x82, 0, 42, 0]),
        reply(&[0xd7, 0x82, 2, 42, 0]),
    ]);
    let started = std::time::Instant::now();
    let result = run(
        &fake,
        &case(
            "[{invoke={behavior='mo',hold_ms=80}},{invoke={behavior='mo',expect_status='busy'}}]",
        ),
    );
    assert_eq!(result.status, Status::Pass);
    assert!(started.elapsed() >= Duration::from_millis(80));
    let fake = Fake::default();
    fake.replies
        .borrow_mut()
        .push_back(reply(&[0xd7, 0x82, 3, 42, 0]));
    let started = std::time::Instant::now();
    assert_eq!(
        run(&fake, &case("[{invoke={behavior='mo',hold_ms=20}}]")).status,
        Status::Fail
    );
    assert!(started.elapsed() >= Duration::from_millis(20));
}

/// Art status reply: left state, error, length, pushed, link, then the right's relay status
/// with `set` at byte 15 and the built-in set count at byte 16.
fn art(cmd: u8, set: i8, count: u8) -> [u8; REPORT] {
    let mut r = [0u8; 17];
    r[..2].copy_from_slice(&[0xd6, 0x80 | cmd]);
    r[8] = 1;
    r[15] = set as u8;
    r[16] = count;
    reply(&r)
}

#[test]
fn art_select_refuses_to_lose_pushed_art() {
    let fake = Fake::default();
    fake.replies
        .borrow_mut()
        .extend([hello(2, 4), art(5, -1, 4), art(5, -1, 4)]);
    let result = run(&fake, &case("[{art_select={set=1}}]"));
    assert_eq!(result.status, Status::Blocked);
    assert!(result.message.contains("pushed art"));
}

#[test]
fn art_select_checks_and_restores() {
    let fake = Fake::default();
    // status ×2 (before), select, status (shows 1), restore select.
    fake.replies.borrow_mut().extend([
        hello(2, 4),
        art(5, 0, 4),
        art(5, 0, 4),
        art(4, 1, 4),
        art(5, 1, 4),
        art(4, 0, 4),
    ]);
    let result = run(&fake, &case("[{art_select={set=1}}]"));
    assert_eq!(result.status, Status::Pass, "{}", result.message);
    assert!(result.steps[0].actual.contains("restored set 0"));
    let fake = Fake::default();
    fake.replies
        .borrow_mut()
        .extend([hello(2, 4), art(5, 0, 2), art(5, 0, 2)]);
    assert_eq!(
        run(&fake, &case("[{art_select={set=3}}]")).status,
        Status::Fail
    );
}

fn position(status: u8, pos: u8) -> [u8; REPORT] {
    reply(&[0xd7, 0x85, status, pos])
}

#[test]
fn position_press_needs_capability_bits_and_releases_held_keys() {
    let steps = "[{press_position={pos=43,state='down'}},{expect_layer={active_contains=[1]}}]";
    let fake = Fake::default();
    assert_eq!(run(&fake, &case(steps)).status, Status::Blocked);
    assert!(
        fake.requests.borrow().is_empty(),
        "nothing may be sent without bits 5 and 7"
    );
    let fake = Fake::default();
    fake.replies
        .borrow_mut()
        .extend([position(0, 43), position(0, 43)]);
    fake.layers.borrow_mut().push_back(0b11);
    assert_eq!(
        run_with(&fake, &case(steps), TEST_HOOKS).status,
        Status::Pass
    );
    // The held position is released at the end of the case.
    assert_eq!(fake.requests.borrow()[1], vec![0xd7, 0x05, 43, 0]);
}

#[test]
fn position_refusal_and_tap() {
    let fake = Fake::default();
    fake.replies.borrow_mut().push_back(position(6, 31));
    let refused = "[{press_position={pos=31,state='down',expect_status='refused'}}]";
    assert_eq!(
        run_with(&fake, &case(refused), TEST_HOOKS).status,
        Status::Pass
    );
    let fake = Fake::default();
    fake.replies
        .borrow_mut()
        .extend([position(0, 1), position(0, 1)]);
    let tap = "[{press_position={pos=1,state='tap',hold_ms=5}}]";
    assert_eq!(run_with(&fake, &case(tap), TEST_HOOKS).status, Status::Pass);
    assert_eq!(fake.requests.borrow().len(), 2);
    let fake = Fake::default();
    fake.replies.borrow_mut().push_back(position(6, 1));
    assert_eq!(run_with(&fake, &case(tap), TEST_HOOKS).status, Status::Fail);
}

/// Scripted SCREEN_SNAP + SCREEN_READ replies for a native image.
fn snapshot_replies(half: u8, image: &[u8], crc: u32) -> Vec<[u8; REPORT]> {
    let mut meta = vec![0xd7, 0x86, 0, half, 1, 0, 160, 68];
    meta.extend(1360u16.to_le_bytes());
    meta.extend(crc.to_le_bytes());
    let mut out = vec![reply(&meta)];
    for (i, chunk) in image.chunks(REPORT - 7).enumerate() {
        let mut r = vec![0xd7, 0x87, 0, half];
        r.extend((((REPORT - 7) * i) as u16).to_le_bytes());
        r.push(chunk.len() as u8);
        r.extend(chunk);
        out.push(reply(&r));
    }
    out
}

#[test]
fn snapshot_reads_checks_crc_and_rotates() {
    let mut image = vec![0u8; 1360];
    image[0] = 0x80;
    let fake = Fake::default();
    fake.replies
        .borrow_mut()
        .extend(snapshot_replies(1, &image, crc32fast::hash(&image)));
    let bits = foxglove_lab::screen::snapshot(&fake, 1).unwrap();
    assert_eq!(bits, image);
    let fake = Fake::default();
    fake.replies
        .borrow_mut()
        .extend(snapshot_replies(0, &image, 1));
    assert!(foxglove_lab::screen::snapshot(&fake, 0)
        .unwrap_err()
        .to_string()
        .contains("CRC"));
}

#[test]
fn screen_without_golden_is_blocked_with_a_candidate() {
    let image = vec![0u8; 1360];
    let fake = Fake::default();
    fake.replies
        .borrow_mut()
        .extend(snapshot_replies(0, &image, crc32fast::hash(&image)));
    let steps = "[{screen={half='left',golden='golden/does-not-exist.png'}}]";
    assert_eq!(run(&fake, &case(steps)).status, Status::Blocked);
    let result = run_with(&fake, &case(steps), TEST_HOOKS);
    assert_eq!(result.status, Status::Blocked, "{}", result.message);
    assert!(result.message.contains("no golden"));
    assert_eq!(
        result.steps[0].attachments,
        ["does-not-exist.candidate.png"]
    );
}

/// HELLO reply with this right link state and relay discovery result.
fn hello(right_link: u8, discovery: u8) -> [u8; REPORT] {
    let mut r = [0u8; REPORT];
    r[..3].copy_from_slice(&[0xd5, 0x81, 1]);
    r[7] = right_link;
    r[9] = discovery;
    r
}

#[test]
fn right_half_steps_wait_for_a_fully_up_relay() {
    // #122: the split peer is connected (link 1) but the relay never came up. Nothing may be
    // pressed on the right; the step is BLOCKED, not failed.
    let fake = Fake::default();
    fake.replies.borrow_mut().push_back(hello(1, 4));
    let result = run_with(
        &fake,
        &case("[{press_position={pos=46,state='down'}}]"),
        TEST_HOOKS,
    );
    assert_eq!(result.status, Status::Blocked, "{}", result.message);
    assert!(result.message.contains("#122"));
    assert_eq!(fake.requests.borrow().len(), 1, "only the HELLO was sent");
    // Left positions don't ask.
    let fake = Fake::default();
    fake.replies
        .borrow_mut()
        .extend([position(0, 1), position(0, 1)]);
    let tap = "[{press_position={pos=1,state='tap',hold_ms=5}}]";
    assert_eq!(run_with(&fake, &case(tap), TEST_HOOKS).status, Status::Pass);
    assert!(foxglove_lab::runner::is_right(23) && !foxglove_lab::runner::is_right(17));
}
