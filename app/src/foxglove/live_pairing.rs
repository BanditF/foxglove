//! Pairing wallpapers to the live feed with one-time tickets (WE handover S2; S1 is the ticket
//! store in `live_wire`). The state is one [`Pairing`] per process, behind one lock ([`store`]):
//! the tickets, which monitors Foxglove pushed to, which of them have a live socket, and the
//! re-push budget. Nothing here is saved, and nothing logs a ticket.
//!
//! - **Mint:** `wallpaper_sync` asks for a ticket when it pushes `foxglove_live` to a monitor
//!   whose wallpaper is allowed, has no live socket and has no ticket outstanding. It goes out as
//!   `foxglove_ticket` in the same `applyProperties` push.
//! - **Redeem:** the wallpaper connects with the sub-protocols `foxglove-live.v1, <ticket>`.
//!   [`Pairing::admit`] finds the ticket's monitor *from the ticket* (the client sends no
//!   monitor, so nothing it says can name another monitor), then `admit_or_ticket` redeems it
//!   under the same lock. The monitor binding is therefore a guard against mix-ups, not a
//!   second secret (S2-2); the origin check and the ticket are the gate. The old pasted token
//!   still admits, unchanged, until the first ticket is redeemed (S4, `set_old_token_off`).
//! - **Refusal:** a refused connection (any local client) may ask for a re-push, but only for
//!   monitors Foxglove itself pushed to, whose wallpaper has no socket and whose ticket is
//!   spent or expired (so a refusal never cancels a ticket still waiting to be used), and at
//!   most [`REPUSH_MAX`] times per monitor per [`REPUSH_WINDOW`]. The count lives here, so a
//!   client cannot reset it; only the user's Reconnect does. No timer: a refusal, a wallpaper
//!   change or Reconnect are the only triggers.
//! - **Disconnect all:** cancels every ticket, closes the sockets and holds the feed's
//!   ticket path (no mint, no re-push) until Reconnect. Allowances stay.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use super::live_wire::{self as wire, Denied, Tickets};

/// Re-pushes after refusals, per monitor, within [`REPUSH_WINDOW`].
pub const REPUSH_MAX: usize = 3;
pub const REPUSH_WINDOW: Duration = Duration::from_secs(600);

/// What Foxglove knows about one monitor it pushed a ticket to.
#[derive(Default)]
struct Slot {
    /// The wallpaper file last pushed there.
    file: String,
    /// The live socket that redeemed a ticket for it.
    conn: Option<u64>,
    /// When it was re-pushed after a refusal, within the window.
    repushes: Vec<Instant>,
    /// Re-push at the next sync.
    due: bool,
    /// A refusal found the budget spent: the user's Reconnect is next.
    capped: bool,
}

/// How one wallpaper's connection stands (the worst-to-best order is the variant order, so
/// `max` of two monitors showing it keeps the better one).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Link {
    /// Nothing out and nothing joined (it isn't on a monitor Foxglove has pushed to).
    Idle,
    /// The budget is spent: the user's Reconnect is next.
    Capped,
    /// A first ticket is out.
    Connecting,
    /// A ticket went out again after a refusal.
    Reconnecting,
    /// A socket is open.
    Connected,
}

/// The pairing state the panel draws from: per wallpaper folder key, plus the holds.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Links {
    pub by_dir: BTreeMap<String, Link>,
    /// Disconnect all is in force.
    pub held: bool,
    /// Connections the old pasted token admitted (they name no wallpaper).
    pub old_way: usize,
}

fn allowed_in(allow: Option<&BTreeSet<String>>, file: &str) -> bool {
    allow.is_none_or(|a| a.contains(&super::wallpaper_manifest::key_of_file(file)))
}

/// The pairing state; see the module docs.
pub struct Pairing {
    tickets: Tickets,
    slots: BTreeMap<String, Slot>,
    /// The wallpaper files the user allowed; `None` allows every wallpaper whose manifest asks
    /// for the feed (today's behaviour, until the Hideout allow list, S5/S6).
    allow: Option<BTreeSet<String>>,
    /// Disconnect all: no tickets until Reconnect.
    held: bool,
    /// A ticket has been redeemed (S4): the old pasted token no longer admits.
    old_off: bool,
    /// Connections admitted with the old pasted token (they hold no monitor).
    old_clients: BTreeSet<u64>,
}

