// SPDX-License-Identifier: MIT
//! The hub's catalog fetch (#1049; contract §12.4, §13.4). Each configured source with an anchor
//! (`hideout.toml` `[[sources]]`) is fetched once at hub start, and the module's own source again
//! on **Check for updates**. Never on a timer: §13.4 fetches sources "on the user's action or at
//! the hub's update", so there is no schedule. Each file is one request with
//! [`crate::https`]'s per-phase timeouts and overall deadline, and no retry; a source that can't
//! be reached is logged and the stored catalog stays in use.
//!
//! - **No registry is held over the network:** a registry is opened to plan the fetch and
//!   dropped, the files are downloaded, then a fresh registry accepts them (verified, saved under
//!   `catalogs\<source>\`, `seen` raised under `inventory.lock`). An install, Go back or remove
//!   made meanwhile is kept.
//! - **One round at a time, and a click rides it:** a Check for updates made while a fetch runs
//!   starts no second one; it waits for that round (bounded by [`RIDE_LIMIT`]) and answers with
//!   its result for the module's source.
//! - The supervisor is told after each accept (`Supervisor::catalog_accepted`), so a revocation
//!   stops the version it names. With no source configured nothing is fetched and Check for
//!   updates reads "Couldn't check", as before.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use hideout_registry::catalogs::{download, now_utc, CatalogInfo, Fetch};
use hideout_registry::Registry;
use hideout_supervisor::driver::{Refresh, Supervisor};

/// What fetches: the hub's [`crate::https::WinHttp`], or a stand-in in tests.
pub type Fetcher = Arc<dyn Fetch + Send + Sync>;

/// The longest a Check for updates waits for a round that is already running before it answers
/// "Couldn't check". A round is bounded anyway (each request has its deadline); this is the
/// fallback.
pub const RIDE_LIMIT: Duration = Duration::from_secs(300);

/// The fetch rounds of this hub: one at a time.
struct Rounds {
    running: bool,
    /// The number of the round running or last run.
    round: u64,
    /// Per hub folder and source ([`key`]): the round that last fetched it and how that went.
    results: BTreeMap<String, (u64, Result<(), String>)>,
    /// Per [`key`]: how many checks are waiting on the running round (the log, and tests).
    riding: BTreeMap<String, usize>,
}

static ROUNDS: Mutex<Rounds> = Mutex::new(Rounds {
    running: false,
    round: 0,
    results: BTreeMap::new(),
    riding: BTreeMap::new(),
});
/// Signalled when a round ends or a check starts riding one.
static ROUND_ENDED: Condvar = Condvar::new();

type Guard = MutexGuard<'static, Rounds>;

fn rounds() -> Guard {
    ROUNDS.lock().unwrap_or_else(|e| e.into_inner())
}

/// The results key of `source` in the hub folder `home`.
fn key(home: &Path, source: &str) -> String {
    format!("{}|{source}", home.display())
}

/// Starts a round (the caller is its only fetcher) and returns its number.
fn begin(mut g: Guard) -> u64 {
    g.running = true;
    g.round += 1;
    g.round
}

/// Ends round `r` with each source's result and wakes whoever rides it.
fn end(r: u64, results: Vec<(String, Result<(), String>)>) {
    let mut g = rounds();
    for (source, res) in results {
        g.results.insert(source, (r, res));
    }
    g.running = false;
    ROUND_ENDED.notify_all();
}

/// Waits (until `until` at most) for the running round to end, as a check for `k`. Returns
/// the guard and that round's number, or `None` when no round was running.
fn ride(mut g: Guard, k: &str, until: Instant) -> (Guard, Option<u64>) {
    if !g.running {
        return (g, None);
    }
    let r = g.round;
    *g.riding.entry(k.to_string()).or_default() += 1;
    ROUND_ENDED.notify_all();
    let (mut g, _) = ROUND_ENDED
        .wait_timeout_while(g, until.saturating_duration_since(Instant::now()), |s| {
            s.running && s.round == r
        })
        .unwrap_or_else(|e| e.into_inner());
    if let Some(n) = g.riding.get_mut(k) {
        *n = n.saturating_sub(1);
    }
    (g, Some(r))
}

