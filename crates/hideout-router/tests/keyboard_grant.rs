// SPDX-License-Identifier: MIT
//! The legacy keyboard grant in the router (#1155-B, contract section 6.3): the adopted module
//! `foxglove` provides `devices.*`, `lighting.*`, `art.*`, `screens.*`, `behaviors.*`,
//! `profiles.*` and `focus.*` only while the user's grant (path and SHA-256 of the program) matches
//! the image the supervisor verified for that connection. Window-free.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use hideout_contract::capability::{Holder, Source};
use hideout_contract::module::{Context, Module};
use hideout_registry::keyboard::{GrantState, KeyboardGrant, GRANT_AREAS, LEGACY_MODULE};
use hideout_router::checks::{never_name, LEGACY_FOXGLOVE_ID};
use hideout_router::{ConnId, ModuleEntry, Origin, Out, Router, Settings};
use serde_json::{json, Value};

const FOX: &str = "foxglove";
const CALLER: &str = "example.tool";
const PATH: &str = r"C:\Apps\Foxglove\foxglove.exe";
const HASH: &str = "ab12ab12ab12ab12ab12ab12ab12ab12ab12ab12ab12ab12ab12ab12ab12ab12";
const OTHER: &str = "cd34cd34cd34cd34cd34cd34cd34cd34cd34cd34cd34cd34cd34cd34cd34cd34";

type Identity = Arc<Mutex<Option<(String, String)>>>;

fn identity(path: &str, sha: &str) -> Identity {
    Arc::new(Mutex::new(Some((path.into(), sha.into()))))
}

fn grant(state: GrantState) -> KeyboardGrant {
    KeyboardGrant {
        state,
        path: PATH.into(),
        sha256: HASH.into(),
        at: "2026-11-12T10:00:00Z".into(),
        reason: None,
    }
}

fn grants(g: Option<KeyboardGrant>) -> HashMap<String, KeyboardGrant> {
    g.into_iter().map(|g| (FOX.to_string(), g)).collect()
}

fn entry(id: &str, holder: Holder, granted: &[&str], catalog: Option<&[&str]>) -> ModuleEntry {
    ModuleEntry {
        id: id.into(),
        name: id.into(),
        holder,
        granted: granted.iter().map(|s| s.to_string()).collect(),
        catalog_provide: catalog.map(|c| c.iter().map(|s| s.to_string()).collect()),
        channel_risk: BTreeMap::new(),
        activate_on_call: false,
        panels: None,
        settings_dir: String::new(),
        data_dir: String::new(),
        registered_version: None,
    }
}

fn adopted(id: &str, granted: &[&str]) -> ModuleEntry {
    let mut e = entry(id, Holder::ThirdParty(Source::Sideload), granted, None);
    e.registered_version = Some("0.1.0".into());
    e
}

fn manifest(id: &str, provides: &[&str]) -> Module {
    let list: Vec<String> = provides.iter().map(|p| format!("\"{p}\"")).collect();
    let text = format!(
        "id = \"{id}\"\nname = \"{id}\"\nversion = \"0.1.0\"\nkind = \"adopted\"\nfamily = \"foxglove\"\nhub = \"1.0\"\nrun = [\"C:\\\\x\\\\x.exe\"]\nprovides = [{}]\n",
        list.join(", ")
    );
    Module::parse(&text, &Context::default()).expect("manifest")
}

fn sends(out: &[Out], to: ConnId) -> Vec<Value> {
    out.iter()
        .filter_map(|o| match o {
            Out::Send(c, v) if *c == to => Some(v.clone()),
            _ => None,
        })
        .collect()
}

fn module(r: &mut Router, id: &str, now: Instant) -> ConnId {
    r.set_token(id, &format!("tok-{id}"));
    let c = r.connect(Origin::Pipe);
    let init = json!({"jsonrpc":"2.0","id":0,"method":"initialize","params":{
        "id":id,"version":"0.1.0","instance":"i","token":format!("tok-{id}"),"hub":"1.3",
        "api":{"foxglove":"1.13"}}});
    let out = r.receive(c, &init.to_string(), now);
    assert!(sends(&out, c)[0].get("result").is_some(), "{out:?}");
    c
}

