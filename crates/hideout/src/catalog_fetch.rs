// SPDX-License-Identifier: MIT
//! The hub's catalog fetch (#1049; contract §12.4, §13.4). Each configured source with an anchor
//! (`hideout.toml` `[[sources]]`) is fetched once at hub start, and the module's own source again
//! on **Check for updates**. Never on a timer: §13.4 fetches sources "on the user's action or at
//! the hub's update", so there is no schedule. Each fetch is one request per file with
//! [`crate::https`]'s timeouts and no retry; a source that can't be reached is logged and the
//! stored catalog stays in use.
//!
//! A catalog is accepted by the registry (`Registry::refresh_source`: verified, saved under
//! `catalogs\<source>\`, `seen` raised), and the supervisor is told after each accept
//! (`Supervisor::catalog_accepted`), so a revocation stops the version it names. With no source
//! configured nothing is fetched and Check for updates reads "Couldn't check", as before.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use hideout_registry::catalogs::{now_utc, Fetch};
use hideout_registry::Registry;
use hideout_supervisor::driver::{Refresh, Supervisor};

/// What fetches: the hub's [`crate::https::WinHttp`], or a stand-in in tests.
pub type Fetcher = Arc<dyn Fetch + Send + Sync>;

/// One fetch at a time in this hub: the start fetch and a Check for updates never write the
/// same source's file or `seen` at once.
static ONE: Mutex<()> = Mutex::new(());

/// The supervisor's Check for updates (`Supervisor::set_refresh`): the module's source, fetched
/// once now. `Err` (the source unknown, offline, refused) reads "Couldn't check" on the card.
pub fn refresh(home: &Path, fetch: Fetcher) -> Refresh {
    let home = home.to_path_buf();
    Arc::new(move |id: &str| {
        let _one = ONE.lock().unwrap_or_else(|e| e.into_inner());
        let mut reg =
            Registry::open_from_env(&home).map_err(|e| format!("registry not read: {e}"))?;
        let s = reg
            .refresh_module_source(id, fetch.as_ref(), &now_utc())
            .map_err(|e| e.to_string())?;
        log::info!(
            "catalogs: {}: sequence {} accepted (check for updates for {id})",
            s.source,
            s.sequence
        );
        Ok(())
    })
}

/// The fetch at hub start, on its own thread so nothing waits for the network: every source
/// with an anchor once, `catalog_accepted` after each accept. Returns the thread (tests join
/// it); `None` if it couldn't start.
pub fn fetch_at_start(home: &Path, sup: Supervisor, fetch: Fetcher) -> Option<JoinHandle<()>> {
    let home = home.to_path_buf();
    let spawned = std::thread::Builder::new()
        .name("hideout-catalog-fetch".into())
        .spawn(move || {
            let _one = ONE.lock().unwrap_or_else(|e| e.into_inner());
            let mut reg = match Registry::open_from_env(&home) {
                Ok(r) => r,
                Err(e) => {
                    log::warn!("catalogs: registry not read ({e}); nothing fetched");
                    return;
                }
            };
            for line in &reg.catalog_report {
                log::info!("catalogs: {line}");
            }
            let ids: Vec<String> = reg
                .hub
                .config
                .sources
                .iter()
                .filter(|s| s.anchor.is_some())
                .map(|s| s.id.clone())
                .collect();
            if ids.is_empty() {
                log::info!("catalogs: no catalog source in hideout.toml; nothing fetched");
                return;
            }
            for id in ids {
                match reg.refresh_source(&id, fetch.as_ref(), &now_utc()) {
                    Ok(s) => {
                        log::info!(
                            "catalogs: {id}: sequence {} accepted at {}",
                            s.sequence,
                            s.accepted
                        );
                        sup.catalog_accepted();
                    }
                    Err(e) => log::warn!("catalogs: {id}: not updated: {e}"),
                }
            }
        });
    match spawned {
        Ok(h) => Some(h),
        Err(e) => {
            log::warn!("catalogs: fetch not started: {e}");
            None
        }
    }
}

#[cfg(all(test, windows))]
mod tests {
    //! Window-free, through the real supervisor: no process starts (the module's exe doesn't
    //! exist) and the network is a stand-in fetcher.
    use super::*;
    use std::path::PathBuf;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    use foxglove_plugin_trust::sign;
    use hideout_router::server::{folder_suffix, Hub};
    use hideout_router::{Out, Router, Settings};
    use serde_json::{json, Value};

    const ID: &str = "example.test.art";
    const URL: &str = "https://catalog.example.invalid/catalog.json";

    struct Keys {
        public: String,
        /// The `.key` file, unencrypted (TEST-ONLY, made per run).
        secret: String,
    }

    fn keys() -> Keys {
        let k = sign::keygen(Some(String::new())).unwrap();
        Keys {
            secret: k.secret,
            public: k.public_base64,
        }
    }

