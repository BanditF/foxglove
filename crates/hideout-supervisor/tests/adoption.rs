// SPDX-License-Identifier: MIT
//! Taking modules back after a hub restart (§7.9, #986), with real windowless processes: a copy
//! of `ping.exe` installed as a developer-mode module. A "hub restart" is the first supervisor
//! dropped (its Job Object handles close, as when hub.exe dies) and a second one started on the
//! same folder. Every process a test starts is ended by its handle when the test ends.
#![cfg(windows)]

use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;

use hideout_registry::Registry;
use hideout_router::server::Hub;
use hideout_router::{Router, Settings};
use hideout_supervisor::driver::Supervisor;
use hideout_supervisor::instances::Instances;
use hideout_supervisor::win::{Job, Process};

const NOW: &str = "2026-11-09T00:00:00Z";

/// Ends every process it holds when dropped, by handle (never by name).
#[derive(Default)]
struct Reaper(Vec<Process>);

impl Reaper {
    fn hold(&mut self, pid: u32) -> &Process {
        self.0
            .push(Process::open(pid).expect("open a process the test started"));
        self.0.last().unwrap()
    }
}

impl Drop for Reaper {
    fn drop(&mut self) {
        for p in &self.0 {
            p.kill(1);
            let _ = p.wait();
        }
    }
}

