//! The one-owner rule for the `foxglove-<user>` alias (contract §8.6, #1143 P4): the hub serves
//! it only when nobody is there, never beside a live foxglove.exe or an unverified server.
//! Test pipe names only; the user's real pipes are never opened.
#![cfg(windows)]

use std::sync::mpsc;
use std::time::{Duration, Instant};

use foxglove_plugin::channel::ServerPipe;
use foxglove_plugin::owner::{Expect, TIMEOUT};
use hideout_router::server::Hub;
use hideout_router::{AliasOutcome, Router, Settings};

fn hub() -> Hub {
    let (tx, _rx) = mpsc::channel();
    Hub::new(
        Router::new(Settings::default(), Box::new(|_, a| a.to_vec())),
        tx,
    )
}

fn name(tag: &str) -> String {
    format!(r"\\.\pipe\foxglove-test-alias-{tag}-{}", std::process::id())
}

/// This test binary's own file name: the "correct identity" for a server in this process.
fn own() -> Expect {
    let n = std::env::current_exe()
        .unwrap()
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    Expect {
        exe: Box::leak(n.into_boxed_str()),
    }
}

/// A server that stays on `name` for the test (keeps accepting).
fn squat(name: &str) {
    let name = name.to_string();
    let first = ServerPipe::create(&name, true).unwrap();
    std::thread::spawn(move || {
        let mut next = Some(first);
        loop {
            let p = next
                .take()
                .unwrap_or_else(|| ServerPipe::create(&name, false).unwrap());
            let _ = p.accept();
        }
    });
}

#[test]
fn nobody_there_the_alias_is_served() {
    let n = name("free");
    match hub().serve_alias_checked(&n, own(), TIMEOUT) {
        AliasOutcome::Served => {}
        other => panic!("{other:?}"),
    }
    // And it is really served now: the probe sees this process as its server.
    assert!(matches!(
        foxglove_plugin::owner::probe(&n, own(), TIMEOUT),
        foxglove_plugin::owner::Probe::Verified { .. }
    ));
}

#[test]
fn a_live_app_keeps_the_alias_and_nothing_is_stolen() {
    let n = name("app");
    squat(&n);
    let started = Instant::now();
    match hub().serve_alias_checked(&n, own(), TIMEOUT) {
        AliasOutcome::Left(why) => assert!(why.contains("keeps its own pipe"), "{why}"),
        other => panic!("{other:?}"),
    }
    assert!(started.elapsed() < Duration::from_millis(600));
}

#[test]
fn a_server_with_the_wrong_identity_is_ignored_and_nothing_claimed() {
    let n = name("fake");
    squat(&n);
    // The server is a real process, but not foxglove.exe.
    match hub().serve_alias_checked(&n, Expect::FOXGLOVE, TIMEOUT) {
        AliasOutcome::Left(why) => assert!(why.contains("not foxglove.exe"), "{why}"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn the_app_first_then_the_hub_and_the_hub_first_then_a_second_claim() {
    // App first: covered above. Hub first: a second claimer finds a verified hub on the name
    // and the first-instance flag refuses it too.
    let n = name("hubfirst");
    let first = hub();
    assert!(matches!(
        first.serve_alias_checked(&n, own(), TIMEOUT),
        AliasOutcome::Served
    ));
    match hub().serve_alias_checked(&n, own(), TIMEOUT) {
        AliasOutcome::Left(_) => {}
        other => panic!("{other:?}"),
    }
}