    fn home() -> (PathBuf, PathBuf) {
        let base =
            std::env::temp_dir().join(format!("hideout-catalog-fetch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let (home, src) = (base.join("home"), base.join("src").join(ID));
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(
            src.join("module.toml"),
            format!(
                "id = \"{ID}\"\nname = \"Test art\"\nversion = \"0.5.0\"\nkind = \"service\"\n\
                 family = \"third-party\"\nhub = \"1.0\"\nvisibility = \"private\"\n\
                 run = [\"missing.exe\"]\n"
            ),
        )
        .unwrap();
        (home, src)
    }

    fn config(home: &Path, key: &str) {
        std::fs::write(
            home.join("hideout.toml"),
            format!(
                "[[trust.anchors]]\nid = \"test-user\"\nkind = \"user\"\nkey = \"{key}\"\n\
                 scope = [\"example.test.*\"]\n\n[[sources]]\nid = \"mine\"\nkind = \"catalog\"\n\
                 visibility = \"private\"\nurl = \"{URL}\"\nanchor = \"test-user\"\n\n\
                 [modules.\"{ID}\"]\nenabled = true\n"
            ),
        )
        .unwrap();
    }

    /// A signed catalog listing the module's folder, revoking 0.5.0 when `revoke`.
    fn catalog(k: &Keys, src: &Path, seq: u64, revoke: bool) -> (Vec<u8>, String) {
        let files = foxglove_plugin_trust::hash_dir(src, &["data"]).unwrap();
        let revocations = if revoke {
            json!([{ "id": ID, "versions": ["0.5.0"], "action": "revoke", "reason": "Broken; please update." }])
        } else {
            json!([])
        };
        let doc = json!({
            "format": "hideout-catalog-v1", "epoch": 0, "sequence": seq,
            "expires": "2099-01-01T00:00:00Z",
            "modules": [{ "id": ID, "version": "0.5.0", "kind": "service", "visibility": "private",
                          "sha256": "00".repeat(32), "files": files }],
            "revocations": revocations,
        });
        let bytes = serde_json::to_vec_pretty(&doc).unwrap();
        let sk = sign::load_secret(&k.secret, Some(String::new())).unwrap();
        let sig = sign::sign(&sk, &bytes).unwrap();
        (bytes, sig)
    }

    /// What the stand-in serves at the catalog's URL and its `.minisig`; `None` is offline.
    type Served = Arc<Mutex<Option<(Vec<u8>, String)>>>;

    fn fetcher(served: &Served) -> Fetcher {
        let s = served.clone();
        Arc::new(move |url: &str, _max: usize| -> Result<Vec<u8>, String> {
            let got = s.lock().unwrap().clone().ok_or("offline (stand-in)")?;
            if url == format!("{URL}.minisig") {
                Ok(got.1.into_bytes())
            } else if url == URL {
                Ok(got.0)
            } else {
                Err(format!("nothing at {url}"))
            }
        })
    }

    fn hub(home: &Path) -> (Hub, mpsc::Receiver<Out>) {
        let mut router = Router::new(Settings::default(), Box::new(|_, asked| asked.to_vec()));
        router.load_registry(&Registry::open_from_env(home).unwrap());
        let _ = router.watch_facts(vec![String::new()]);
        let (events, rx) = mpsc::channel();
        let hub = Hub::new(router, events);
        hub.set_folder_suffix(&folder_suffix(home));
        (hub, rx)
    }

    fn fact(hub: &Hub, k: &str) -> Value {
        hub.read(|r| r.fact(&format!("module.{ID}.{k}")).cloned())
            .unwrap_or(Value::Null)
    }

    /// Waits for `k` to become `want`. Start event: now; stop event: the fact change on the
    /// hub's event channel (`Out::Fact`); 10 s is the fallback only.
    fn wait_fact(hub: &Hub, rx: &mpsc::Receiver<Out>, k: &str, want: &Value) -> bool {
        let key = format!("module.{ID}.{k}");
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if fact(hub, k) == *want {
                return true;
            }
            match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                Ok(Out::Fact { key: got, value }) if got == key && value == *want => return true,
                Ok(_) => {}
                Err(_) => return false,
            }
        }
    }

    fn stored_seq(home: &Path) -> Option<u64> {
        hideout_registry::catalogs::read_stored(home, "mine")
            .unwrap()
            .map(|s| s.sequence)
    }

    /// Check for updates once and wait for its answer on the card (`module.<id>.check`).
    fn check(sup: &Supervisor, h: &Hub, rx: &mpsc::Receiver<Out>, want: &str) {
        sup.check_for_updates(ID);
        assert!(
            wait_fact(h, rx, "check", &json!(want)),
            "check: {:?}, state {:?}",
            fact(h, "check"),
            fact(h, "state")
        );
    }