/// The one store: a [`Pairing`] behind a lock.
pub type Store = &'static Mutex<Pairing>;

static PAIRING: Mutex<Pairing> = Mutex::new(Pairing::new());

/// The process's one store.
pub fn store() -> Store {
    &PAIRING
}

impl Default for Pairing {
    fn default() -> Self {
        Pairing::new()
    }
}

impl Pairing {
    pub const fn new() -> Pairing {
        Pairing {
            tickets: Tickets::new(),
            slots: BTreeMap::new(),
            allow: None,
            held: false,
            old_off: false,
            old_clients: BTreeSet::new(),
        }
    }

    /// Stops admitting the old pasted token. Set once a ticket has really been redeemed and that
    /// is saved (S4), and at a feed start whose saved settings say so.
    pub fn set_old_token_off(&mut self, off: bool) {
        self.old_off = self.old_off || off;
    }

    /// The old token is retired now: it stops admitting, and the connections it admitted are
    /// returned so the feed closes them.
    pub fn retire_old_token(&mut self) -> Vec<u64> {
        self.old_off = true;
        std::mem::take(&mut self.old_clients).into_iter().collect()
    }

    pub fn old_token_off(&self) -> bool {
        self.old_off
    }

    /// Sets who may get tickets (`None`: every wallpaper that asks, the old way; `Some`: the
    /// folders the user allowed, as `wallpaper_manifest::key_of_dir` writes them). A monitor whose
    /// wallpaper is no longer allowed loses its ticket and its slot's socket at once: the
    /// connections to close are returned (S5a).
    pub fn set_allowed(&mut self, files: Option<BTreeSet<String>>) -> Vec<u64> {
        let before = std::mem::replace(&mut self.allow, files);
        let mut close = Vec::new();
        // Wallpapers just allowed are pushed again, with a ticket, at the next sync.
        for s in self.slots.values_mut() {
            if s.conn.is_none()
                && !allowed_in(before.as_ref(), &s.file)
                && allowed_in(self.allow.as_ref(), &s.file)
            {
                s.due = true;
            }
        }
        let off: Vec<String> = self
            .slots
            .iter()
            .filter(|(_, s)| !allowed_in(self.allow.as_ref(), &s.file))
            .map(|(m, _)| m.clone())
            .collect();
        for m in off {
            self.tickets.cancel(&m);
            if let Some(s) = self.slots.get_mut(&m) {
                close.extend(s.conn.take());
                s.due = false;
                s.capped = false;
                s.repushes.clear();
            }
        }
        close
    }

    fn allowed(&self, file: &str) -> bool {
        allowed_in(self.allow.as_ref(), file)
    }

    /// A ticket for the monitor `monitor` showing `file`, being pushed to now. `None` (the push
    /// goes without one) when held, not allowed, the wallpaper has a socket, a ticket is still
    /// outstanding for it, or the store is full or has no random source.
    pub fn mint(&mut self, monitor: &str, file: &str, now: Instant) -> Option<String> {
        let (held, allowed) = (self.held, self.allowed(file));
        let busy = self.tickets.has_open(monitor, now);
        // The monitor is known even when nothing is minted, so allowing its wallpaper later has
        // a slot to mark for a push.
        let slot = self.slots.entry(monitor.to_string()).or_default();
        slot.file = file.to_string();
        slot.due = false;
        if held || !allowed || busy || slot.conn.is_some() {
            return None;
        }
        self.tickets.mint(monitor, now)
    }

    /// The push carrying `monitor`'s ticket failed: cancel it.
    pub fn cancel(&mut self, monitor: &str) {
        self.tickets.cancel(monitor);
    }

