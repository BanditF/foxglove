//! No device enumeration or OS keyboard injection in these tests.
use anyhow::Result;
use foxglove_lab::{
    behaviors::Catalog,
    report::{Report, Status},
    runner::{Device, Runner},
    spec::{self, Case, Step},
};
use foxglove_link::{
    protocol::{Hello, REPORT},
    Transport,
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
        ..Default::default()
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
    assert_eq!(spec::load(&root.join("smoke.toml")).unwrap().cases.len(), 7);
    assert_eq!(
        spec::load(&root.join("resilience.toml"))
            .unwrap()
            .cases
            .len(),
        14
    );
    assert_eq!(
        spec::load(&root.join("app-actions.toml"))
            .unwrap()
            .cases
            .len(),
        1
    );
    assert!(spec::non_typing_key(0x0007_0068) && spec::non_typing_key(0x0007_0073));
    // F12, F13 with left shift, and A all type or change typing.
    assert!(![0x0007_0045, 0x0207_0068, 0x0007_0004]
        .into_iter()
        .any(spec::non_typing_key));
    assert_eq!(
        spec::load(&root.join("release.toml")).unwrap().cases.len(),
        24
    );
    for text in [
        "[{press_position={pos=12,state='down'}}]",
        "[{screen={half='left',golden='golden/a.png',mask=[{x=0,y=0,width=4,height=5}]}}]",
        "[{expect_rgb={right_age_max_s=2,left_on=false,right_on=false,within_ms=45000}}]",
        "[{expect_screen_still={half='right',region={x=0,y=20,width=68,height=140},ms=3000}}]",
        "[{idle_cpu={seconds=20,max_cpu_ms=60,settle_s=120}}]",
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
        "[{idle_cpu={seconds=20,max_cpu_ms=60,settle_s=601}}]",
        "[{expect_layer={top=0,within_ms=0}}]",
        "[{expect_conn={}}]",
        "[{screen={half='middle',golden='a.png'}}]",
        "[{screen={half='left',golden='a.png',mask=[{x=159,y=0,width=2,height=1}]}}]",
        "[{press_position={pos=48,state='down'}}]",
        "[{expect_rgb={right_age_max_s=2,within_ms=120001}}]",
        "[{expect_screen_still={half='right',ms=0}}]",
        "[{expect_screen_still={half='right',region={x=0,y=20,width=69,height=1},ms=10}}]",
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
fn rgb_on_off_is_checked_per_half() {
    // RGB_STATE: the left on; the right's reading 1 s old, off (#603).
    let mut rgb = reply(&[0xd7, 0x83, 1]);
    rgb[9] = 1;
    let fake = Fake::default();
    fake.replies.borrow_mut().push_back(rgb);
    let result = run(
        &fake,
        &case("[{expect_rgb={right_age_max_s=2,left_on=true,right_on=false}}]"),
    );
    assert_eq!(result.status, Status::Pass);
    for steps in [
        "[{expect_rgb={right_age_max_s=2,right_on=true,within_ms=1}}]",
        "[{expect_rgb={right_age_max_s=2,left_on=false,within_ms=1}}]",
        "[{expect_rgb={right_age_max_s=0,right_on=false,within_ms=1}}]",
    ] {
        let fake = Fake::default();
        fake.replies.borrow_mut().push_back(rgb);
        assert_eq!(run(&fake, &case(steps)).status, Status::Fail, "{steps}");
    }
}

/// #618: wait_relay reads HELLO until the left's relay is up (right link 2, discovery done), and
/// fails "relay not up" when it never comes within its time.
#[test]
fn wait_relay_waits_for_the_relay_and_names_it_when_it_never_comes() {
    let mut up = reply(&[0xd5, 0x81, 1]);
    up[7] = 2; // right link: relay up
    up[9] = 4; // discovery done
    let mut down = reply(&[0xd5, 0x81, 1]);
    down[7] = 1; // connected, relay not found yet
    down[9] = 1; // discovery running
    let fake = Fake::default();
    fake.replies.borrow_mut().extend([down, down, up]);
    let result = run_with(&fake, &case("[{wait_relay={within_ms=5000}}]"), 0);
    assert_eq!(result.status, Status::Pass, "{}", result.message);
    assert!(result.steps[0].actual.starts_with("relay up after"));
    let fake = Fake::default();
    fake.replies.borrow_mut().extend([down, down, down, down]);
    let result = run_with(&fake, &case("[{wait_relay={within_ms=600}}]"), 0);
    assert_eq!(result.status, Status::Fail);
    assert!(
        result
            .message
            .contains("relay not up within 600 ms (right link 1, discovery 1"),
        "{}",
        result.message
    );
}

/// D7 8E with the right's redraw count (#618): `known` byte 16, the count in bytes 17-20.
fn dim_with_redraws(known: bool, n: u32) -> [u8; REPORT] {
    let mut r = reply(&[0xd7, 0x8e, 0]);
    r[11] = 1; // the right's dim state is 1 s old
    r[16] = u8::from(known);
    r[17..21].copy_from_slice(&n.to_le_bytes());
    r
}

/// #618: note_right_redraws reads the count twice (the second is fresh), and
/// expect_right_redraws passes once it grew by at_least; a right without it is BLOCKED, and a
/// count that doesn't grow fails.
#[test]
fn right_redraws_are_noted_and_must_grow() {
    const REDRAWS: u32 = 1 << 25;
    let mut hello = reply(&[0xd5, 0x81, 1]);
    hello[7] = 2; // right link: relay up
    hello[9] = 4; // discovery done
    let steps = "[{note_right_redraws={}}, {expect_right_redraws={at_least=2,within_ms=3000}}]";
    let fake = Fake::default();
    fake.replies.borrow_mut().extend([
        hello,
        dim_with_redraws(true, 3),
        dim_with_redraws(true, 4), // still settling from the idle-off's redraw
        dim_with_redraws(true, 5),
        dim_with_redraws(true, 5),
        hello, // expect_right_redraws notes the relay's state before it waits
        dim_with_redraws(true, 6),
        dim_with_redraws(true, 7),
    ]);
    let result = run_with(&fake, &case(steps), REDRAWS);
    assert_eq!(result.status, Status::Pass, "{}", result.message);
    assert_eq!(result.steps[0].actual, "right redraws 5");
    assert!(result.steps[1].actual.starts_with("right redraws 5 -> 7"));
    let requests = fake.requests.borrow();
    assert!(
        requests[1..]
            .iter()
            .all(|r| r[..3] == [0xd7, 0x0e, 0] || r[..2] == [0xd5, 0x01]),
        "reads only (D7 0E, and HELLO for the relay's state)"
    );
    // A right before #618 reports nothing: BLOCKED, not a failure.
    let fake = Fake::default();
    fake.replies.borrow_mut().extend([
        hello,
        dim_with_redraws(false, 0),
        dim_with_redraws(false, 0),
    ]);
    assert_eq!(
        run_with(&fake, &case(steps), REDRAWS).status,
        Status::Blocked
    );
    // No rewrite after the wake: fails at the timeout, with HELLO read before and after the wait.
    let c = case("[{note_right_redraws={}}, {expect_right_redraws={within_ms=500}}]");
    let no_rewrite = |before: [u8; 32], after: [u8; 32]| {
        let fake = Fake::default();
        let mut replies = fake.replies.borrow_mut();
        replies.push_back(hello);
        replies.extend(std::iter::repeat_n(dim_with_redraws(true, 5), 3));
        replies.push_back(before);
        replies.extend(std::iter::repeat_n(dim_with_redraws(true, 5), 2));
        replies.push_back(after);
        drop(replies);
        run_with(&fake, &c, REDRAWS)
    };
    let result = no_rewrite(hello, hello);
    assert_eq!(result.status, Status::Fail);
    assert!(
        result.message.contains("expected at least +1") && !result.message.contains("stuck"),
        "{}",
        result.message
    );
    // #618 soak: a relay request in flight the whole wait, none answered: the lab says so.
    let mut stuck = hello;
    stuck[11..21].copy_from_slice(&[3, 1, 0, 0, 1, 23, 1, 2, 23, 0]);
    let result = no_rewrite(stuck, stuck);
    assert_eq!(result.status, Status::Fail);
    assert!(
        result.message.contains("the left's relay is stuck"),
        "{}",
        result.message
    );
    // Without HELLO bit 25 nothing is asked.
    let fake = Fake::default();
    assert_eq!(run_with(&fake, &case(steps), 0).status, Status::Blocked);
    assert!(fake.requests.borrow().is_empty());
}

#[test]
fn rgb_look_and_shown_are_checked_per_half() {
    // RGB_STATE: both halves Swirl, hue 39, sat 70, 42 % (#624), each saying what it shows (#605).
    let mut rgb = reply(&[
        0xd7, 0x83, 1, 39, 0, 70, 42, 3, 3, 0, 1, 39, 0, 70, 42, 3, 3,
    ]);
    rgb[17] = 0b11; // valid, lit
    rgb[23] = 0b11;
    let pass = "[{expect_rgb={right_age_max_s=2,shown=true,right_same_look=true,left_on=true,right_on=true,left_look={hue=39,sat=70,brightness=42,effect=3}}}]";
    let fake = Fake::default();
    fake.replies.borrow_mut().push_back(rgb);
    assert_eq!(run(&fake, &case(pass)).status, Status::Pass);
    // The right kept another look (Spectrum, 9 %: the #624 state).
    let mut stale = rgb;
    stale[14] = 9;
    stale[15] = 2;
    // The right not heard from on this link: no shown block for it.
    let mut unheard = rgb;
    unheard[23] = 0;
    for (r, steps) in [
        (
            rgb,
            "[{expect_rgb={right_age_max_s=2,left_look={hue=40,sat=70,brightness=42,effect=3},within_ms=1}}]",
        ),
        (
            stale,
            "[{expect_rgb={right_age_max_s=2,right_same_look=true,within_ms=1}}]",
        ),
        (
            unheard,
            "[{expect_rgb={right_age_max_s=2,shown=true,within_ms=1}}]",
        ),
    ] {
        let fake = Fake::default();
        fake.replies.borrow_mut().push_back(r);
        assert_eq!(run(&fake, &case(steps)).status, Status::Fail, "{steps}");
    }
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
        // HELLO bit 23 (#603): the left relays its activity to the right.
        (vec!["activity_relay"], "[{expect_layer={top=0}}]"),
        // HELLO bit 24 (#605): RGB_STATE says what the LEDs show.
        (vec!["rgb_shown"], "[{expect_layer={top=0}}]"),
        // HELLO bit 25 (#618): the right's redraw count.
        (vec!["right_redraws"], "[{expect_layer={top=0}}]"),
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
    let mut report = Report::new(
        "release",
        "2026-10-24T00:00:00Z".into(),
        BTreeMap::from([("runner_version".into(), "test".into())]),
    );
    report.firmware.hello = serde_json::json!({"hello_version":1});
    report.cases.push(blocked);
    assert!(!report.overall_pass());
    assert!(!report.exit_success());
    // A blocked release case with nothing failed is BLOCKED (exit 2), not FAIL.
    assert_eq!(report.verdict(), Status::Blocked);
    assert_eq!(report.exit_code(), 2);
    report.write(&dir.0).unwrap();
    let json: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.0.join("report.json")).unwrap()).unwrap();
    assert_eq!(json["cases"][0]["status"], "BLOCKED");
    // Stable v2 shape (docs/test-lab/report.schema.json lists these as required).
    let schema: serde_json::Value =
        serde_json::from_str(include_str!("../../../docs/test-lab/report.schema.json")).unwrap();
    for key in schema["required"].as_array().unwrap() {
        assert!(
            json.get(key.as_str().unwrap()).is_some(),
            "report.json lacks {key}"
        );
    }
    assert_eq!(json["schema"], "foxglove-lab-report");
    assert_eq!(json["version"], 2);
    assert_eq!(json["verdict"], "BLOCKED");
    assert_eq!(json["areas"][0]["name"], "general");
    assert_eq!(json["areas"][0]["verdict"], "BLOCKED");
    assert_eq!(json["cases"][0]["area"], "general");
    assert!(json["firmware"]["left"].is_null());
    let junit = std::fs::read_to_string(dir.0.join("junit.xml")).unwrap();
    assert!(junit.contains("<skipped message="));
    assert!(junit.contains("<testsuites name=\"foxglove-lab\""));
    assert!(junit.contains("classname=\"foxglove-lab.general\""));
    assert!(junit.contains("<property name=\"verdict\" value=\"BLOCKED\"/>"));
    assert!(junit.contains("&lt;script&gt;&quot;&amp;"));
    let html = std::fs::read_to_string(dir.0.join("report.html")).unwrap();
    assert!(html.contains("data:image/png;base64,"));
    assert!(!html.contains("<script>"));
    assert!(html.find("<table>").unwrap() < html.find("<article").unwrap());
    report.cases[0].tags.clear();
    assert!(report.overall_pass());
    report.cases[0].manual = false;
    assert!(!report.exit_success());
    assert_eq!(report.exit_code(), 2);
    report.cases[0].status = Status::Fail;
    assert!(!report.overall_pass());
    assert_eq!(report.verdict(), Status::Fail);
    assert_eq!(report.exit_code(), 1);
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
    use foxglove_lab::runner::Hardware;
    use foxglove_link::link::{Connected, Link, RawHid};
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
                bus: foxglove_link::link::Bus::Unknown,
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
    // status ×2 (before), select, status (shows 1), restore select, status (shows 0 again).
    fake.replies.borrow_mut().extend([
        hello(2, 4),
        art(5, 0, 4),
        art(5, 0, 4),
        art(4, 1, 4),
        art(5, 1, 4),
        art(4, 0, 4),
        art(5, 0, 4),
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

/// A Fake that also hears art relay reports it didn't ask for (another client's STATUS reads).
struct Overheard {
    fake: Fake,
    reports: RefCell<Vec<[u8; REPORT]>>,
}
impl Transport for Overheard {
    fn send(&self, r: &[u8]) -> Result<()> {
        self.fake.send(r)
    }
    fn request(&self, r: &[u8], p: u8, c: u8, t: Duration) -> Result<[u8; REPORT]> {
        self.fake.request(r, p, c, t)
    }
}
impl Device for Overheard {
    fn layer(&self, t: Duration) -> Result<Option<u32>> {
        self.fake.layer(t)
    }
    fn art_status_events(&self) -> Option<foxglove_lab::runner::Events> {
        let (tx, rx) = std::sync::mpsc::channel();
        for r in self.reports.borrow_mut().drain(..) {
            tx.send(r).unwrap();
        }
        // The sender stays open past the reports, as a live link's would.
        std::mem::forget(tx);
        Some(foxglove_lab::runner::Events::from_channel(rx))
    }
}

/// `art` with the left's relay in `state` (2 = pushing).
fn art_in(state: u8, cmd: u8, set: i8, count: u8) -> [u8; REPORT] {
    let mut r = art(cmd, set, count);
    r[2] = state;
    r
}

#[test]
fn art_select_waits_for_the_left_to_finish_another_job() {
    // #618: after a re-link the app re-applies the right's art, and the left reads PUSHING; a
    // SELECT then is refused without an error. The step waits on the reports that arrive (the
    // app's own STATUS reads) until one says the job ended, then selects.
    let fake = Overheard {
        fake: Fake::default(),
        reports: RefCell::new(vec![
            art_in(2, 5, 1, 4),            // the app's read: still pushing
            art_in(1, 5, 1, 4),            // a BEGIN from another client: still busy
            reply(&[0xd6, 0x87, 0, 3, 1]), // a SLOT reply: no relay status, ignored
            art_in(3, 5, 1, 4),            // done
        ]),
    };
    fake.fake.replies.borrow_mut().extend([
        hello(2, 4),
        art_in(2, 5, 1, 4), // fresh pair: busy
        art_in(2, 5, 1, 4),
        art_in(2, 5, 1, 4), // the read after subscribing: busy
        art(5, 1, 4),       // fresh pair again: idle, set 1
        art(5, 1, 4),
        art(4, 0, 4), // select 0
        art(5, 0, 4), // shows 0
    ]);
    let result = run(&fake, &case("[{art_select={set=0,restore=false}}]"));
    assert_eq!(result.status, Status::Pass, "{}", result.message);
    assert!(
        result.steps[0].actual.contains("the left was busy"),
        "{}",
        result.steps[0].actual
    );
    // One STATUS after subscribing, then nothing until the job ended: no reads on a timer.
    let selects = fake
        .fake
        .requests
        .borrow()
        .iter()
        .filter(|r| r[..2] == [0xd6, 4])
        .count();
    assert_eq!(selects, 1);
    assert_eq!(fake.fake.requests.borrow().len(), 8);

    // Still busy at the deadline: one last read, then FAIL (no SELECT sent).
    let fake = Overheard {
        fake: Fake::default(),
        reports: RefCell::new(vec![art_in(2, 5, 1, 4)]),
    };
    fake.fake.replies.borrow_mut().extend([
        hello(2, 4),
        art_in(2, 5, 1, 4),
        art_in(2, 5, 1, 4),
        art_in(2, 5, 1, 4),
        art_in(2, 5, 1, 4), // at the deadline
    ]);
    let started = std::time::Instant::now();
    let result = run(
        &fake,
        &case("[{art_select={set=0,restore=false,idle_within_ms=200}}]"),
    );
    assert_eq!(result.status, Status::Fail);
    assert!(result.message.contains("stayed busy"), "{}", result.message);
    assert!(started.elapsed() >= Duration::from_millis(200));
    assert!(!fake
        .fake
        .requests
        .borrow()
        .iter()
        .any(|r| r[..2] == [0xd6, 4]));
}

#[test]
fn art_status_reports_are_recognised() {
    use foxglove_lab::runner::is_art_status;
    assert!(is_art_status(&art(5, 0, 4)));
    assert!(is_art_status(&art(4, 0, 4)));
    assert!(!is_art_status(&reply(&[0xd6, 0x87, 0, 3])));
    assert!(!is_art_status(&reply(&[0xd6, 0x05])));
    assert!(!is_art_status(&reply(&[0xd7, 0x85])));
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

/// #618: a right snapshot that timed out in the relay (the app's traffic on the split link) is
/// taken once more; a second timeout is the error, and a left one isn't retried.
#[test]
fn a_right_snapshot_timeout_is_taken_once_more() {
    let mut image = vec![0u8; 1360];
    image[5] = 0x10;
    let timeout = |half| reply(&[0xd7, 0x86, 3, half]);
    let fake = Fake::default();
    fake.replies.borrow_mut().push_back(timeout(1));
    fake.replies
        .borrow_mut()
        .extend(snapshot_replies(1, &image, crc32fast::hash(&image)));
    assert_eq!(foxglove_lab::screen::snapshot(&fake, 1).unwrap(), image);
    let snaps = |f: &Fake| {
        f.requests
            .borrow()
            .iter()
            .filter(|r| r[..2] == [0xd7, 0x06])
            .count()
    };
    assert_eq!(snaps(&fake), 2);

    let fake = Fake::default();
    fake.replies.borrow_mut().extend([timeout(1), timeout(1)]);
    let e = foxglove_lab::screen::snapshot(&fake, 1).unwrap_err();
    assert!(e.to_string().contains("right half timed out"), "{e}");
    assert_eq!(snaps(&fake), 2);

    let fake = Fake::default();
    fake.replies.borrow_mut().push_back(timeout(0));
    assert!(foxglove_lab::screen::snapshot(&fake, 0).is_err());
    assert_eq!(snaps(&fake), 1);
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
        ["test-does-not-exist.candidate.png"]
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

fn hello_caps(caps: u32) -> [u8; REPORT] {
    let mut r = hello(2, 4);
    r[21..25].copy_from_slice(&caps.to_le_bytes());
    r
}
fn build_reply(half: u8, profile: u8, sha: &[u8]) -> [u8; REPORT] {
    let mut r = vec![0xd7, 0x89, 0, half, 0, 1, 0, profile];
    r.extend(sha);
    reply(&r)
}

#[test]
fn ga_gate_steps() {
    // HELLO bits: GA clears 5 and 9, keeps 7 and 12.
    let ga = (1u32 << 7) | (1 << 12) | (1 << 6);
    let fake = Fake::default();
    fake.replies.borrow_mut().push_back(hello_caps(ga));
    let hello = "[{expect_hello={caps_clear=[5,9],caps_set=[7,12]}}]";
    assert_eq!(run(&fake, &case(hello)).status, Status::Pass);
    let fake = Fake::default();
    fake.replies
        .borrow_mut()
        .push_back(hello_caps(ga | (1 << 5)));
    let r = run(&fake, &case(hello));
    assert_eq!(r.status, Status::Fail);
    assert!(r.message.contains("bit 5 set"));
    // Test hooks answer 4; ARM only, never TRIGGER; POSITION is a release.
    let fake = Fake::default();
    fake.replies
        .borrow_mut()
        .extend([reply(&[0xd7, 0x85, 4, 1]), reply(&[0xd7, 0x88, 4])]);
    let steps =
        "[{expect_not_built_in={command='position'}},{expect_not_built_in={command='bootloader'}}]";
    assert_eq!(run(&fake, &case(steps)).status, Status::Pass);
    assert_eq!(fake.requests.borrow()[0], vec![0xd7, 0x05, 1, 0]);
    assert_eq!(fake.requests.borrow()[1], vec![0xd7, 0x08, 0]);
    let fake = Fake::default();
    fake.replies
        .borrow_mut()
        .push_back(reply(&[0xd7, 0x85, 0, 1]));
    assert_eq!(
        run(&fake, &case("[{expect_not_built_in={command='position'}}]")).status,
        Status::Fail
    );
    // BUILD_INFO profile.
    let fake = Fake::default();
    fake.replies
        .borrow_mut()
        .extend([build_reply(0, 1, b"abc1234"), build_reply(1, 0, b"abc1234")]);
    let r = run(
        &fake,
        &case("[{expect_build={half='left',profile='ga',sha='abc1234'}},{expect_build={half='right',profile='ga'}}]"),
    );
    assert_eq!(r.status, Status::Fail);
    assert!(r.message.contains("right 0.1.0 dev"), "{}", r.message);
}

#[test]
fn hello_profile_and_split_guard_drops() {
    let mut r = hello_caps((1 << 7) | (1 << 12));
    r[27..31].copy_from_slice(&[0, 1, 0, 1]); // 0.1.0, profile 1 = GA
    r[31] = 0;
    let fake = Fake::default();
    fake.replies.borrow_mut().push_back(r);
    let steps = "[{expect_hello={profile='ga',guard_drops_max=0}}]";
    assert_eq!(run(&fake, &case(steps)).status, Status::Pass);
    r[30] = 0; // dev
    r[31] = 2;
    let fake = Fake::default();
    fake.replies.borrow_mut().push_back(r);
    let result = run(&fake, &case(steps));
    assert_eq!(result.status, Status::Fail);
    assert!(result.message.contains("expected ga") && result.message.contains("2 times"));
}

/// The left half in memory for the replug case (#374): it answers HELLO and the screen readback
/// (D7 06/07) from `screen`, and its cable can be pulled and plugged back in.
mod left_half {
    use super::*;
    use foxglove_link::link::{Connected, Link, RawHid};
    use std::sync::{Condvar, Mutex};

    #[derive(Default)]
    pub struct Board {
        /// The native 160x68 screen, 1 bpp, 20 bytes a row, MSB first, 1 = ink.
        pub screen: Mutex<Vec<u8>>,
        plugged: AtomicBool,
        /// The open connection's input.
        wire: Mutex<Option<Arc<Line>>>,
    }
    #[derive(Default)]
    struct Queue {
        reports: VecDeque<[u8; REPORT]>,
        stopped: bool,
        gone: bool,
    }
    type Line = (Mutex<Queue>, Condvar);
    /// One USB connection; a replug makes a new one.
    struct Wire {
        board: Arc<Board>,
        queue: Arc<Line>,
    }
    impl RawHid for Wire {
        fn read(&self, buf: &mut [u8; REPORT]) -> Result<usize> {
            let (lock, ready) = &*self.queue;
            let mut q = lock.lock().unwrap();
            loop {
                if let Some(r) = q.reports.pop_front() {
                    *buf = r;
                    return Ok(REPORT);
                }
                if q.stopped {
                    return Ok(0);
                }
                if q.gone {
                    anyhow::bail!("device gone");
                }
                q = ready.wait(q).unwrap();
            }
        }
        fn interrupt(&self) {
            let (lock, ready) = &*self.queue;
            lock.lock().unwrap().stopped = true;
            ready.notify_all();
        }
        fn write(&self, r: &[u8]) -> Result<()> {
            let (lock, ready) = &*self.queue;
            let mut q = lock.lock().unwrap();
            anyhow::ensure!(!q.gone, "device gone");
            let screen = self.board.screen.lock().unwrap().clone();
            let answer = match (r[0], r[1]) {
                (0xd5, 0x01) => hello_caps(1 << 6),
                (0xd7, 0x06) => {
                    let mut meta = vec![0xd7, 0x86, 0, r[2], 1, 0, 160, 68];
                    meta.extend(1360u16.to_le_bytes());
                    meta.extend(crc32fast::hash(&screen).to_le_bytes());
                    reply(&meta)
                }
                (0xd7, 0x07) => {
                    let at = usize::from(u16::from_le_bytes([r[3], r[4]]));
                    let chunk = &screen[at..(at + REPORT - 7).min(screen.len())];
                    let mut out = vec![0xd7, 0x87, 0, r[2], r[3], r[4], chunk.len() as u8];
                    out.extend(chunk);
                    reply(&out)
                }
                other => panic!("the replug case sent {other:x?}"),
            };
            q.reports.push_back(answer);
            ready.notify_all();
            Ok(())
        }
    }

    impl Board {
        /// A link to this half, as `foxglove-lab run` opens one.
        pub fn connect(self: &Arc<Self>) -> Link {
            self.plugged.store(true, Ordering::Relaxed);
            let board = self.clone();
            let link = Link::start(Box::new(move || {
                anyhow::ensure!(board.plugged.load(Ordering::Relaxed), "no keyboard");
                let queue = Arc::new((Mutex::new(Queue::default()), Condvar::new()));
                *board.wire.lock().unwrap() = Some(queue.clone());
                Ok((
                    Box::new(Wire {
                        board: board.clone(),
                        queue,
                    }) as Box<dyn RawHid>,
                    Connected {
                        product: "left half".into(),
                        hello: Some(Hello {
                            version: 1,
                            capabilities: 1 << 6,
                            ..Default::default()
                        }),
                        bus: foxglove_link::link::Bus::Unknown,
                    },
                ))
            }));
            link.wait_connected(Duration::from_secs(2)).unwrap();
            link
        }

        /// Pulls the cable: the open connection fails its reads and writes, and opening fails.
        pub fn unplug(&self) {
            self.plugged.store(false, Ordering::Relaxed);
            if let Some(queue) = self.wire.lock().unwrap().as_ref() {
                queue.0.lock().unwrap().gone = true;
                queue.1.notify_all();
            }
        }

        /// Plugs it back in; the device-arrival event is `Link::reconnect`.
        pub fn plug(&self, link: &Link) {
            self.plugged.store(true, Ordering::Relaxed);
            link.reconnect();
        }
    }
}

/// #374, the Test lab case for #350: `replug-hello` in resilience.toml, run against a simulated
/// left half over a real link. The cable is pulled and plugged back in (no key pressed); the run
/// that saw the replug refuses to go on, and a fresh run reads HELLO and must find Foxglove's
/// last frame on the left screen. A screen left blank after the replug (#350) fails.
#[test]
fn replug_hello_case_finds_the_redrawn_left_panel_after_a_replug() {
    use foxglove_lab::runner::Hardware;
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/hil");
    let mut suite = spec::load(&root.join("resilience.toml")).unwrap();
    let at = suite
        .cases
        .iter()
        .position(|c| c.name == "replug-hello")
        .unwrap();
    let case = suite.cases.swap_remove(at);
    assert!(
        case.manual && matches!(case.steps[0], Step::Prompt(_)),
        "the replug is a human step"
    );
    assert!(case.requires.iter().any(|r| r == "unlocked"));
    assert!(case.requires.iter().any(|r| r == "screen_readback"));

    // Locked PC, or no human to confirm the replug: BLOCKED before anything is sent.
    let fake = Fake::default();
    let cat = catalog();
    let dir = Temp::new();
    let hello = Hello {
        version: 1,
        capabilities: TEST_HOOKS,
        ..Default::default()
    };
    let mut runner = Runner::new(
        &fake,
        &cat,
        hello,
        Arc::new(AtomicBool::new(false)),
        false,
        &dir.0,
    );
    runner.locked = || true;
    let r = runner.case(&case, true);
    assert_eq!(r.status, Status::Blocked);
    assert!(r.message.contains("locked"), "{}", r.message);
    runner.locked = || false;
    runner.non_interactive = true;
    let r = runner.case(&case, true);
    assert_eq!(r.status, Status::Blocked);
    assert!(r.message.contains("manual"), "{}", r.message);
    assert!(fake.requests.borrow().is_empty());

    // The steps after the human's "y": the scripted replug below stands in for it.
    drop(runner);
    let mut steps = case;
    steps.manual = false;
    steps.steps.retain(|s| !matches!(s, Step::Prompt(_)));
    let dump = dir.0.join("foxglove-replug-panel.pbm");
    for s in &mut steps.steps {
        if let Step::PanelDump(d) = s {
            d.path = dump.to_string_lossy().into_owned();
            d.within_ms = 400;
        }
    }
    // Foxglove's last frame, as FOXGLOVE_DUMP_PANEL writes it: the portrait panel, 1 = ink.
    let frame = foxglove_lab::resilience::test_frame(160, 68, 1);
    let portrait = foxglove_lab::screen::portrait(&frame).unwrap();
    let (w, h) = portrait.dimensions();
    let mut bits = vec![0u8; w.div_ceil(8) as usize * h as usize];
    for (x, y, p) in portrait.enumerate_pixels() {
        if p[0] == 0 {
            bits[(y * w.div_ceil(8) + x / 8) as usize] |= 0x80 >> (x % 8);
        }
    }
    std::fs::write(
        &dump,
        foxglove_lab::resilience::pbm(w as usize, h as usize, &bits),
    )
    .unwrap();

    let board = Arc::new(left_half::Board::default());
    *board.screen.lock().unwrap() = frame.clone();
    let link = board.connect();
    let mut hw = Hardware::new(link);
    let run_on = |device: &Hardware| {
        let out = Temp::new();
        let mut r = Runner::new(
            device,
            &cat,
            hello,
            Arc::new(AtomicBool::new(false)),
            true,
            &out.0,
        );
        r.locked = || false;
        r.case(&steps, true)
    };
    assert_eq!(
        run_on(&hw).status,
        Status::Pass,
        "the frame shows before the replug"
    );

    for redrawn in [true, false] {
        // Replug: the panel comes back blank; Foxglove (#354) draws its frame again, or not.
        let generation = hw.link.state().generation;
        board.unplug();
        let end = std::time::Instant::now() + Duration::from_secs(2);
        while hw.link.state().connected.is_some() && std::time::Instant::now() < end {
            std::thread::sleep(Duration::from_millis(5));
        }
        *board.screen.lock().unwrap() = if redrawn {
            frame.clone()
        } else {
            vec![0; 1360]
        };
        board.plug(&hw.link);
        let end = std::time::Instant::now() + Duration::from_secs(2);
        while hw.link.state().generation == generation && std::time::Instant::now() < end {
            std::thread::sleep(Duration::from_millis(5));
        }
        hw.link.wait_connected(Duration::from_secs(2)).unwrap();
        // The run that saw the replug can't go on; the case runs in a fresh run.
        let stale = run_on(&hw);
        assert_eq!(stale.status, Status::Fail);
        assert!(
            stale.message.contains("start a new run"),
            "{}",
            stale.message
        );
        let link = hw.link;
        hw = Hardware::new(link);
        let r = run_on(&hw);
        if redrawn {
            assert_eq!(r.status, Status::Pass, "{}", r.message);
            assert!(
                r.steps[0].actual.starts_with("HELLO caps"),
                "{}",
                r.steps[0].actual
            );
            assert!(
                r.steps[1].actual.contains("frame shown"),
                "{}",
                r.steps[1].actual
            );
            assert_eq!(r.steps[1].attachments, ["replug-hello-left-panel.png"]);
        } else {
            assert_eq!(
                r.status,
                Status::Fail,
                "a blank panel after the replug is #350"
            );
            assert!(
                r.message.contains("not found on the left screen"),
                "{}",
                r.message
            );
        }
    }
}

/// #613: the right-art-select-golden case compares the right's readback after a built-in SELECT
/// with goldens rendered from the firmware's own custom_art.c. A regenerated art file must come
/// with new goldens, or the hardware case would fail on a correct board.
#[test]
fn right_art_goldens_match_the_firmware_art() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let suite = spec::load(&root.join("tests/hil/screens.toml")).unwrap();
    let case = suite
        .cases
        .iter()
        .find(|c| c.name == "right-art-select-golden")
        .unwrap();
    let src =
        std::fs::read_to_string(root.join("boards/shields/nice_view_custom/widgets/custom_art.c"))
            .unwrap();
    let array = |name: &str| -> Vec<u8> {
        let head = format!("static const uint8_t {name}[] = {{");
        let start = src.find(&head).unwrap() + head.len();
        let body = &src[start..start + src[start..].find("};").unwrap()];
        body.split(',')
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(|t| match t.strip_prefix("0x") {
                Some(hex) => u8::from_str_radix(hex, 16).unwrap(),
                None => t.parse().unwrap(),
            })
            .collect()
    };
    // The art's first frame, unpacked as peripheral_status.c's art_apply does.
    let frame = |set: usize| -> Vec<u8> {
        let mut buf = vec![0u8; 18 * 68];
        let first = array(&format!("set_{set}_order"))[0];
        for k in 0..=usize::from(first) {
            let d = array(&format!("set_{set}_delta_{k}"));
            let (mut i, mut at) = (0, 0);
            while at < buf.len() {
                let c = d[i];
                i += 1;
                let n = (usize::from(c & 0x3f) + 1).min(buf.len() - at);
                for j in 0..n {
                    buf[at + j] ^= match c >> 6 {
                        0 => d[i + j],
                        1 => 0,
                        2 => 0xff,
                        _ => d[i],
                    };
                }
                i += match c >> 6 {
                    0 => n,
                    3 => 1,
                    _ => 0,
                };
                at += n;
            }
        }
        buf
    };
    let mut checked = 0;
    for step in &case.steps {
        let Step::Screen(s) = step else { continue };
        let set = if s.golden.ends_with("set0.png") { 0 } else { 1 };
        // The right's readback: the art at native x 0..139, a blank status strip after it.
        let art = frame(set);
        let mut native = vec![0u8; 1360];
        for y in 0..68 {
            for x in 0..140 {
                if art[y * 18 + x / 8] & (0x80 >> (x % 8)) != 0 {
                    native[y * 20 + x / 8] |= 0x80 >> (x % 8);
                }
            }
        }
        let actual = foxglove_lab::screen::portrait(&native).unwrap();
        let golden = std::fs::read(root.join("tests/hil").join(&s.golden)).unwrap();
        let c = foxglove_lab::screen::compare(&actual, &golden, &s.mask).unwrap();
        assert_eq!(c.different, 0, "{} is set {set}'s first frame", s.golden);
        let blank = foxglove_lab::screen::portrait(&[0; 1360]).unwrap();
        let c = foxglove_lab::screen::compare(&blank, &golden, &s.mask).unwrap();
        assert!(c.different > 500, "{} is a picture, not a blank", s.golden);
        checked += 1;
    }
    assert_eq!(checked, 2);
}

/// The app's API for the art path (#618): records each call, answers as the app would.
struct FakeApp {
    calls: RefCell<Vec<String>>,
    shows: Option<String>,
    fails: bool,
}
impl foxglove_lab::app_api::AppArt for FakeApp {
    fn select_built_in(
        &self,
        label: &str,
        _: Duration,
    ) -> Result<foxglove_plugin_api::methods::ArtStatus> {
        self.calls.borrow_mut().push(format!("select {label}"));
        anyhow::ensure!(!self.fails, "another art job is running (code 3)");
        self.status(Duration::ZERO)
    }
    fn status(&self, _: Duration) -> Result<foxglove_plugin_api::methods::ArtStatus> {
        self.calls.borrow_mut().push("status".into());
        Ok(foxglove_plugin_api::methods::ArtStatus {
            built_in: self.shows.clone(),
            slot: None,
            pushed: false,
            flash_slots: 4,
            name: self.shows.clone(),
        })
    }
}
fn run_app(fake: &Fake, app: Option<&FakeApp>, case: &Case) -> foxglove_lab::report::CaseResult {
    let cat = catalog();
    let dir = Temp::new();
    let mut r = Runner::new(
        fake,
        &cat,
        Hello {
            version: 1,
            capabilities: 20,
            ..Default::default()
        },
        Arc::new(AtomicBool::new(false)),
        true,
        &dir.0,
    );
    r.app = app.map(|a| a as &dyn foxglove_lab::app_api::AppArt);
    r.case(case, true)
}

/// #618: `direct = true` sends the D6 SELECT even while the app runs (a select behind the
/// app's back, for the re-apply cases), never `art.select`, and says "direct (forced)".
#[test]
fn art_select_direct_forces_the_d6_select_while_the_app_runs() {
    let fake = Fake::default();
    fake.replies.borrow_mut().extend([
        hello(2, 4),
        art(5, 0, 17),
        art(5, 0, 17),
        art(4, 1, 17),
        art(5, 1, 17),
    ]);
    let app = FakeApp {
        calls: RefCell::default(),
        shows: Some("ZMK balloon".into()),
        fails: false,
    };
    let result = run_app(
        &fake,
        Some(&app),
        &case("[{art_select={set=1,restore=false,direct=true}}]"),
    );
    assert_eq!(result.status, Status::Pass, "{}", result.message);
    let actual = &result.steps[0].actual;
    assert!(actual.starts_with("direct (forced) SELECT"), "{actual}");
    assert_eq!(
        fake.requests
            .borrow()
            .iter()
            .filter(|r| r[..2] == [0xd6, 4])
            .count(),
        1
    );
    assert!(app.calls.borrow().is_empty(), "{:?}", app.calls.borrow());
    // `direct` defaults to off and is checked as a known field.
    assert!(toml::from_str::<Case>(
        "name='x'
description='x'
steps=[{art_select={set=0,direkt=true}}]"
    )
    .is_err());
}

/// #618: the report's `art_select_path` follows the app and the steps' `direct`.
#[test]
fn art_select_path_says_direct_forced() {
    let cases = |steps: &str| vec![case(steps)];
    let path = foxglove_lab::runner::art_select_path;
    let forced = cases("[{art_select={set=1,direct=true}}]");
    let api = cases("[{art_select={set=1}}]");
    let mixed = cases("[{art_select={set=1}},{art_select={set=0,direct=true}}]");
    assert_eq!(path(true, &forced), "direct (forced)");
    assert_eq!(path(true, &api), "app API art.select (the app is running)");
    assert!(path(true, &mixed).contains("direct (forced) on direct = true steps"));
    assert_eq!(
        path(false, &forced),
        "direct D6 SELECT (the app isn't running)"
    );
    assert_eq!(
        path(false, &api),
        "direct D6 SELECT (the app isn't running)"
    );
}

/// #618: with the app running the select goes through `art.select` (the app would undo a
/// direct SELECT, #622) and is read back with D6 STATUS and `art.status`; without it, the
/// direct SELECT. The step says which path it took.
#[test]
fn art_select_goes_through_the_app_while_it_runs() {
    let selects = |f: &Fake| {
        f.requests
            .borrow()
            .iter()
            .filter(|r| r[..2] == [0xd6, 4])
            .count()
    };
    // 17 sets: the bundled manifest's labels.
    let fake = Fake::default();
    fake.replies.borrow_mut().extend([
        hello(2, 4),
        art(5, 0, 17),
        art(5, 0, 17),
        art(5, 1, 17), // the fresh pair after art.select
        art(5, 1, 17),
    ]);
    let app = FakeApp {
        calls: RefCell::default(),
        shows: Some("ZMK mountain".into()),
        fails: false,
    };
    let result = run_app(
        &fake,
        Some(&app),
        &case("[{art_select={set=1,restore=false}}]"),
    );
    assert_eq!(result.status, Status::Pass, "{}", result.message);
    let actual = &result.steps[0].actual;
    assert!(
        actual.contains("through the app's API (art.select \"ZMK mountain\""),
        "{actual}"
    );
    assert!(actual.contains("art.status ZMK mountain"), "{actual}");
    assert_eq!(selects(&fake), 0, "no direct SELECT while the app runs");
    assert_eq!(app.calls.borrow()[0], "select ZMK mountain");

    // The app said yes but the right shows another set: FAIL, naming the path.
    let fake = Fake::default();
    fake.replies.borrow_mut().extend([
        hello(2, 4),
        art(5, 0, 17),
        art(5, 0, 17),
        art(5, 0, 17),
        art(5, 0, 17),
    ]);
    let app = FakeApp {
        calls: RefCell::default(),
        shows: Some("ZMK balloon".into()),
        fails: false,
    };
    let result = run_app(
        &fake,
        Some(&app),
        &case("[{art_select={set=1,restore=false}}]"),
    );
    assert_eq!(result.status, Status::Fail);
    assert!(
        result.message.contains("through the app's API"),
        "{}",
        result.message
    );

    // The app refuses (another art job): FAIL with its words, and still no direct SELECT.
    let fake = Fake::default();
    fake.replies
        .borrow_mut()
        .extend([hello(2, 4), art(5, 0, 17), art(5, 0, 17)]);
    let app = FakeApp {
        calls: RefCell::default(),
        shows: None,
        fails: true,
    };
    let result = run_app(
        &fake,
        Some(&app),
        &case("[{art_select={set=1,restore=false}}]"),
    );
    assert_eq!(result.status, Status::Fail);
    assert!(
        result.message.contains("another art job"),
        "{}",
        result.message
    );
    assert_eq!(selects(&fake), 0);

    // No app: the direct SELECT, said so.
    let fake = Fake::default();
    fake.replies.borrow_mut().extend([
        hello(2, 4),
        art(5, 0, 17),
        art(5, 0, 17),
        art(4, 1, 17),
        art(5, 1, 17),
    ]);
    let result = run_app(&fake, None, &case("[{art_select={set=1,restore=false}}]"));
    assert_eq!(result.status, Status::Pass, "{}", result.message);
    assert!(
        result.steps[0]
            .actual
            .contains("direct SELECT (D6 04; the app isn't running)"),
        "{}",
        result.steps[0].actual
    );
    assert_eq!(selects(&fake), 1);
}

/// The resumed 0.2 sitting's agent steps (docs/test-lab/sitting-0.2-h-i-r.md): tests/hil/sitting.toml
/// parses, its steps validate, the GAME toggle is a layer toggle (never a typing key), and the
/// right-screen case reads the redraw count and compares with a golden that exists.
#[test]
fn the_sitting_suite_parses_and_stays_safe() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/hil");
    let suite = spec::load(&root.join("sitting.toml")).unwrap();
    let names: Vec<&str> = suite.cases.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "game-layer-on",
            "game-layer-off",
            "right-redraw-after-select-while-idle"
        ]
    );
    for c in &suite.cases {
        for s in &c.steps {
            s.validate().unwrap();
            if let Step::Invoke(i) = s {
                assert_eq!(i.behavior.as_deref(), Some("tog"), "{}", c.name);
                assert_eq!(i.param1, 3, "GAME is layer 3 (docs/layers.md)");
            }
        }
    }
    let right = &suite.cases[2];
    for need in ["right_redraws", "position_press", "screen_readback"] {
        assert!(right.requires.iter().any(|r| r == need), "{need}");
    }
    assert!(right
        .steps
        .iter()
        .any(|s| matches!(s, Step::ExpectRightRedraws(_))));
    for s in &right.steps {
        if let Step::Screen(g) = s {
            assert!(root.join(&g.golden).is_file(), "{}", g.golden);
        }
    }
}

// The typing sink and the app's Settings window (6a2632f gate: typing-abc and typing-both-halves
// failed on focus because the test app's Settings, opened at its start, kept the foreground).
mod sink_focus {
    use foxglove_lab::platform::{
        focus_blocked, front_move, is_app_image, moved_note, FrontMove, MAX_MOVED,
    };

    #[test]
    fn only_the_app_itself_is_the_app() {
        assert!(is_app_image(
            r"C:\Users\x\worktrees\oncall\app\target\release\foxglove.exe"
        ));
        assert!(is_app_image(r"C:\Program Files\Foxglove\FOXGLOVE.EXE"));
        assert!(is_app_image("foxglove.exe"));
        assert!(!is_app_image(r"C:\x\target\release\foxglove-lab.exe"));
        assert!(!is_app_image(r"C:\x\target\release\foxglove-cli.exe"));
        assert!(!is_app_image(r"C:\Windows\explorer.exe"));
        assert!(!is_app_image(r"C:\foxglove.exe\WindowsTerminal.exe"));
        assert!(!is_app_image(""));
    }

    #[test]
    fn settings_in_front_is_minimized_once_then_the_sink_asks() {
        let app = Some(r"C:\a\foxglove.exe");
        assert_eq!(front_move(app, false, false, 0), FrontMove::MinimizeApp);
        // The same window again (still in front while the app closes it): no second minimize.
        assert_eq!(front_move(app, false, true, 1), FrontMove::Ask);
        // A different app window after one: still moved, up to the cap.
        assert_eq!(front_move(app, false, false, 1), FrontMove::MinimizeApp);
        assert_eq!(front_move(app, false, false, MAX_MOVED), FrontMove::Ask);
    }

    #[test]
    fn other_windows_and_the_overlay_are_never_touched() {
        // The user's programs: only asked over, never minimized.
        for other in [
            r"C:\Windows\explorer.exe",
            r"C:\x\WindowsTerminal.exe",
            r"C:\x\foxglove-lab.exe",
        ] {
            assert_eq!(
                front_move(Some(other), false, false, 0),
                FrontMove::Ask,
                "{other}"
            );
        }
        // The overlay is topmost; the app restores it when minimized (#612).
        assert_eq!(
            front_move(Some(r"C:\a\foxglove.exe"), true, false, 0),
            FrontMove::Ask
        );
        // No front window, or an owner Windows won't name.
        assert_eq!(front_move(None, false, false, 0), FrontMove::Ask);
    }

    #[test]
    fn a_focus_failure_is_blocked_with_who_kept_it() {
        let plain = focus_blocked("explorer.exe \"\"", &[]);
        // The flasher's typing check keys on this phrase (foxglove_flash.py).
        assert!(plain.contains("foreground focus"));
        assert!(plain.contains("explorer.exe"));
        assert!(!plain.contains("minimized first"));
        let moved = focus_blocked(
            "foxglove.exe \"Foxglove\"",
            &["foxglove.exe \"Foxglove\"".to_string()],
        );
        assert!(moved.contains("minimized first: foxglove.exe \"Foxglove\""));
        assert!(moved.contains("not the keyboard"));
    }

    #[test]
    fn the_note_says_what_the_sink_moved_and_put_back() {
        assert_eq!(moved_note(&[], 0), None);
        let names = ["foxglove.exe \"Foxglove\"".to_string()];
        let closed = moved_note(&names, 0).unwrap();
        assert!(closed.contains("minimized for the sink"));
        assert!(closed.contains("closes it"));
        assert!(!closed.contains("shown again"));
        let shown = moved_note(&names, 1).unwrap();
        assert!(shown.contains("1 shown again, unfocused"));
        assert!(!shown.contains("closes it"));
    }
}
