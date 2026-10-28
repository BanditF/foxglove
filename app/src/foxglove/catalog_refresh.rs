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
//! ([`super::plugin_trust::accept_download`]). `FOXGLOVE_PLUGIN_CATALOG_URL` points it elsewhere
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
    Failed(String),
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
    let state: State = super::store::try_load("plugins.refresh").unwrap_or_default();
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
    let state: State = super::store::try_load("plugins.refresh").unwrap_or_default();
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
    let mut state: State = super::store::try_load("plugins.refresh").unwrap_or_default();
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
    let result = fetch(&base(), &state.etags, &cached).and_then(|fetched| match fetched {
        Fetched::Unchanged => Ok((None, state.etags.clone())),
        Fetched::New(set, etags) => {
            super::plugin_trust::accept_download(&set).map(|seq| (Some(seq), etags))
        }
    });
    let t = now();
    state.last_attempt = t;
    match result {
        Ok((sequence, etags)) => {
            state.last_success = t;
            state.failures = 0;
            state.retry_after = 0;
            state.etags = etags;
            set_status(Status::UpToDate { sequence });
        }
        Err(e) => {
            state.failures = state.failures.saturating_add(1);
            state.retry_after = t + backoff(state.failures);
            set_status(Status::Failed(e));
        }
    }
    if let Err(e) = super::store::save("plugins.refresh", &state) {
        set_status(Status::Failed(format!("can't save the refresh state: {e}")));
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

fn get(agent: &ureq::Agent, url: &str, etag: Option<&str>) -> Result<Got, String> {
    let mut req = agent.get(url).header("User-Agent", "Foxglove-plugins");
    if let Some(etag) = etag {
        req = req.header("If-None-Match", etag);
    }
    let mut resp = req.call().map_err(|e| format!("{url}: {e}"))?;
    match resp.status().as_u16() {
        304 => Ok(Got::NotModified),
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
                .map_err(|e| format!("{url}: {e}"))?;
            Ok(Got::Body(body, etag))
        }
        code => Err(format!("{url}: HTTP {code}")),
    }
}

/// Fetches the four files, conditionally where there's a cached copy to fall back on.
fn fetch(
    base: &str,
    etags: &BTreeMap<String, String>,
    cached: &dyn Fn(&str) -> Option<Vec<u8>>,
) -> Result<Fetched, String> {
    if !(base.starts_with("https://")
        || base.starts_with("http://127.0.0.1")
        || base.starts_with("http://localhost"))
    {
        return Err(format!("the catalog URL must use HTTPS: {base}"));
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
                files.push(have.ok_or_else(|| format!("{name}: 304 without a cached copy"))?);
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
        .map_err(|_| "incomplete catalog".to_string())?;
    Ok(Fetched::New(Signed::from_files(files), new_etags))
}

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
            .unwrap_err()
            .contains("HTTPS"));
    }
}
