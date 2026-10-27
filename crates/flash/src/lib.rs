//! Updating the keyboard's parts from the app (#382): both halves of a split, or a one-piece
//! board (#559), with the safe flasher's rules (`scripts/flash/foxglove_flash.py`):
//!
//! - every image passes the UF2 checks for its half first ([`uf2::preflight`]);
//! - the left must be on USB (a Bluetooth-only left can't show its bootloader drive), and a right
//!   half with a low battery is refused, since it may sit in its bootloader for a while;
//! - a dev/GA mix, and a downgrade the user didn't allow, are refused before anything starts;
//! - the left first, then the right, and only once the left is back and runs the new build;
//! - each half's bootloader drive is identified by its USB serial: the left's is the running
//!   left's own serial, the right's is any other one (and the one seen before, once known). Two
//!   drives at once, or a drive that is the wrong half's, never get a write;
//! - identity is checked again right before writing; a write the bootloader ignores is tried
//!   once more; the build is read back over the link afterwards;
//! - every write goes to the flash history ([`Board::record`]).
//!
//! The steps run against a [`Board`], so the whole flow is tested with a mock; the app provides
//! the real one. All waits are the board's, which waits on device events with a timeout.
//!
//! [`Half`] is the firmware's number for a part (D7 08, D7 09, the image marker): `Left` is the
//! one the PC talks to, `Right` the other half of a split. What the steps call each part comes
//! from [`Parts`], the app's names from the board profile.

pub mod uf2;

use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Half {
    Left,
    Right,
}

impl Half {
    pub fn name(self) -> &'static str {
        match self {
            Half::Left => "left",
            Half::Right => "right",
        }
    }
    /// The firmware's number for it (D7 08, D7 09).
    pub fn index(self) -> u8 {
        match self {
            Half::Left => 0,
            Half::Right => 1,
        }
    }
}

/// What the update calls the keyboard's parts (#559), first the one the PC talks to: "Left
/// half" and "Right half" on a split (the default), "Keyboard" on a one-piece board.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Parts {
    names: Vec<String>,
}

impl Default for Parts {
    fn default() -> Self {
        Parts::split()
    }
}

impl Parts {
    /// The parts by name, first the one the PC talks to. The firmware updates two at most.
    pub fn new(names: Vec<String>) -> Parts {
        let mut names: Vec<String> = names.into_iter().filter(|n| !n.is_empty()).collect();
        names.truncate(2);
        if names.is_empty() {
            names.push("Keyboard".into());
        }
        Parts { names }
    }

    /// A split with a left and a right half.
    pub fn split() -> Parts {
        Parts::new(vec!["Left half".into(), "Right half".into()])
    }

    pub fn is_split(&self) -> bool {
        self.names.len() > 1
    }

    /// Whether the keyboard has this part.
    pub fn has(&self, half: Half) -> bool {
        usize::from(half.index()) < self.names.len()
    }

    /// "Left half", "Keyboard".
    pub fn name(&self, half: Half) -> String {
        self.names
            .get(usize::from(half.index()))
            .cloned()
            .unwrap_or_else(|| Parts::split().names[usize::from(half.index())].clone())
    }

    /// "Left", "Keyboard": a line's label.
    pub fn short(&self, half: Half) -> String {
        let name = self.name(half);
        match name.strip_suffix(" half") {
            Some(s) if !s.is_empty() => s.to_string(),
            _ => name,
        }
    }

    /// "the left half", "the keyboard".
    pub fn the(&self, half: Half) -> String {
        let name = self.name(half);
        let mut c = name.chars();
        let lower = match (c.next(), name.chars().nth(1)) {
            (Some(f), Some(n)) if n.is_lowercase() || n == ' ' => {
                f.to_lowercase().chain(c).collect()
            }
            _ => name.clone(),
        };
        format!("the {lower}")
    }

    /// "The left half", "The keyboard": a sentence's start.
    pub fn the_cap(&self, half: Half) -> String {
        let the = self.the(half);
        format!("T{}", &the[1..])
    }

    /// Every part: "both halves", "both parts", "the keyboard".
    pub fn all(&self) -> &'static str {
        match self.names.len() {
            1 => "the keyboard",
            _ if self.names.iter().all(|n| n.ends_with(" half")) => "both halves",
            _ => "both parts",
        }
    }
}

/// A half's running build, as BUILD_INFO (D7 09) reports it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Build {
    pub version: String,
    /// "dev" or "ga".
    pub profile: String,
    pub sha: String,
}

