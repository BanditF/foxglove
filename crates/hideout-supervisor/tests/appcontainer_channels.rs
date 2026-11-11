// SPDX-License-Identifier: MIT
//! #1092, window-free, with real contained children (copies of `hideout-ac-probe`) started by a
//! lab hub with `[supervisor] appcontainer = true`:
//! - a contained consumer reads a channel of a contained provider, and of one outside a
//!   container (granted `net`): the provider's pipe lets the consumer's SID in, a contained
//!   provider makes its pipe in its own namespace, the consumer checks the provider's pid
//!   before it sends the nonce, and the provider admits only the consumer's pid;
//! - a stdio (bridged) module runs contained: its stdin and stdout are the hub's anonymous
//!   pipes, it can't read the user's file, it can write its `data\`, and the router pipe gets
//!   no guest for it.
#![cfg(windows)]

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use hideout_registry::Registry;
use hideout_router::server::{folder_suffix, Hub};
use hideout_router::{Out, Router, Settings};
use hideout_supervisor::driver::Supervisor;
use hideout_supervisor::instances::Instances;
use hideout_supervisor::sandbox;
use hideout_supervisor::win::{Job, Process};

const NOW: &str = "2026-11-10T00:00:00Z";

/// One module of a lab hub: its id and its manifest's lines after `hub`.
struct Spec<'a> {
    id: &'a str,
    rest: String,
    /// Enabled at hub start; else turned on by the test.
    on: bool,
}

struct Lab {
    base: PathBuf,
    home: PathBuf,
    hub: Hub,
    sup: Supervisor,
    rx: Receiver<Out>,
    ids: Vec<String>,
}

