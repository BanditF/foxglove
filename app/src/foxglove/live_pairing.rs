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
//! - **Who:** a ticket is only looked at when the connection's peer is the wallpaper host
//!   (`live_peer`, #1445). From any other peer it is not compared, not used up and not
//!   cancelled; only the old token can admit that peer.
//! - **Refusal:** a refused connection from the wallpaper host (or from a peer that couldn't
//!   be checked) may ask for a re-push, but only for monitors Foxglove itself pushed to whose
//!   wallpaper has no socket, and at most [`REPUSH_MAX`] times per monitor per
//!   [`REPUSH_WINDOW`]. A refusal never cancels a ticket still waiting to be used: while one
//!   waits, that same ticket is handed to its monitor once more (a wallpaper that reloaded
//!   lost it; nothing new is minted and its deadline stays). The count lives here, so a client
//!   cannot reset it; only the user's Reconnect does. A refused program that isn't the
//!   wallpaper host never causes a re-push.
//! - **Unused:** a ticket that runs out unused, on a monitor still without a socket, is
//!   replaced at most [`LAPSE_MAX`] times in a row ([`Pairing::lapsed`]; the push may have
//!   reached a wallpaper that wasn't loaded yet). One bounded wait per ticket, started by the
//!   pusher; a connection, another wallpaper or Reconnect starts the count again. After that
//!   only a refusal, a wallpaper change or Reconnect hand out a ticket.
//! - **Another program:** a refused program that isn't the wallpaper host is recorded by
//!   [`Pairing::intruder`]: a fact from the peer check on that connection, whatever the
//!   wallpapers' connections are. It is one section-level fact ([`Links::foreign`]), never a
//!   per-wallpaper one. It clears on Reconnect, on Disconnect all, when the feed stops, and
//!   once [`wire::REFUSAL_QUIET`] passed without another (worked out when it is read). The
//!   wallpapers' sockets are left alone: the program never got one.
//! - **Disconnect all:** cancels every ticket, closes the sockets and holds the feed's
//!   ticket path (no mint, no re-push) until Reconnect. Allowances stay.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use super::live_wire::{self as wire, Denied, Tickets};

/// Re-pushes after refusals, per monitor, within [`REPUSH_WINDOW`].
pub const REPUSH_MAX: usize = 3;
pub const REPUSH_WINDOW: Duration = Duration::from_secs(600);
/// Tickets replaced in a row because they ran out unused, per monitor.
pub const LAPSE_MAX: usize = 2;

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
    /// The re-push hands over the ticket that is still waiting, not a new one.
    again: bool,
    /// The waiting ticket has been handed over a second time already.
    handed_again: bool,
    /// When the ticket last minted for it runs out, while nothing has used it.
    out: Option<Instant>,
    /// Tickets replaced in a row because they ran out unused.
    lapses: usize,
}

impl Slot {
    /// Nothing is waiting any more: no ticket to hand again, none to run out.
    fn settled(&mut self) {
        self.again = false;
        self.handed_again = false;
        self.out = None;
    }
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
    /// A program that isn't the wallpaper host tried to connect and was refused (#1445). The
    /// section says so once; no row does.
    pub foreign: Option<Intruder>,
}

/// The refused program, as the section may name it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Intruder {
    /// Its file name, already cleaned and cut; `None` when it has none to show.
    pub name: Option<String>,
}