    #[test]
    fn a_fetched_revocation_stops_the_module_survives_a_restart_and_bad_catalogs_are_refused() {
        let (home, src) = home();
        let (k, rogue) = (keys(), keys());
        config(&home, &k.public);
        let served: Served = Arc::default();

        // Installed from sequence 1, which withdraws nothing.
        *served.lock().unwrap() = Some(catalog(&k, &src, 1, false));
        let mut reg = Registry::open(&home, false).unwrap();
        reg.refresh_source("mine", fetcher(&served).as_ref(), "2026-11-10T12:00:00Z")
            .unwrap();
        assert_eq!(
            reg.install(&src, "2026-11-10T12:00:00Z").unwrap().source,
            "mine"
        );
        drop(reg);

        // Hub start: sequence 2 revokes 0.5.0. The start fetch accepts and saves it, and the
        // supervisor stops the version and shows it withdrawn.
        *served.lock().unwrap() = Some(catalog(&k, &src, 2, true));
        let (h, rx) = hub(&home);
        let sup = Supervisor::start(&home, h.clone());
        sup.set_refresh(refresh(&home, fetcher(&served)));
        fetch_at_start(&home, sup.clone(), fetcher(&served))
            .unwrap()
            .join()
            .unwrap();
        assert_eq!(stored_seq(&home), Some(2));
        assert!(
            wait_fact(&h, &rx, "state", &json!("revoked")),
            "{:?}",
            fact(&h, "state")
        );
        assert_eq!(
            fact(&h, "revoked"),
            json!({"version": "0.5.0", "reason": "Broken; please update."})
        );
        assert_eq!(sup.pid(ID), None);
        let d = sup.diagnostics(ID, "0.0.0");
        assert!(
            d.contains("Catalog: source mine, sequence 2, accepted 20"),
            "{d}"
        );

        // Reached, still withdrawn: nothing newer.
        check(&sup, &h, &rx, "none");
        // Offline: couldn't check.
        *served.lock().unwrap() = None;
        check(&sup, &h, &rx, "failed");
        // A rollback to sequence 1 is refused: couldn't check, and 2 stays.
        *served.lock().unwrap() = Some(catalog(&k, &src, 1, false));
        check(&sup, &h, &rx, "failed");
        assert_eq!(stored_seq(&home), Some(2));
        // Signed by a key the anchor doesn't name: refused the same way.
        *served.lock().unwrap() = Some(catalog(&rogue, &src, 3, false));
        check(&sup, &h, &rx, "failed");
        assert_eq!(stored_seq(&home), Some(2));
        assert_eq!(fact(&h, "state"), json!("revoked"));
        drop(sup);

        // A restart, offline: the stored catalog loads and the version still reads withdrawn.
        *served.lock().unwrap() = None;
        let (h, rx) = hub(&home);
        let sup = Supervisor::start(&home, h.clone());
        assert_eq!(fact(&h, "state"), json!("revoked"));
        assert_eq!(sup.pid(ID), None);

        // Sequence 3 no longer withdraws it: Check for updates clears it and the hub tries to
        // start it (its exe is missing: Couldn't start, not Withdrawn).
        sup.set_refresh(refresh(&home, fetcher(&served)));
        *served.lock().unwrap() = Some(catalog(&k, &src, 3, false));
        sup.check_for_updates(ID);
        assert!(wait_fact(&h, &rx, "revoked", &Value::Null));
        assert!(
            wait_fact(&h, &rx, "state", &json!("install-failed")),
            "{:?}",
            fact(&h, "state")
        );
        assert_eq!(stored_seq(&home), Some(3));
        drop(sup);
        let _ = std::fs::remove_dir_all(home.parent().unwrap());
    }

    #[test]
    fn without_a_source_nothing_is_fetched_and_check_says_couldnt() {
        let base =
            std::env::temp_dir().join(format!("hideout-catalog-none-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let asked = Arc::new(Mutex::new(0usize));
        let a = asked.clone();
        let fetch: Fetcher = Arc::new(move |_: &str, _: usize| -> Result<Vec<u8>, String> {
            *a.lock().unwrap() += 1;
            Err("unused".into())
        });
        let (h, _rx) = hub(&base);
        let sup = Supervisor::start(&base, h);
        fetch_at_start(&base, sup.clone(), fetch.clone())
            .unwrap()
            .join()
            .unwrap();
        assert_eq!(*asked.lock().unwrap(), 0);
        assert!(refresh(&base, fetch)("example.test.none").is_err());
        assert_eq!(*asked.lock().unwrap(), 0);
        assert!(!base.join("catalogs").exists());
        drop(sup);
        let _ = std::fs::remove_dir_all(base);
    }
}