impl std::fmt::Display for Build {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let or = |s: &str| {
            if s.is_empty() {
                "?".to_string()
            } else {
                s.to_string()
            }
        };
        write!(
            f,
            "{} {} {}",
            or(&self.version),
            or(&self.profile),
            or(&self.sha)
        )
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum WriteError {
    /// The drive went away mid-write: the bootloader may have taken all of it and reset (#176).
    /// Only the build read back afterwards tells.
    DriveGone(String),
    Failed(String),
}

/// The keyboard and the PC, as the update needs them.
pub trait Board {
    /// Whether the left is connected to this PC over USB (CONN_STATE); `None` if it can't say.
    fn left_on_usb(&self) -> Option<bool>;
    fn right_battery(&self) -> Option<u8>;
    /// Whether the right half is plugged into this PC by USB too (its drive can only appear
    /// then); `None` if it can't say.
    fn right_on_usb(&self) -> Option<bool>;
    /// The running left half's USB serial; its bootloader shows the same one.
    fn left_serial(&self) -> Option<String>;
    fn build(&self, half: Half) -> Option<Build>;
    /// The firmware can restart a half into its bootloader over the link (HELLO bit 9).
    fn hands_free(&self) -> bool;
    fn enter_bootloader(&self, half: Half) -> Result<(), String>;
    /// The UF2 bootloaders present now, by USB serial.
    fn bootloaders(&self) -> Vec<String>;
    /// Waits until bootloaders come or go (a device event), or `timeout`.
    fn wait_bootloaders(&self, timeout: Duration);
    /// The clock the drive wait is measured on (tests move it).
    fn now(&self) -> std::time::Instant {
        std::time::Instant::now()
    }
    /// The bootloader drive with this serial (it must have INFO_UF2.TXT).
    fn volume(&self, serial: &str) -> Result<PathBuf, String>;
    /// The `Board-ID:` line of the drive's INFO_UF2.TXT.
    fn board_id(&self, volume: &Path) -> Result<String, String>;
    /// A moment after the drive appears, before writing (a write ~1 s after it is sometimes ignored).
    fn settle(&self);
    fn write(&self, volume: &Path, data: &[u8]) -> Result<(), WriteError>;
    /// Waits for the drive to go (the bootloader took the image and restarted), up to `timeout`.
    fn wait_drive_gone(&self, volume: &Path, timeout: Duration) -> bool;
    /// Waits for the left to be back as a keyboard with this serial and the host link answering.
    fn wait_left_back(&self, serial: &str, timeout: Duration) -> bool;
    /// Waits for the left's relay to the right to be up again.
    fn wait_right_back(&self, timeout: Duration) -> bool;
    /// One line in the flash history.
    fn record(&self, half: Half, sha: &str, outcome: &str, detail: &str);
}

/// The serials each half's bootloader had before; kept by the app between updates.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Known {
    pub left: Option<String>,
    pub right: Option<String>,
}

/// One image to write.
#[derive(Clone, Debug)]
pub struct Image {
    pub half: Half,
    pub data: Vec<u8>,
    pub check: uf2::Preflight,
}

/// What an update would do, for the confirm step.
#[derive(Clone, Debug, Default)]
pub struct Plan {
    /// Left first; halves already on the image's build are left out.
    pub images: Vec<Image>,
    /// What each half runs now (left, right).
    pub current: [Option<Build>; 2],
    pub skipped: Vec<Half>,
    /// A half would get an older version: the user has to allow it.
    pub downgrade: bool,
    pub hands_free: bool,
    /// What the steps call each part.
    pub parts: Parts,
}

/// Below this, the right half isn't updated: it may wait in its bootloader for a while.
pub const MIN_RIGHT_BATTERY: u8 = 15;
/// How long a half gets to show its bootloader drive (taps take a person).
pub const DRIVE_WAIT: Duration = Duration::from_secs(180);
/// The left's relay to the right comes back 20-40 s after the left restarts (#122).
pub const RELAY_WAIT: Duration = Duration::from_secs(60);
pub const LEFT_BACK_WAIT: Duration = Duration::from_secs(60);
pub const RIGHT_BACK_WAIT: Duration = Duration::from_secs(90);

fn version(v: &str) -> Option<(u32, u32, u32)> {
    let mut parts = v.split(['.', '-', '+']).map(|p| p.parse::<u32>().ok());
    Some((parts.next()??, parts.next()??, parts.next()??))
}