fn register(r: &mut Router, c: ConnId, methods: &[&str], now: Instant) -> Vec<String> {
    let reg = json!({"jsonrpc":"2.0","id":1,"method":"module.register","params":
        {"methods":methods,"facts":[],"channels":[],"apis":{}}});
    let out = r.receive(c, &reg.to_string(), now);
    r.receive(
        c,
        r#"{"jsonrpc":"2.0","method":"module.ready","params":{"instance":"i"}}"#,
        now,
    );
    sends(&out, c)[0]["result"]["rejected"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["item"].as_str().unwrap().to_string())
        .collect()
}

fn call(r: &mut Router, c: ConnId, id: u64, method: &str, now: Instant) -> Vec<Out> {
    let m = json!({"jsonrpc":"2.0","id":id,"method":method,"params":{}});
    r.receive(c, &m.to_string(), now)
}

/// Calls `method` as `c`, returns what `to` received (and answers it), and the caller's error
/// code, if the caller was answered with one.
fn try_call(
    r: &mut Router,
    c: ConnId,
    to: ConnId,
    method: &str,
    now: Instant,
) -> (Vec<Value>, Option<i64>) {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(5000);
    let out = call(r, c, NEXT.fetch_add(1, Ordering::SeqCst), method, now);
    let got = sends(&out, to);
    for g in &got {
        let ans = json!({"jsonrpc":"2.0","id":g["id"],"result":{}});
        r.receive(to, &ans.to_string(), now);
    }
    let code = sends(&out, c)
        .iter()
        .find_map(|v| v["error"]["code"].as_i64());
    (got, code)
}

fn areas() -> Vec<String> {
    GRANT_AREAS.iter().map(|a| a.to_string()).collect()
}

fn decls(list: &[String]) -> Vec<String> {
    list.iter().map(|m| format!("method:{m}")).collect()
}

fn strs(v: &[String]) -> Vec<&str> {
    v.iter().map(String::as_str).collect()
}

/// A hub with the adopted module `foxglove` (no `provide:` grants, the manifest declaring the
/// seven areas), a caller allowed to call them, the supervisor's checks passing and the verified
/// identity `id`.
fn rig(id: &Identity) -> Router {
    let mut r = Router::new(Settings::default(), Box::new(|_, a| a.to_vec()));
    r.add_module(adopted(FOX, &[]));
    r.apply_manifest(&manifest(FOX, &strs(&decls(&areas()))));
    r.add_module(entry(
        CALLER,
        Holder::FirstParty,
        &[
            "devices.read",
            "devices.write",
            "lighting.write",
            "app.windows",
        ],
        None,
    ));
    r.set_launch_check(Box::new(|_, _| Ok(())));
    let id = id.clone();
    r.set_identity_lookup(Box::new(move |_, _| id.lock().unwrap().clone()));
    r
}

fn connect_both(r: &mut Router, now: Instant) -> (ConnId, ConnId) {
    let f = module(r, FOX, now);
    let c = module(r, CALLER, now);
    (f, c)
}

#[test]
fn the_ids_agree() {
    assert_eq!(LEGACY_MODULE, LEGACY_FOXGLOVE_ID);
}

#[test]
fn without_a_grant_none_of_the_areas_is_provided() {
    let now = Instant::now();
    let id = identity(PATH, HASH);
    let mut r = rig(&id);
    let (f, c) = connect_both(&mut r, now);
    let rejected = register(&mut r, f, &strs(&areas()), now);
    assert_eq!(rejected.len(), 7, "{rejected:?}");
    let (got, code) = try_call(&mut r, c, f, "devices.list", now);
    assert!(got.is_empty());
    assert_eq!(code, Some(-32601));
}

