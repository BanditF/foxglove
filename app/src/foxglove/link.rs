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
