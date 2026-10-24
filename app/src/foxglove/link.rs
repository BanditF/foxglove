//! One shared `digitalis` link per keyboard: the overlay, the screen panel and art pushes all go
//! through the same Raw HID handle instead of each opening their own.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::Duration;

use digitalis::hid::Match;
use digitalis::link::Link;
use digitalis::protocol::Hello;

/// Live links by VID/PID; a link stops once its last holder drops it.
type Links = HashMap<(u16, u16), Weak<Link>>;

static LINKS: OnceLock<Mutex<Links>> = OnceLock::new();
type Listener = (Weak<()>, Box<dyn Fn(Arc<Link>) + Send>);
static LISTENERS: OnceLock<Mutex<Vec<Listener>>> = OnceLock::new();

/// Announces existing and newly shared links. Register before startup discovery so a link
/// created concurrently cannot be missed. Keep the returned token to stay subscribed.
/// Listeners must only enqueue work.
pub fn subscribe_links(notify: impl Fn(Arc<Link>) + Send + 'static) -> Arc<()> {
    let links = LINKS.get_or_init(Default::default).lock().unwrap();
    let mut listeners = LISTENERS.get_or_init(Default::default).lock().unwrap();
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
    let api = hidapi::HidApi::new().ok()?;
    let keyboards: std::collections::BTreeSet<(u16, u16)> = api
        .device_list()
        .filter(|d| {
            d.usage_page() == digitalis::protocol::USAGE_PAGE
                && d.usage() == digitalis::protocol::USAGE
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
    (0..3).find_map(|_| digitalis::panel::hello(link).ok().filter(usable))
}

/// The link to keyboard `vid:pid`, started on first use and kept while anyone holds it. It also
/// takes keyboards without Foxglove's host link, so the overlay works on any ZMK board.
pub fn shared(vid: u16, pid: u16) -> Result<Arc<Link>, String> {
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