#[test]
fn a_valid_grant_provides_all_seven_areas_and_routes_calls() {
    let now = Instant::now();
    let id = identity(PATH, HASH);
    let mut r = rig(&id);
    r.set_keyboard_grants(grants(Some(grant(GrantState::On))));
    let (f, c) = connect_both(&mut r, now);
    let rejected = register(&mut r, f, &strs(&areas()), now);
    assert!(rejected.is_empty(), "{rejected:?}");
    for area in areas() {
        let method = area.replace('*', "ping");
        let (got, code) = try_call(&mut r, c, f, &method, now);
        // The caller may lack the permission for some area: the point is that the router never
        // answers "method not found" and a permitted call reaches the provider.
        assert!(code != Some(-32601), "{method}");
        if method == "devices.ping" || method == "lighting.ping" {
            assert_eq!(got.len(), 1, "{method}: {code:?}");
        }
    }
}

#[test]
fn a_hash_a_path_or_a_missing_identity_voids_it() {
    let now = Instant::now();
    for (what, who) in [
        ("another hash", Some((PATH.to_string(), OTHER.to_string()))),
        (
            "another path",
            Some((
                r"C:\Apps\Elsewhere\foxglove.exe".to_string(),
                HASH.to_string(),
            )),
        ),
        ("no verified identity", None),
    ] {
        let id: Identity = Arc::new(Mutex::new(who));
        let mut r = rig(&id);
        r.set_keyboard_grants(grants(Some(grant(GrantState::On))));
        let (f, _) = connect_both(&mut r, now);
        let rejected = register(&mut r, f, &strs(&areas()), now);
        assert_eq!(rejected.len(), 7, "{what}: {rejected:?}");
    }
}

#[test]
fn path_spelling_does_not_matter_but_the_hash_does() {
    let now = Instant::now();
    let id = identity(
        &PATH.to_lowercase().replace('\\', "/"),
        &HASH.to_uppercase(),
    );
    let mut r = rig(&id);
    r.set_keyboard_grants(grants(Some(grant(GrantState::On))));
    let (f, _) = connect_both(&mut r, now);
    assert!(register(&mut r, f, &strs(&areas()), now).is_empty());
}

#[test]
fn off_void_and_unknown_records_provide_nothing() {
    let now = Instant::now();
    for state in [GrantState::Off, GrantState::Void, GrantState::Unknown] {
        let id = identity(PATH, HASH);
        let mut r = rig(&id);
        r.set_keyboard_grants(grants(Some(grant(state))));
        let (f, _) = connect_both(&mut r, now);
        assert_eq!(
            register(&mut r, f, &strs(&areas()), now).len(),
            7,
            "{state:?}"
        );
    }
}

#[test]
fn a_grant_for_another_id_or_a_catalog_module_with_the_id_gives_nothing() {
    let now = Instant::now();
    let id = identity(PATH, HASH);
    // The grant map holding an entry for another adopted module changes nothing for it.
    let mut r = rig(&id);
    r.add_module(adopted("foxglove.twin", &[]));
    r.apply_manifest(&manifest("foxglove.twin", &strs(&decls(&areas()))));
    let mut map = grants(Some(grant(GrantState::On)));
    map.insert("foxglove.twin".into(), grant(GrantState::On));
    r.set_keyboard_grants(map);
    let t = module(&mut r, "foxglove.twin", now);
    assert_eq!(register(&mut r, t, &strs(&areas()), now).len(), 7);
    // A catalog (not adopted) module with the id foxglove is not the legacy module.
    let mut r = rig(&id);
    r.add_module(entry(
        FOX,
        Holder::ThirdParty(Source::Catalog),
        &[],
        Some(&["devices.*"]),
    ));
    r.set_keyboard_grants(grants(Some(grant(GrantState::On))));
    let f = module(&mut r, FOX, now);
    assert_eq!(register(&mut r, f, &strs(&areas()), now).len(), 7);
}

#[test]
fn an_area_the_manifest_does_not_declare_is_not_provided() {
    let now = Instant::now();
    let id = identity(PATH, HASH);
    let mut r = rig(&id);
    r.apply_manifest(&manifest(FOX, &["method:devices.*"]));
    r.set_keyboard_grants(grants(Some(grant(GrantState::On))));
    let (f, _) = connect_both(&mut r, now);
    let rejected = register(&mut r, f, &["devices.*", "lighting.*", "gpu.*"], now);
    assert_eq!(rejected, ["lighting.*", "gpu.*"]);
}

