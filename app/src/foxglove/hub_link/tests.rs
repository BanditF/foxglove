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
    // The facts and the served methods, nothing else: no device, no `facts.write:*`.
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
    /// The user granted `granted` and every served method (`provide:`).
    fn new(granted: &[&str]) -> Rig {
        let mut all: Vec<String> = granted.iter().map(|s| s.to_string()).collect();
        all.extend(SERVED.iter().map(|m| format!("provide:{m}")));
        let all: Vec<&str> = all.iter().map(String::as_str).collect();
        Rig::with(&all, TOKEN)
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
    assert!(include_str!("../identity.rs").contains("hub_link::module_mode()"));
    // The GPU reset and the updater take the planned exit in module mode instead (#1143 P7).
    for src in [
        include_str!("../gpu_reset.rs"),
        include_str!("../update.rs"),
    ] {
        assert!(src.contains("hub_link::module_mode()"));
        assert!(src.contains("hub_link::planned_exit("));
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

// ---- the v1 methods through the router (P6) ----

/// Every served name as a `provide:` grant, the way `--adopt` records the user's yes.
fn granted_all() -> Vec<String> {
    let mut g = vec![GRANT.to_string()];
    g.extend(SERVED.iter().map(|m| format!("provide:{m}")));
    g
}

/// Counts what reaches the app's handlers.
#[derive(Default)]
struct Counting {
    seen: std::sync::Mutex<Vec<(String, Value)>>,
}

impl foxglove_plugin_host::Devices for Counting {
    fn call_from(
        &self,
        _conn: u64,
        method: &str,
        params: Value,
    ) -> Result<Value, foxglove_plugin_api::rpc::Error> {
        self.seen.lock().unwrap().push((method.into(), params));
        Ok(json!({"served": method}))
    }
}

struct Tool {
    conn: ConnId,
}

impl Rig {
    fn with_devices(
        granted: &[String],
        devices: std::sync::Arc<dyn foxglove_plugin_host::Devices>,
    ) -> Rig {
        let g: Vec<&str> = granted.iter().map(String::as_str).collect();
        let mut rig = Rig::with(&g, TOKEN);
        rig.link = Link::new(Vec::new(), launch()).with_devices(devices);
        rig
    }

    /// An outside tool asking for `asked`; the test router approves what is asked.
    fn tool(&mut self, asked: &[&str]) -> Tool {
        let conn = self.router.connect(Origin::Pipe);
        let init = json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
            "api":"1.13","client":{"name":"cli","version":"1"},"permissions":asked}});
        let got = self.deliver(conn, &init.to_string());
        assert!(got[0]["result"].is_object(), "{got:?}");
        Tool { conn }
    }

    /// One call by the tool, carried to the module and back; what the tool receives.
    fn call(&mut self, tool: &Tool, method: &str, params: Value) -> Value {
        let line = json!({"jsonrpc":"2.0","id":7,"method":method,"params":params});
        let got = self.deliver(tool.conn, &line.to_string());
        assert_eq!(got.len(), 1, "{got:?}");
        got.into_iter().next().unwrap()
    }

    /// Runs a line from `from` through the router, and every hop between the router and the
    /// module; returns what the router sent `from`.
    fn deliver(&mut self, from: ConnId, line: &str) -> Vec<Value> {
        let mut mine = Vec::new();
        let mut queue = vec![(from, line.to_string())];
        while let Some((c, l)) = queue.pop() {
            for o in self.router.receive(c, &l, self.now) {
                let Out::Send(to, v) = o else { continue };
                if to == from {
                    mine.push(v);
                } else if to == self.conn {
                    self.link.line(&v.to_string()).unwrap();
                    let sent = std::mem::take(self.link.writer_mut());
                    for l in String::from_utf8(sent).unwrap().lines() {
                        queue.push((self.conn, l.to_string()));
                    }
                }
            }
        }
        mine
    }
}

fn error_code(v: &Value) -> i64 {
    v["error"]["code"].as_i64().unwrap_or_else(|| panic!("{v}"))
}