    /// The handshake's decision, at the moment of the handshake (`now`): the origin first, then
    /// a ticket, then the old token (`admit_or_ticket`). `Ok(Some(monitor))` when a ticket
    /// admitted, `Ok(None)` when the old token did.
    pub fn admit(
        &mut self,
        origin: Option<&str>,
        given: Option<&str>,
        token: &str,
        now: Instant,
    ) -> Result<Option<String>, Denied> {
        let monitor = given
            .and_then(|g| self.tickets.monitor_of(g, now))
            .unwrap_or_default();
        // A blank stored token admits nothing, so once verified only tickets pass. Disconnect
        // all holds the old path too (sec-1298 L4).
        let token = if self.old_off || self.held { "" } else { token };
        wire::admit_or_ticket(origin, given, token, &mut self.tickets, &monitor, now)?;
        Ok((!monitor.is_empty()).then_some(monitor))
    }

    /// Connection `conn` was admitted; `monitor` is the one its ticket named, if it used one.
    /// `false` when Disconnect all came between the redeem and now: the socket must close.
    pub fn joined(&mut self, conn: u64, monitor: Option<&str>) -> bool {
        if monitor.is_some() && self.held {
            return false;
        }
        if monitor.is_none() {
            // Admitted with the old token just before it was retired, or while held: not kept.
            if self.old_off || self.held {
                return false;
            }
            self.old_clients.insert(conn);
        }
        if let Some(s) = monitor.and_then(|m| self.slots.get_mut(m)) {
            s.conn = Some(conn);
            s.capped = false;
        }
        true
    }

    /// Connection `conn` closed.
    pub fn left(&mut self, conn: u64) {
        self.old_clients.remove(&conn);
        for s in self.slots.values_mut().filter(|s| s.conn == Some(conn)) {
            s.conn = None;
        }
    }

    /// A connection was refused at `now`: marks the monitors that get a fresh ticket at the next
    /// sync and returns how many. See the module docs for who qualifies.
    pub fn refused(&mut self, now: Instant) -> usize {
        if self.held {
            return 0;
        }
        let mut n = 0;
        for (monitor, s) in &mut self.slots {
            // A wallpaper the user hasn't allowed is never re-pushed, and never counts as stuck.
            if !allowed_in(self.allow.as_ref(), &s.file)
                || s.conn.is_some()
                || s.due
                || self.tickets.has_open(monitor, now)
            {
                continue;
            }
            s.repushes
                .retain(|t| now.saturating_duration_since(*t) < REPUSH_WINDOW);
            if s.repushes.len() >= REPUSH_MAX {
                s.capped = true;
                continue;
            }
            s.repushes.push(now);
            s.due = true;
            n += 1;
        }
        n
    }

    /// The monitors to push again (taken: they are marked sent once the push goes).
    pub fn take_due(&mut self) -> Vec<String> {
        self.slots
            .iter_mut()
            .filter_map(|(m, s)| std::mem::take(&mut s.due).then(|| m.clone()))
            .collect()
    }

    /// The monitors showing wallpapers now; the others are forgotten, tickets and all.
    pub fn keep(&mut self, monitors: &[&str]) {
        let gone: Vec<String> = self
            .slots
            .keys()
            .filter(|m| !monitors.contains(&m.as_str()))
            .cloned()
            .collect();
        for m in gone {
            self.slots.remove(&m);
            self.tickets.cancel(&m);
        }
    }

    /// The user's Reconnect: the budget is new, and every monitor without a socket gets a fresh
    /// ticket at the next sync (the outstanding one is cancelled).
    pub fn reconnect(&mut self) {
        self.held = false;
        for (m, s) in &mut self.slots {
            s.repushes.clear();
            s.capped = false;
            if s.conn.is_none() {
                self.tickets.cancel(m);
                s.due = true;
            }
        }
    }

    /// Disconnect all: every ticket is cancelled and the ticket path is held until Reconnect.
    /// The old pasted token stops admitting too (S5a): the connections it admitted are returned.
    /// (The caller closes every socket, and blanks the saved token.)
    pub fn disconnect_all(&mut self) -> Vec<u64> {
        self.tickets.clear();
        self.held = true;
        for s in self.slots.values_mut() {
            s.conn = None;
            s.due = false;
        }
        self.retire_old_token()
    }

    /// The feed stopped: nothing outstanding, nothing connected.
    pub fn stopped(&mut self) {
        self.tickets.clear();
        for s in self.slots.values_mut() {
            s.conn = None;
            s.due = false;
        }
    }

