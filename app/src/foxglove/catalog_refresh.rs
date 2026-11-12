//! Fetching the signed plugin catalog (#192, Scout's #190) from GitHub Pages: `root.json`,
//! `catalog.json` and their signatures under `/plugins/v2/`, by direct URL, no API.
//!
//! When (the user's decision: no daily check, no polling):
//! - at start, if the last success is more than 24 h old, after 5–60 s of jitter;
//! - when the Plugins tab opens (at most every 10 minutes);
//! - on "Check now", whatever the backoff says.
//!
//! Each file goes out as a conditional GET (`If-None-Match` with the last ETag). A failure backs
//! off 15 min, 1 h, 6 h, then 24 h, persisted in the store's `[plugins.refresh]` so a restart
//! doesn't hammer the server. What arrives is kept only if it verifies
//! ([`super::plugin_trust::accept_download`]).
//!
//! A catalog that isn't there yet (a 404, or a build whose root key is still the placeholder,
//! so nothing could verify) isn't a failure: it waits the longest step (a day) without adding
//! to `failures`, and the page says "Catalog: not available yet". With the placeholder key no
//! request goes out at all. `FOXGLOVE_PLUGIN_CATALOG_URL` points it elsewhere
//! (tests); `FOXGLOVE_DISABLE=catalog` turns the start-up check off.

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use super::plugin_trust::{Signed, FILES};

/// Where the signed catalog is published.
pub const BASE: &str = "https://banditf.github.io/foxglove/plugins/v2/";
const LIMIT: u64 = 4 * 1024 * 1024;
const DAY: u64 = 24 * 3600;
const TAB_MIN_INTERVAL: u64 = 10 * 60;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trigger {
    Startup,
    TabOpened,
    Manual,
}

/// The store's `[plugins.refresh]`. Times are Unix seconds.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct State {
    last_success: u64,
    last_attempt: u64,
    failures: u32,
    retry_after: u64,
    /// The last check found no catalog to fetch yet (not a failure).
    unavailable: bool,
    /// File name to the ETag it came with.
    etags: BTreeMap<String, String>,
}

/// How long to wait after `failures` failures in a row.
fn backoff(failures: u32) -> u64 {
    match failures {
        0 => 0,
        1 => 15 * 60,
        2 => 3600,
        3 => 6 * 3600,
        _ => DAY,
    }
}