#[test]
fn the_manifest_and_the_provider_table_are_the_same_list() {
    let text = manifest(r"C:\x\foxglove.exe");
    Module::parse(&text, &Context::default()).unwrap();
    let found = |prefix: &str| -> Vec<String> {
        text.lines()
            .filter_map(|l| {
                l.trim()
                    .trim_end_matches(',')
                    .trim_matches('"')
                    .strip_prefix(prefix)
            })
            .map(String::from)
            .collect()
    };
    let mut table: Vec<String> = SERVED.iter().map(|s| s.to_string()).collect();
    table.sort();
    let (mut declared, mut asked) = (found("method:"), found("provide:"));
    declared.sort();
    asked.sort();
    assert_eq!(declared, table, "method: entries");
    assert_eq!(asked, table, "provide: permissions");
    // Exact names, never a pattern: nothing the table does not list can be claimed.
    assert!(declared.iter().all(|d| !d.contains('*')));
}

#[test]
fn every_served_name_has_a_router_permission_and_none_is_never_list() {
    // The router's own area and hub-name rules are exercised by the registration test below:
    // a name either rule refuses would come back `rejected` and be logged.
    for m in SERVED {
        // The router decides the caller's permission from the API table: every served name
        // has a row with a permission (a row without one is open to every caller).
        let row = foxglove_plugin_api::methods::find(m).unwrap_or_else(|| panic!("{m} has no row"));
        let need = row
            .permission
            .unwrap_or_else(|| panic!("{m} needs a permission"));
        assert!(
            ![
                "key.events",
                "hid.raw",
                "wallpaper.control",
                "secrets",
                "modules.manage"
            ]
            .contains(&need),
            "{m} needs {need}, a never-list permission"
        );
    }
    for never in [
        "behaviors.invoke",
        "behaviors.list",
        "lighting.set",
        "lighting.lease",
        "lighting.update",
        "lighting.release",
        "devices.list",
        "art.select",
        "screens.panel_override",
        "profiles.activate",
        "focus.set",
        "key.event",
        "hid.raw",
        "wallpaper-engine.pause",
        "wallpaper-engine.release",
        "wallpaper-engine.status",
        "secrets.get",
        "facts.set",
        "modules.manage",
    ] {
        assert!(!SERVED.contains(&never), "{never} must not be served");
    }
}

#[test]
fn the_hub_registers_every_served_method_and_refuses_none() {
    let mut rig = Rig::with_devices(&granted_all(), std::sync::Arc::new(Counting::default()));
    let steps = rig.connect(&keyboard());
    assert!(steps.contains(&Step::Ready), "{steps:?}");
    assert!(rig.link.take_log().is_empty(), "no rejected registration");
    let t = rig.tool(&["devices.read", "app.windows", "lighting.write"]);
    for m in SERVED {
        let params = if *m == "overlay.set_shown" {
            json!({"on": true})
        } else {
            json!({"device": "x"})
        };
        let r = rig.call(&t, m, params);
        assert_eq!(r["result"]["served"], json!(m), "{r}");
    }
}

#[test]
fn without_the_users_provide_grant_nothing_is_routed() {
    // Registered, but the user granted no `provide:` (only the facts): the hub refuses the
    // methods at registration, and a call finds no provider.
    let counting = std::sync::Arc::new(Counting::default());
    let mut rig = Rig::with_devices(&[GRANT.to_string()], counting.clone());
    rig.connect(&keyboard());
    let log = rig.link.take_log();
    assert_eq!(log.len(), 1, "{log:?}");
    assert!(
        log[0].contains("health.read") && log[0].contains("permission.provider"),
        "{log:?}"
    );
    let t = rig.tool(&["devices.read"]);
    let r = rig.call(&t, "health.read", json!({"device": "x"}));
    assert!(r["error"].is_object(), "{r}");
    assert!(counting.seen.lock().unwrap().is_empty());
}

#[test]
fn a_caller_without_the_permission_is_refused_by_the_router_and_never_reaches_the_app() {
    let counting = std::sync::Arc::new(Counting::default());
    let mut rig = Rig::with_devices(&granted_all(), counting.clone());
    rig.connect(&keyboard());
    // Reads only: every write method needs more.
    let t = rig.tool(&["devices.read"]);
    for (m, p) in [
        ("tray.set_panel", json!({"open": true})),
        ("overlay.restore", json!({})),
        ("overlay.set_shown", json!({"on": false})),
        ("pc_lighting.set_zone", json!({})),
    ] {
        let r = rig.call(&t, m, p);
        assert_eq!(error_code(&r), 5, "{m}: {r}");
    }
    assert!(
        counting.seen.lock().unwrap().is_empty(),
        "the app was never asked"
    );
    // A caller with nothing at all is refused for the reads too.
    let none = rig.tool(&[]);
    for m in [
        "health.read",
        "lights.page",
        "scenes.page",
        "tray.panel",
        "overlay.window",
        "overlay.shown",
        "pc_lighting.list",
    ] {
        let r = rig.call(&none, m, json!({"device": "x"}));
        assert_eq!(error_code(&r), 5, "{m}: {r}");
    }
    assert!(counting.seen.lock().unwrap().is_empty());
    // With the permission the same call is answered.
    let r = rig.call(&t, "tray.panel", json!({}));
    assert_eq!(r["result"]["served"], "tray.panel");
    assert_eq!(counting.seen.lock().unwrap().len(), 1);
}