impl Plan {
    /// One line per part: what it runs now and what it gets.
    pub fn lines(&self) -> Vec<String> {
        let mut out = Vec::new();
        for half in [Half::Left, Half::Right]
            .into_iter()
            .filter(|h| self.parts.has(*h))
        {
            let now = self.current[half.index() as usize]
                .as_ref()
                .map_or("unknown".to_string(), |b| b.to_string());
            if let Some(i) = self.images.iter().find(|i| i.half == half) {
                let new = Build {
                    version: i.check.version.clone(),
                    profile: i.check.profile.clone(),
                    sha: i.check.sha.clone(),
                };
                out.push(format!("{}: {now} -> {new}", self.parts.short(half)));
            } else if self.skipped.contains(&half) {
                out.push(format!(
                    "{}: {now}, already up to date",
                    self.parts.short(half)
                ));
            }
        }
        out
    }
}

/// Checks the images and a split keyboard and says what an update would do; nothing is touched.
pub fn plan(
    left: Option<Vec<u8>>,
    right: Option<Vec<u8>>,
    board: &dyn Board,
) -> Result<Plan, String> {
    plan_for(left, right, board, &Parts::split())
}

/// [`plan`] for a keyboard of these parts (#559): a one-piece board takes one image, which
/// needs no half marker, and has no second cable or battery to check.
pub fn plan_for(
    left: Option<Vec<u8>>,
    right: Option<Vec<u8>>,
    board: &dyn Board,
    parts: &Parts,
) -> Result<Plan, String> {
    if right.is_some() && !parts.has(Half::Right) {
        return Err(format!(
            "{} is one piece: it takes one firmware file, not a left and a right.",
            parts.the_cap(Half::Left)
        ));
    }
    let mut images = Vec::new();
    for (half, data) in [(Half::Left, left), (Half::Right, right)] {
        let Some(data) = data else { continue };
        let check = if parts.is_split() {
            uf2::preflight(&data, half)
        } else {
            uf2::preflight_one(&data)
        };
        if !check.ok() {
            let which = if parts.is_split() {
                half.name()
            } else {
                "firmware"
            };
            return Err(format!(
                "The {which} image can't be used: {}.",
                check.reasons.join("; ")
            ));
        }
        images.push(Image { half, data, check });
    }
    if images.is_empty() {
        return Err("No firmware to write.".into());
    }
    let first = parts.the(Half::Left);
    match board.left_on_usb() {
        Some(true) => {}
        Some(false) => {
            return Err(format!("{} is connected over Bluetooth only. Plug its USB cable into this PC: an update needs the cable.", parts.the_cap(Half::Left)))
        }
        None => return Err(format!("Can't tell how {first} is connected, so nothing is written. Connect it by USB and try again.")),
    }
    if images.iter().any(|i| i.half == Half::Right) {
        // Restarted into its bootloader without a cable, it would wait there, not typing.
        if board.right_on_usb() == Some(false) {
            return Err(format!(
                "Plug {} into this PC by USB too: its new firmware goes over its own cable.",
                parts.the(Half::Right)
            ));
        }
        if let Some(b) = board.right_battery().filter(|b| *b < MIN_RIGHT_BATTERY) {
            return Err(format!(
                "{}'s battery is at {b}%. Charge it (or plug it in) before updating.",
                parts.the_cap(Half::Right)
            ));
        }
    }
    let current = [
        board.build(Half::Left),
        if parts.has(Half::Right) {
            board.build(Half::Right)
        } else {
            None
        },
    ];
    // The profile each half would run afterwards: dev and GA together are refused.
    let after: Vec<String> = [Half::Left, Half::Right]
        .iter()
        .map(|h| {
            images
                .iter()
                .find(|i| i.half == *h)
                .map(|i| i.check.profile.clone())
                .filter(|p| !p.is_empty())
                .or_else(|| {
                    current[h.index() as usize]
                        .as_ref()
                        .map(|b| b.profile.clone())
                })
                .unwrap_or_default()
        })
        .collect();
    if !after[0].is_empty() && !after[1].is_empty() && after[0] != after[1] {
        return Err(format!(
            "This would leave the {} {} and the {} {}. Update {} with the same kind of build.",
            parts.short(Half::Left).to_lowercase(),
            after[0],
            parts.short(Half::Right).to_lowercase(),
            after[1],
            parts.all()
        ));
    }
    let mut skipped = Vec::new();
    let mut downgrade = false;
    images.retain(|i| {
        let now = current[i.half.index() as usize].as_ref();
        if now.is_some_and(|b| !i.check.sha.is_empty() && b.sha == i.check.sha) {
            skipped.push(i.half);
            return false;
        }
        if let (Some(new), Some(old)) = (
            version(&i.check.version),
            now.and_then(|b| version(&b.version)),
        ) {
            downgrade |= new < old;
        }
        true
    });
    Ok(Plan {
        images,
        current,
        skipped,
        downgrade,
        hands_free: board.hands_free(),
        parts: parts.clone(),
    })
}

