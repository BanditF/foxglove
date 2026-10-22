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

/// A keyboard with Foxglove's host link and what it said to HELLO: on a link already open (the
/// overlay's) if one answers, else the first Raw HID keyboard that does.
pub fn host_link() -> Option<(Arc<Link>, Hello)> {
    let open: Vec<Arc<Link>> = LINKS
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .values()
        .filter_map(Weak::upgrade)
        .filter(|l| l.state().connected.is_some())
        .collect();
    if let Some(found) = open.into_iter().find_map(|l| ask_hello(&l).map(|h| (l, h))) {
        return Some(found);
    }

    let api = hidapi::HidApi::new().ok()?;
    let keyboards: std::collections::BTreeSet<(u16, u16)> = api
        .device_list()
        .filter(|d| {
            d.usage_page() == digitalis::protocol::USAGE_PAGE
                && d.usage() == digitalis::protocol::USAGE
        })
        .map(|d| (d.vendor_id(), d.product_id()))
        .collect();
    keyboards.into_iter().find_map(|(vid, pid)| {
        let link = shared(vid, pid).ok()?;
        link.wait_connected(Duration::from_secs(3)).ok()?;
        ask_hello(&link).map(|h| (link, h))
    })
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
    let mut links = LINKS.get_or_init(Default::default).lock().unwrap();
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
    Ok(link)
}