/// The last refused program that isn't the wallpaper host, and when.
struct Foreign {
    name: Option<String>,
    at: Instant,
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
    /// Another program was refused: see the module docs.
    foreign: Option<Foreign>,
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
            foreign: None,
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
                s.lapses = 0;
                s.settled();
            }
        }
        close
    }

    fn allowed(&self, file: &str) -> bool {
        allowed_in(self.allow.as_ref(), file)
    }

    /// A ticket for the monitor `monitor` showing `file`, being pushed to now. `None` (the push
    /// goes without one) when held, not allowed, the wallpaper has a socket, a ticket is still
    /// outstanding for it, or the store is full or has no random source. After a refusal while
    /// a ticket waits ([`Pairing::refused`]) it is that same ticket, once.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn mint(&mut self, monitor: &str, file: &str, now: Instant) -> Option<String> {
        self.hand(monitor, file, now).map(|(ticket, _)| ticket)
    }

    /// [`Pairing::mint`], saying whether the ticket is a new one (`true`) or the waiting one
    /// handed over once more (`false`: its wait is already running, and a push that fails
    /// leaves it as it was). A ticket belongs to the wallpaper it was minted for: another
    /// wallpaper on the monitor never gets it.
    pub fn hand(&mut self, monitor: &str, file: &str, now: Instant) -> Option<(String, bool)> {
        let (held, allowed) = (self.held, self.allowed(file));
        // The monitor is known even when nothing is minted, so allowing its wallpaper later has
        // a slot to mark for a push.
        let slot = self.slots.entry(monitor.to_string()).or_default();
        if slot.file != file {
            // Another wallpaper: the one before it loses its waiting ticket, and this one
            // starts its own count.
            self.tickets.cancel(monitor);
            slot.settled();
            slot.lapses = 0;
        }
        let busy = self.tickets.has_open(monitor, now);
        slot.file = file.to_string();
        slot.due = false;
        let again = std::mem::take(&mut slot.again);
        if held || !allowed || slot.conn.is_some() {
            return None;
        }
        if busy {
            if !again {
                return None;
            }
            slot.handed_again = true;
            return self.tickets.open_for(monitor, now).map(|t| (t, false));
        }
        let ticket = self.tickets.mint(monitor, now);
        slot.handed_again = false;
        slot.out = ticket.as_ref().map(|_| now + wire::TICKET_TTL);
        ticket.map(|t| (t, true))
    }

    /// `monitor` shows `file` and gets no ticket for it (the wallpaper doesn't ask for the feed,
    /// or the feed is off): what Foxglove knew about another wallpaper there is forgotten, and
    /// a ticket that waited for that one is cancelled.
    pub fn showing(&mut self, monitor: &str, file: &str) {
        if self.slots.get(monitor).is_some_and(|s| s.file != file) {
            self.slots.remove(monitor);
            self.tickets.cancel(monitor);
        }
    }

    /// The push carrying a new ticket for `monitor` failed: cancel it. (A waiting ticket handed
    /// over again is left as it was: see [`Pairing::hand`].)
    pub fn cancel(&mut self, monitor: &str) {
        self.tickets.cancel(monitor);
        if let Some(s) = self.slots.get_mut(monitor) {
            s.settled();
        }
    }

    /// The handshake's decision, at the moment of the handshake (`now`): the origin first, then
    /// a ticket, then the old token (`admit_or_ticket`). `Ok(Some(monitor))` when a ticket
    /// admitted, `Ok(None)` when the old token did. `host`: the peer is the wallpaper host
    /// (`live_peer`); from any other peer the ticket store is not asked at all.
    pub fn admit(
        &mut self,
        origin: Option<&str>,
        given: Option<&str>,
        token: &str,
        host: bool,
        now: Instant,
    ) -> Result<Option<String>, Denied> {
        let monitor = given
            .filter(|_| host)
            .and_then(|g| self.tickets.monitor_of(g, now))
            .unwrap_or_default();
        // A blank stored token admits nothing, so once verified only tickets pass. Disconnect
        // all holds the old path too (sec-1298 L4).
        let token = if self.old_off || self.held { "" } else { token };
        wire::admit_or_ticket(origin, given, token, &mut self.tickets, &monitor, host, now)?;
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
        if let Some(m) = monitor {
            // Connected: nothing may wait for this monitor (a ticket minted between the redeem
            // and now).
            self.tickets.cancel(m);
        }
        if let Some(s) = monitor.and_then(|m| self.slots.get_mut(m)) {
            s.conn = Some(conn);
            s.capped = false;
            s.lapses = 0;
            s.settled();
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

    /// A program that isn't the wallpaper host was refused at `now` (`name`: its cleaned file
    /// name, if it has one). Recorded for the section's line; no monitor is marked, no ticket
    /// is touched and no budget is spent.
    pub fn intruder(&mut self, name: Option<String>, now: Instant) {
        self.foreign = Some(Foreign { name, at: now });
    }

    /// A connection from the wallpaper host (or from a peer that couldn't be checked) was
    /// refused at `now`: marks the monitors that get a ticket at the next sync (a new one, or
    /// the one still waiting once more) and returns how many. See the module docs for who
    /// qualifies.
    pub fn refused(&mut self, now: Instant) -> usize {
        if self.held {
            return 0;
        }
        let mut n = 0;
        for (monitor, s) in &mut self.slots {
            // A wallpaper the user hasn't allowed is never re-pushed, and never counts as stuck.
            if !allowed_in(self.allow.as_ref(), &s.file) || s.conn.is_some() || s.due {
                continue;
            }
            // A ticket still waiting is not cancelled: it goes to its monitor once more (the
            // wallpaper may have reloaded since the push), and no further time while it waits.
            let waiting = self.tickets.has_open(monitor, now);
            if waiting && s.handed_again {
                continue;
            }
            s.repushes
                .retain(|t| now.saturating_duration_since(*t) < REPUSH_WINDOW);
            if s.repushes.len() >= REPUSH_MAX {
                // With a ticket still waiting the wallpaper isn't stuck yet. Only ever set
                // here: a connection or Reconnect clears it.
                if !waiting {
                    s.capped = true;
                }
                continue;
            }
            s.repushes.push(now);
            s.due = true;
            s.again = waiting;
            n += 1;
        }
        n
    }

    /// Tickets that ran out unused by `now`, on monitors still without a socket: each is
    /// replaced at the next sync, at most [`LAPSE_MAX`] times in a row per monitor. Returns how
    /// many monitors were marked. Not a refusal: no budget is spent and nothing counts as stuck.
    pub fn lapsed(&mut self, now: Instant) -> usize {
        if self.held {
            return 0;
        }
        let mut n = 0;
        for (monitor, s) in &mut self.slots {
            if s.out.is_none_or(|out| out > now) {
                continue;
            }
            s.out = None;
            if !allowed_in(self.allow.as_ref(), &s.file)
                || s.conn.is_some()
                || s.due
                || self.tickets.has_open(monitor, now)
                || s.lapses >= LAPSE_MAX
            {
                continue;
            }
            s.lapses += 1;
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
    /// ticket at the next sync (the outstanding one is cancelled). A connected wallpaper keeps
    /// its socket. The line about another program goes.
    pub fn reconnect(&mut self) {
        self.held = false;
        self.foreign = None;
        for (m, s) in &mut self.slots {
            s.repushes.clear();
            s.capped = false;
            s.lapses = 0;
            if s.conn.is_none() {
                self.tickets.cancel(m);
                s.settled();
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
        self.foreign = None;
        for s in self.slots.values_mut() {
            s.conn = None;
            s.due = false;
            s.settled();
        }
        self.retire_old_token()
    }

    /// The feed stopped: nothing outstanding, nothing connected.
    pub fn stopped(&mut self) {
        self.foreign = None;
        self.tickets.clear();
        for s in self.slots.values_mut() {
            s.conn = None;
            s.due = false;
            s.lapses = 0;
            s.settled();
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
    #[cfg_attr(test, allow(dead_code))]
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
            // Said for a while after the last one, then it goes on its own.
            foreign: self
                .foreign
                .as_ref()
                .filter(|f| now.saturating_duration_since(f.at) < wire::REFUSAL_QUIET)
                .map(|f| Intruder {
                    name: f.name.clone(),
                }),
        }
    }

    /// The ticket for `monitor` is gone unused, as if it ran out at `at` (for the pusher's tests,
    /// whose pushes mint at the real time).
    #[cfg(test)]
    pub fn cancel_for_test(&mut self, monitor: &str, at: Instant) {
        self.tickets.cancel(monitor);
        if let Some(s) = self.slots.get_mut(monitor) {
            s.out = Some(at);
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
pub fn reconnect(store: Store) {
    locked(store).reconnect()
}

/// Disconnect all, the store's half: see [`Pairing::disconnect_all`].
pub fn disconnect_all(store: Store) -> Vec<u64> {
    locked(store).disconnect_all()
}

/// What the Hideout panel shows per wallpaper folder.
#[cfg_attr(test, allow(dead_code))]
pub fn links(store: Store) -> Links {
    locked(store).links(Instant::now())
}

/// Tickets that ran out unused are replaced: see [`Pairing::lapsed`]. The caller wakes the
/// pusher when this isn't 0.
#[cfg_attr(test, allow(dead_code))]
pub fn lapsed(store: Store) -> usize {
    locked(store).lapsed(Instant::now())
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
            p.admit(Some("null"), Some(&t), "", true, now),
            Ok(Some(A.to_string()))
        );
        assert!(
            p.admit(Some("null"), Some(&t), "", true, now).is_err(),
            "single use"
        );
        // A web page doesn't burn it.
        let t = ticket(&mut p, A, now);
        assert!(p
            .admit(Some("https://x.test"), Some(&t), "", true, now)
            .is_err());
        assert_eq!(p.open(now), 1);
        // The old token still admits, and names no monitor.
        assert_eq!(
            p.admit(None, Some("old-token"), "old-token", true, now),
            Ok(None)
        );
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
        assert!(
            p.admit(None, Some(&t), "", true, now).is_err(),
            "ticket cancelled"
        );
        p.set_allowed(only(&["hud"]));
        let t = ticket(&mut p, A, now);
        assert_eq!(p.admit(None, Some(&t), "", true, now), Ok(Some(A.into())));
        p.joined(7, Some(A));
        assert_eq!(p.set_allowed(only(&[])), vec![7]);
        assert!(
            p.mint(A, "hud/index.html", now).is_none(),
            "off mints nothing"
        );
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
        assert_eq!(p.admit(None, Some("old"), "old", true, now), Ok(None));
        p.joined(9, None);
        assert_eq!(p.disconnect_all(), vec![9]);
        assert!(
            p.admit(None, Some("old"), "old", true, now).is_err(),
            "token retired"
        );
    }

    #[test]
    fn no_second_ticket_while_one_waits_or_a_socket_is_open() {
        let (mut p, now) = (Pairing::new(), Instant::now());
        let t = ticket(&mut p, A, now);
        assert!(p.mint(A, "hud/index.html", now).is_none(), "one waits");
        assert_eq!(p.admit(None, Some(&t), "", true, now), Ok(Some(A.into())));
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
        // Both have tickets waiting: a refusal cancels neither; each goes out once more, the
        // same ticket.
        assert_eq!(p.refused(now), 2);
        assert_eq!(p.open(now), 2);
        assert_eq!(p.take_due(), [A.to_string(), B.to_string()]);
        assert_eq!(p.mint(A, "hud/index.html", now), Some(ta.clone()));
        assert_eq!(p.mint(B, "hud/index.html", now), Some(_tb.clone()));
        assert_eq!(p.open(now), 2);
        // Once per ticket: more refusals while they wait get nothing.
        for _ in 0..20 {
            assert_eq!(p.refused(now), 0);
        }
        assert!(p.mint(A, "hud/index.html", now).is_none(), "one waits");
        p.admit(None, Some(&ta), "", true, now).unwrap();
        assert!(p.joined(1, Some(A)));
        // A has a socket, B a ticket handed twice: nobody.
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

    /// A wallpaper that reloaded inside its ticket's two minutes lost it: the host's refused
    /// knock gets that ticket again, with its old deadline, and it still works once.
    #[test]
    fn a_refusal_while_a_ticket_waits_hands_the_same_one_again_with_its_deadline() {
        let (mut p, now) = (Pairing::new(), Instant::now());
        let t = ticket(&mut p, A, now);
        let soon = now + Duration::from_secs(60);
        assert_eq!(p.refused(soon), 1);
        assert_eq!(p.take_due(), [A.to_string()]);
        assert_eq!(p.mint(A, "hud/index.html", soon), Some(t.clone()));
        // The deadline is the first push's.
        let late = now + wire::TICKET_TTL;
        assert!(p.admit(None, Some(&t), "", true, late).is_err());
        // In time it admits, once.
        let (mut p, now) = (Pairing::new(), Instant::now());
        let t = ticket(&mut p, A, now);
        assert_eq!(p.refused(now), 1);
        p.take_due();
        assert_eq!(p.mint(A, "hud/index.html", now), Some(t.clone()));
        assert_eq!(p.admit(None, Some(&t), "", true, now), Ok(Some(A.into())));
        assert!(
            p.admit(None, Some(&t), "", true, now).is_err(),
            "single use"
        );
    }

    #[test]
    fn handing_a_waiting_ticket_again_counts_in_the_budget_and_never_for_another_wallpaper() {
        let (mut p, start) = (Pairing::new(), Instant::now());
        p.set_allowed(Some(BTreeSet::from(["hud".to_string()])));
        assert!(p.mint(B, "other/index.html", start).is_none());
        let mut now = start;
        for _ in 0..REPUSH_MAX {
            ticket(&mut p, A, now);
            assert_eq!(p.refused(now), 1, "only the allowed monitor");
            assert_eq!(p.take_due(), [A.to_string()]);
            assert!(p.mint(A, "hud/index.html", now).is_some());
            now += wire::TICKET_TTL + Duration::from_secs(1);
        }
        ticket(&mut p, A, now);
        assert_eq!(p.refused(now), 0, "the budget is spent");
        assert!(!p.stuck(), "its ticket still waits");
        assert_eq!(p.refused(now + wire::TICKET_TTL), 0);
        assert!(p.stuck());
    }

    #[test]
    fn a_ticket_that_runs_out_unused_is_replaced_twice_then_no_more() {
        let (mut p, start) = (Pairing::new(), Instant::now());
        let mut now = start;
        ticket(&mut p, A, now);
        assert_eq!(p.lapsed(now + Duration::from_secs(60)), 0, "it still waits");
        for round in 0..LAPSE_MAX {
            now += wire::TICKET_TTL;
            assert_eq!(p.lapsed(now), 1, "replacement {round}");
            assert_eq!(p.lapsed(now), 0, "marked once");
            assert_eq!(p.take_due(), [A.to_string()]);
            ticket(&mut p, A, now);
        }
        now += wire::TICKET_TTL;
        assert_eq!(p.lapsed(now), 0, "no loop");
        assert!(!p.stuck(), "not a refusal");
        assert_eq!(p.links(now).by_dir.get("hud"), Some(&Link::Idle));
        // A connection starts the count again.
        assert_eq!(p.refused(now), 1);
        p.take_due();
        let t = ticket(&mut p, A, now);
        p.admit(None, Some(&t), "", true, now).unwrap();
        assert!(p.joined(3, Some(A)));
        assert_eq!(p.lapsed(now + wire::TICKET_TTL), 0, "it was used");
        p.left(3);
        ticket(&mut p, A, now);
        assert_eq!(p.lapsed(now + wire::TICKET_TTL), 1);
    }

    #[test]
    fn nothing_is_replaced_while_held_not_allowed_connected_or_after_the_feed_stops() {
        let (mut p, now) = (Pairing::new(), Instant::now());
        let later = now + wire::TICKET_TTL;
        ticket(&mut p, A, now);
        p.disconnect_all();
        // Held is checked by itself: even a ticket that would count as run out is not replaced.
        p.cancel_for_test(A, later);
        assert_eq!(p.lapsed(later), 0, "held");
        assert!(p.take_due().is_empty());
        p.reconnect();
        p.take_due();
        ticket(&mut p, A, now);
        p.set_allowed(Some(BTreeSet::new()));
        assert_eq!(p.lapsed(later), 0, "no longer allowed");
        p.set_allowed(None);
        p.take_due();
        ticket(&mut p, A, now);
        p.stopped();
        assert_eq!(p.lapsed(later), 0, "the feed stopped");
        ticket(&mut p, A, now);
        p.cancel(A);
        assert_eq!(p.lapsed(later), 0, "its push failed");
        ticket(&mut p, B, now);
        p.keep(&[A]);
        assert_eq!(p.lapsed(later), 0, "off screen");
    }

    /// A ticket belongs to the wallpaper it was minted for: another wallpaper put on the
    /// monitor gets its own, and the old one is worth nothing.
    #[test]
    fn another_wallpaper_on_the_monitor_never_gets_the_waiting_ticket() {
        let (mut p, now) = (Pairing::new(), Instant::now());
        let old = ticket(&mut p, A, now);
        assert_eq!(p.refused(now), 1);
        p.take_due();
        let (new, fresh) = p.hand(A, "reef/index.html", now).expect("its own ticket");
        assert!(fresh);
        assert_ne!(new, old);
        assert_eq!(p.open(now), 1);
        assert!(p.admit(None, Some(&old), "", true, now).is_err());
        // Without a refusal too.
        let (mut p, now) = (Pairing::new(), Instant::now());
        let old = ticket(&mut p, A, now);
        let new = p.mint(A, "reef/index.html", now).expect("its own ticket");
        assert_ne!(new, old);
        assert!(p.admit(None, Some(&old), "", true, now).is_err());
        assert_eq!(p.admit(None, Some(&new), "", true, now), Ok(Some(A.into())));
    }

    #[test]
    fn a_wallpaper_that_takes_no_ticket_ends_the_one_waiting_for_the_wallpaper_before_it() {
        let (mut p, now) = (Pairing::new(), Instant::now());
        let old = ticket(&mut p, A, now);
        let kept = ticket(&mut p, B, now);
        p.showing(A, "plain/index.html");
        p.showing(B, "hud/index.html");
        p.showing("M9", "plain/index.html");
        assert_eq!(p.open(now), 1, "only B's, whose wallpaper is the same");
        assert!(p.admit(None, Some(&old), "", true, now).is_err());
        assert_eq!(
            p.admit(None, Some(&kept), "", true, now),
            Ok(Some(B.into()))
        );
        assert!(p.joined(1, Some(B)));
        // And A is no longer a monitor a refusal could mark.
        assert_eq!(p.refused(now + wire::TICKET_TTL), 0);
    }

    #[test]
    fn a_connection_leaves_no_ticket_waiting_and_the_budget_cap_stays_until_then() {
        let (mut p, start) = (Pairing::new(), Instant::now());
        let mut now = start;
        ticket(&mut p, A, now);
        for _ in 0..REPUSH_MAX {
            now += wire::TICKET_TTL + Duration::from_secs(1);
            assert_eq!(p.refused(now), 1);
            p.take_due();
            ticket(&mut p, A, now);
        }
        now += wire::TICKET_TTL + Duration::from_secs(1);
        assert_eq!(p.refused(now), 0);
        assert!(p.stuck());
        // A replacement for a ticket that ran out waits: a refusal doesn't hide the notice.
        assert_eq!(p.lapsed(now), 1);
        p.take_due();
        let t = ticket(&mut p, A, now);
        assert_eq!(p.refused(now), 0);
        assert!(p.stuck(), "still capped");
        // Redeemed; a ticket minted before the join is gone once it joins.
        p.admit(None, Some(&t), "", true, now).unwrap();
        let stray = ticket(&mut p, A, now);
        assert!(p.joined(7, Some(A)));
        assert_eq!(p.open(now), 0);
        assert!(p.admit(None, Some(&stray), "", true, now).is_err());
        assert!(!p.stuck());
    }

    #[test]
    fn switching_a_wallpaper_off_and_on_starts_its_count_of_unused_tickets_again() {
        let (mut p, start) = (Pairing::new(), Instant::now());
        let mut now = start;
        ticket(&mut p, A, now);
        for _ in 0..LAPSE_MAX {
            now += wire::TICKET_TTL;
            assert_eq!(p.lapsed(now), 1);
            p.take_due();
            ticket(&mut p, A, now);
        }
        p.set_allowed(Some(BTreeSet::new()));
        p.set_allowed(None);
        p.take_due();
        ticket(&mut p, A, now);
        assert_eq!(p.lapsed(now + wire::TICKET_TTL), 1);
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
            p.admit(None, Some(&t), "", true, now).is_err(),
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
        p.admit(None, Some(&tb), "", true, now).unwrap();
        p.joined(2, Some(B));
        p.reconnect();
        assert_eq!(p.take_due(), [A.to_string()]);
        assert!(p.admit(None, Some(&ta), "", true, now).is_err());
    }

    /// Joins `m` the way the feed does: mint, redeem, join.
    fn join(p: &mut Pairing, m: &str, conn: u64, now: Instant) {
        let t = ticket(p, m, now);
        assert_eq!(
            p.admit(None, Some(&t), "", true, now),
            Ok(Some(m.to_string()))
        );
        assert!(p.joined(conn, Some(m)));
    }

    fn named(name: &str) -> Option<Intruder> {
        Some(Intruder {
            name: Some(name.into()),
        })
    }

    #[test]
    fn a_ticket_from_a_peer_that_is_not_the_host_is_left_alone() {
        let (mut p, now) = (Pairing::new(), Instant::now());
        let t = ticket(&mut p, A, now);
        assert_eq!(
            p.admit(None, Some(&t), "", false, now),
            Err(Denied::Token),
            "not the host"
        );
        assert_eq!(p.open(now), 1, "not used up, not cancelled");
        // The wallpaper host then pairs with that same ticket.
        assert_eq!(
            p.admit(None, Some(&t), "", true, now),
            Ok(Some(A.to_string()))
        );
    }

    #[test]
    fn the_old_token_admits_a_program_until_a_ticket_was_redeemed() {
        let (mut p, now) = (Pairing::new(), Instant::now());
        assert_eq!(p.admit(None, Some("old"), "old", false, now), Ok(None));
        assert!(p.admit(None, Some("wrong"), "old", false, now).is_err());
        p.set_old_token_off(true);
        assert!(p.admit(None, Some("old"), "old", false, now).is_err());
    }

    #[test]
    fn another_program_is_reported_whatever_is_connected() {
        let now = Instant::now();
        // No wallpaper on screen.
        let mut p = Pairing::new();
        assert_eq!(p.links(now).foreign, None, "nothing refused yet");
        p.intruder(Some("tool.exe".into()), now);
        assert_eq!(p.links(now).foreign, named("tool.exe"));
        // One connected, one still connecting.
        let mut p = Pairing::new();
        join(&mut p, A, 1, now);
        let tb = ticket(&mut p, B, now);
        p.intruder(Some("tool.exe".into()), now);
        assert_eq!(p.links(now).foreign, named("tool.exe"));
        // All connected; a program with no name to show is still reported.
        p.admit(None, Some(&tb), "", true, now).unwrap();
        assert!(p.joined(2, Some(B)));
        p.intruder(None, now);
        assert_eq!(p.links(now).foreign, Some(Intruder { name: None }));
    }

    #[test]
    fn joins_and_closes_do_not_hide_another_program_but_reconnect_does() {
        let (mut p, now) = (Pairing::new(), Instant::now());
        join(&mut p, A, 1, now);
        p.intruder(Some("tool.exe".into()), now);
        join(&mut p, B, 2, now);
        assert_eq!(p.links(now).foreign, named("tool.exe"), "a new join");
        p.left(2);
        assert_eq!(p.links(now).foreign, named("tool.exe"), "a socket closed");
        p.keep(&[A]);
        assert_eq!(p.links(now).foreign, named("tool.exe"), "a monitor went");
        p.set_allowed(None);
        assert_eq!(p.links(now).foreign, named("tool.exe"), "the allow list");
        p.reconnect();
        assert_eq!(p.links(now).foreign, None, "Reconnect");
        p.intruder(None, now);
        p.disconnect_all();
        assert_eq!(p.links(now).foreign, None, "Disconnect all");
        p.intruder(None, now);
        p.stopped();
        assert_eq!(p.links(now).foreign, None, "the feed stopped");
    }

    #[test]
    fn another_program_is_no_longer_reported_after_the_quiet_time() {
        let (mut p, now) = (Pairing::new(), Instant::now());
        p.intruder(Some("tool.exe".into()), now);
        let almost = now + wire::REFUSAL_QUIET - Duration::from_secs(1);
        assert_eq!(p.links(almost).foreign, named("tool.exe"));
        assert_eq!(p.links(now + wire::REFUSAL_QUIET).foreign, None);
        // A later one starts the time again, with its own name.
        p.intruder(Some("other.exe".into()), almost);
        assert_eq!(
            p.links(now + wire::REFUSAL_QUIET).foreign,
            named("other.exe")
        );
    }

    #[test]
    fn another_program_never_marks_a_monitor_for_a_new_ticket() {
        let (mut p, now) = (Pairing::new(), Instant::now());
        ticket(&mut p, A, now);
        // A's ticket has expired: a refusal of the host would re-push it.
        let later = now + wire::TICKET_TTL + Duration::from_secs(1);
        for _ in 0..50 {
            p.intruder(Some("tool.exe".into()), later);
        }
        assert!(p.take_due().is_empty());
        assert!(!p.stuck(), "no budget spent");
        assert_eq!(p.refused(later), 1, "the host's own refusal still does");
    }

    #[test]
    fn reconnect_leaves_connected_wallpapers_alone() {
        let (mut p, now) = (Pairing::new(), Instant::now());
        join(&mut p, A, 1, now);
        join(&mut p, B, 2, now);
        p.intruder(Some("tool.exe".into()), now);
        p.reconnect();
        assert!(p.take_due().is_empty(), "nothing to push again");
        let links = p.links(now);
        assert!(links.by_dir.values().all(|l| *l == Link::Connected));
    }

    #[test]
    fn monitors_off_screen_are_forgotten_with_their_tickets() {
        let (mut p, now) = (Pairing::new(), Instant::now());
        let t = ticket(&mut p, A, now);
        ticket(&mut p, B, now);
        p.keep(&[B]);
        assert!(p.admit(None, Some(&t), "", true, now).is_err());
        assert_eq!(p.open(now), 1);
    }

    /// S2-4's sweep for this module: the store has no `Debug`, the ticket store's prints a count,
    /// and a refused connection leaves nothing to print but counts.
    #[test]
    fn nothing_the_store_can_print_holds_a_ticket() {
        let (mut p, now) = (Pairing::new(), Instant::now());
        let t = ticket(&mut p, A, now);
        assert!(!format!("{:?}", p.tickets).contains(&t));
        assert_eq!(p.refused(now), 1);
        assert!(!format!("{:?}", p.tickets).contains(&t));
    }

    /// Disconnect all against a connection that has been admitted but not yet joined: the join
    /// is refused once the hold is set (the live feed does both under this lock).
    #[test]
    fn a_connection_admitted_before_disconnect_all_cannot_join_after_it() {
        let (mut p, now) = (Pairing::new(), Instant::now());
        let t = ticket(&mut p, A, now);
        assert_eq!(p.admit(None, Some(&t), "", true, now), Ok(Some(A.into())));
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