/// Where an update is, for the progress list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Progress {
    pub half: Half,
    pub text: String,
}

/// Why an update stopped, and what to do about it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Failure {
    pub half: Half,
    pub message: String,
    pub recovery: String,
}

fn recovery(parts: &Parts, half: Half) -> String {
    format!(
        "If {} doesn't type: double-tap its reset button, copy its firmware file onto the drive that appears, \
         and wait for the drive to go. Earlier firmware works the same way (RESCUE.md).",
        parts.the(half)
    )
}

/// Whether `serial` may be `half`'s bootloader.
fn identify(
    parts: &Parts,
    half: Half,
    serial: &str,
    left_serial: Option<&str>,
    known: &Known,
) -> Result<(), String> {
    let is_left = left_serial == Some(serial) || known.left.as_deref() == Some(serial);
    let is_right = known.right.as_deref() == Some(serial);
    let (left, right) = (parts.the(Half::Left), parts.the(Half::Right));
    match half {
        Half::Left if is_right => Err(format!("the bootloader that appeared is {right}'s")),
        Half::Left if !is_left => Err(format!("the bootloader that appeared isn't {left}'s")),
        Half::Right if is_left => Err(format!("the bootloader that appeared is {left}'s")),
        Half::Right if known.right.is_some() && !is_right => Err(format!(
            "the bootloader that appeared isn't {right} seen before"
        )),
        _ => Ok(()),
    }
}

/// Runs the update: each half in turn, the right only after the left is back on its new build.
/// `allow_downgrade` is the user's answer when [`Plan::downgrade`] is set.
pub fn run(
    plan: &Plan,
    board: &dyn Board,
    known: &mut Known,
    allow_downgrade: bool,
    progress: &mut dyn FnMut(Progress),
) -> Result<(), Failure> {
    if plan.downgrade && !allow_downgrade {
        let half = plan.images.first().map_or(Half::Left, |i| i.half);
        return Err(Failure {
            half,
            message: "This is an older version; allow the downgrade to go on.".into(),
            recovery: String::new(),
        });
    }
    // Learned once, while the left still runs: its bootloader has the same serial.
    let left_serial = board.left_serial().or_else(|| known.left.clone());
    for image in &plan.images {
        one(
            image,
            &plan.parts,
            board,
            known,
            left_serial.as_deref(),
            progress,
        )?;
    }
    Ok(())
}

