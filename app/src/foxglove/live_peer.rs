//! Which program is at the other end of a live-feed connection (#1445). The feed listens on
//! 127.0.0.1, so every peer is a program on this PC; the system names the process that owns the
//! peer's end of the connection, and a one-time ticket only counts when that process's image
//! lies inside the folder of the wallpaper program Foxglove drives (`wallpaper_engine::host_dir`,
//! the same record every push uses). No program or vendor name decides anything here.
//!
//! The connection is found by all four of its ends' numbers (both addresses are 127.0.0.1, both
//! ports match), and its owner must have been running before the connection's socket was made
//! and before the connection came in.
//!
//! One lookup per accepted connection, on that connection's own thread: one read of the
//! system's connection table (a second at once when the row is missing), one process opened.
//! No timer, no retry loop. A peer that cannot be identified is refused, never trusted.
//!
//! A program's name is its image's file name only, cleaned and cut: never a path or a pid.

use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// What the lookup says about one connection's peer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Peer {
    /// A program inside the wallpaper host's folder, running before the connection came in.
    Host,
    /// Some other program. `name`: its file name made safe to show; `None` when nothing is left.
    Other { name: Option<String> },
    /// It couldn't be told.
    Unknown(Unknown),
}

/// Why a peer couldn't be told.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unknown {
    /// The system lists no such connection: the peer is gone already.
    Gone,
    /// The owning process couldn't be opened or read.
    Unreadable,
    /// The owning process was created after the connection came in.
    BornLater,
    /// Foxglove knows no wallpaper host folder.
    NoHostFolder,
    /// The wallpaper host's folder isn't on a local drive.
    HostNotLocal,
    /// The system's connection table couldn't be read.
    Table,
    /// The peer's address isn't 127.0.0.1, the only one a wallpaper connects from.
    Elsewhere,
}

impl Unknown {
    /// One fixed word for the log line.
    pub fn word(self) -> &'static str {
        match self {
            Unknown::Gone => "gone",
            Unknown::Unreadable => "unreadable",
            Unknown::BornLater => "started later",
            Unknown::NoHostFolder => "no host folder",
            Unknown::HostNotLocal => "host folder not local",
            Unknown::Table => "table",
            Unknown::Elsewhere => "peer address",
        }
    }
}

/// The process that owns a connection's peer end.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Owner {
    pub image: PathBuf,
    pub born: SystemTime,
    /// When the connection's socket was made, when the system dates it.
    pub made: Option<SystemTime>,
}

/// [`PeerLookup::owner`]'s error when the connection has an owner that couldn't be read.
pub const UNREADABLE: &str = "unreadable";

/// Where the facts come from: the system ([`System`]), or a test's stand-in.
pub trait PeerLookup: Send + Sync {
    /// Ok(None): no such connection (the peer is gone).
    fn owner(&self, peer_port: u16, feed_port: u16) -> Result<Option<Owner>, String>;
    fn host_dir(&self) -> Option<PathBuf>;
    fn inside(&self, image: &Path, dir: &Path) -> bool;
    /// Whether `dir` is on a local drive (the only place a host folder counts).
    fn local(&self, dir: &Path) -> bool;
    /// The host folder looked up afresh, for a peer the known folder didn't cover (the host may
    /// have moved). `None` when it wasn't looked up; an answer is at most one fresh look per
    /// `REFUSAL_QUIET`.
    fn host_dir_again(&self) -> Option<PathBuf> {
        None
    }
}

/// A program's file name made safe to show and log: no path, no control or direction
/// characters, cut. `None` when nothing is left.
fn shown_name(image: &Path) -> Option<String> {
    let raw = image.file_name()?.to_string_lossy();
    let name = super::api::clean_client_name(&raw);
    (name != super::api::NAMELESS_CALLER).then_some(name)
}

