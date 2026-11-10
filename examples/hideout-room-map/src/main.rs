// SPDX-License-Identifier: MIT
//! `example.room-map`: started by the hub with HIDEOUT_PIPE, HIDEOUT_TOKEN and
//! HIDEOUT_INSTANCE. Registers its page and serves the room map's stream when the hub asks.

use foxglove_plugin::hideout::{connect_env, InitializeModule};
use hideout_room_map::{panels, registration, RoomMap, ID};

fn main() {
    if let Err(e) = run() {
        eprintln!("{ID}: {e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let (mut session, launch) = connect_env()?;
    let mut map = RoomMap::default();
    let init = InitializeModule {
        id: ID.into(),
        version: env!("CARGO_PKG_VERSION").into(),
        instance: launch.instance.clone(),
        token: launch.token.clone(),
        hub: "1.0".into(),
        api: Default::default(),
    };
    session.call_initialize(&init, &mut map)?;
    session.call_register(&registration(), &mut map)?;
    session.call("ui.register", panels(&launch.instance), &mut map)?;
    session.ready(&launch.instance)?;
    while let Some(line) = session.read_line()? {
        if !line.trim().is_empty() {
            session.handle_line(&line, &mut map)?;
        }
        if map.exit {
            break;
        }
    }
    Ok(())
}
