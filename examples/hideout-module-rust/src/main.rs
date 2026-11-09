//! `example.counter`: started by the hub with HIDEOUT_PIPE, HIDEOUT_TOKEN and HIDEOUT_INSTANCE.

use foxglove_plugin::hideout::{connect_env, InitializeModule};
use hideout_example_module::{Counter, ID};

fn main() {
    if let Err(e) = run() {
        eprintln!("{ID}: {e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let (mut session, launch) = connect_env()?;
    let mut counter = Counter::default();
    let init = InitializeModule {
        id: ID.into(),
        version: env!("CARGO_PKG_VERSION").into(),
        instance: launch.instance.clone(),
        token: launch.token.clone(),
        hub: "1.0".into(),
        api: Default::default(),
    };
    session.call_initialize(&init, &mut counter)?;
    session.call_register(&Counter::registration(), &mut counter)?;
    let panels = counter.panels(&launch.instance);
    session.call("ui.register", &panels, &mut counter)?;
    session.ready(&launch.instance)?;
    while let Some(line) = session.read_line()? {
        if !line.trim().is_empty() {
            session.handle_line(&line, &mut counter)?;
        }
        if let Some(patch) = counter.patch.take() {
            session.ui_patch(&patch)?;
        }
        if counter.exit {
            break;
        }
    }
    Ok(())
}