    /// A wallpaper keeps being refused and its re-push budget is spent: the health view says so
    /// and offers Reconnect.
    pub fn stuck(&self) -> bool {
        self.slots
            .values()
            .any(|s| s.capped && s.conn.is_none() && allowed_in(self.allow.as_ref(), &s.file))
    }

    /// What the Hideout panel needs to know, per wallpaper folder (S5a).
    #[allow(dead_code)] // S5b wires the panel
    pub fn links(&self, now: Instant) -> Links {
        let mut by_dir: BTreeMap<String, Link> = BTreeMap::new();
        for (monitor, s) in &self.slots {
            let link = if s.conn.is_some() {
                Link::Connected
            } else if s.capped {
                Link::Capped
            } else if self.tickets.has_open(monitor, now) || s.due {
                if s.repushes.is_empty() {
                    Link::Connecting
                } else {
                    Link::Reconnecting
                }
            } else {
                Link::Idle
            };
            let slot = by_dir.entry(super::wallpaper_manifest::key_of_file(&s.file));
            slot.and_modify(|l| *l = (*l).max(link)).or_insert(link);
        }
        Links {
            by_dir,
            held: self.held,
            old_way: self.old_clients.len(),
        }
    }

    /// Tickets outstanding at `now` (for the tests).
    #[cfg(test)]
    pub fn open(&self, now: Instant) -> usize {
        self.tickets.outstanding(now)
    }
}

fn locked(store: Store) -> std::sync::MutexGuard<'static, Pairing> {
    store.lock().unwrap_or_else(|e| e.into_inner())
}

/// The user's Reconnect (the Hideout panel's button calls this, S5): see
/// [`Pairing::reconnect`]. The caller (`wallpaper_sync::reconnect`) wakes the pusher.
#[allow(dead_code)] // Entry point for the Hideout panel (S5); no UI calls it yet.
pub fn reconnect(store: Store) {
    locked(store).reconnect();
}

/// Disconnect all, the store's half: see [`Pairing::disconnect_all`].
#[allow(dead_code)] // Entry point for the Hideout panel (S5); no UI calls it yet.
pub fn disconnect_all(store: Store) -> Vec<u64> {
    locked(store).disconnect_all()
}

/// Whether a wallpaper couldn't reconnect (for the health row).
pub fn stuck(store: Store) -> bool {
    locked(store).stuck()
}

