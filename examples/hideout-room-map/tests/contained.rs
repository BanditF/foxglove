// SPDX-License-Identifier: MIT
//! #1092: the room map's live preview works with the module in an AppContainer, end to end and
//! window-free. A lab hub with `[supervisor] appcontainer = true` starts the real
//! `hideout-room-map.exe` (a copy, installed as a module) in its container; the hub's panel host
//! opens its `map` channel through the router as it does for a page on screen, and draws its
//! frames. Before, the contained module couldn't make its channel pipe at all (an AppContainer
//! may make pipes only in its own namespace), and the preview said "This preview couldn't be
//! shown".
#![cfg(windows)]

use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use hideout_panels::stream_io::StreamIo;
use hideout_panels::streams::{StreamCommand, StreamKey};
use hideout_panels::{PanelHost, PanelKey};
use hideout_registry::Registry;
use hideout_room_map::{panels, ID};
use hideout_router::server::{folder_suffix, Hub};
use hideout_router::{Out, Router, Settings};
use hideout_supervisor::driver::Supervisor;
use hideout_supervisor::instances::Instances;
use hideout_supervisor::sandbox;
use hideout_supervisor::win::{Job, Process};

const NOW: &str = "2026-11-10T00:00:00Z";

#[test]
fn the_room_maps_preview_streams_from_its_appcontainer() {
    std::env::set_var("HIDEOUT_DEV_UNSIGNED", "1");
    let base = std::env::temp_dir().join(format!("hideout-room-map-ac-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let folder = base.join("src");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::copy(
        env!("CARGO_BIN_EXE_hideout-room-map"),
        folder.join("hideout-room-map.exe"),
    )
    .unwrap();
    std::fs::write(folder.join("module.toml"), include_str!("../module.toml")).unwrap();
    let home = base.join("home");
    let mut reg = Registry::open(&home, true).unwrap();
    reg.install(&folder, NOW).unwrap();
    let list = reg.permission_list(ID).unwrap();
    let all: Vec<String> = list.items.iter().map(|i| i.raw.clone()).collect();
    reg.approve_listed(ID, list.code(), &all, &[], NOW, "dialog")
        .unwrap();
    reg.set_enabled(ID, true).unwrap();
    let toml = home.join("hideout.toml");
    let mut text = std::fs::read_to_string(&toml).unwrap_or_default();
    text.push_str("\n[supervisor]\nappcontainer = true\n");
    std::fs::write(&toml, text).unwrap();

    let mut router = Router::new(Settings::default(), Box::new(|_, asked| asked.to_vec()));
    router.load_registry(&Registry::open(&home, true).unwrap());
    let (events, outs) = mpsc::channel();
    let hub = Hub::new(router, events);
    hub.set_folder_suffix(&folder_suffix(&home));
    hub.serve_pipe(&hub.router_pipe()).unwrap();
    let sup = Supervisor::start(&home, hub.clone());

    // Each router event goes to the supervisor too; `stop` picks the one waited for (30 s
    // only as the fallback).
    let next = |stop: &mut dyn FnMut(Out) -> bool| {
        let deadline = Instant::now() + Duration::from_secs(30);
        while let Some(left) = deadline.checked_duration_since(Instant::now()) {
            match outs.recv_timeout(left) {
                Ok(o) => {
                    sup.on_router(&o);
                    if stop(o) {
                        return true;
                    }
                }
                Err(_) => break,
            }
        }
        false
    };
    let log =
        || std::fs::read_to_string(home.join("logs").join(format!("{ID}.log"))).unwrap_or_default();

    // Its page registers (with the instance the supervisor gave it), then it is ready.
    let mut instance = None;
    let up = next(&mut |o| match o {
        Out::Panels {
            module,
            instance: i,
            ..
        } if module == ID => {
            instance = Some(i);
            false
        }
        Out::Ready(m) => m == ID,
        Out::Refused(m) => panic!("the hub refused {m}"),
        _ => false,
    });
    assert!(up, "the room map never got ready; its log: {}", log());
    let pid = sup.pid(ID).expect("started");
    let contained = sandbox::is_app_container(&Process::open(pid).unwrap()).unwrap();

    // The panel host shows its page: the stream opens through the router.
    let mut host = PanelHost::default();
    let instance = instance.expect("ui.register before module.ready");
    assert!(host
        .register(ID, "Room map", None, &panels(&instance))
        .is_empty());
    let page = PanelKey::new(ID, "map");
    let rooms = StreamKey::new(page.clone(), "rooms");
    let (wake_tx, wake) = mpsc::channel();
    let wake_tx = Mutex::new(wake_tx);
    let mut io = StreamIo::new(Arc::new(move || {
        let _ = wake_tx.lock().unwrap().send(());
    }));
    host.set_visible(std::slice::from_ref(&page));
    host.take_visibility();
    host.sync_streams(Instant::now());
    let (_, opens) = io.pump(&mut host.streams, Instant::now());
    assert_eq!(opens.len(), 1);
    for o in opens {
        let StreamCommand::Open {
            module, channel, ..
        } = o
        else {
            unreachable!()
        };
        hub.act(|r| r.open_host_channel(&module, &channel, Instant::now()));
    }
    let mut grant = None;
    let answered = next(&mut |o| match o {
        Out::HostChannel {
            module,
            channel,
            result,
        } => {
            grant = Some((module, channel, result));
            true
        }
        _ => false,
    });
    assert!(answered, "no answer to the stream's open");
    let (module, channel, result) = grant.unwrap();
    let granted = result.clone();
    io.granted(&mut host.streams, &module, &channel, result);

    // Frames arrive: the reader wakes the host, which draws (no polling).
    let until = Instant::now() + Duration::from_secs(10);
    let mut drawn = 0;
    while drawn < 5 && Instant::now() < until {
        io.pump(&mut host.streams, Instant::now());
        drawn = host.streams.get(&rooms).map_or(0, |s| s.drawn);
        let next = host.streams.next_wake().map_or(until, |t| t.min(until));
        let _ = wake.recv_timeout(next.saturating_duration_since(Instant::now()));
    }
    let profile = sandbox::profile_sid(ID);

    let rec = Instances::load(&home).modules.get(ID).cloned();
    if let Some(job) = rec.and_then(|r| Job::open(&r.job).ok()) {
        job.kill(1);
    }
    drop(io);
    drop(sup);
    sandbox::delete_profile(ID);
    let log =
        std::fs::read_to_string(home.join("logs").join(format!("{ID}.log"))).unwrap_or_default();
    let _ = std::fs::remove_dir_all(&base);

    assert!(contained, "the room map isn't in an AppContainer");
    let g = granted.unwrap_or_else(|e| panic!("the stream wasn't granted: {e}; log: {log}"));
    let pipe = g["pipe"].as_str().unwrap_or_default();
    assert!(
        pipe.contains(&format!(
            r"\AppContainerNamedObjects\{}\hideout-ch-",
            profile.unwrap()
        )),
        "its pipe is in its own namespace: {pipe}"
    );
    assert_eq!(g["pid"], pid, "the grant names the module's pid");
    assert!(
        drawn >= 5,
        "{drawn} frames drawn from the contained room map; its log: {log}"
    );
}