/// A lab hub folder (`name`) with each module installed from its own folder holding a copy of
/// the probe, approved and enabled, the AppContainer setting on, and the hub started.
fn lab(name: &str, modules: &[Spec]) -> Lab {
    std::env::set_var("HIDEOUT_DEV_UNSIGNED", "1");
    let base = std::env::temp_dir().join(format!("hideout-acch-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let home = base.join("home");
    let mut reg = Registry::open(&home, true).unwrap();
    for m in modules {
        let folder = base.join("src").join(m.id);
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::copy(
            env!("CARGO_BIN_EXE_hideout-ac-probe"),
            folder.join("probe.exe"),
        )
        .unwrap();
        std::fs::write(
            folder.join("module.toml"),
            format!(
                "id = \"{}\"\nname = \"Probe {}\"\nversion = \"0.1.0\"\nkind = \"service\"\n\
                 family = \"third-party\"\nhub = \"1.0\"\nvisibility = \"private\"\n{}\n",
                m.id, m.id, m.rest
            ),
        )
        .unwrap();
        reg.install(&folder, NOW).unwrap();
        reg.approve(m.id).unwrap();
        reg.set_enabled(m.id, m.on).unwrap();
    }
    let toml = home.join("hideout.toml");
    let mut text = std::fs::read_to_string(&toml).unwrap_or_default();
    text.push_str("\n[supervisor]\nappcontainer = true\n");
    std::fs::write(&toml, text).unwrap();
    let mut router = Router::new(Settings::default(), Box::new(|_, asked| asked.to_vec()));
    router.load_registry(&Registry::open(&home, true).unwrap());
    let (events, rx) = mpsc::channel();
    let hub = Hub::new(router, events);
    hub.set_folder_suffix(&folder_suffix(&home));
    hub.serve_pipe(&hub.router_pipe()).unwrap();
    let sup = Supervisor::start(&home, hub.clone());
    Lab {
        base,
        home,
        hub,
        sup,
        rx,
        ids: modules.iter().map(|m| m.id.to_string()).collect(),
    }
}

impl Lab {
    /// Hands the router's events to the supervisor (so a consumer starts once its provider
    /// runs) until `stop` matches one: the stop event. 30 s only as the fallback.
    fn until(&self, stop: impl Fn(&Out) -> bool) -> bool {
        let deadline = Instant::now() + Duration::from_secs(30);
        while let Some(left) = deadline.checked_duration_since(Instant::now()) {
            match self.rx.recv_timeout(left) {
                Ok(out) => {
                    self.sup.on_router(&out);
                    if let Out::Refused(m) = &out {
                        panic!("the hub refused its own module {m}");
                    }
                    if stop(&out) {
                        return true;
                    }
                }
                Err(_) => break,
            }
        }
        false
    }

    fn log(&self, id: &str) -> String {
        std::fs::read_to_string(self.home.join("logs").join(format!("{id}.log")))
            .unwrap_or_default()
    }

    /// Ends every module's job, removes the profiles and the folder.
    fn finish(self) {
        let records = Instances::load(&self.home);
        for id in &self.ids {
            if let Some(rec) = records.modules.get(id) {
                if let Ok(job) = Job::open(&rec.job) {
                    job.kill(1);
                }
            }
        }
        drop(self.sup);
        for id in &self.ids {
            sandbox::delete_profile(id);
        }
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

fn in_container(pid: Option<u32>) -> Option<String> {
    sandbox::container_sid(&Process::open(pid?).ok()?).ok()?
}

/// The consumer probe's run line for `provider`'s channel `c`.
fn consumer(id: &str, provider: &str) -> String {
    format!(
        "permissions = [\"channel.open:{provider}/c\"]\n\
         run = [\"probe.exe\", \"--consume\", \"{id}\", \"{provider}/c\"]"
    )
}

fn provider(id: &str, permissions: &str) -> String {
    format!(
        "provides = [\"channel:c\"]\npermissions = [{permissions}]\n\
         run = [\"probe.exe\", \"--provide\", \"{id}\"]"
    )
}

/// Runs a provider and a contained consumer of its channel `c`; returns the provider's pid and
/// container, the consumer's container and both logs.
fn channel_run(
    name: &str,
    prov: &str,
    prov_permissions: &str,
    cons: &str,
) -> (Option<u32>, Option<String>, Option<String>, String, String) {
    let lab = lab(
        name,
        &[
            Spec {
                id: prov,
                rest: provider(prov, prov_permissions),
                on: true,
            },
            Spec {
                id: cons,
                rest: consumer(cons, prov),
                on: false,
            },
        ],
    );
    // The consumer starts once its provider is ready, as a user would turn it on.
    let up = lab.until(|o| matches!(o, Out::Ready(m) if m == prov));
    assert!(up, "the provider never got ready: {}", lab.log(prov));
    lab.sup.turn_on(cons).unwrap();
    // The consumer exits after one frame: its connection ending is the stop event.
    let gone = lab.until(|o| matches!(o, Out::ModuleGone(m) if m == cons));
    let prov_pid = lab.sup.pid(prov);
    let prov_sid = in_container(prov_pid);
    let (cons_log, prov_log) = (lab.log(cons), lab.log(prov));
    // The consumer's container: its profile's SID (it has exited by now).
    let cons_sid = sandbox::profile_sid(cons);
    let guests = lab.hub.pipe_guests();
    lab.finish();
    assert!(
        gone,
        "the consumer never ended; its log: {cons_log}\nprovider: {prov_log}"
    );
    assert!(
        cons_sid.as_ref().is_some_and(|s| guests.contains(s)),
        "the consumer ran contained: {guests:?}"
    );
    (prov_pid, prov_sid, cons_sid, cons_log, prov_log)
}

#[test]
fn a_contained_consumer_reads_a_contained_providers_channel() {
    const PROV: &str = "example.lab.acch-prov";
    const CONS: &str = "example.lab.acch-cons";
    let (prov_pid, prov_sid, _, cons_log, prov_log) = channel_run("both", PROV, "", CONS);
    let prov_sid = prov_sid.expect("the provider runs in an AppContainer");
    assert!(
        cons_log.contains(&format!("probe: channel frame: from {PROV}")),
        "consumer: {cons_log}\nprovider: {prov_log}"
    );
    // Its pipe is in its own namespace, and the grant named its pid.
    assert!(
        cons_log.contains(&format!(r"AppContainerNamedObjects\{prov_sid}\hideout-ch-")),
        "{cons_log}"
    );
    assert!(
        cons_log.contains(&format!("(pid Some({}))", prov_pid.unwrap())),
        "{cons_log}"
    );
}

#[test]
fn a_contained_consumer_reads_a_channel_from_outside_a_container() {
    const PROV: &str = "example.lab.acch-out";
    const CONS: &str = "example.lab.acch-in";
    // `net`, granted, keeps the provider outside (§7.7).
    let (prov_pid, prov_sid, _, cons_log, prov_log) = channel_run("mixed", PROV, "\"net\"", CONS);
    assert_eq!(prov_sid, None, "the provider runs outside");
    assert!(
        cons_log.contains(&format!("probe: channel frame: from {PROV}")),
        "consumer: {cons_log}\nprovider: {prov_log}"
    );
    assert!(
        cons_log.contains(r"granted \\.\pipe\hideout-ch-"),
        "{cons_log}"
    );
    assert!(
        cons_log.contains(&format!("(pid Some({}))", prov_pid.unwrap())),
        "{cons_log}"
    );
}

#[test]
fn a_stdio_module_runs_contained() {
    const ID: &str = "example.lab.acch-stdio";
    let outside = std::env::temp_dir().join(format!("hideout-acch-outside-{}", std::process::id()));
    std::fs::write(&outside, "the user's").unwrap();
    let lab = lab(
        "stdio",
        &[Spec {
            id: ID,
            rest: format!(
                "transport = \"stdio\"\napi = {{ foxglove = \"1.2\" }}\n\
                 permissions = [\"lighting.write\"]\nrun = [\"probe.exe\", \"--stdio\", '{}']",
                outside.display()
            ),
            on: true,
        }],
    );
    let up = lab.until(|o| matches!(o, Out::Ready(m) if m == ID));
    let pid = lab.sup.pid(ID);
    let sid = in_container(pid);
    let guests = lab.hub.pipe_guests();
    // The probe wrote its lines and its file before it connected.
    let data = installed(&lab.home, ID).map(|d| d.join("data").join("probe.txt"));
    let wrote = data.as_ref().is_some_and(|d| d.is_file());
    let log = lab.log(ID);
    lab.finish();
    let _ = std::fs::remove_file(&outside);
    assert!(up, "the bridged module never got ready; its log: {log}");
    assert_eq!(sid, sandbox::profile_sid(ID), "in its own AppContainer");
    assert!(sid.is_some(), "not contained");
    assert!(
        guests.is_empty(),
        "a stdio module needs no router pipe guest: {guests:?}"
    );
    assert!(log.contains("probe: outside file: refused"), "{log}");
    assert!(log.contains("probe: data write: ok") && wrote, "{log}");
}

/// The installed version folder of `id` (where it runs and its `data\` is).
fn installed(home: &Path, id: &str) -> Option<PathBuf> {
    let reg = Registry::open(home, true).ok()?;
    reg.pre_launch(id).ok().map(|l| l.dir)
}
