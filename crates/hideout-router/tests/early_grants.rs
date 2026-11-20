// SPDX-License-Identifier: MIT
//! An Allow kept for a module the router has not learnt yet (adopted into a running hub,
//! #1375), and what takes it back before the module is learnt: a turn-off, a forget (the app
//! left the inventory) and a build tag that does not match. In memory only. Window-free.

use std::collections::BTreeMap;

use hideout_contract::capability::{Holder, Source};
use hideout_router::{ModuleEntry, Router, Settings};

const ID: &str = "late.app";

fn entry(granted: &[&str]) -> ModuleEntry {
    ModuleEntry {
        id: ID.into(),
        name: ID.into(),
        holder: Holder::ThirdParty(Source::Sideload),
        granted: granted.iter().map(|s| s.to_string()).collect(),
        catalog_provide: None,
        channel_risk: BTreeMap::new(),
        activate_on_call: false,
        panels: None,
        settings_dir: String::new(),
        data_dir: String::new(),
        registered_version: Some("0.1.0".into()),
    }
}

fn router() -> Router {
    Router::new(Settings::default(), Box::new(|_, asked| asked.to_vec()))
}

fn s(v: &[&str]) -> Vec<String> {
    v.iter().map(|x| x.to_string()).collect()
}

/// The router learns the module (build `tag`), as the inventory watch does.
fn learn(r: &mut Router, tag: &str) -> Vec<String> {
    r.add_module(entry(&[]));
    r.apply_early_grants(ID, tag);
    r.module_grants(ID)
}

/// Turn off before the router learnt the app: the items turned off never arrive.
#[test]
fn a_turn_off_before_the_app_is_learnt_takes_the_kept_allow_back() {
    // One item of two.
    let mut r = router();
    r.set_module_grants_for(ID, s(&["net", "wallpaper.control"]), "list-a");
    r.drop_module_grants(ID, Some(&s(&["net"])));
    assert_eq!(learn(&mut r, "list-a"), s(&["wallpaper.control"]));

    // Everything.
    let mut r = router();
    r.set_module_grants_for(ID, s(&["net", "wallpaper.control"]), "list-a");
    r.drop_module_grants(ID, None);
    assert!(learn(&mut r, "list-a").is_empty());

    // Every item by name leaves nothing either, and an item never allowed changes nothing.
    let mut r = router();
    r.set_module_grants_for(ID, s(&["net"]), "list-a");
    r.drop_module_grants(ID, Some(&s(&["wallpaper.control"])));
    r.drop_module_grants(ID, Some(&s(&["net"])));
    assert!(learn(&mut r, "list-a").is_empty());
}

/// An Allow after the turn-off counts again: only what that Allow named.
#[test]
fn an_allow_after_the_turn_off_holds_only_what_it_named() {
    let mut r = router();
    r.set_module_grants_for(ID, s(&["net", "wallpaper.control"]), "list-a");
    r.drop_module_grants(ID, None);
    r.set_module_grants_for(ID, s(&["net"]), "list-a");
    assert_eq!(learn(&mut r, "list-a"), s(&["net"]));
}

/// The app left the inventory while its Allow waited: an app of the same id and build that
/// joins later holds nothing.
#[test]
fn a_forgotten_app_does_not_leave_its_kept_allow_to_the_next_one() {
    let mut r = router();
    r.set_module_grants_for(ID, s(&["net"]), "list-a");
    r.forget_module(ID);
    assert!(!r.knows_module(ID));
    assert!(learn(&mut r, "list-a").is_empty());

    // Learnt, allowed, forgotten, added again: the entry's grants went with it too.
    let mut r = router();
    assert!(learn(&mut r, "list-a").is_empty());
    r.set_module_grants_for(ID, s(&["net"]), "list-a");
    assert_eq!(r.module_grants(ID), s(&["net"]));
    r.forget_module(ID);
    assert!(learn(&mut r, "list-a").is_empty());
}

/// An Allow with no build tag (the plain `set_module_grants`) never lands on a tagged build,
/// and an Allow for another build does not join one that waits: the newer one replaces it.
#[test]
fn a_kept_allow_only_fits_the_build_it_was_given_for() {
    let mut r = router();
    r.set_module_grants(ID, s(&["net"]));
    assert!(learn(&mut r, "list-a").is_empty());

    let mut r = router();
    r.set_module_grants_for(ID, s(&["net"]), "list-a");
    r.set_module_grants_for(ID, s(&["wallpaper.control"]), "list-b");
    assert_eq!(learn(&mut r, "list-b"), s(&["wallpaper.control"]));

    let mut r = router();
    r.set_module_grants_for(ID, s(&["net"]), "list-a");
    r.set_module_grants_for(ID, s(&["wallpaper.control"]), "list-b");
    assert!(learn(&mut r, "list-a").is_empty());
}

/// A kept Allow is used once: applying it again (the watch reads twice) adds nothing back
/// after a turn-off, and it never widens what the entry was loaded with beyond the Allow.
#[test]
fn a_kept_allow_is_applied_once() {
    let mut r = router();
    r.set_module_grants_for(ID, s(&["net"]), "list-a");
    assert_eq!(learn(&mut r, "list-a"), s(&["net"]));
    r.drop_module_grants(ID, None);
    r.apply_early_grants(ID, "list-a");
    assert!(r.module_grants(ID).is_empty());

    // The entry came with a grant from the file; the kept Allow joins it, nothing else does.
    let mut r = router();
    r.set_module_grants_for(ID, s(&["net"]), "list-a");
    r.add_module(entry(&["wallpaper.control"]));
    r.apply_early_grants(ID, "list-a");
    assert_eq!(r.module_grants(ID), s(&["net", "wallpaper.control"]));
}