/// The verdict from the facts. `host_local`: the host folder is on a local drive.
pub fn judge(
    owner: Result<Option<Owner>, String>,
    host_dir: Option<&Path>,
    host_local: bool,
    accepted: SystemTime,
    inside: &dyn Fn(&Path, &Path) -> bool,
) -> Peer {
    let owner = match owner {
        Ok(Some(o)) => o,
        Ok(None) => return Peer::Unknown(Unknown::Gone),
        Err(e) if e == UNREADABLE => return Peer::Unknown(Unknown::Unreadable),
        Err(_) => return Peer::Unknown(Unknown::Table),
    };
    let Some(dir) = host_dir else {
        return Peer::Unknown(Unknown::NoHostFolder);
    };
    if !host_local {
        return Peer::Unknown(Unknown::HostNotLocal);
    }
    // A process created after the connection came in, or after the connection's socket was
    // made, was given a finished process's number.
    if owner.born > accepted || owner.made.is_some_and(|made| owner.born > made) {
        return Peer::Unknown(Unknown::BornLater);
    }
    if inside(&owner.image, dir) {
        Peer::Host
    } else {
        Peer::Other {
            name: shown_name(&owner.image),
        }
    }
}

/// Who is at the other end of `stream`, accepted at `accepted` on the feed's port `feed_port`.
pub fn peer_of(
    stream: &TcpStream,
    feed_port: u16,
    accepted: SystemTime,
    lookup: &dyn PeerLookup,
) -> Peer {
    let Ok(addr) = stream.peer_addr() else {
        return Peer::Unknown(Unknown::Gone);
    };
    peer_at(addr, feed_port, accepted, lookup)
}

/// [`peer_of`] for a peer at `addr`.
fn peer_at(
    addr: SocketAddr,
    feed_port: u16,
    accepted: SystemTime,
    lookup: &dyn PeerLookup,
) -> Peer {
    // Another address can share a wallpaper's port number: only 127.0.0.1 is looked up.
    if addr.ip() != IpAddr::V4(Ipv4Addr::LOCALHOST) {
        return Peer::Unknown(Unknown::Elsewhere);
    }
    let mut owner = lookup.owner(addr.port(), feed_port);
    if matches!(owner, Ok(None)) {
        // The row may not be listed yet: one more read, at once.
        owner = lookup.owner(addr.port(), feed_port);
    }
    let judged = |dir: Option<&Path>| {
        let local = dir.is_some_and(|d| lookup.local(d));
        judge(owner.clone(), dir, local, accepted, &|i, d| {
            lookup.inside(i, d)
        })
    };
    let dir = lookup.host_dir();
    let first = judged(dir.as_deref());
    // Only when the known host program is no longer where it was: a connection never makes
    // Foxglove look for a new host while the known one is in place.
    if first == Peer::Unknown(Unknown::NoHostFolder) {
        // The host may have moved since it was last found.
        if let Some(again) = lookup.host_dir_again().filter(|a| Some(a) != dir.as_ref()) {
            return judged(Some(again.as_path()));
        }
    }
    first
}

/// 8-byte words in one row of the table [`owner_in`] reads (160 bytes).
const ROW: usize = 20;

/// The two 4-byte numbers in one 8-byte word of the table, in memory order.
fn halves(word: u64) -> (u32, u32) {
    let b = word.to_ne_bytes();
    (
        u32::from_ne_bytes([b[0], b[1], b[2], b[3]]),
        u32::from_ne_bytes([b[4], b[5], b[6], b[7]]),
    )
}

/// The owning process id, and when the socket was made (100 ns ticks since 1601; 0 when the
/// system doesn't say), of the connection from 127.0.0.1:`peer_port` to 127.0.0.1:`feed_port`,
/// in a table read as `GetExtendedTcpTable` fills it for IPv4 with owner modules: a row count
/// in the first 8 bytes, then 160 bytes a row (4-byte state, local address, local port, remote
/// address, remote port, pid; the 8-byte time; 128 bytes not read here), addresses and ports
/// in network byte order.
#[cfg_attr(not(any(test, target_os = "windows")), allow(dead_code))]
fn owner_in(table: &[u64], peer_port: u16, feed_port: u16) -> Option<(u32, u64)> {
    let port = |v: u32| {
        let b = v.to_ne_bytes();
        u16::from_be_bytes([b[0], b[1]])
    };
    let loopback = u32::from_ne_bytes([127, 0, 0, 1]);
    let (count, rows) = table.split_first()?;
    let count = halves(*count).0 as usize;
    rows.as_chunks::<ROW>().0.iter().take(count).find_map(|r| {
        let (_state, local) = halves(r[0]);
        let (local_port, remote) = halves(r[1]);
        let (remote_port, pid) = halves(r[2]);
        let ours = local == loopback
            && remote == loopback
            && port(local_port) == peer_port
            && port(remote_port) == feed_port;
        ours.then_some((pid, r[3]))
    })
}