#[test]
fn never_list_and_first_party_methods_are_unreachable_through_the_module() {
    let counting = std::sync::Arc::new(Counting::default());
    let mut rig = Rig::with_devices(&granted_all(), counting.clone());
    rig.connect(&keyboard());
    // A caller holding every permission a tool could be given still reaches none of these.
    let t = rig.tool(&[
        "devices.read",
        "lighting.write",
        "behaviors.invoke",
        "app.windows",
        "screens.write",
        "art.write",
        "profiles.write",
        "focus.write",
        "wallpaper.control",
        "key.events",
        "hid.raw",
    ]);
    for m in [
        "behaviors.invoke",
        "behaviors.list",
        "lighting.set",
        "lighting.lease",
        "devices.list",
        "art.select",
        "screens.panel_override",
        "profiles.activate",
        "focus.set",
        "wallpaper-engine.pause",
        "wallpaper-engine.status",
        "secrets.get",
        "definitely.not.a.method",
    ] {
        let r = rig.call(&t, m, json!({"device": "x", "name": "x"}));
        assert!(r["error"].is_object(), "{m} must not be answered: {r}");
    }
    assert!(
        counting.seen.lock().unwrap().is_empty(),
        "none of them reached the app's handlers: {:?}",
        counting.seen.lock().unwrap()
    );
}

#[test]
fn the_module_answers_only_its_table_even_if_the_hub_forwards_more() {
    // A (wrongly) forwarded call for a name outside the table is -32601 and never reaches the
    // handlers.
    let counting = std::sync::Arc::new(Counting::default());
    let mut rig = Rig::with_devices(&granted_all(), counting.clone());
    rig.connect(&keyboard());
    for m in [
        "behaviors.invoke",
        "lighting.set",
        "key.event",
        "wallpaper-engine.pause",
        "hid.raw",
    ] {
        let call = json!({"jsonrpc":"2.0","id":5,"method":m,"params":{}});
        rig.link.line(&call.to_string()).unwrap();
        let out: Value = serde_json::from_slice(rig.link.writer_mut()).unwrap();
        assert_eq!(out["error"]["code"], json!(-32601), "{m}");
        rig.link.writer_mut().clear();
    }
    assert!(counting.seen.lock().unwrap().is_empty());
}

#[test]
fn a_call_before_the_hub_has_the_module_is_not_answered() {
    let counting = std::sync::Arc::new(Counting::default());
    let mut link = Link::new(Vec::new(), launch()).with_devices(counting.clone());
    let call = json!({"jsonrpc":"2.0","id":5,"method":"tray.panel","params":{}});
    link.line(&call.to_string()).unwrap();
    let out: Value = serde_json::from_slice(link.writer_mut()).unwrap();
    assert_eq!(out["error"]["code"], json!(2));
    assert!(counting.seen.lock().unwrap().is_empty());
    // No handlers at all answers the same.
    let mut bare = Link::new(Vec::new(), launch());
    bare.line(&call.to_string()).unwrap();
    let out: Value = serde_json::from_slice(bare.writer_mut()).unwrap();
    assert_eq!(out["error"]["code"], json!(2));
}

/// The standalone answer: the app's own handler, as the pipe's host would call it.
fn standalone(method: &str, params: &Value) -> Value {
    use foxglove_plugin_host::Devices;
    match crate::foxglove::api::AppDevices.call_from(0, method, params.clone()) {
        Ok(v) => json!({"result": v}),
        Err(e) => json!({"error": {"code": e.code, "message": e.message}}),
    }
}

