//! One shared `digitalis` link per keyboard: the overlay, the screen panel and art pushes all go
//! through the same Raw HID handle instead of each opening their own.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock, Weak};

use digitalis::hid::Match;
use digitalis::link::Link;

/// Live links by VID/PID; a link stops once its last holder drops it.
type Links = HashMap<(u16, u16), Weak<Link>>;

static LINKS: OnceLock<Mutex<Links>> = OnceLock::new();

/// The link to keyboard `vid:pid`, started on first use and kept while anyone holds it. It also
/// takes keyboards without Foxglove's host link, so the overlay works on any ZMK board.
/// A link to a keyboard that answers HELLO (has Foxglove's host link): the overlay's, if it
/// opened one, else the first Raw HID keyboard that answers.
pub fn host_link() -> Option<Arc<Link>> {
    let has_host_link = |link: &Link| link.state().connected.is_some_and(|c| c.hello.is_some());
    let open: Vec<Arc<Link>> = LINKS
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .values()
        .filter_map(Weak::upgrade)
        .collect();
    if let Some(link) = open.into_iter().find(|l| has_host_link(l)) {
        return Some(link);
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
        let connected = link
            .wait_connected(std::time::Duration::from_secs(3))
            .ok()?;
        connected.hello.is_some().then_some(link)
    })
}

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