/// The time `ticks` (100 ns since 1601) names; `None` for 0 and for what can't be a time.
#[cfg_attr(not(any(test, target_os = "windows")), allow(dead_code))]
fn made_at(ticks: u64) -> Option<SystemTime> {
    // The Unix epoch is 11 644 473 600 s after 1601.
    const EPOCH: u64 = 116_444_736_000_000_000;
    let since = ticks.checked_sub(EPOCH)?;
    SystemTime::UNIX_EPOCH.checked_add(std::time::Duration::new(
        since / 10_000_000,
        (since % 10_000_000) as u32 * 100,
    ))
}

/// One bounded read of the system's IPv4 connection table.
#[cfg(target_os = "windows")]
fn owner_row(peer_port: u16, feed_port: u16) -> Result<Option<(u32, u64)>, String> {
    use windows::Win32::Foundation::ERROR_INSUFFICIENT_BUFFER;
    use windows::Win32::NetworkManagement::IpHelper::{
        GetExtendedTcpTable, TCP_TABLE_OWNER_MODULE_CONNECTIONS,
    };
    const AF_INET: u32 = 2;
    let mut table: Vec<u64> = Vec::new();
    let mut size = 0u32;
    // The size is asked first; the table can grow between the two calls, so three tries.
    for _ in 0..3 {
        let buf = (!table.is_empty()).then(|| table.as_mut_ptr().cast::<core::ffi::c_void>());
        // SAFETY: `buf` is null or `size` writable bytes; `size` is a valid in/out pointer.
        let code = unsafe {
            GetExtendedTcpTable(
                buf,
                &mut size,
                false,
                AF_INET,
                TCP_TABLE_OWNER_MODULE_CONNECTIONS,
                0,
            )
        };
        if code == 0 && !table.is_empty() {
            return Ok(owner_in(&table, peer_port, feed_port));
        }
        if code != 0 && code != ERROR_INSUFFICIENT_BUFFER.0 {
            return Err(format!("connection table: error {code}"));
        }
        // Room for the rows named, and for a few that appear meanwhile.
        table = vec![0u64; (size as usize).div_ceil(8) + 16 * ROW];
        size = (table.len() * 8) as u32;
    }
    Err("connection table: kept growing".into())
}

/// The real lookup. Off Windows nothing can be asked: every peer is unknown.
#[cfg_attr(test, allow(dead_code))]
pub struct System;

impl PeerLookup for System {
    fn owner(&self, peer_port: u16, feed_port: u16) -> Result<Option<Owner>, String> {
        #[cfg(target_os = "windows")]
        {
            let Some((pid, made)) = owner_row(peer_port, feed_port)? else {
                return Ok(None);
            };
            let made = made_at(made);
            match foxglove_plugin::owner::image_and_birth(pid) {
                Some((image, born)) => Ok(Some(Owner { image, born, made })),
                None => Err(UNREADABLE.into()),
            }
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = (peer_port, feed_port);
            Err("not on this system".into())
        }
    }

    fn host_dir(&self) -> Option<PathBuf> {
        super::wallpaper_engine::host_dir()
    }

    fn inside(&self, image: &Path, dir: &Path) -> bool {
        foxglove_plugin::owner::inside(image, dir, &foxglove_plugin::owner::NoRecords)
    }

    fn local(&self, dir: &Path) -> bool {
        foxglove_plugin::owner::local(dir, &foxglove_plugin::owner::NoRecords)
    }

    fn host_dir_again(&self) -> Option<PathBuf> {
        use std::time::Instant;
        static LOOKED: std::sync::Mutex<Option<Instant>> = std::sync::Mutex::new(None);
        {
            let mut looked = LOOKED.lock().unwrap_or_else(|e| e.into_inner());
            let now = Instant::now();
            if looked
                .is_some_and(|t| now.saturating_duration_since(t) < super::live_wire::REFUSAL_QUIET)
            {
                return None;
            }
            *looked = Some(now);
        }
        let found = super::wallpaper_engine::refresh();
        super::wallpaper_engine::cli_exe(&found)?
            .parent()
            .map(Path::to_path_buf)
    }
}

/// A test's lookup: fixed answers, and a count of the table reads asked of it.
#[cfg(test)]
pub struct Fake {
    owner: std::sync::Mutex<Result<Option<Owner>, String>>,
    dir: Option<PathBuf>,
    again: Option<PathBuf>,
    reads: std::sync::atomic::AtomicUsize,
}