#[test]
fn routed_answers_equal_the_standalone_handlers() {
    crate::foxglove::store::test_config_dir();
    let mut rig = Rig::with_devices(
        &granted_all(),
        std::sync::Arc::new(crate::foxglove::api::AppDevices),
    );
    rig.connect(&keyboard());
    let t = rig.tool(&["devices.read", "app.windows", "lighting.write"]);
    let nothing = json!({"device": "nothing"});
    let cases: [(&str, Value); 7] = [
        ("scenes.page", json!({})),
        ("tray.panel", json!({})),
        ("overlay.window", json!({})),
        ("pc_lighting.list", json!({})),
        ("lights.page", nothing.clone()),
        ("health.read", nothing),
        ("pc_lighting.set_zone", json!({"bogus": 1})),
    ];
    for (m, p) in cases {
        let via_hub = rig.call(&t, m, p.clone());
        let alone = standalone(m, &p);
        if alone.get("result").is_some() {
            assert_eq!(via_hub["result"], alone["result"], "{m}");
        } else {
            assert_eq!(
                via_hub["error"]["code"], alone["error"]["code"],
                "{m}: {via_hub} vs {alone}"
            );
            assert_eq!(
                via_hub["error"]["message"], alone["error"]["message"],
                "{m}"
            );
        }
    }
}

#[test]
fn module_mode_is_off_by_default_and_the_standalone_pipe_stays() {
    // This process was started without --module.
    assert!(!module_mode());
    // The link, the only code that registers the methods, starts only from a launch.
    let src = include_str!("../hub_link.rs");
    let start = src.find("pub fn start()").unwrap();
    assert!(src[start..start + 400].contains("LAUNCH.get()"));
    // api.rs serves the pipe in a process without the flag and none with it.
    let api = include_str!("../api.rs");
    let at = |s: &str| api.find(s).unwrap_or_else(|| panic!("{s}"));
    assert!(at("if super::hub_link::module_mode()") < at("pipe::listen"));
}

// ---- #1156: the legacy list, connection-bound arms, and the bounded worker ----

#[test]
fn served_is_exactly_the_routers_legacy_exception_list() {
    // The router lets exactly these names through the reserved areas for the adopted module
    // `foxglove` (#1155: temporary); the module serves exactly the same names.
    let mut served: Vec<&str> = SERVED.to_vec();
    let mut legacy: Vec<&str> = hideout_router::checks::LEGACY_FOXGLOVE_METHODS.to_vec();
    served.sort();
    legacy.sort();
    assert_eq!(served, legacy);
    assert_eq!(ID, hideout_router::checks::LEGACY_FOXGLOVE_ID);
}

/// The names of the arms of `call_from` that use the connection number (leases, wallpaper
/// pause, screens overrides), read from api.rs.
fn connection_bound_arms() -> Vec<String> {
    let api = include_str!("../api.rs");
    let start = api
        .find("fn call_from(&self, conn: u64")
        .expect("call_from");
    let end = start + api[start..].find("fn closed(&self").expect("closed");
    let mut arms: Vec<(Vec<String>, bool)> = Vec::new();
    for line in api[start..end].lines().skip(1) {
        let t = line.trim_start();
        if t.starts_with('"') && t.contains("=>") {
            let head = t.split("=>").next().unwrap();
            let names = head
                .split('"')
                .skip(1)
                .step_by(2)
                .map(String::from)
                .collect();
            arms.push((names, false));
        }
        if let Some(last) = arms.last_mut() {
            if line
                .split(|c: char| !(c.is_alphanumeric() || c == '_'))
                .any(|w| w == "conn")
            {
                last.1 = true;
            }
        }
    }
    arms.into_iter()
        .filter(|(_, uses)| *uses)
        .flat_map(|(names, _)| names)
        .collect()
}

#[test]
fn no_served_name_reaches_a_connection_bound_arm() {
    // All routed calls share ROUTED_CONN (0) and `closed(0)` never runs, so a served name that
    // kept anything per connection would share it between callers and never release it.
    let bound = connection_bound_arms();
    for must in [
        "lighting.lease",
        "lighting.update",
        "lighting.release",
        "wallpaper-engine.pause",
        "screens.panel_override",
    ] {
        assert!(
            bound.iter().any(|b| b == must),
            "{must} not found in {bound:?}"
        );
    }
    for m in SERVED {
        assert!(
            !bound.iter().any(|b| b == m),
            "{m} uses the connection: {bound:?}"
        );
    }
}