/// Which wallpaper files may get tickets (`None`: all that ask for the feed).
pub fn allow(store: Store, files: Option<BTreeSet<String>>) -> Vec<u64> {
    locked(store).set_allowed(files)
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "//?/DISPLAY#A";
    const B: &str = "//?/DISPLAY#B";

    fn ticket(p: &mut Pairing, m: &str, now: Instant) -> String {
        p.mint(m, "hud/index.html", now).expect("a ticket")
    }

    #[test]
    fn a_ticket_names_its_monitor_and_is_used_once() {
        let (mut p, now) = (Pairing::new(), Instant::now());
        let t = ticket(&mut p, A, now);
        assert_eq!(
            p.admit(Some("null"), Some(&t), "", now),
            Ok(Some(A.to_string()))
        );
        assert!(
            p.admit(Some("null"), Some(&t), "", now).is_err(),
            "single use"
        );
        // A web page doesn't burn it.
        let t = ticket(&mut p, A, now);
        assert!(p.admit(Some("https://x.test"), Some(&t), "", now).is_err());
        assert_eq!(p.open(now), 1);
        // The old token still admits, and names no monitor.
        assert_eq!(p.admit(None, Some("old-token"), "old-token", now), Ok(None));
    }

    fn only(keys: &[&str]) -> Option<BTreeSet<String>> {
        Some(keys.iter().map(|k| k.to_string()).collect())
    }

    #[test]
    fn switching_off_cancels_the_ticket_and_returns_the_socket_to_close() {
        let (mut p, now) = (Pairing::new(), Instant::now());
        p.set_allowed(only(&["hud"]));
        let t = ticket(&mut p, A, now);
        assert!(p.set_allowed(only(&[])).is_empty(), "no socket yet");
        assert!(p.admit(None, Some(&t), "", now).is_err(), "ticket cancelled");
        p.set_allowed(only(&["hud"]));
        let t = ticket(&mut p, A, now);
        assert_eq!(p.admit(None, Some(&t), "", now), Ok(Some(A.into())));
        p.joined(7, Some(A));
        assert_eq!(p.set_allowed(only(&[])), vec![7]);
        assert!(p.mint(A, "hud/index.html", now).is_none(), "off mints nothing");
        assert!(!p.stuck());
        p.set_allowed(only(&["hud"]));
        assert!(p.mint(A, "hud/index.html", now).is_some(), "on mints again");
    }

    #[test]
    fn a_disallowed_file_is_never_stuck_or_refused() {
        let (mut p, now) = (Pairing::new(), Instant::now());
        p.set_allowed(only(&["other"]));
        assert!(p.mint(A, "hud/index.html", now).is_none());
        assert!(!p.stuck());
    }

    #[test]
    fn disconnect_all_returns_old_way_sockets_and_retires_the_token() {
        let (mut p, now) = (Pairing::new(), Instant::now());
        assert_eq!(p.admit(None, Some("old"), "old", now), Ok(None));
        p.joined(9, None);
        assert_eq!(p.disconnect_all(), vec![9]);
        assert!(p.admit(None, Some("old"), "old", now).is_err(), "token retired");
    }

    #[test]
    fn no_second_ticket_while_one_waits_or_a_socket_is_open() {
        let (mut p, now) = (Pairing::new(), Instant::now());
        let t = ticket(&mut p, A, now);
        assert!(p.mint(A, "hud/index.html", now).is_none(), "one waits");
        assert_eq!(p.admit(None, Some(&t), "", now), Ok(Some(A.into())));
        p.joined(7, Some(A));
        assert!(p.mint(A, "hud/index.html", now).is_none(), "socket open");
        p.left(7);
        assert!(p.mint(A, "hud/index.html", now).is_some(), "socket gone");
    }

    #[test]
    fn the_allow_list_decides_who_gets_a_ticket() {
        let (mut p, now) = (Pairing::new(), Instant::now());
        p.set_allowed(Some(BTreeSet::from(["hud".to_string()])));
        assert!(p.mint(A, "other/index.html", now).is_none());
        assert!(p.mint(B, "hud/index.html", now).is_some());
    }

    #[test]
    fn a_refusal_re_pushes_only_pushed_monitors_without_a_ticket_or_socket() {
        let (mut p, now) = (Pairing::new(), Instant::now());
        // Nothing pushed yet: a refusal names nobody.
        assert_eq!(p.refused(now), 0);
        let ta = ticket(&mut p, A, now);
        let _tb = ticket(&mut p, B, now);
        // Both have tickets waiting: a refusal cancels neither.
        assert_eq!(p.refused(now), 0);
        assert_eq!(p.open(now), 2);
        p.admit(None, Some(&ta), "", now).unwrap();
        assert!(p.joined(1, Some(A)));
        // A has a socket, B a ticket: still nobody.
        assert_eq!(p.refused(now), 0);
        // B's expires: only B.
        let later = now + wire::TICKET_TTL + Duration::from_secs(1);
        assert_eq!(p.refused(later), 1);
        assert_eq!(p.take_due(), [B.to_string()]);
        assert!(p.take_due().is_empty());
        assert!(p.mint(B, "hud/index.html", later).is_some());
    }

    #[test]
    fn three_re_pushes_per_monitor_per_ten_minutes_and_the_client_cannot_reset_it() {
        let (mut p, start) = (Pairing::new(), Instant::now());
        let mut now = start;
        ticket(&mut p, A, now);
        for round in 0..REPUSH_MAX {
            now += wire::TICKET_TTL + Duration::from_secs(1);
            assert_eq!(p.refused(now), 1, "re-push {round}");
            p.take_due();
            ticket(&mut p, A, now);
        }
        assert!(!p.stuck());
        now += wire::TICKET_TTL + Duration::from_secs(1);
        // A flood of refusals gets nothing more out of it.
        for _ in 0..50 {
            assert_eq!(p.refused(now), 0);
        }
        assert!(p.stuck());
        // The window passing frees it again; Reconnect does at once.
        p.reconnect();
        assert!(!p.stuck());
        assert_eq!(p.take_due(), [A.to_string()]);
        assert!(p.mint(A, "hud/index.html", now).is_some());
    }

    #[test]
    fn the_window_frees_the_budget() {
        let (mut p, start) = (Pairing::new(), Instant::now());
        let mut now = start;
        ticket(&mut p, A, now);
        for _ in 0..REPUSH_MAX {
            now += wire::TICKET_TTL + Duration::from_secs(1);
            assert_eq!(p.refused(now), 1);
            p.take_due();
            ticket(&mut p, A, now);
        }
        now += REPUSH_WINDOW;
        assert_eq!(p.refused(now), 1, "the earlier ones are out of the window");
    }

    #[test]
    fn disconnect_all_cancels_tickets_and_holds_until_reconnect() {
        let (mut p, now) = (Pairing::new(), Instant::now());
        let t = ticket(&mut p, A, now);
        p.disconnect_all();
        assert!(
            p.admit(None, Some(&t), "", now).is_err(),
            "ticket cancelled"
        );
        assert!(p.mint(A, "hud/index.html", now).is_none(), "held");
        assert_eq!(
            p.refused(now + wire::TICKET_TTL),
            0,
            "no re-push while held"
        );
        p.reconnect();
        assert_eq!(p.take_due(), [A.to_string()]);
        assert!(p.mint(A, "hud/index.html", now).is_some());
    }

    #[test]
    fn reconnect_cancels_a_waiting_ticket_but_leaves_a_connected_monitor() {
        let (mut p, now) = (Pairing::new(), Instant::now());
        let ta = ticket(&mut p, A, now);
        let tb = ticket(&mut p, B, now);
        p.admit(None, Some(&tb), "", now).unwrap();
        p.joined(2, Some(B));
        p.reconnect();
        assert_eq!(p.take_due(), [A.to_string()]);
        assert!(p.admit(None, Some(&ta), "", now).is_err());
    }

    #[test]
    fn monitors_off_screen_are_forgotten_with_their_tickets() {
        let (mut p, now) = (Pairing::new(), Instant::now());
        let t = ticket(&mut p, A, now);
        ticket(&mut p, B, now);
        p.keep(&[B]);
        assert!(p.admit(None, Some(&t), "", now).is_err());
        assert_eq!(p.open(now), 1);
    }

    /// S2-4's sweep for this module: the store has no `Debug`, the ticket store's prints a count,
    /// and a refused connection leaves nothing to print but counts.
    #[test]
    fn nothing_the_store_can_print_holds_a_ticket() {
        let (mut p, now) = (Pairing::new(), Instant::now());
        let t = ticket(&mut p, A, now);
        assert!(!format!("{:?}", p.tickets).contains(&t));
        assert_eq!(p.refused(now), 0);
        assert!(!format!("{:?}", p.tickets).contains(&t));
    }

    /// Disconnect all against a connection that has been admitted but not yet joined: the join
    /// is refused once the hold is set (the live feed does both under this lock).
    #[test]
    fn a_connection_admitted_before_disconnect_all_cannot_join_after_it() {
        let (mut p, now) = (Pairing::new(), Instant::now());
        let t = ticket(&mut p, A, now);
        assert_eq!(p.admit(None, Some(&t), "", now), Ok(Some(A.into())));
        p.disconnect_all();
        assert!(!p.joined(9, Some(A)), "held: the socket must close");
        // Nothing recorded a socket, so Reconnect hands the monitor a fresh ticket.
        p.reconnect();
        assert_eq!(p.take_due(), [A.to_string()]);
    }

    #[test]
    fn stopping_the_feed_clears_everything_outstanding() {
        let (mut p, now) = (Pairing::new(), Instant::now());
        ticket(&mut p, A, now);
        p.joined(3, Some(A));
        p.stopped();
        assert_eq!(p.open(now), 0);
        assert!(
            p.mint(A, "hud/index.html", now).is_some(),
            "no socket remembered"
        );
    }
}