#[cfg(test)]
impl Fake {
    /// The folder every fake host lives in.
    pub const HOST_DIR: &'static str = "C:/Host";

    pub fn new(owner: Result<Option<Owner>, String>) -> Fake {
        Fake {
            owner: std::sync::Mutex::new(owner),
            dir: Some(PathBuf::from(Fake::HOST_DIR)),
            again: None,
            reads: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    /// A process at `image`, running since long before any test.
    pub fn program(image: &str) -> Result<Option<Owner>, String> {
        Ok(Some(Owner {
            image: PathBuf::from(image),
            born: SystemTime::UNIX_EPOCH,
            made: None,
        }))
    }

    /// Every peer is the wallpaper host.
    pub fn host() -> Fake {
        Fake::new(Fake::program("C:/Host/host64.exe"))
    }

    pub fn with_dir(mut self, dir: Option<&str>) -> Fake {
        self.dir = dir.map(PathBuf::from);
        self
    }

    pub fn with_again(mut self, dir: &str) -> Fake {
        self.again = Some(PathBuf::from(dir));
        self
    }

    /// The answer from now on.
    pub fn set(&self, owner: Result<Option<Owner>, String>) {
        *self.owner.lock().unwrap_or_else(|e| e.into_inner()) = owner;
    }

    pub fn reads(&self) -> usize {
        self.reads.load(std::sync::atomic::Ordering::SeqCst)
    }

    fn key(p: &Path) -> String {
        p.to_string_lossy().replace('\\', "/").to_lowercase()
    }
}

#[cfg(test)]
impl PeerLookup for Fake {
    fn owner(&self, _peer_port: u16, _feed_port: u16) -> Result<Option<Owner>, String> {
        self.reads.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.owner.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn host_dir(&self) -> Option<PathBuf> {
        self.dir.clone()
    }

    fn inside(&self, image: &Path, dir: &Path) -> bool {
        let dir = format!("{}/", Fake::key(dir).trim_end_matches('/'));
        self.local(Path::new(&dir)) && Fake::key(image).starts_with(&dir)
    }

    fn local(&self, dir: &Path) -> bool {
        !Fake::key(dir).starts_with("//")
    }

    fn host_dir_again(&self) -> Option<PathBuf> {
        self.again.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::time::Duration;

    /// A connected pair on loopback: (the accepted end, the client end, the listener's port).
    fn pair() -> (TcpStream, TcpStream, u16) {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let client = TcpStream::connect(("127.0.0.1", port)).unwrap();
        let (served, _) = listener.accept().unwrap();
        (served, client, port)
    }

    fn of(fake: &Fake) -> Peer {
        let (served, _client, port) = pair();
        peer_of(&served, port, SystemTime::now(), fake)
    }

    #[test]
    fn a_program_inside_the_host_folder_is_the_host() {
        assert_eq!(of(&Fake::host()), Peer::Host);
        // A helper in a subfolder too; case is no difference.
        let helper = Fake::new(Fake::program("c:/host/bin/Helper.exe"));
        assert_eq!(of(&helper), Peer::Host);
        assert_eq!(helper.reads(), 1, "one lookup");
    }

    #[test]
    fn a_sibling_folder_with_the_same_prefix_is_another_program() {
        let sibling = Fake::new(Fake::program("C:/Host2/host64.exe"));
        assert_eq!(
            of(&sibling),
            Peer::Other {
                name: Some("host64.exe".into())
            }
        );
        let beside = Fake::new(Fake::program("C:/Hostile.exe"));
        assert!(matches!(of(&beside), Peer::Other { .. }));
    }

    #[test]
    fn another_program_is_named_by_its_cleaned_cut_file_name_only() {
        let long = format!("C:/Tools/Sub/a\u{202e}b{}.exe", "x".repeat(80));
        let Peer::Other { name: Some(name) } = of(&Fake::new(Fake::program(&long))) else {
            panic!("expected another program with a name");
        };
        assert!(name.chars().count() <= 32, "{name}");
        assert!(
            !name.contains('\u{202e}') && name.starts_with("ab"),
            "{name}"
        );
        assert!(!name.contains("Tools") && !name.contains('/'), "{name}");
        // Nothing left of the name: no name, never an empty one.
        let blank = Fake::new(Fake::program("C:/Tools/\u{202e}"));
        assert_eq!(of(&blank), Peer::Other { name: None });
    }

    #[test]
    fn no_row_is_gone_after_one_more_read() {
        let gone = Fake::new(Ok(None));
        assert_eq!(of(&gone), Peer::Unknown(Unknown::Gone));
        assert_eq!(gone.reads(), 2, "read once more, and no further");
    }

    #[test]
    fn an_unreadable_owner_and_a_failed_read_are_unknown() {
        let unreadable = Fake::new(Err(UNREADABLE.into()));
        assert_eq!(of(&unreadable), Peer::Unknown(Unknown::Unreadable));
        assert_eq!(unreadable.reads(), 1);
        let failed = Fake::new(Err("connection table: error 5".into()));
        assert_eq!(of(&failed), Peer::Unknown(Unknown::Table));
    }

    #[test]
    fn a_process_born_after_the_accept_is_unknown() {
        let accepted = SystemTime::now();
        let inside = |_: &Path, _: &Path| true;
        let made_and_born = |made: Option<SystemTime>, at: SystemTime| {
            let owner = Owner {
                image: PathBuf::from("C:/Host/host64.exe"),
                born: at,
                made,
            };
            judge(
                Ok(Some(owner)),
                Some(Path::new(Fake::HOST_DIR)),
                true,
                accepted,
                &inside,
            )
        };
        let born = |at: SystemTime| made_and_born(None, at);
        assert_eq!(born(accepted), Peer::Host);
        assert_eq!(born(accepted - Duration::from_secs(60)), Peer::Host);
        assert_eq!(
            born(accepted + Duration::from_millis(1)),
            Peer::Unknown(Unknown::BornLater)
        );
        // The owner must have been running when the connection's socket was made, too.
        let before = accepted - Duration::from_secs(60);
        let made = Some(accepted - Duration::from_secs(30));
        assert_eq!(made_and_born(made, before), Peer::Host);
        assert_eq!(made_and_born(Some(before), before), Peer::Host);
        assert_eq!(
            made_and_born(made, accepted - Duration::from_secs(10)),
            Peer::Unknown(Unknown::BornLater)
        );
    }

    /// What must hold (#1445): a peer is looked up only at 127.0.0.1.
    #[test]
    fn a_peer_at_another_address_is_never_looked_up() {
        let host = Fake::host();
        let now = SystemTime::now();
        for ip in ["127.0.0.2", "127.1.2.3", "192.168.0.9", "::1"] {
            let addr = SocketAddr::new(ip.parse().unwrap(), 51_000);
            assert_eq!(
                peer_at(addr, 47_000, now, &host),
                Peer::Unknown(Unknown::Elsewhere),
                "{ip}"
            );
        }
        assert_eq!(host.reads(), 0);
        let addr = SocketAddr::new("127.0.0.1".parse().unwrap(), 51_000);
        assert_eq!(peer_at(addr, 47_000, now, &host), Peer::Host);
    }

    /// The system's own table, asked about a connection this test made to itself.
    #[cfg(target_os = "windows")]
    #[test]
    fn this_process_owns_its_own_loopback_connection() {
        let (served, _client, port) = pair();
        let peer = served.peer_addr().unwrap().port();
        let (pid, made) = owner_row(peer, port).unwrap().expect("the row");
        assert_eq!(pid, std::process::id());
        let (_, born) = foxglove_plugin::owner::image_and_birth(pid).unwrap();
        let made = made_at(made).expect("the system dates the connection");
        assert!(born <= made, "{born:?} {made:?}");
        assert!(made <= SystemTime::now() + Duration::from_secs(1));
        assert_eq!(owner_row(peer, port.wrapping_add(1)).unwrap(), None);
    }

    #[test]
    fn no_host_folder_or_one_on_a_network_drive_is_unknown() {
        let none = Fake::host().with_dir(None);
        assert_eq!(of(&none), Peer::Unknown(Unknown::NoHostFolder));
        let far = Fake::new(Fake::program("//server/share/Host/host64.exe"))
            .with_dir(Some("//server/share/Host"));
        assert_eq!(of(&far), Peer::Unknown(Unknown::HostNotLocal));
    }

    #[test]
    fn a_moved_host_is_found_by_one_fresh_look() {
        // The known host program is gone from where it was: one fresh look finds it.
        let moved = Fake::new(Fake::program("D:/Games/Host/host64.exe"))
            .with_dir(None)
            .with_again("D:/Games/Host");
        assert_eq!(of(&moved), Peer::Host);
        let found = Fake::host().with_dir(None).with_again(Fake::HOST_DIR);
        assert_eq!(of(&found), Peer::Host);
        let other = Fake::new(Fake::program("C:/Tools/tool.exe"))
            .with_dir(None)
            .with_again("D:/Games/Host");
        assert!(matches!(of(&other), Peer::Other { .. }));
    }

    /// What must hold (#1445): while the known host program is in place, no connection makes
    /// Foxglove look for another one.
    #[test]
    fn a_connection_never_moves_a_host_that_is_still_in_place() {
        let elsewhere =
            Fake::new(Fake::program("D:/Games/Host/host64.exe")).with_again("D:/Games/Host");
        assert!(matches!(of(&elsewhere), Peer::Other { .. }));
        let far = Fake::new(Fake::program("D:/Games/Host/host64.exe"))
            .with_dir(Some("//server/share/Host"))
            .with_again("D:/Games/Host");
        assert_eq!(of(&far), Peer::Unknown(Unknown::HostNotLocal));
    }

    #[test]
    fn the_table_row_is_matched_on_both_addresses_and_both_ports() {
        let port = |p: u16| {
            let b = p.to_be_bytes();
            u32::from_ne_bytes([b[0], b[1], 0, 0])
        };
        let word = |a: u32, b: u32| {
            let (a, b) = (a.to_ne_bytes(), b.to_ne_bytes());
            u64::from_ne_bytes([a[0], a[1], a[2], a[3], b[0], b[1], b[2], b[3]])
        };
        let lo = u32::from_ne_bytes([127, 0, 0, 1]);
        let lo2 = u32::from_ne_bytes([127, 0, 0, 2]);
        let lan = u32::from_ne_bytes([192, 168, 0, 9]);
        let row = |from: u32, from_port: u16, to: u32, to_port: u16, pid: u32, made: u64| {
            let mut r = [0u64; ROW];
            r[0] = word(5, from);
            r[1] = word(port(from_port), to);
            r[2] = word(port(to_port), pid);
            r[3] = made;
            r
        };
        let table = |rows: &[[u64; ROW]]| {
            let mut t = vec![word(rows.len() as u32, 0)];
            t.extend(rows.iter().flatten());
            t
        };
        let all = table(&[
            row(lo, 47_000, lo, 51_000, 11, 1),  // the feed's own end
            row(lan, 51_000, lo, 47_000, 12, 2), // not from loopback
            row(lo, 51_000, lo, 47_001, 13, 3),  // another server
            row(lo2, 51_000, lo, 47_000, 15, 4), // another loopback address, same port
            row(lo, 51_000, lan, 47_000, 16, 5), // not to loopback
            row(lo, 51_000, lo, 47_000, 14, 6),
        ]);
        assert_eq!(owner_in(&all, 51_000, 47_000), Some((14, 6)));
        assert_eq!(owner_in(&all, 51_001, 47_000), None);
        // Another address with a wallpaper's port number never answers for it.
        let borrowed = table(&[row(lo2, 51_000, lo, 47_000, 15, 4)]);
        assert_eq!(owner_in(&borrowed, 51_000, 47_000), None);
        // A count beyond the rows read is not followed; part of a row is not a row.
        let mut short = table(&[row(lo, 51_000, lo, 47_000, 14, 6)]);
        short[0] = word(9, 0);
        assert_eq!(owner_in(&short, 51_000, 47_000), Some((14, 6)));
        short.pop();
        assert_eq!(owner_in(&short, 51_000, 47_000), None);
        assert_eq!(owner_in(&[], 51_000, 47_000), None);
        // The time: none for 0, and 1970 is 1970.
        assert_eq!(made_at(0), None);
        assert_eq!(
            made_at(116_444_736_000_000_000),
            Some(SystemTime::UNIX_EPOCH)
        );
    }

    #[test]
    fn the_reason_words_are_fixed_and_differ() {
        let all = [
            Unknown::Gone,
            Unknown::Unreadable,
            Unknown::BornLater,
            Unknown::NoHostFolder,
            Unknown::HostNotLocal,
            Unknown::Table,
            Unknown::Elsewhere,
        ];
        let words: std::collections::BTreeSet<_> = all.iter().map(|u| u.word()).collect();
        assert_eq!(words.len(), all.len());
    }
}