/// A hub folder with `id` installed (developer mode) and enabled: `run = ping.exe -n 60`.
fn home_with(id: &str) -> (PathBuf, PathBuf) {
    std::env::set_var("HIDEOUT_DEV_UNSIGNED", "1");
    let base = std::env::temp_dir().join(format!("hideout-adopt-{}-{id}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let folder = base.join("src");
    std::fs::create_dir_all(&folder).unwrap();
    let windir = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
    std::fs::copy(
        Path::new(&windir).join("System32").join("PING.EXE"),
        folder.join("ping.exe"),
    )
    .unwrap();
    std::fs::write(
        folder.join("module.toml"),
        format!(
            "id = \"{id}\"\nname = \"Lab\"\nversion = \"0.1.0\"\nkind = \"service\"\n\
             family = \"third-party\"\nhub = \"1.0\"\nvisibility = \"private\"\n\
             run = [\"ping.exe\", \"-n\", \"60\", \"127.0.0.1\"]\n"
        ),
    )
    .unwrap();
    let home = base.join("home");
    let mut reg = Registry::open(&home, true).unwrap();
    reg.install(&folder, NOW).unwrap();
    reg.set_enabled(id, true).unwrap();
    let exe = reg.module_dir(id, "0.1.0").join("ping.exe");
    (home, exe)
}

fn hub() -> Hub {
    let router = Router::new(Settings::default(), Box::new(|_, asked| asked.to_vec()));
    let (events, _rx) = mpsc::channel();
    Hub::new(router, events)
}

fn state(hub: &Hub, id: &str) -> Option<String> {
    hub.read(|r| {
        r.fact(&format!("module.{id}.state"))
            .and_then(|v| v.as_str())
            .map(String::from)
    })
}

/// The first supervisor goes the way a dying hub does: its handles close, its module runs on.
fn hub_dies(sup: Supervisor) {
    drop(sup);
}

#[test]
fn a_restarted_hub_takes_the_module_back_and_starts_no_second_one() {
    const ID: &str = "example.lab.adopt";
    let (home, exe) = home_with(ID);
    let mut reaper = Reaper::default();

    let sup = Supervisor::start(&home, hub());
    let pid = sup.pid(ID).expect("the module was not started");
    let first = reaper.hold(pid);
    let rec = Instances::load(&home).modules[ID].clone();
    assert_eq!(rec.pid, pid);
    assert_eq!(rec.started, Some(first.started().unwrap()));
    assert!(rec.exe.ends_with("ping.exe") && rec.exe == exe);

    hub_dies(sup);
    // The premise of #986: the job's name went with the last handle; the module runs on.
    assert!(
        Job::open(&rec.job).is_err(),
        "the job name outlived the hub"
    );
    assert!(first.running());
    // The state file survives the hub.
    assert_eq!(Instances::load(&home).modules[ID], rec);

    let hub2 = hub();
    let sup2 = Supervisor::start(&home, hub2.clone());
    assert_eq!(sup2.pid(ID), Some(pid), "started a second instance");
    assert_eq!(state(&hub2, ID).as_deref(), Some("starting"));
    assert!(hub2.read(|r| r
        .fact(&format!("module.{ID}.restart"))
        .is_none_or(|v| v.is_null())));
    let now = Instances::load(&home).modules[ID].clone();
    assert_eq!(
        (now.pid, now.started, &now.token),
        (pid, rec.started, &rec.token)
    );
    assert!(!now.job.is_empty(), "not put in a fresh Job Object");
    // The fresh job holds it: ending the job ends the module.
    let job = Job::open(&now.job).unwrap();
    drop(sup2);
    job.kill(5);
    assert_eq!(first.wait().unwrap(), 5);
    drop(reaper);
    let _ = std::fs::remove_dir_all(home.parent().unwrap());
}

#[test]
fn our_process_with_a_changed_exe_is_ended_and_started_fresh() {
    const ID: &str = "example.lab.orphan";
    let (home, _) = home_with(ID);
    let mut reaper = Reaper::default();

    let sup = Supervisor::start(&home, hub());
    let pid = sup.pid(ID).expect("the module was not started");
    let first = reaper.hold(pid);
    hub_dies(sup);
    // The record's hash no longer matches the running exe.
    let mut inst = Instances::load(&home);
    inst.modules.get_mut(ID).unwrap().sha256 = "0".repeat(64);
    inst.save(&home).unwrap();

    let sup2 = Supervisor::start(&home, hub());
    assert_eq!(first.wait().unwrap(), 1, "the orphan was not ended");
    let fresh = sup2.pid(ID).expect("no fresh instance");
    reaper.hold(fresh);
    assert_ne!(fresh, pid);
    drop(sup2);
    drop(reaper);
    let _ = std::fs::remove_dir_all(home.parent().unwrap());
}

#[test]
fn a_reused_pid_is_left_alone_and_the_module_starts_fresh() {
    const ID: &str = "example.lab.reuse";
    let (home, exe) = home_with(ID);
    let mut reaper = Reaper::default();

    // A process at the recorded pid, same exe, but started at another time than recorded: not
    // the one the last hub started, so not ours to end or take back.
    let mut cmd = Command::new(&exe);
    cmd.args(["-n", "60", "127.0.0.1"])
        .stdout(Stdio::null())
        .creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    let mut stranger = cmd.spawn().unwrap();
    let other = reaper.hold(stranger.id());
    let real = other.started().unwrap();
    let mut inst = Instances::default();
    inst.modules.insert(
        ID.into(),
        hideout_supervisor::instances::Record {
            instance: 1,
            pid: stranger.id(),
            started: Some(real - 1),
            token: "t".into(),
            exe: exe.clone(),
            sha256: hideout_supervisor::sha256_file(&exe).unwrap(),
            job: String::new(),
        },
    );
    inst.save(&home).unwrap();

    let sup = Supervisor::start(&home, hub());
    let fresh = sup.pid(ID).expect("no fresh instance");
    reaper.hold(fresh);
    assert_ne!(fresh, stranger.id());
    assert!(reaper.0[0].running(), "a process that isn't ours was ended");
    drop(sup);
    drop(reaper);
    let _ = stranger.wait();
    let _ = std::fs::remove_dir_all(home.parent().unwrap());
}

/// Safe mode (§7.10, #1029): a hub started with `hideout.toml`'s `safe_mode` starts nothing;
/// **Start normally** clears the flag, turns `hub.safe_mode` off and starts the enabled module
/// in the same hub, with no restart.
#[test]
fn start_normally_leaves_safe_mode_and_starts_the_module() {
    const ID: &str = "example.lab.safe";
    let (home, _) = home_with(ID);
    let mut reaper = Reaper::default();
    let mut reg = Registry::open(&home, true).unwrap();
    reg.set_safe_mode(true).unwrap();
    assert!(Registry::open(&home, true).unwrap().hub.safe_mode());

    let settings = Settings {
        safe_mode: true,
        ..Settings::default()
    };
    let router = Router::new(settings, Box::new(|_, asked| asked.to_vec()));
    let (events, _rx) = mpsc::channel();
    let hub = Hub::new(router, events);
    let sup = Supervisor::start(&home, hub.clone());
    assert!(sup.safe_mode());
    assert_eq!(sup.pid(ID), None, "safe mode started a module");
    let safe = |h: &Hub| h.read(|r| r.fact("hub.safe_mode").and_then(|v| v.as_bool()));
    assert_eq!(safe(&hub), Some(true));

    sup.start_normally();
    let pid = sup.pid(ID).expect("Start normally didn't start the module");
    reaper.hold(pid);
    assert!(!sup.safe_mode());
    assert_eq!(safe(&hub), Some(false));
    assert_eq!(state(&hub, ID).as_deref(), Some("starting"));
    assert!(
        !Registry::open(&home, true).unwrap().hub.safe_mode(),
        "the flag is still in hideout.toml"
    );
    // A second press starts no second instance.
    sup.start_normally();
    assert_eq!(sup.pid(ID), Some(pid));
    drop(sup);
    drop(reaper);
    let _ = std::fs::remove_dir_all(home.parent().unwrap());
}

/// Finding the module that breaks Hideout (Â§7.10, #1033), with a real windowless module: from
/// safe mode the banner offers "Check whether Lab breaks Hideout"; the round starts it in this
/// hub; Stop finding ends it with nothing saved and safe mode still on; a round that closes
/// names it, and Turn off saves it off and leaves safe mode.
#[test]
fn a_search_round_starts_its_module_and_stop_changes_nothing() {
    use hideout_supervisor::bisect::{words, Act, Exit, Kind, Search};
    use hideout_supervisor::finder::Finder;
    const ID: &str = "example.lab.bisect";
    let (home, _) = home_with(ID);
    let mut reaper = Reaper::default();
    Registry::open(&home, true)
        .unwrap()
        .set_safe_mode(true)
        .unwrap();
    let settings = Settings {
        safe_mode: true,
        ..Settings::default()
    };
    let router = Router::new(settings, Box::new(|_, asked| asked.to_vec()));
    let (events, _rx) = mpsc::channel();
    let hub = Hub::new(router, events);
    let sup = Supervisor::start(&home, hub.clone());
    let finder = Finder::start(&home, sup.clone(), Exit::Clean, "test");
    assert!(!finder.searching());
    assert_eq!(finder.banner().find, Some(words::check_one("Lab")));

    finder.begin();
    let pid = sup.pid(ID).expect("the round didn't start its module");
    reaper.hold(pid);
    assert_eq!(state(&hub, ID).as_deref(), Some("starting"));
    let v = finder.view().unwrap();
    assert_eq!(
        (v.kind, v.title.as_str()),
        (Kind::Round, words::title(1, 1).as_str())
    );
    assert_eq!(v.lines[0], "Only Lab is on.");
    assert!(
        v.ends_in.is_some_and(|d| d.as_secs() > 290),
        "{:?}",
        v.ends_in
    );
    assert!(Search::path(&home).exists(), "progress not kept");
    assert_eq!(
        finder.tray().unwrap().line,
        "Hideout \u{b7} finding the module (round 1 of 1)"
    );

    finder.act(Act::Stop);
    assert!(!finder.searching());
    assert!(finder.banner().stopped);
    assert!(!Search::path(&home).exists(), "Stop left the progress file");
    assert_eq!(state(&hub, ID), None, "the module still runs after Stop");
    assert!(sup.safe_mode(), "Stop left safe mode");
    let reg = Registry::open(&home, true).unwrap();
    assert!(
        reg.hub.enabled(ID) && reg.hub.safe_mode(),
        "Stop changed what is saved"
    );

    // Again; this time it closes (the user says so): found, nothing on until the choice.
    finder.begin();
    if let Some(pid) = sup.pid(ID) {
        reaper.hold(pid);
    }
    finder.act(Act::Closed);
    let v = finder.view().unwrap();
    assert_eq!(v.kind, Kind::Found);
    assert_eq!(v.title, "Lab seems to make Hideout close.");
    assert_eq!(state(&hub, ID), None, "the module still runs at the result");
    assert!(Registry::open(&home, true).unwrap().hub.enabled(ID));
    finder.act(Act::TurnOff);
    assert!(!finder.searching());
    let reg = Registry::open(&home, true).unwrap();
    assert!(!reg.hub.enabled(ID), "Turn off wasn't saved");
    assert!(
        !reg.hub.safe_mode(),
        "the result's choice didn't leave safe mode"
    );
    assert!(!sup.safe_mode());
    drop(finder);
    drop(sup);
    drop(reaper);
    let _ = std::fs::remove_dir_all(home.parent().unwrap());
}