fn one(
    image: &Image,
    parts: &Parts,
    board: &dyn Board,
    known: &mut Known,
    left_serial: Option<&str>,
    progress: &mut dyn FnMut(Progress),
) -> Result<(), Failure> {
    let half = image.half;
    let sha = image.check.sha.as_str();
    // "the left half", "the keyboard"; "The left half" to start a sentence.
    let (the, the_cap) = (parts.the(half), parts.the_cap(half));
    let mut say = |text: String| progress(Progress { half, text });
    let fail = |message: String| Failure {
        half,
        message,
        recovery: recovery(parts, half),
    };
    // Nothing is written while it's unclear which drive is which.
    if !board.bootloaders().is_empty() {
        return Err(Failure {
            half,
            message: "A keyboard bootloader drive is already connected. Unplug it (or let it finish) first.".into(),
            recovery: String::new(),
        });
    }
    if half == Half::Left && left_serial.is_none() {
        return Err(Failure {
            half,
            message: format!("Can't identify {the}'s USB connection, so nothing is written."),
            recovery: String::new(),
        });
    }
    if half == Half::Right {
        say(format!("Waiting for the link to {the}"));
        if !board.wait_right_back(RELAY_WAIT) {
            say(format!("The link to {the} isn't up; trying anyway"));
        }
    }
    let hands_free = board.hands_free() && board.enter_bootloader(half).is_ok();
    say(if hands_free {
        format!("Restarting {the} into its bootloader")
    } else if parts.is_split() {
        format!(
            "Double-tap the reset button on {the} (only that {}), with its cable plugged in",
            if parts.all() == "both halves" {
                "half"
            } else {
                "part"
            }
        )
    } else {
        format!("Double-tap the reset button on {the}, with its cable plugged in")
    });
    // Its bootloader drive: exactly one, and the right one.
    let deadline = board.now() + DRIVE_WAIT;
    let serial = loop {
        let present = board.bootloaders();
        match present.as_slice() {
            [one] => {
                identify(parts, half, one, left_serial, known).map_err(|e| fail(format!("Stopped before writing: {e}.")))?;
                break one.clone();
            }
            [] => {}
            _ => say("Two bootloaders are connected. Unplug the one you're not updating; nothing is written meanwhile.".into()),
        }
        let left = deadline.saturating_duration_since(board.now());
        if left.is_zero() {
            return Err(fail(format!("{the_cap}'s bootloader drive didn't appear.")));
        }
        board.wait_bootloaders(left);
    };
    // The drive gets a moment to mount before it's looked up and written.
    board.settle();
    let volume = board
        .volume(&serial)
        .map_err(|e| fail(format!("Stopped before writing: {e}.")))?;
    let id = board
        .board_id(&volume)
        .map_err(|e| fail(format!("Stopped before writing: {e}.")))?;
    say(format!("Found {the}'s bootloader ({id})"));
    let mut unconfirmed = false;
    for attempt in 1..=2 {
        // Identity once more, right before touching the drive.
        if board.bootloaders() != [serial.clone()]
            || board.volume(&serial).ok().as_deref() != Some(volume.as_path())
        {
            return Err(fail(
                "Stopped before writing: the bootloader drive changed.".into(),
            ));
        }
        say(format!("Writing {the}'s firmware"));
        match board.write(&volume, &image.data) {
            Ok(()) => {}
            Err(WriteError::DriveGone(e)) => {
                board.record(half, sha, "written-drive-gone", &e);
                unconfirmed = true;
                break;
            }
            Err(WriteError::Failed(e)) => {
                board.record(half, sha, "write-failed", &e);
                return Err(fail(format!("Writing failed: {e}.")));
            }
        }
        if board.wait_drive_gone(
            &volume,
            Duration::from_secs(if attempt == 1 { 10 } else { 15 }),
        ) {
            break;
        }
        if attempt == 2 {
            board.record(half, sha, "not-taken", "the drive stayed after two writes");
            return Err(fail("The bootloader didn't take the firmware.".into()));
        }
        say("The bootloader didn't take it; writing once more".into());
    }
    if unconfirmed && sha.is_empty() {
        return Err(fail("The drive went away mid-write and the image has no build ID to check, so it's unknown whether it was written.".into()));
    }
    if !unconfirmed {
        board.record(half, sha, "written", "");
    }
    say(format!("Waiting for {the} to restart"));
    let back = match half {
        Half::Left => board.wait_left_back(&serial, LEFT_BACK_WAIT),
        Half::Right => board.wait_right_back(RIGHT_BACK_WAIT),
    };
    if !back && half == Half::Left {
        board.record(half, sha, "not-back", "");
        return Err(fail(if parts.is_split() {
            format!(
                "{the_cap} didn't come back as a keyboard; the {} wasn't touched.",
                parts.short(Half::Right).to_lowercase()
            )
        } else {
            format!("{the_cap} didn't come back on USB after the write.")
        }));
    }
    match board.build(half) {
        Some(b) if !sha.is_empty() && b.sha != sha => {
            board.record(half, sha, "verify-failed", &b.to_string());
            return Err(fail(format!("{the_cap} runs {b}, not the new build.")));
        }
        Some(b) if !sha.is_empty() => {
            board.record(half, sha, "verified", "");
            say(format!("{the_cap} runs {b}"));
        }
        _ if unconfirmed => {
            board.record(
                half,
                sha,
                "verify-failed",
                "no read-back after the drive went away",
            );
            return Err(fail(format!(
                "Couldn't read {the}'s build back, so it's unknown whether it was written."
            )));
        }
        _ => {
            board.record(half, sha, "unverified", "");
            say(format!(
                "{the_cap} restarted; its build couldn't be read back"
            ));
        }
    }
    match half {
        Half::Left => known.left = Some(serial),
        Half::Right => known.right = Some(serial),
    }
    Ok(())
}

#[cfg(test)]
mod tests;