#[test]
fn a_catalog_that_assigns_the_area_to_another_module_wins() {
    let now = Instant::now();
    let id = identity(PATH, HASH);
    let mut r = rig(&id);
    r.add_module(entry(
        "foxglove.devices",
        Holder::FirstParty,
        &[],
        Some(&["devices.*"]),
    ));
    r.set_keyboard_grants(grants(Some(grant(GrantState::On))));
    let (f, _) = connect_both(&mut r, now);
    let rejected = register(&mut r, f, &strs(&areas()), now);
    assert_eq!(rejected, ["devices.*"]);
    // A root-signed area of another module wins too.
    let mut r = rig(&id);
    r.set_root_areas(vec![("foxglove.lights".into(), "lighting.*".into())]);
    r.set_keyboard_grants(grants(Some(grant(GrantState::On))));
    let f = module(&mut r, FOX, now);
    assert_eq!(register(&mut r, f, &strs(&areas()), now), ["lighting.*"]);
}

#[test]
fn names_that_reset_flash_erase_or_bond_are_never_routed() {
    let now = Instant::now();
    for bad in [
        "devices.reset",
        "devices.reset_all",
        "devices.enter-bootloader",
        "devices.bootloader",
        "devices.flash",
        "lighting.flash_firmware",
        "devices.erase",
        "devices.DFU",
        "devices.bond",
        "devices.unbond",
    ] {
        let expect = !bad.contains("unbond");
        assert_eq!(never_name(bad), expect, "{bad}");
    }
    // `unbond` is one word, not the word bond: the list is whole words (and bootload*).
    let id = identity(PATH, HASH);
    let mut r = rig(&id);
    r.set_keyboard_grants(grants(Some(grant(GrantState::On))));
    let (f, c) = connect_both(&mut r, now);
    // An exact never-name is refused at register.
    assert_eq!(
        register(&mut r, f, &["devices.reset", "devices.read"], now),
        ["devices.reset"]
    );
    // A wildcard is accepted, and the router refuses the never-names at call time.
    assert!(register(&mut r, f, &["devices.*"], now).is_empty());
    for bad in ["devices.reset_all", "devices.bootloader", "devices.flash"] {
        let (got, code) = try_call(&mut r, c, f, bad, now);
        assert!(got.is_empty(), "{bad}");
        assert_eq!(code, Some(-32601), "{bad}");
    }
    let (got, _) = try_call(&mut r, c, f, "devices.read", now);
    assert_eq!(got.len(), 1);
}

#[test]
fn turning_it_off_acts_at_once_and_foxglove_stays_connected() {
    let now = Instant::now();
    let id = identity(PATH, HASH);
    let mut r = rig(&id);
    r.set_keyboard_grants(grants(Some(grant(GrantState::On))));
    let (f, c) = connect_both(&mut r, now);
    assert!(register(&mut r, f, &strs(&areas()), now).is_empty());
    assert_eq!(try_call(&mut r, c, f, "devices.read", now).0.len(), 1);
    // Off: the very next call is refused like a method nobody provides; the connection lives.
    r.set_keyboard_grants(grants(Some(KeyboardGrant {
        state: GrantState::Off,
        path: String::new(),
        sha256: String::new(),
        ..grant(GrantState::Off)
    })));
    let (got, code) = try_call(&mut r, c, f, "devices.read", now);
    assert!(got.is_empty());
    assert_eq!(code, Some(-32601));
    let out = call(&mut r, f, 77, "ping", now);
    assert!(sends(&out, f)[0].get("result").is_some(), "{out:?}");
    assert!(!out.iter().any(|o| matches!(o, Out::Close(_))));
    // Removing the record has the same effect; turning it on again brings the routes back.
    r.set_keyboard_grants(grants(None));
    assert_eq!(
        try_call(&mut r, c, f, "lighting.write", now).1,
        Some(-32601)
    );
    // Allowed again: what Foxglove asked for at register comes back, it need not register again.
    r.set_keyboard_grants(grants(Some(grant(GrantState::On))));
    assert_eq!(try_call(&mut r, c, f, "devices.read", now).0.len(), 1);
}