/// One source, fetched once: plan with a registry, download with none, accept with a fresh one.
fn fetch_one(home: &Path, source: &str, fetch: &dyn Fetch) -> Result<CatalogInfo, String> {
    let open = || Registry::open_from_env(home).map_err(|e| format!("registry not read: {e}"));
    let plan = open()?.plan_download(source).map_err(|e| e.to_string())?;
    let got = download(plan, fetch).map_err(|e| e.to_string())?;
    open()?
        .accept_download(got, &now_utc())
        .map_err(|e| e.to_string())
}

/// The supervisor's Check for updates (`Supervisor::set_refresh`): the module's source, fetched
/// once now, or the answer of the round already running. `Err` (the source unknown, offline,
/// refused, or a round still running after [`RIDE_LIMIT`]) reads "Couldn't check" on the card.
pub fn refresh(home: &Path, fetch: Fetcher) -> Refresh {
    let home = home.to_path_buf();
    Arc::new(move |id: &str| {
        let source = Registry::open_from_env(&home)
            .map_err(|e| format!("registry not read: {e}"))?
            .module_source(id)
            .map_err(|e| e.to_string())?;
        let k = key(&home, &source);
        // Ride each round that runs (rounds may follow one another) until one fetched this
        // source, or none runs and this check starts its own; [`RIDE_LIMIT`] in all.
        let until = Instant::now() + RIDE_LIMIT;
        let mut g = rounds();
        while g.running {
            if Instant::now() >= until {
                return Err("a check is already running".into());
            }
            let (next, rode) = ride(g, &k, until);
            g = next;
            if let Some(r) = rode {
                if let Some((got, res)) = g.results.get(&k) {
                    if *got == r {
                        log::info!("catalogs: {source}: check for updates for {id} rode round {r}");
                        return res.clone();
                    }
                }
            }
        }
        let r = begin(g);
        let res = fetch_one(&home, &source, fetch.as_ref()).map(|s| {
            log::info!(
                "catalogs: {source}: sequence {} accepted (check for updates for {id})",
                s.sequence
            );
        });
        end(r, vec![(key(&home, &source), res.clone())]);
        res
    })
}