/// A routed request line.
fn routed_line(id: u64, method: &str) -> String {
    json!({"jsonrpc":"2.0","id":id,"method":method,"params":{}}).to_string()
}

fn written(rig: &mut Rig) -> Vec<Value> {
    let bytes = std::mem::take(rig.link.writer_mut());
    String::from_utf8(bytes)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

#[test]
fn calls_over_the_queue_bound_are_busy_and_nothing_runs_on_the_link_thread() {
    let counting = std::sync::Arc::new(Counting::default());
    let mut rig = Rig::with_devices(&granted_all(), counting.clone());
    rig.connect(&keyboard());
    let queue = rig.link.with_worker();
    let now = Instant::now();
    for i in 0..MAX_QUEUED_CALLS as u64 {
        let step = rig
            .link
            .line_at(&routed_line(i, "overlay.restore"), now)
            .unwrap();
        assert_eq!(step, Step::Nothing);
    }
    assert!(written(&mut rig).is_empty(), "queued, not answered");
    assert!(
        counting.seen.lock().unwrap().is_empty(),
        "nothing ran inline"
    );
    rig.link
        .line_at(&routed_line(99, "health.read"), now)
        .unwrap();
    let out = written(&mut rig);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0]["id"], json!(99));
    assert_eq!(error_code(&out[0]), 3, "BUSY (contract section 8.1)");
    assert!(counting.seen.lock().unwrap().is_empty());
    // The worker drains in order and the answers go out through the link.
    let mut n = 0;
    while let Some(job) = queue.try_pop() {
        let reply = execute(counting.as_ref(), job, now).expect("fresh");
        rig.link.respond(reply).unwrap();
        n += 1;
    }
    assert_eq!(n, MAX_QUEUED_CALLS);
    let out = written(&mut rig);
    assert_eq!(out.len(), MAX_QUEUED_CALLS);
    assert_eq!(out[0]["id"], json!(0));
    assert_eq!(out[0]["result"]["served"], "overlay.restore");
}

#[test]
fn a_call_the_router_already_timed_out_is_dropped_not_run() {
    let counting = std::sync::Arc::new(Counting::default());
    let mut rig = Rig::with_devices(&granted_all(), counting.clone());
    rig.connect(&keyboard());
    let queue = rig.link.with_worker();
    let t0 = Instant::now();
    rig.link
        .line_at(&routed_line(1, "tray.set_panel"), t0)
        .unwrap();
    let job = queue.try_pop().unwrap();
    // The worker got to it only after the expiry: no handler, no answer.
    let late = t0 + CALL_EXPIRY + Duration::from_secs(1);
    assert!(execute(counting.as_ref(), job, late).is_none());
    assert!(counting.seen.lock().unwrap().is_empty());
    // Just inside the expiry it still runs.
    rig.link
        .line_at(&routed_line(2, "tray.set_panel"), t0)
        .unwrap();
    let job = queue.try_pop().unwrap();
    let ok = t0 + CALL_EXPIRY - Duration::from_secs(1);
    assert!(execute(counting.as_ref(), job, ok).is_some());
    assert_eq!(counting.seen.lock().unwrap().len(), 1);
}

#[test]
fn expired_jobs_do_not_hold_the_queue_full() {
    let counting = std::sync::Arc::new(Counting::default());
    let mut rig = Rig::with_devices(&granted_all(), counting);
    rig.connect(&keyboard());
    let queue = rig.link.with_worker();
    let t0 = Instant::now();
    for i in 0..MAX_QUEUED_CALLS as u64 {
        rig.link
            .line_at(&routed_line(i, "overlay.restore"), t0)
            .unwrap();
    }
    let later = t0 + CALL_EXPIRY + Duration::from_secs(1);
    rig.link
        .line_at(&routed_line(500, "health.read"), later)
        .unwrap();
    assert!(written(&mut rig).is_empty(), "accepted, not BUSY");
    let job = queue.try_pop().unwrap();
    assert_eq!(job.id, json!(500));
    assert!(queue.try_pop().is_none(), "the stale ones are gone");
}

/// Stands for a 5-10 s device wait: it must never run on the link thread.
struct Stuck;

impl foxglove_plugin_host::Devices for Stuck {
    fn call_from(
        &self,
        _conn: u64,
        _method: &str,
        _params: Value,
    ) -> Result<Value, foxglove_plugin_api::rpc::Error> {
        panic!("a slow handler must not run on the link thread");
    }
}

