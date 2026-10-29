//! One shared `foxglove-link` link per keyboard: the overlay, the screen panel and art pushes all go
//! through the same Raw HID handle instead of each opening their own.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::Duration;

use foxglove_link::hid::Match;
use foxglove_link::link::Link;
use foxglove_link::protocol::Hello;

/// Live links by VID/PID; a link stops once its last holder drops it.
type Links = HashMap<(u16, u16), Weak<Link>>;

static LINKS: OnceLock<Mutex<Links>> = OnceLock::new();
type Listener = (Weak<()>, Box<dyn Fn(Arc<Link>) + Send>);
static LISTENERS: OnceLock<Mutex<Vec<Listener>>> = OnceLock::new();

/// Announces existing and newly shared links. Register before startup discovery so a link
/// created concurrently cannot be missed. Keep the returned token to stay subscribed.
/// Listeners must only enqueue work.
pub fn subscribe_links(notify: impl Fn(Arc<Link>) + Send + 'static) -> Arc<()> {
    let links = LINKS
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let mut listeners = LISTENERS
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    for link in links.values().filter_map(Weak::upgrade) {
        notify(link);
    }
    listeners.retain(|(alive, _)| alive.strong_count() > 0);
    let alive = Arc::new(());
    listeners.push((Arc::downgrade(&alive), Box::new(notify)));
    alive
}

/// A keyboard with Foxglove's host link, its VID/PID and what it said to HELLO: on a link already
/// open (the overlay's) if one answers, else the first Raw HID keyboard that does.
pub fn host_keyboard() -> Option<((u16, u16), Arc<Link>, Hello)> {
    let open: Vec<((u16, u16), Arc<Link>)> = LINKS
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .filter_map(|(id, l)| Some((*id, l.upgrade()?)))
        .filter(|(_, l)| l.state().connected.is_some())
        .collect();
    if let Some(found) = open
        .into_iter()
        .find_map(|(id, l)| ask_hello(&l).map(|h| (id, l, h)))
    {
        return Some(found);
    }

    keyboards()?.into_iter().find_map(|(vid, pid)| {
        let link = shared(vid, pid).ok()?;
        link.wait_connected(Duration::from_secs(3)).ok()?;
        ask_hello(&link).map(|h| ((vid, pid), link, h))
    })
}

/// Starts discovery without waiting for a connection or HELLO. Keep the returned links (or
/// subscribe to shared links first) so their normal connection lifecycle can notify the caller.
pub fn discover_links() -> Vec<Arc<Link>> {
    keyboards()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|(vid, pid)| shared(vid, pid).ok())
        .collect()
}

fn keyboards() -> Option<std::collections::BTreeSet<(u16, u16)>> {
    // Unit tests never reach the keyboard on this PC: its LED counts leaked into led_map's
    // tests, and a lab run on the keyboard must not see a second host (#667).
    if cfg!(test) {
        return Some(Default::default());
    }
    let api = hidapi::HidApi::new().ok()?;
    let keyboards: std::collections::BTreeSet<(u16, u16)> = api
        .device_list()
        .filter(|d| {
            d.usage_page() == foxglove_link::protocol::USAGE_PAGE
                && d.usage() == foxglove_link::protocol::USAGE
        })
        .map(|d| (d.vendor_id(), d.product_id()))
        .collect();
    Some(keyboards)
}

/// Asks the keyboard on `link` for HELLO (a few tries; replies sometimes go missing) and
/// returns it if it describes a usable panel.
pub fn ask_hello(link: &Link) -> Option<Hello> {
    let usable = |h: &Hello| (1..=256).contains(&h.width) && (1..=256).contains(&h.height);
    if let Some(h) = link.state().connected.and_then(|c| c.hello) {
        if usable(&h) {
            return Some(h);
        }
    }
    (0..3).find_map(|_| foxglove_link::panel::hello(link).ok().filter(usable))
}

/// The link to keyboard `vid:pid`, started on first use and kept while anyone holds it. It also
/// takes keyboards without Foxglove's host link, so the overlay works on any ZMK board.
pub fn shared(vid: u16, pid: u16) -> Result<Arc<Link>, String> {
    if cfg!(test) {
        return Err("unit tests open no keyboard".into());
    }
    let mut links = LINKS
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if let Some(link) = links.get(&(vid, pid)).and_then(Weak::upgrade) {
        return Ok(link);
    }
    let m = Match {
        vid_pid: Some((vid, pid)),
        without_host_link: true,
        ..Match::default()
    };
    let link = Arc::new(Link::hid(m).map_err(|e| e.to_string())?);
    links.insert((vid, pid), Arc::downgrade(&link));
    watch_arrivals();
    LISTENERS
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .retain(|(alive, notify)| {
            if alive.strong_count() == 0 {
                return false;
            }
            notify(link.clone());
            true
        });
    Ok(link)
}

/// Cached identity only; never probes HID from a UI thread.
pub fn connected_name() -> Option<String> {
    LINKS
        .get()?
        .try_lock()
        .ok()?
        .values()
        .filter_map(Weak::upgrade)
        .find_map(|link| link.state().connected.map(|c| c.product))
}

/// A link gives up retrying on a timer after a few failed opens (#410) and waits for this:
/// a HID interface of one of its keyboards arriving. Registered once, with the first link, and
/// kept for the life of the process; the callback runs on a system thread and only flags links.
fn watch_arrivals() {
    #[cfg(target_os = "windows")]
    {
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| {
            let watch = super::device_watch::DeviceWatch::interfaces(
                true,
                Box::new(|path| {
                    let ours = LINKS
                        .get()
                        .and_then(|links| links.try_lock().ok())
                        .is_some_and(|links| {
                            links.keys().any(|(vid, pid)| {
                                super::connection::matches_interface(path, *vid, *pid)
                            })
                        });
                    if ours {
                        reconnect();
                    }
                }),
            );
            // Process-lifetime registration: unregistering would only happen at exit.
            std::mem::forget(watch);
        });
    }
}

/// Device notifications only wake existing workers; enumeration stays off the UI thread.
pub fn reconnect() {
    let Some(links) = LINKS.get().and_then(|links| links.try_lock().ok()) else {
        return;
    };
    for link in links.values().filter_map(Weak::upgrade) {
        if link.state().connected.is_none() {
            link.reconnect();
        }
    }
}

/// A matching serial arrival can replace a BLE handle that has not reported its loss yet.
pub fn transport_changed(spec: &crate::protocols::ConnectionSpec) {
    let crate::protocols::ConnectionSpec::Zmk { vid, pid, .. } = spec else {
        return;
    };
    let Some(links) = LINKS.get().and_then(|links| links.try_lock().ok()) else {
        return;
    };
    if let Some(link) = links.get(&(*vid, *pid)).and_then(Weak::upgrade) {
        link.reconnect();
    }
}

/// Looks up a selected identity without enumeration or I/O.
pub fn cached(vid: u16, pid: u16) -> Option<Arc<Link>> {
    LINKS.get()?.lock().ok()?.get(&(vid, pid))?.upgrade()
}
