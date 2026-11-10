// SPDX-License-Identifier: MIT
//! #1013: a module's own channel, declared only by its manifest's `provides =
//! ["channel:<name>"]` (§4.5, §8.4), registers and opens. The sample room map, loaded the way a
//! lab hub loads it (`<config>\lab-modules\`, #1001), registers `map` over a real router pipe and
//! the panel host opens it. Before, only the grant `channel.offer:map` let a module offer a
//! channel; nothing turned the manifest's declaration into one, so the registration's channel
//! was refused and the stream failed with "Room map has no channel called map".
#![cfg(windows)]

use std::sync::mpsc;
use std::time::{Duration, Instant};

use foxglove_plugin::hideout::{open_pipe, Handler, InitializeModule, Session};
use hideout_registry::lab::LAB_MODULES_DIR;
use hideout_registry::Registry;
use hideout_room_map::{registration, RoomMap, CHANNEL, ID};
use hideout_router::server::Hub;
use hideout_router::{Out, Router, Settings};

const MODULE_TOML: &str = include_str!("../../../examples/hideout-room-map/module.toml");

#[test]
fn the_room_map_from_lab_mode_opens_its_declared_channel() {
    let home = std::env::temp_dir()
        .join("hideout-router-lab-channels")
        .join(std::process::id().to_string());
    let _ = std::fs::remove_dir_all(&home);
    let dir = home.join(LAB_MODULES_DIR).join(ID);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("module.toml"), MODULE_TOML).unwrap();
    std::fs::write(dir.join("hideout-room-map.exe"), b"stand-in").unwrap();
    let mut reg = Registry::open(&home, false).unwrap();
    reg.load_lab();
    assert!(reg.is_lab(ID), "{:?}", reg.lab_report);
    // The manifest grants nothing: the declaration alone offers the channel.
    assert!(
        !reg.inventory.modules[ID]
            .granted
            .iter()
            .any(|g| g.starts_with("channel.offer")),
        "{:?}",
        reg.inventory.modules[ID].granted
    );

    let mut router = Router::new(Settings::default(), Box::new(|_, _| Vec::new()));
    router.load_registry(&reg);
    router.set_token(ID, "tok-lab-room");
    let (events, outs) = mpsc::channel();
    let hub = Hub::new(router, events);
    let name = format!(r"\\.\pipe\hideout-lab-channels-test-{}", std::process::id());
    hub.serve_pipe(&name).expect("serve the test pipe");

    let (ready_tx, ready) = mpsc::channel();
    std::thread::spawn(move || {
        let mut map = RoomMap::default();
        let p = open_pipe(&name).expect("router pipe");
        let mut s = Session::new(std::io::BufReader::new(p.try_clone().unwrap()), p);
        let init = InitializeModule {
            id: ID.into(),
            version: "0.1.0".into(),
            instance: "inst-lab".into(),
            token: "tok-lab-room".into(),
            hub: "1.0".into(),
            api: Default::default(),
        };
        s.call_initialize(&init, &mut map).expect("initialize");
        let r = s
            .call_register(&registration(), &mut map)
            .expect("register");
        s.ready("inst-lab").unwrap();
        ready_tx.send(r.rejected).unwrap();
        let _ = s.run(&mut map as &mut dyn Handler);
    });
    let rejected = ready.recv_timeout(Duration::from_secs(10)).unwrap();
    assert!(rejected.is_empty(), "the channel registers: {rejected:?}");
    // `module.ready` is a notification: open only once the router has taken it.
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match outs.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(Out::Ready(id)) if id == ID => break,
            Ok(_) => {}
            Err(e) => panic!("no module.ready: {e}"),
        }
    }

    hub.act(|r| r.open_host_channel(ID, CHANNEL, Instant::now()));
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match outs.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(Out::HostChannel {
                module,
                channel,
                result,
            }) => {
                assert_eq!((module.as_str(), channel.as_str()), (ID, CHANNEL));
                assert!(result.is_ok(), "the channel opens: {result:?}");
                break;
            }
            Ok(_) => {}
            Err(e) => panic!("no channel answer: {e}"),
        }
    }
    let _ = std::fs::remove_dir_all(&home);
}