#[test]
fn a_slow_handler_cannot_hold_up_facts_or_exit() {
    let mut rig = Rig::with_devices(&granted_all(), std::sync::Arc::new(Stuck));
    let facts = keyboard();
    rig.connect(&facts);
    let _queue = rig.link.with_worker();
    let now = Instant::now();
    for i in 0..3 {
        rig.link
            .line_at(&routed_line(i, "pc_lighting.set_zone"), now)
            .unwrap();
    }
    // Facts still go out at once...
    rig.link.writer_mut().clear();
    rig.link
        .change(&Change {
            key: "keyboard.game_mode".into(),
            value: Some(Fact::Bool(true)),
        })
        .unwrap();
    let out = written(&mut rig);
    assert_eq!(out.len(), 1, "{out:?}");
    assert_eq!(out[0]["method"], "facts.set");
    // ...and the hub's exit is taken at once.
    let exit = json!({"jsonrpc":"2.0","method":"module.exit","params":{}});
    assert_eq!(rig.link.line(&exit.to_string()).unwrap(), Step::Exit);
}

#[test]
fn a_dropped_link_closes_its_queue_and_runs_nothing_more() {
    let counting = std::sync::Arc::new(Counting::default());
    let mut rig = Rig::with_devices(&granted_all(), counting.clone());
    rig.connect(&keyboard());
    let queue = rig.link.with_worker();
    rig.link
        .line_at(&routed_line(1, "overlay.restore"), Instant::now())
        .unwrap();
    drop(rig);
    assert!(queue.wait().is_none(), "closed: the worker ends");
    assert!(counting.seen.lock().unwrap().is_empty());
}

#[test]
fn a_call_before_ready_is_refused_even_with_a_worker() {
    let counting = std::sync::Arc::new(Counting::default());
    let mut link = Link::new(Vec::new(), launch()).with_devices(counting);
    let queue = link.with_worker();
    link.line_at(&routed_line(5, "tray.panel"), Instant::now())
        .unwrap();
    let out: Value = serde_json::from_slice(link.writer_mut()).unwrap();
    assert_eq!(out["error"]["code"], json!(2));
    assert!(queue.try_pop().is_none());
}

/// A routed request line carrying the router's remaining time.
fn routed_with_deadline(id: u64, method: &str, left_ms: u64) -> String {
    json!({"jsonrpc":"2.0","id":id,"method":method,"params":{"_deadline_ms":left_ms,"k":1}})
        .to_string()
}

#[test]
fn a_call_expires_against_the_router_deadline_not_its_own_clock() {
    let counting = std::sync::Arc::new(Counting::default());
    let mut rig = Rig::with_devices(&granted_all(), counting.clone());
    rig.connect(&keyboard());
    let queue = rig.link.with_worker();
    let t0 = Instant::now();
    // 6 s left at the router: it is dropped once less than the margin remains.
    rig.link
        .line_at(&routed_with_deadline(1, "tray.set_panel", 6000), t0)
        .unwrap();
    let job = queue.try_pop().unwrap();
    assert!(execute(counting.as_ref(), job, t0 + Duration::from_secs(5)).is_none());
    rig.link
        .line_at(&routed_with_deadline(2, "tray.set_panel", 6000), t0)
        .unwrap();
    let job = queue.try_pop().unwrap();
    assert!(execute(counting.as_ref(), job, t0 + Duration::from_secs(3)).is_some());
    // The key never reaches the handler.
    let seen = counting.seen.lock().unwrap();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].1, json!({"k": 1}));
}

#[test]
fn a_call_that_arrives_with_no_time_left_is_dropped_at_once() {
    let counting = std::sync::Arc::new(Counting::default());
    let mut rig = Rig::with_devices(&granted_all(), counting.clone());
    rig.connect(&keyboard());
    let queue = rig.link.with_worker();
    rig.link
        .line_at(&routed_with_deadline(1, "tray.panel", 1500), Instant::now())
        .unwrap();
    assert!(queue.try_pop().is_none(), "never queued");
    assert!(
        written(&mut rig).is_empty(),
        "the caller was told by the router"
    );
    assert!(counting.seen.lock().unwrap().is_empty());
}