#[test]
fn a_changed_identity_on_a_second_instance_closes_the_areas() {
    // A swap: a second instance of the module connects with another image. Every live
    // instance must match, so the areas close for both until the grant matches again.
    let now = Instant::now();
    let id = identity(PATH, HASH);
    let mut r = rig(&id);
    r.set_keyboard_grants(grants(Some(grant(GrantState::On))));
    let (f, _) = connect_both(&mut r, now);
    assert!(register(&mut r, f, &strs(&areas()), now).is_empty());
    *id.lock().unwrap() = Some((PATH.into(), OTHER.into()));
    let f2 = module(&mut r, FOX, now);
    assert_eq!(register(&mut r, f2, &strs(&areas()), now).len(), 7);
    assert!(r
        .set_keyboard_grants(grants(Some(grant(GrantState::On))))
        .iter()
        .all(|o| !matches!(o, Out::Close(_))));
}

#[test]
fn the_registry_carries_an_existing_grant_over_and_never_adds_one() {
    use hideout_registry::Registry;
    let tmp = std::env::temp_dir().join(format!("hideout-router-kb-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let home = tmp.join("home");
    let install = tmp.join("install");
    let folder = tmp.join("manifest");
    for d in [&home, &install, &folder] {
        std::fs::create_dir_all(d).unwrap();
    }
    let exe = install.join("foxglove.exe");
    std::fs::write(&exe, b"fake").unwrap();
    let run = exe.display().to_string().replace('\\', "/");
    let list = decls(&areas())
        .iter()
        .map(|d| format!("\"{d}\""))
        .collect::<Vec<_>>()
        .join(", ");
    std::fs::write(
        folder.join("module.toml"),
        format!(
            "id = \"foxglove\"\nname = \"Foxglove\"\nversion = \"0.1.0\"\nkind = \"adopted\"\n\
             family = \"foxglove\"\nhub = \"1.0\"\nrun = ['{run}']\nprovides = [{list}]\n\
             api = {{ foxglove = \"1.13\" }}\n"
        ),
    )
    .unwrap();
    let mut reg = Registry::open(&home, false).unwrap();
    reg.register_adopted(&folder, "2026-11-12T10:00:00Z")
        .unwrap();
    let now = Instant::now();

    // Adopted, never allowed: loading the registry gives no area.
    let id = identity(PATH, HASH);
    let mut r = Router::new(Settings::default(), Box::new(|_, a| a.to_vec()));
    r.set_launch_check(Box::new(|_, _| Ok(())));
    let idc = id.clone();
    r.set_identity_lookup(Box::new(move |_, _| idc.lock().unwrap().clone()));
    r.load_registry(&reg);
    let f = module(&mut r, FOX, now);
    assert_eq!(register(&mut r, f, &strs(&areas()), now).len(), 7);

    // Allowed in the registry, loaded again (a restart, an upgrade): it carries over.
    let canon = reg.keyboard_exe(FOX).unwrap();
    reg.allow_keyboard(FOX, &canon, HASH, "2026-11-12T10:00:00Z")
        .unwrap();
    *id.lock().unwrap() = Some((canon.clone(), HASH.into()));
    let mut r = Router::new(Settings::default(), Box::new(|_, a| a.to_vec()));
    r.set_launch_check(Box::new(|_, _| Ok(())));
    let idc = id.clone();
    r.set_identity_lookup(Box::new(move |_, _| idc.lock().unwrap().clone()));
    r.load_registry(&Registry::open(&home, false).unwrap());
    let f = module(&mut r, FOX, now);
    assert!(register(&mut r, f, &strs(&areas()), now).is_empty());
    let _ = std::fs::remove_dir_all(&tmp);
}
