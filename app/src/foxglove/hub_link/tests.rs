//! The module link against the hub's own router, in process: no pipe, no window, no keyboard.

use std::time::Instant;

use hideout_contract::capability::{Holder, Source};
use hideout_contract::module::{Context, Module};
use hideout_router::{ConnId, ModuleEntry, Origin, Out, Router, Settings};

use super::*;

const TOKEN: &str = "tok-secret-7";

fn launch() -> Launch {
    Launch {
        pipe: r"\\.\pipe\hideout-test".into(),
        token: TOKEN.into(),
        instance: "i-1".into(),
        hub_pid: 4242,
    }
}

fn args(a: &[&str]) -> Vec<OsString> {
    a.iter().map(OsString::from).collect()
}

fn env_with(pairs: &[(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
    let pairs: Vec<_> = pairs.to_vec();
    move |k| {
        pairs
            .iter()
            .find(|(n, _)| *n == k)
            .map(|(_, v)| v.to_string())
    }
}

const GOOD_ENV: &[(&str, &str)] = &[
    ("HIDEOUT_PIPE", r"\\.\pipe\hideout-me"),
    ("HIDEOUT_TOKEN", "t"),
    ("HIDEOUT_INSTANCE", "i"),
    ("HIDEOUT_HUB_PID", "17"),
];

// ---- start-up: the flag and the environment ----

#[test]
fn without_the_flag_the_app_is_standalone_and_the_environment_is_not_read() {
    for a in [args(&["foxglove.exe"]), args(&["foxglove.exe", "--tray"])] {
        assert_eq!(parse_launch(a, env_with(GOOD_ENV)), Ok(None));
    }
    // The flag counts only as an argument after the program name.
    assert_eq!(
        parse_launch(args(&["--module"]), env_with(GOOD_ENV)),
        Ok(None)
    );
    assert_eq!(
        parse_launch(args(&["foxglove.exe", "--module-x"]), env_with(GOOD_ENV)),
        Ok(None)
    );
}

#[test]
fn the_flag_with_the_hub_values_is_module_mode() {
    let l = parse_launch(args(&["foxglove.exe", "--module"]), env_with(GOOD_ENV))
        .unwrap()
        .unwrap();
    assert_eq!(l.pipe, r"\\.\pipe\hideout-me");
    assert_eq!((l.instance.as_str(), l.hub_pid), ("i", 17));
    assert!(
        !format!("{l:?}").contains("\"t\"") && format!("{l:?}").contains("redacted"),
        "the token is not printed"
    );
}

#[test]
fn the_flag_without_the_hub_values_refuses_and_names_what_is_missing() {
    for missing in 0..GOOD_ENV.len() {
        let mut env: Vec<_> = GOOD_ENV.to_vec();
        let (name, _) = env.remove(missing);
        let e = parse_launch(args(&["foxglove.exe", "--module"]), env_with(&env)).unwrap_err();
        assert!(e.contains(name), "{e}");
        assert!(e.contains("Nothing was started"), "{e}");
        assert!(!e.contains("hideout-me"), "no value is echoed: {e}");
    }
    // Empty is missing.
    let env = [("HIDEOUT_PIPE", ""), ("HIDEOUT_TOKEN", "t")];
    assert!(parse_launch(args(&["x", "--module"]), env_with(&env)).is_err());
}

#[test]
fn a_launch_naming_the_users_own_pipe_or_a_bad_pid_is_refused() {
    // The foxglove-<user> alias is the hub's to serve; the module never opens it.
    for pipe in [
        r"\\.\pipe\foxglove-me",
        r"\\evil\pipe\hideout-me",
        "hideout-me",
    ] {
        let env = [
            ("HIDEOUT_PIPE", pipe),
            ("HIDEOUT_TOKEN", "t"),
            ("HIDEOUT_INSTANCE", "i"),
            ("HIDEOUT_HUB_PID", "17"),
        ];
        let e = parse_launch(args(&["x", "--module"]), env_with(&env)).unwrap_err();
        assert!(e.contains("router pipe"), "{e}");
    }
    for pid in ["0", "abc", "-1", ""] {
        let env = [
            ("HIDEOUT_PIPE", r"\\.\pipe\hideout-me"),
            ("HIDEOUT_TOKEN", "t"),
            ("HIDEOUT_INSTANCE", "i"),
            ("HIDEOUT_HUB_PID", pid),
        ];
        assert!(
            parse_launch(args(&["x", "--module"]), env_with(&env)).is_err(),
            "{pid}"
        );
    }
}

#[test]
fn module_mode_starts_quiet_and_standalone_is_unchanged() {
    use crate::foxglove::settings_open::tray_start_in;
    assert!(tray_start_in(["foxglove.exe", "--module"]));
    assert!(tray_start_in(["foxglove.exe", "--tray"]));
    assert!(!tray_start_in(["foxglove.exe"]));
    assert!(
        !tray_start_in(["--module"]),
        "the program name is not an argument"
    );
    // This test process is not the hub's module.
    assert!(!module_mode());
}

// ---- the manifest ----

#[test]
fn the_embedded_manifest_is_a_valid_adopted_module() {
    let exe = r"C:\Program Files\Foxglove\foxglove.exe";
    let m = Module::parse(&manifest(exe), &Context::default()).expect("manifest");
    assert_eq!(m.id, ID);
    assert_eq!(m.version, env!("CARGO_PKG_VERSION"));
    assert_eq!(m.run, Some(vec![exe.to_string(), FLAG.to_string()]));
    let text = manifest(exe);
    assert!(text.contains("kind = \"adopted\""));
    assert!(text.contains(GRANT) && text.contains("facts:keyboard.*"));
    // Nothing but the keyboard's facts: no method, no device, no `facts.write:*`.
    assert!(!text.contains("method:"), "no method until P6");
    assert!(!text.contains("hid.raw") && !text.contains("facts.write:*"));
    assert!(!text.contains("facts.write:device"));
}

// ---- the link against the hub's router ----

struct Rig {
    router: Router,
    conn: ConnId,
    link: Link<Vec<u8>>,
    now: Instant,
}

fn adopted_entry(granted: &[&str]) -> ModuleEntry {
    ModuleEntry {
        id: ID.into(),
        name: "Foxglove".into(),
        holder: Holder::ThirdParty(Source::Sideload),
        granted: granted.iter().map(|s| s.to_string()).collect(),
        catalog_provide: None,
        channel_risk: Default::default(),
        activate_on_call: false,
        panels: None,
        settings_dir: String::new(),
        data_dir: String::new(),
        registered_version: Some(env!("CARGO_PKG_VERSION").into()),
    }
}

impl Rig {
    fn new(granted: &[&str]) -> Rig {
        Rig::with(granted, TOKEN)
    }

    /// The hub holds `hub_token` for the module; the link says `TOKEN`.
    fn with(granted: &[&str], hub_token: &str) -> Rig {
        let mut router = Router::new(Settings::default(), Box::new(|_, a| a.to_vec()));
        router.add_module(adopted_entry(granted));
        let m = Module::parse(&manifest(r"C:\x\foxglove.exe"), &Context::default()).unwrap();
        router.apply_manifest(&m);
        router.set_token(ID, hub_token);
        let conn = router.connect(Origin::Pipe);
        Rig {
            router,
            conn,
            link: Link::new(Vec::new(), launch()),
            now: Instant::now(),
        }
    }

    /// Runs the module's output through the router and the router's replies back, until quiet.
    /// Returns the steps the link reported.
    fn pump(&mut self) -> Vec<Step> {
        let mut steps = Vec::new();
        loop {
            let sent = std::mem::take(self.link.writer_mut());
            if sent.is_empty() {
                return steps;
            }
            for line in String::from_utf8(sent).unwrap().lines() {
                let out = self.router.receive(self.conn, line, self.now);
                for o in out {
                    if let Out::Send(c, v) = o {
                        if c == self.conn {
                            steps.push(self.link.line(&v.to_string()).unwrap());
                        }
                    }
                }
            }
        }
    }

    /// The handshake and the snapshot of `facts`, as the runner does it.
    fn connect(&mut self, facts: &foxglove_devices::facts::Facts) -> Vec<Step> {
        self.link.start().unwrap();
        let steps = self.pump();
        if steps.contains(&Step::Ready) {
            self.link.snapshot(facts.list(FACTS)).unwrap();
            self.pump();
        }
        steps
    }

    fn fact(&self, key: &str) -> Option<Value> {
        self.router.fact(key).cloned()
    }
}

fn keyboard() -> foxglove_devices::facts::Facts {
    let facts = foxglove_devices::facts::Facts::default();
    facts.set("keyboard.connected", Fact::Bool(true));
    super::super::keyboard_facts::layers(&facts, &[0], &[Some("QWERTY".into())]);
    facts
}

#[test]
fn registers_and_the_hub_sees_the_keyboard_facts_including_game_mode() {
    let facts = keyboard();
    let mut rig = Rig::new(&[GRANT]);
    let steps = rig.connect(&facts);
    assert!(steps.contains(&Step::Ready), "{steps:?}");
    assert!(rig.link.live());
    assert_eq!(rig.fact("keyboard.connected"), Some(json!(true)));
    assert_eq!(rig.fact("keyboard.layer"), Some(json!(0)));
    assert_eq!(rig.fact("keyboard.game_mode"), Some(json!(false)));
    // The ready notification went out after the registration (the handshake order of §7.5).
    assert!(
        rig.link.take_log().is_empty(),
        "a clean registration says nothing"
    );
}

#[test]
fn facts_follow_a_layer_change_and_a_disconnect() {
    let facts = keyboard();
    let changes = facts.subscribe(FACTS);
    let mut rig = Rig::new(&[GRANT]);
    rig.connect(&facts);
    let names: Vec<Option<String>> = ["QWERTY", "NUMBER", "SYMBOL", "GAME"]
        .iter()
        .map(|n| Some(n.to_string()))
        .collect();
    super::super::keyboard_facts::layers(&facts, &[0, 3], &names);
    for c in changes.try_iter() {
        rig.link.change(&c).unwrap();
    }
    rig.pump();
    assert_eq!(rig.fact("keyboard.layer"), Some(json!(3)));
    assert_eq!(rig.fact("keyboard.game_mode"), Some(json!(true)));
    // The keyboard goes: the layer facts are removed, connection stays and says false.
    facts.remove_prefix("keyboard.", &["keyboard.connected"]);
    facts.set("keyboard.connected", Fact::Bool(false));
    for c in changes.try_iter() {
        rig.link.change(&c).unwrap();
    }
    rig.pump();
    assert_eq!(rig.fact("keyboard.layer"), None);
    assert_eq!(rig.fact("keyboard.game_mode"), None);
    assert_eq!(rig.fact("keyboard.connected"), Some(json!(false)));
}

#[test]
fn bytes_travel_as_base64_and_only_changes_are_sent() {
    let facts = keyboard();
    let mut rig = Rig::new(&[GRANT]);
    rig.connect(&facts);
    assert_eq!(
        rig.fact("keyboard.layers"),
        Some(json!({"base64": "AA=="})),
        "layers [0] as base64"
    );
    // Facts::set drops a repeat, so the subscription never offers the link a duplicate.
    let changes = facts.subscribe(FACTS);
    facts.set("keyboard.layer", Fact::Int(0));
    assert_eq!(changes.try_iter().count(), 0);
}

#[test]
fn nothing_but_keyboard_facts_leaves_the_app() {
    let mut rig = Rig::new(&[GRANT]);
    rig.connect(&foxglove_devices::facts::Facts::default());
    for key in [
        "device.k.battery",
        "gpu.reserved",
        "module.foxglove.state",
        "hub.x",
        "keyboardx",
    ] {
        rig.link
            .change(&Change {
                key: key.into(),
                value: Some(Fact::Int(1)),
            })
            .unwrap();
    }
    assert!(rig.link.writer_mut().is_empty(), "nothing was written");
}

#[test]
fn without_the_grant_the_module_registers_but_publishes_nothing_and_says_so_once() {
    let facts = keyboard();
    let mut rig = Rig::new(&[]);
    let steps = rig.connect(&facts);
    assert!(steps.contains(&Step::Ready));
    assert!(!rig.link.live());
    rig.link
        .change(&Change {
            key: "keyboard.layer".into(),
            value: Some(Fact::Int(5)),
        })
        .unwrap();
    assert!(rig.link.writer_mut().is_empty());
    let log = rig.link.take_log();
    assert_eq!(log.len(), 1, "{log:?}");
    assert!(log[0].contains(GRANT));
}

#[test]
fn a_denied_fact_stops_publishing_without_a_retry() {
    let mut rig = Rig::new(&[GRANT]);
    rig.connect(&keyboard());
    rig.link
        .change(&Change {
            key: "keyboard.layer".into(),
            value: Some(Fact::Int(2)),
        })
        .unwrap();
    // The hub answers the next facts.set with 5 (a grant withdrawn).
    let id = *rig
        .link
        .session
        .pending()
        .keys()
        .next()
        .expect("a call waits");
    let denied = json!({"jsonrpc":"2.0","id":id,"error":{"code":5,"message":"denied","data":{"reason":"not allowed"}}});
    rig.link.line(&denied.to_string()).unwrap();
    assert!(!rig.link.live());
    rig.link.writer_mut().clear();
    rig.link
        .change(&Change {
            key: "keyboard.layer".into(),
            value: Some(Fact::Int(3)),
        })
        .unwrap();
    assert!(rig.link.writer_mut().is_empty(), "no retry");
    assert_eq!(rig.link.take_log().len(), 1);
}

#[test]
fn a_bad_token_is_refused_and_the_module_ends() {
    let mut rig = Rig::with(&[GRANT], "another-token");
    rig.link.start().unwrap();
    let steps = rig.pump();
    match steps.as_slice() {
        [Step::Refused(why)] => {
            assert!(why.contains("refused"), "{why}");
            assert!(!why.contains(TOKEN), "the token is never logged");
        }
        other => panic!("{other:?}"),
    }
    assert!(!rig.link.live());
}

#[test]
fn another_owner_of_the_facts_is_logged_and_the_grant_still_decides() {
    // Another adopted module also declares `keyboard.*`: neither gets the area, so the
    // registration is refused for it (#1150).
    let mut rig = Rig::new(&[GRANT]);
    let mut other = adopted_entry(&[GRANT]);
    other.id = "example.keys".into();
    rig.router.add_module(other);
    let text = "id = \"example.keys\"
name = \"Keys\"
version = \"0.1.0\"
kind = \"adopted\"
family = \"arete\"
hub = \"1.0\"
run = [\"C:/x/k.exe\"]
provides = [\"facts:keyboard.*\"]
";
    let m = Module::parse(text, &Context::default()).unwrap();
    rig.router.apply_manifest(&m);
    rig.router.set_token("example.keys", "k");
    let c = rig.router.connect(Origin::Pipe);
    let init = json!({"jsonrpc":"2.0","id":0,"method":"initialize","params":{
        "id":"example.keys","version":env!("CARGO_PKG_VERSION"),"instance":"i","token":"k","hub":"1.0","api":{}}});
    rig.router.receive(c, &init.to_string(), rig.now);
    let reg = json!({"jsonrpc":"2.0","id":1,"method":"module.register","params":{"facts":["keyboard.*"]}});
    rig.router.receive(c, &reg.to_string(), rig.now);
    rig.connect(&keyboard());
    let log = rig.link.take_log();
    assert_eq!(log.len(), 1, "{log:?}");
    assert!(log[0].contains("keyboard.*") && log[0].contains("permission.provider"));
}

#[test]
fn the_hub_can_drain_and_exit_the_module() {
    let mut rig = Rig::new(&[GRANT]);
    rig.connect(&keyboard());
    let drain =
        json!({"jsonrpc":"2.0","id":99,"method":"module.drain","params":{"reason":"update"}});
    assert_eq!(rig.link.line(&drain.to_string()).unwrap(), Step::Nothing);
    let out: Value = serde_json::from_slice(rig.link.writer_mut()).unwrap();
    assert_eq!(out["result"], json!({"state": "done"}));
    let exit = json!({"jsonrpc":"2.0","method":"module.exit","params":{"reason":"disable"}});
    assert_eq!(rig.link.line(&exit.to_string()).unwrap(), Step::Exit);
}

// ---- no tray, no pipe: the wiring is in the places that make them ----

#[test]
fn module_mode_makes_no_tray_and_serves_no_pipe() {
    let host = include_str!("../../platform/eframe_host.rs");
    let at = |s: &str| host.find(s).unwrap_or_else(|| panic!("{s}"));
    assert!(
        at("hub_link::module_mode()") < at("create_tray_icon("),
        "the tray is made only after the module-mode check"
    );
    assert!(
        at("hub_link::module_mode()") < at("taskbar::watch("),
        "the taskbar watch too"
    );
    let api = include_str!("../api.rs");
    assert!(
        api.find("hub_link::module_mode()").unwrap() < api.find("pipe::listen(").unwrap(),
        "the foxglove-<user> pipe is claimed only outside module mode"
    );
    // The self-restarts that would start a plain copy.
    for src in [
        include_str!("../gpu_reset.rs"),
        include_str!("../identity.rs"),
    ] {
        assert!(src.contains("hub_link::module_mode()"));
    }
}

#[test]
fn a_module_tray_is_empty_and_stays_empty() {
    let mut tray = crate::tray::Tray::none();
    tray.update();
    assert!(!tray.has_icon());
}

#[cfg(windows)]
#[test]
fn the_backoff_is_bounded_and_inside_the_grace_period() {
    use super::runner::delay;
    let steps: Vec<_> = (0..8).map(|a| delay(a).as_millis()).collect();
    assert_eq!(steps, [500, 1000, 2000, 4000, 8000, 8000, 8000, 8000]);
    assert!(
        delay(1_000).as_secs() <= 8,
        "no overflow, no unbounded wait"
    );
    assert_eq!(GRACE.as_secs(), 30, "contract §7.9");
}

// ---- reconnect trust, grace, queue and reader cleanup (the review's M1, M2, L1, L2) ----

#[cfg(windows)]
mod reconnect {
    use std::io::{BufRead, Write};
    use std::path::PathBuf;
    use std::sync::atomic::Ordering;
    use std::sync::mpsc;
    use std::time::Duration;

    use foxglove_plugin::channel::{connect_router, unique_pipe_name, ServerPipe};
    use foxglove_plugin::owner::Probe;
    use hideout_acl::Peer;

    use super::super::runner::{self, Ended, Msg, READERS};
    use super::super::{pick, Grace, HubId, Pending, Pick};
    use super::*;

    const HUB_EXE: &str = r"C:\Program Files\Hideout\hideout.exe";

    fn hub() -> HubId {
        HubId {
            pid: 100,
            path: Some(PathBuf::from(HUB_EXE)),
        }
    }

    fn medium() -> std::io::Result<Peer> {
        Ok(Peer {
            integrity: hideout_acl::MEDIUM_RID,
            app_container: None,
        })
    }

    fn verified(pid: u32, image: &str) -> Probe {
        Probe::Verified {
            pid,
            image: PathBuf::from(image),
        }
    }

    #[test]
    fn while_the_original_hub_lives_only_its_pid_is_allowed_and_the_probe_is_never_asked() {
        let alive = |pid| (pid == 100).then(|| PathBuf::from(HUB_EXE));
        let picked = pick(
            &hub(),
            || panic!("no name-only downgrade"),
            alive,
            |_| panic!("not asked"),
        );
        assert_eq!(picked, Pick::Connect(100));
        // Pid 100 reused by another program: that is not the hub, so the new-hub rules apply.
        let reused = |pid| (pid == 100).then(|| PathBuf::from(r"C:\x\other.exe"));
        let picked = pick(&hub(), || Probe::Free, reused, |_| medium());
        assert!(matches!(picked, Pick::Wait(_)), "{picked:?}");
    }

    #[test]
    fn an_impostor_with_the_right_name_but_another_path_is_refused() {
        let gone = |_| None;
        let a = pick(
            &hub(),
            || verified(666, r"C:\Users\x\AppData\hideout.exe"),
            gone,
            |_| medium(),
        );
        assert!(matches!(a, Pick::Refuse(_)), "{a:?}");
        let b = pick(
            &hub(),
            || Probe::Foreign {
                pid: Some(7),
                why: "pid 7 runs another.exe".into(),
            },
            gone,
            |_| medium(),
        );
        assert!(matches!(b, Pick::Refuse(_)), "{b:?}");
        // The right path but low integrity, in an AppContainer, or unreadable: refused.
        for p in [
            Ok(Peer {
                integrity: 0x1000,
                app_container: None,
            }),
            Ok(Peer {
                integrity: hideout_acl::MEDIUM_RID,
                app_container: Some("S-1-15-2-1".into()),
            }),
            Err(std::io::Error::other("no")),
        ] {
            let c = std::cell::RefCell::new(Some(p));
            let got = pick(
                &hub(),
                || verified(300, HUB_EXE),
                gone,
                |_| c.borrow_mut().take().unwrap(),
            );
            assert!(matches!(got, Pick::Refuse(_)), "{got:?}");
        }
    }

    #[test]
    fn the_original_hub_gone_and_a_new_hub_at_the_recorded_path_is_accepted() {
        let gone = |_| None;
        // Case differences in a Windows path are not a different file.
        let got = pick(
            &hub(),
            || verified(300, &HUB_EXE.to_uppercase()),
            gone,
            |_| medium(),
        );
        assert_eq!(got, Pick::Connect(300));
        // Nothing serving yet: wait, inside the grace period.
        let nothing = pick(&hub(), || Probe::Free, gone, |_| medium());
        assert!(matches!(nothing, Pick::Wait(_)));
        // A hub whose path was never read can't be replaced by a guess.
        let unknown = HubId {
            pid: 100,
            path: None,
        };
        let got = pick(&unknown, || verified(300, HUB_EXE), gone, |_| medium());
        assert!(matches!(got, Pick::Wait(_)), "{got:?}");
    }

    #[test]
    fn the_grace_restarts_only_when_a_connection_was_live() {
        let t0 = Instant::now();
        let g = Grace::new(t0);
        // Connections that opened and sent garbage never call lost_live: it runs out.
        assert!(!g.expired(t0 + Duration::from_secs(29)));
        assert!(g.expired(t0 + GRACE));
        let mut g = Grace::new(t0);
        g.lost_live(t0 + Duration::from_secs(20));
        assert!(!g.expired(t0 + Duration::from_secs(40)));
        assert!(g.expired(t0 + Duration::from_secs(50)));
    }

    #[test]
    fn queued_fact_changes_are_bounded_by_the_keys_and_the_latest_wins() {
        let p = Pending::default();
        let set = |k: &str, v: i64| Change {
            key: k.into(),
            value: Some(Fact::Int(v)),
        };
        assert!(
            p.push(set("keyboard.layer", 0)),
            "the first change wakes the link"
        );
        for i in 1..10_000 {
            assert!(
                !p.push(set("keyboard.layer", i)),
                "no second wake while one waits"
            );
        }
        p.push(Change {
            key: "keyboard.layers".into(),
            value: None,
        });
        let got = p.take();
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].value, Some(Fact::Int(9_999)));
        assert!(
            p.push(set("keyboard.layer", 1)),
            "taken: the next change wakes again"
        );
    }

    /// A hub that answers with garbage, and one that never answers: each connection ends, its
    /// reader thread and handle go with it, and the handshake has a deadline.
    #[test]
    fn a_hub_sending_bad_lines_or_silence_leaves_no_reader_behind() {
        let facts = foxglove_devices::facts::Facts::default();
        let (tx, rx) = mpsc::channel::<Msg>();
        let pending = Pending::default();
        let me = std::process::id();
        for round in 0..6u64 {
            let name = unique_pipe_name("test.garbage", &format!("r{round}"));
            let server = ServerPipe::create(&name, true).unwrap();
            let silent = round == 5;
            let hub = std::thread::spawn(move || {
                let mut conn = server.accept().unwrap();
                let mut line = String::new();
                std::io::BufReader::new(conn.clone())
                    .read_line(&mut line)
                    .unwrap();
                assert!(line.contains("\"initialize\""), "{line}");
                if !silent {
                    // Keeps sending bad lines; the write ends when the app closes the pipe.
                    while conn.write_all(b"this is not json\n").is_ok() {}
                } else {
                    std::thread::sleep(Duration::from_secs(6));
                }
            });
            let conn = connect_router(&name, me).unwrap();
            let t = Instant::now();
            let (ended, live) =
                runner::serve(&launch(), conn, round + 1, &tx, &rx, &pending, &facts);
            match ended {
                Ended::Lost(why) => assert!(!why.is_empty()),
                _ => panic!("a bad hub does not end the module cleanly"),
            }
            assert!(!live, "never reached ready");
            if silent {
                let took = t.elapsed();
                assert!(
                    took >= Duration::from_secs(4) && took < Duration::from_secs(8),
                    "{took:?}"
                );
            }
            assert_eq!(
                READERS.load(Ordering::SeqCst),
                0,
                "round {round}: the reader is gone"
            );
            hub.join().unwrap();
        }
    }
}