#[test]
fn the_deadline_key_is_taken_out_of_the_params() {
    let counting = std::sync::Arc::new(Counting::default());
    let mut rig = Rig::with_devices(&granted_all(), counting.clone());
    rig.connect(&keyboard());
    // Inline (no worker): the handler is called with the key removed.
    rig.link
        .line(&routed_with_deadline(1, "overlay.restore", 30000))
        .unwrap();
    let out = written(&mut rig);
    assert_eq!(out.len(), 1, "{out:?}");
    let seen = counting.seen.lock().unwrap();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].1, json!({"k": 1}));
}

/// A handler that panics.
struct Boom;

impl foxglove_plugin_host::Devices for Boom {
    fn call_from(
        &self,
        _conn: u64,
        _method: &str,
        _params: Value,
    ) -> Result<Value, foxglove_plugin_api::rpc::Error> {
        panic!("a handler bug");
    }
}

#[test]
fn a_panicking_handler_answers_an_internal_error_and_the_worker_lives() {
    let mut rig = Rig::with_devices(&granted_all(), std::sync::Arc::new(Boom));
    rig.connect(&keyboard());
    let queue = rig.link.with_worker();
    let now = Instant::now();
    rig.link
        .line_at(&routed_line(1, "overlay.restore"), now)
        .unwrap();
    rig.link
        .line_at(&routed_line(2, "overlay.restore"), now)
        .unwrap();
    let boom = Boom;
    while let Some(job) = queue.try_pop() {
        let reply = execute(&boom, job, now).expect("answered, not dropped");
        rig.link.respond(reply).unwrap();
    }
    let out = written(&mut rig);
    assert_eq!(out.len(), 2);
    assert_eq!(error_code(&out[0]), -32603);
    assert_eq!(error_code(&out[1]), -32603);
}

#[test]
fn a_dead_worker_closes_the_queue_and_tells_the_link() {
    let counting = std::sync::Arc::new(Counting::default());
    let queue = CallQueue::default();
    let now = Instant::now();
    assert!(queue.push(
        Job {
            id: json!(1),
            method: "tray.panel".into(),
            params: json!({}),
            expires: now + CALL_EXPIRY,
        },
        now
    ));
    let lost = std::cell::Cell::new(false);
    // Delivering the answer panics (outside the handler): the worker dies.
    run_worker(
        &queue,
        counting.as_ref(),
        |_| panic!("deliver failed"),
        || lost.set(true),
    );
    assert!(lost.get(), "the link is told");
    assert!(queue.wait().is_none(), "the queue is closed");
}

#[test]
fn a_slow_call_does_not_hold_the_other_workers() {
    use std::sync::mpsc;
    // The first call blocks until released; the pool still answers the next one.
    struct Gate(std::sync::Mutex<Option<mpsc::Receiver<()>>>);
    impl foxglove_plugin_host::Devices for Gate {
        fn call_from(
            &self,
            _conn: u64,
            method: &str,
            _params: Value,
        ) -> Result<Value, foxglove_plugin_api::rpc::Error> {
            if method == "pc_lighting.set_zone" {
                let rx = self.0.lock().unwrap().take();
                if let Some(rx) = rx {
                    let _ = rx.recv_timeout(Duration::from_secs(10));
                }
            }
            Ok(json!({"served": method}))
        }
    }
    let (release, hold) = mpsc::channel();
    let devices = std::sync::Arc::new(Gate(std::sync::Mutex::new(Some(hold))));
    let queue = std::sync::Arc::new(CallQueue::default());
    let (rtx, rrx) = mpsc::channel::<Reply>();
    let mut threads = Vec::new();
    for _ in 0..CALL_WORKERS {
        let (q, d, tx) = (queue.clone(), devices.clone(), rtx.clone());
        threads.push(std::thread::spawn(move || {
            work(&q, d.as_ref(), |r| {
                let _ = tx.send(r);
            })
        }));
    }
    let now = Instant::now();
    let job = |id: u64, m: &str| Job {
        id: json!(id),
        method: m.into(),
        params: json!({}),
        expires: now + Duration::from_secs(60),
    };
    assert!(queue.push(job(1, "pc_lighting.set_zone"), now));
    assert!(queue.push(job(2, "health.read"), now));
    let quick = rrx
        .recv_timeout(Duration::from_secs(5))
        .expect("not held up");
    assert_eq!(quick.id, json!(2));
    let _ = release.send(());
    let slow = rrx.recv_timeout(Duration::from_secs(5)).expect("released");
    assert_eq!(slow.id, json!(1));
    queue.close();
    for t in threads {
        t.join().unwrap();
    }
}
