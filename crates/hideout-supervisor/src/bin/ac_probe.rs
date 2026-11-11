// SPDX-License-Identifier: MIT
//! A test module for tests/appcontainer.rs: connects to `HIDEOUT_PIPE` the way every module
//! does (the SDK's `connect_env`), says `initialize` and `module.ready`, and says on stderr (its
//! log) whether it could read the file named in its second argument, one outside its folder.
//! Then it waits for the hub to end its job (60 s at most), or with `--exit` exits 0 at once.
//! It exits 3 when the pipe can't be opened. Never shipped, never windowed.
//!
//! `hideout-ac-probe <module id> [<file outside its folder> | -] [--exit]`

use std::time::Duration;

use foxglove_plugin::hideout::connect_env;
use serde_json::json;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let id = args.get(1).cloned().unwrap_or_default();
    if let Some(p) = args.get(2).filter(|p| *p != "-") {
        let what = match std::fs::read(p) {
            Ok(_) => "read".to_string(),
            Err(e) => format!("refused ({:?})", e.kind()),
        };
        eprintln!("probe: outside file: {what}");
    }
    let (mut s, launch) = match connect_env() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("probe: connect failed: {e}");
            std::process::exit(3);
        }
    };
    let said = s
        .request(
            "initialize",
            json!({"id": id, "version": "0.1.0", "instance": launch.instance,
                   "token": launch.token, "hub": "1.0", "api": {}}),
        )
        .and_then(|_| s.notify("module.ready", json!({})));
    match said {
        Ok(()) if args.iter().any(|a| a == "--exit") => std::process::exit(0),
        Ok(()) => eprintln!("probe: ready sent"),
        Err(e) => {
            eprintln!("probe: write failed: {e:?}");
            std::process::exit(4);
        }
    }
    std::thread::sleep(Duration::from_secs(60));
}