/// The fetch at hub start, on its own thread so nothing waits for the network: every source
/// with an anchor once, `catalog_accepted` after each accept. Returns the thread (tests join
/// it); `None` if it couldn't start.
pub fn fetch_at_start(home: &Path, sup: Supervisor, fetch: Fetcher) -> Option<JoinHandle<()>> {
    let home: PathBuf = home.to_path_buf();
    let spawned = std::thread::Builder::new()
        .name("hideout-catalog-fetch".into())
        .spawn(move || {
            let ids: Vec<String> = match Registry::open_from_env(&home) {
                Ok(reg) => {
                    for line in &reg.catalog_report {
                        log::info!("catalogs: {line}");
                    }
                    reg.hub
                        .config
                        .sources
                        .iter()
                        .filter(|s| s.anchor.is_some())
                        .map(|s| s.id.clone())
                        .collect()
                }
                Err(e) => {
                    log::warn!("catalogs: registry not read ({e}); nothing fetched");
                    return;
                }
            };
            if ids.is_empty() {
                log::info!("catalogs: no catalog source in hideout.toml; nothing fetched");
                return;
            }
            // Wait out any round running (bounded by [`RIDE_LIMIT`] in all), then fetch.
            let until = Instant::now() + RIDE_LIMIT;
            let mut g = rounds();
            while g.running {
                if Instant::now() >= until {
                    log::warn!("catalogs: a fetch is still running; the start fetch is skipped");
                    return;
                }
                g = ride(g, &key(&home, "(start)"), until).0;
            }
            let r = begin(g);
            let mut results = Vec::new();
            for id in ids {
                let res = fetch_one(&home, &id, fetch.as_ref());
                match &res {
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
                results.push((key(&home, &id), res.map(|_| ())));
            }
            end(r, results);
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

    fn home(name: &str) -> (PathBuf, PathBuf) {
        let base = std::env::temp_dir().join(format!(
            "hideout-catalog-fetch-{name}-{}",
            std::process::id()
        ));
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
        let (home, src) = home("e2e");
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

    /// Installs the module from sequence 1 and returns the home, the module's folder and keys.
    fn installed(name: &str) -> (PathBuf, PathBuf, Keys) {
        let (home, src) = home(name);
        let k = keys();
        config(&home, &k.public);
        let served: Served = Arc::default();
        *served.lock().unwrap() = Some(catalog(&k, &src, 1, false));
        let mut reg = Registry::open(&home, false).unwrap();
        reg.refresh_source("mine", fetcher(&served).as_ref(), "2026-11-10T12:00:00Z")
            .unwrap();
        reg.install(&src, "2026-11-10T12:00:00Z").unwrap();
        (home, src, k)
    }

    #[test]
    fn a_check_made_while_a_fetch_runs_rides_it_instead_of_fetching_again() {
        let (home, src, k) = installed("ride");
        let body = catalog(&k, &src, 2, false);
        let (started_tx, started) = mpsc::channel::<()>();
        let (release, release_rx) = mpsc::channel::<()>();
        let release_rx = Mutex::new(release_rx);
        let started_tx = Mutex::new(started_tx);
        let catalog_gets = Arc::new(Mutex::new(0usize));
        let gets = catalog_gets.clone();
        // The first catalog read says it started, then waits to be let go (10 s fallback).
        let fetch: Fetcher = Arc::new(move |url: &str, _: usize| -> Result<Vec<u8>, String> {
            if url == URL {
                let first = {
                    let mut n = gets.lock().unwrap();
                    *n += 1;
                    *n == 1
                };
                if first {
                    let _ = started_tx.lock().unwrap().send(());
                    let _ = release_rx
                        .lock()
                        .unwrap()
                        .recv_timeout(Duration::from_secs(10));
                }
                Ok(body.0.clone())
            } else {
                Ok(body.1.clone().into_bytes())
            }
        });
        let check = refresh(&home, fetch);
        let leader = {
            let c = check.clone();
            std::thread::spawn(move || c(ID))
        };
        started.recv_timeout(Duration::from_secs(10)).unwrap();
        let rider = {
            let c = check.clone();
            std::thread::spawn(move || c(ID))
        };
        // Start event: the rider joins the round (ROUND_ENDED is signalled); 10 s fallback.
        let k = key(&home, "mine");
        let (g, waited) = ROUND_ENDED
            .wait_timeout_while(rounds(), Duration::from_secs(10), |s| {
                s.riding.get(&k).copied().unwrap_or(0) == 0
            })
            .unwrap();
        assert!(!waited.timed_out(), "the rider never joined the round");
        drop(g);
        // The rider is waiting on the round: let the leader finish, then both answer.
        release.send(()).unwrap();
        assert_eq!(leader.join().unwrap(), Ok(()));
        assert_eq!(rider.join().unwrap(), Ok(()));
        assert_eq!(*catalog_gets.lock().unwrap(), 1, "the rider fetched again");
        assert_eq!(stored_seq(&home), Some(2));
        let _ = std::fs::remove_dir_all(home.parent().unwrap());
    }

    #[test]
    fn a_change_saved_during_a_fetch_is_kept() {
        let (home, src, k) = installed("concurrent");
        let body = catalog(&k, &src, 2, false);
        let h = home.clone();
        // While the catalog downloads, another writer turns the module's previous version on
        // (as Go back or an install would) by writing inventory.json.
        let fetch: Fetcher = Arc::new(move |url: &str, _: usize| -> Result<Vec<u8>, String> {
            if url == URL {
                hideout_registry::inventory::Inventory::update(&h.join("inventory.json"), |inv| {
                    inv.modules.get_mut(ID).unwrap().previous = Some("0.4.0".into());
                })
                .map_err(|e| e.to_string())?;
                Ok(body.0.clone())
            } else {
                Ok(body.1.clone().into_bytes())
            }
        });
        refresh(&home, fetch)(ID).unwrap();
        let reg = Registry::open(&home, false).unwrap();
        assert_eq!(reg.inventory.modules[ID].previous.as_deref(), Some("0.4.0"));
        assert_eq!(reg.inventory.seen["test-user"].catalogs["mine"], 2);
        let _ = std::fs::remove_dir_all(home.parent().unwrap());
    }
}