fn due(trigger: Trigger, now: u64, s: &State) -> bool {
    match trigger {
        Trigger::Manual => true,
        Trigger::Startup => now >= s.retry_after && now.saturating_sub(s.last_success) > DAY,
        Trigger::TabOpened => {
            now >= s.retry_after && now.saturating_sub(s.last_attempt) >= TAB_MIN_INTERVAL
        }
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// 5 to 60 seconds, so many installs starting together don't arrive together.
fn jitter() -> Duration {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    Duration::from_secs(5 + u64::from(nanos) % 56)
}

#[derive(Clone, Debug, Default, PartialEq)]
pub enum Status {
    #[default]
    Idle,
    Checking,
    UpToDate {
        sequence: Option<u64>,
    },
    /// Nothing published yet, or this build can't verify a catalog (placeholder key).
    NotAvailable,
    Failed(String),
}

/// What a check that didn't succeed ran into.
#[derive(Debug, PartialEq)]
enum Error {
    /// The catalog isn't published yet, or this build has no key to verify one with.
    NotAvailable,
    /// A real problem: network, HTTP, a bad signature, a parse error.
    Failed(String),
}

impl From<String> for Error {
    fn from(e: String) -> Self {
        Error::Failed(e)
    }
}

static STATUS: Mutex<Status> = Mutex::new(Status::Idle);

pub fn status() -> Status {
    STATUS.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Repaints the Plugins tab when the status changes (#282), instead of every 250 ms while a
/// check runs. Set by the tab (`set_wake`).
static WAKE: std::sync::OnceLock<crate::ui_wake::UiWake> = std::sync::OnceLock::new();

pub fn set_wake(wake: crate::ui_wake::UiWake) {
    let _ = WAKE.set(wake);
}

fn set_status(s: Status) {
    *STATUS.lock().unwrap_or_else(|e| e.into_inner()) = s;
    if let Some(w) = WAKE.get() {
        w.request_repaint();
    }
}

fn base() -> String {
    let base = std::env::var("FOXGLOVE_PLUGIN_CATALOG_URL").unwrap_or_else(|_| BASE.into());
    if base.ends_with('/') {
        base
    } else {
        format!("{base}/")
    }
}

/// The start-up check, in the background.
pub fn start() {
    if super::repaint_trace::disabled("catalog") {
        return;
    }
    let state: State = super::store::section(STORE_OWNER, "plugins.refresh")
        .try_load()
        .unwrap_or_default();
    if state.unavailable && status() == Status::Idle {
        set_status(Status::NotAvailable);
    }
    if due(Trigger::Startup, now(), &state) {
        super::services::spawn_named("catalog-refresh", || {
            std::thread::sleep(jitter());
            run_guarded(Trigger::Startup);
        });
    }
}

/// A check for the Plugins tab or "Check now", in the background, when due.
pub fn request(trigger: Trigger) {
    if status() == Status::Checking {
        return;
    }
    let state: State = super::store::section(STORE_OWNER, "plugins.refresh")
        .try_load()
        .unwrap_or_default();
    if due(trigger, now(), &state) {
        set_status(Status::Checking);
        super::services::spawn_named("catalog-refresh", move || run_guarded(trigger));
    }
}

/// One check behind a panic boundary (#587). It isn't restarted (the next trigger checks
/// again), but "Checking" can't stick, which would refuse every later check.
fn run_guarded(trigger: Trigger) {
    if super::services::guard("catalog-refresh", || run(trigger)).is_none() {
        set_status(Status::Failed("Foxglove hit an internal error".into()));
    }
}

fn run(trigger: Trigger) {
    let state: State = super::store::section(STORE_OWNER, "plugins.refresh")
        .try_load()
        .unwrap_or_default();
    if !due(trigger, now(), &state) {
        return;
    }
    set_status(Status::Checking);
    let cache = super::plugin_trust::cache_dir();
    let cached = |name: &str| {
        cache
            .as_ref()
            .and_then(|d| std::fs::read(d.join(name)).ok())
    };
    let result = attempt(
        &base(),
        &state.etags,
        &cached,
        super::plugin_trust::signing_key_missing(),
        &super::plugin_trust::accept_download,
    );
    let (state, status) = settle(state, result, now());
    set_status(status);
    if let Err(e) = super::store::section(STORE_OWNER, "plugins.refresh").save(&state) {
        set_status(Status::Failed(format!("can't save the refresh state: {e}")));
    }
}

type Outcome = Result<(Option<u64>, BTreeMap<String, String>), Error>;

/// One check: the new sequence (`None` when nothing changed) and ETags. With the placeholder
/// key nothing could verify, so it doesn't ask the server.
fn attempt(
    base: &str,
    etags: &BTreeMap<String, String>,
    cached: &dyn Fn(&str) -> Option<Vec<u8>>,
    key_missing: bool,
    accept: &dyn Fn(&Signed) -> Result<u64, String>,
) -> Outcome {
    if key_missing {
        return Err(Error::NotAvailable);
    }
    match fetch(base, etags, cached)? {
        Fetched::Unchanged => Ok((None, etags.clone())),
        Fetched::New(set, etags) => Ok((Some(accept(&set)?), etags)),
    }
}

/// The refresh state and status after a check at `t`.
fn settle(mut state: State, result: Outcome, t: u64) -> (State, Status) {
    state.last_attempt = t;
    let status = match result {
        Ok((sequence, etags)) => {
            state.last_success = t;
            state.failures = 0;
            state.retry_after = 0;
            state.unavailable = false;
            state.etags = etags;
            Status::UpToDate { sequence }
        }
        Err(Error::NotAvailable) => {
            // Not a failure, so the count starts over; and no sooner than the longest step.
            state.failures = 0;
            state.retry_after = t + DAY;
            state.unavailable = true;
            Status::NotAvailable
        }
        Err(Error::Failed(e)) => {
            state.failures = state.failures.saturating_add(1);
            state.retry_after = t + backoff(state.failures);
            state.unavailable = false;
            Status::Failed(e)
        }
    };
    (state, status)
}

/// The status in plain words, for the Plugins page.
pub fn words(status: &Status) -> String {
    match status {
        Status::Idle => "Catalog: not checked yet".to_string(),
        Status::Checking => "Catalog: checking…".to_string(),
        Status::UpToDate { .. } => "Catalog: up to date".to_string(),
        Status::NotAvailable => "Catalog: not available yet".to_string(),
        Status::Failed(e) => format!("Catalog: couldn't check ({e})"),
    }
}

#[derive(Debug, PartialEq)]
enum Fetched {
    /// Every file answered 304.
    Unchanged,
    /// The full set (unchanged files from the cache) and the new ETags.
    New(Signed, BTreeMap<String, String>),
}

enum Got {
    NotModified,
    Body(Vec<u8>, Option<String>),
}

fn get(agent: &ureq::Agent, url: &str, etag: Option<&str>) -> Result<Got, Error> {
    let mut req = agent.get(url).header("User-Agent", "Foxglove-plugins");
    if let Some(etag) = etag {
        req = req.header("If-None-Match", etag);
    }
    let mut resp = req
        .call()
        .map_err(|e| Error::Failed(format!("{url}: {e}")))?;
    match resp.status().as_u16() {
        304 => Ok(Got::NotModified),
        // GitHub Pages answers 404 until the catalog is published.
        404 => Err(Error::NotAvailable),
        200 => {
            let etag = resp
                .headers()
                .get("ETag")
                .and_then(|v| v.to_str().ok())
                .map(String::from);
            let body = resp
                .body_mut()
                .with_config()
                .limit(LIMIT)
                .read_to_vec()
                .map_err(|e| Error::Failed(format!("{url}: {e}")))?;
            Ok(Got::Body(body, etag))
        }
        code => Err(Error::Failed(format!("{url}: HTTP {code}"))),
    }
}

/// Fetches the four files, conditionally where there's a cached copy to fall back on.
fn fetch(
    base: &str,
    etags: &BTreeMap<String, String>,
    cached: &dyn Fn(&str) -> Option<Vec<u8>>,
) -> Result<Fetched, Error> {
    if !(base.starts_with("https://")
        || base.starts_with("http://127.0.0.1")
        || base.starts_with("http://localhost"))
    {
        return Err(format!("the catalog URL must use HTTPS: {base}").into());
    }
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(30)))
        .http_status_as_error(false)
        .build()
        .into();
    let mut files: Vec<Vec<u8>> = Vec::new();
    let mut new_etags = BTreeMap::new();
    let mut changed = false;
    for name in FILES {
        let have = cached(name);
        let etag = have
            .as_ref()
            .and_then(|_| etags.get(name))
            .map(String::as_str);
        match get(&agent, &format!("{base}{name}"), etag)? {
            Got::NotModified => {
                files.push(
                    have.ok_or_else(|| Error::from(format!("{name}: 304 without a cached copy")))?,
                );
                if let Some(e) = etag {
                    new_etags.insert(name.to_string(), e.to_string());
                }
            }
            Got::Body(body, etag) => {
                changed = true;
                files.push(body);
                if let Some(e) = etag {
                    new_etags.insert(name.to_string(), e);
                }
            }
        }
    }
    if !changed {
        return Ok(Fetched::Unchanged);
    }
    let files: [Vec<u8>; 4] = files
        .try_into()
        .map_err(|_| Error::from("incomplete catalog".to_string()))?;
    Ok(Fetched::New(Signed::from_files(files), new_etags))
}

/// The feature that owns this file's store sections.
const STORE_OWNER: &str = "foxglove.plugins";

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};

    #[test]
    fn when_a_refresh_is_due() {
        let s = State {
            last_success: 1000,
            last_attempt: 1000,
            ..State::default()
        };
        assert!(
            !due(Trigger::Startup, 1000 + DAY, &s),
            "not more than once a day at start"
        );
        assert!(due(Trigger::Startup, 1001 + DAY, &s));
        assert!(
            !due(Trigger::TabOpened, 1000 + 60, &s),
            "the tab doesn't re-check every opening"
        );
        assert!(due(Trigger::TabOpened, 1000 + TAB_MIN_INTERVAL, &s));
        let backing_off = State {
            retry_after: 5000 + DAY,
            ..s.clone()
        };
        assert!(
            !due(Trigger::Startup, 4999 + DAY, &backing_off),
            "a day old, but still backing off"
        );
        assert!(due(Trigger::Startup, 5000 + DAY, &backing_off));
        assert!(!due(Trigger::TabOpened, 4999 + DAY, &backing_off));
        assert!(
            due(Trigger::Manual, 0, &backing_off),
            "Check now ignores the backoff"
        );
    }

    #[test]
    fn backoff_grows_to_a_day() {
        let steps: Vec<u64> = (0..6).map(backoff).collect();
        assert_eq!(steps, [0, 900, 3600, 21600, DAY, DAY]);
        let j = jitter().as_secs();
        assert!((5..=60).contains(&j), "{j}");
    }

    /// A tiny HTTP server: each file's body is its name, its ETag `"<name>-1"`; it answers 304
    /// when the ETag matches, and counts requests.
    fn serve(requests: usize) -> (String, std::thread::JoinHandle<Vec<String>>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!(
            "http://127.0.0.1:{}/plugins/v2/",
            listener.local_addr().unwrap().port()
        );
        let handle = std::thread::spawn(move || {
            let mut seen = Vec::new();
            for stream in listener.incoming().take(requests) {
                let mut stream = stream.unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                let name = line
                    .split_whitespace()
                    .nth(1)
                    .unwrap()
                    .rsplit('/')
                    .next()
                    .unwrap()
                    .to_string();
                let mut if_none_match = None;
                loop {
                    let mut h = String::new();
                    reader.read_line(&mut h).unwrap();
                    if h.trim().is_empty() {
                        break;
                    }
                    if let Some(v) = h
                        .strip_prefix("if-none-match: ")
                        .or_else(|| h.strip_prefix("If-None-Match: "))
                    {
                        if_none_match = Some(v.trim().to_string());
                    }
                }
                let etag = format!("\"{name}-1\"");
                let reply = if if_none_match.as_deref() == Some(etag.as_str()) {
                    seen.push(format!("{name} 304"));
                    "HTTP/1.1 304 Not Modified\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                        .to_string()
                } else {
                    seen.push(format!("{name} 200"));
                    format!("HTTP/1.1 200 OK\r\nETag: {etag}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{name}", name.len())
                };
                stream.write_all(reply.as_bytes()).unwrap();
            }
            seen
        });
        (base, handle)
    }

    #[test]
    fn conditional_get_reuses_the_cache() {
        // First time: no cache, so no ETags are sent and everything comes back.
        let (base, server) = serve(4);
        let Fetched::New(set, etags) = fetch(&base, &BTreeMap::new(), &|_| None).unwrap() else {
            panic!("expected files")
        };
        assert_eq!(set.catalog_json, b"catalog.json");
        assert_eq!(
            etags.get("root.json").map(String::as_str),
            Some("\"root.json-1\"")
        );
        assert!(server.join().unwrap().iter().all(|r| r.ends_with("200")));
        // Again with the cache: every file answers 304, so nothing changed.
        let (base, server) = serve(4);
        let cache = |name: &str| Some(name.as_bytes().to_vec());
        assert_eq!(fetch(&base, &etags, &cache).unwrap(), Fetched::Unchanged);
        assert!(server.join().unwrap().iter().all(|r| r.ends_with("304")));
        // A cached file gone missing: its ETag isn't sent, so it's fetched whole.
        let (base, server) = serve(4);
        let partial = |name: &str| (name != "catalog.json").then(|| name.as_bytes().to_vec());
        assert!(matches!(
            fetch(&base, &etags, &partial).unwrap(),
            Fetched::New(..)
        ));
        assert!(server
            .join()
            .unwrap()
            .contains(&"catalog.json 200".to_string()));
    }

    #[test]
    fn only_https_or_local_test_servers() {
        assert!(fetch("http://example.com/", &BTreeMap::new(), &|_| None)
            .is_err_and(|e| matches!(e, Error::Failed(m) if m.contains("HTTPS"))));
    }

    /// A server that answers one request with `status` and an empty body.
    fn answer(status: &'static str) -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!(
            "http://127.0.0.1:{}/plugins/v2/",
            listener.local_addr().unwrap().port()
        );
        std::thread::spawn(move || {
            if let Some(Ok(mut stream)) = listener.incoming().next() {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                loop {
                    let mut h = String::new();
                    if reader.read_line(&mut h).unwrap_or(0) == 0 || h.trim().is_empty() {
                        break;
                    }
                }
                let reply =
                    format!("HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                let _ = stream.write_all(reply.as_bytes());
            }
        });
        base
    }

    fn never(_: &Signed) -> Result<u64, String> {
        panic!("nothing should be verified")
    }

    #[test]
    fn a_catalog_not_published_yet_is_not_a_failure() {
        // GitHub Pages' 404: not available, without a failure.
        let base = answer("404 Not Found");
        let got = attempt(&base, &BTreeMap::new(), &|_| None, false, &never);
        assert_eq!(got, Err(Error::NotAvailable));
        let before = State {
            failures: 9,
            last_attempt: 10,
            retry_after: 20,
            ..State::default()
        };
        let (s, status) = settle(before, got, 1000);
        assert_eq!(status, Status::NotAvailable);
        assert_eq!(s.failures, 0, "not counted");
        assert_eq!(s.last_success, 0);
        assert_eq!(s.last_attempt, 1000);
        assert_eq!(s.retry_after, 1000 + DAY, "the longest step, no sooner");
        assert!(s.unavailable);
        assert!(!due(Trigger::Startup, 999 + DAY, &s));
        assert!(!due(Trigger::TabOpened, 999 + DAY, &s));
        assert!(due(Trigger::Startup, 1000 + DAY, &s));
        assert_eq!(words(&status), "Catalog: not available yet");

        // The placeholder root key: no request at all (nothing listens on port 1).
        let got = attempt(
            "http://127.0.0.1:1/",
            &BTreeMap::new(),
            &|_| None,
            true,
            &never,
        );
        assert_eq!(got, Err(Error::NotAvailable));
    }

    #[test]
    fn real_errors_still_count_and_show() {
        // A server error.
        let base = answer("500 Internal Server Error");
        let got = attempt(&base, &BTreeMap::new(), &|_| None, false, &never);
        assert!(matches!(&got, Err(Error::Failed(m)) if m.ends_with("HTTP 500")));
        let (s, status) = settle(State::default(), got, 1000);
        assert_eq!(s.failures, 1);
        assert_eq!(s.retry_after, 1000 + 15 * 60);
        assert!(!s.unavailable);
        assert!(words(&status).starts_with("Catalog: couldn't check ("));

        // No server (network).
        let got = attempt(
            "http://127.0.0.1:1/",
            &BTreeMap::new(),
            &|_| None,
            false,
            &never,
        );
        assert!(matches!(got, Err(Error::Failed(_))));

        // The files arrive but don't verify (a bad signature, or a parse error, with a real key).
        let (base, server) = serve(4);
        let bad = |_: &Signed| Err("the catalog's signature doesn't verify".to_string());
        let got = attempt(&base, &BTreeMap::new(), &|_| None, false, &bad);
        server.join().unwrap();
        let after_unavailable = State {
            unavailable: true,
            ..State::default()
        };
        let (s, status) = settle(after_unavailable, got, 1000);
        assert_eq!(s.failures, 1);
        assert!(!s.unavailable);
        assert_eq!(
            status,
            Status::Failed("the catalog's signature doesn't verify".into())
        );

        // And a good one clears it all.
        let (base, server) = serve(4);
        let got = attempt(&base, &BTreeMap::new(), &|_| None, false, &|_| Ok(7));
        server.join().unwrap();
        let (s, status) = settle(s, got, 2000);
        assert_eq!(status, Status::UpToDate { sequence: Some(7) });
        assert_eq!((s.failures, s.retry_after, s.last_success), (0, 0, 2000));
    }

    #[test]
    fn an_older_saved_state_still_reads() {
        // The user's file before this change: no `unavailable` key.
        let old: State =
            toml::from_str("failures = 9\nlast_attempt = 5\nlast_success = 0\nretry_after = 6\n")
                .unwrap();
        assert!(!old.unavailable);
        assert_eq!(old.failures, 9);
        let (s, _) = settle(old, Err(Error::NotAvailable), 1000);
        let back: State = toml::from_str(&toml::to_string(&s).unwrap()).unwrap();
        assert_eq!(back, s);
    }
}
