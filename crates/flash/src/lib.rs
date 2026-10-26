//! Updating both halves of the keyboard from the app (#382), with the safe flasher's rules
//! (`scripts/flash/foxglove_flash.py`):
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
    /// One line per half: what it runs now and what it gets.
    pub fn lines(&self) -> Vec<String> {
        let mut out = Vec::new();
        for half in [Half::Left, Half::Right] {
            let now = self.current[half.index() as usize]
                .as_ref()
                .map_or("unknown".to_string(), |b| b.to_string());
            if let Some(i) = self.images.iter().find(|i| i.half == half) {
                let new = Build {
                    version: i.check.version.clone(),
                    profile: i.check.profile.clone(),
                    sha: i.check.sha.clone(),
                };
                out.push(format!("{}: {now} -> {new}", cap(half)));
            } else if self.skipped.contains(&half) {
                out.push(format!("{}: {now}, already up to date", cap(half)));
            }
        }
        out
    }
}

fn cap(half: Half) -> &'static str {
    match half {
        Half::Left => "Left",
        Half::Right => "Right",
    }
}

/// Checks the images and the keyboard and says what an update would do; nothing is touched.
pub fn plan(
    left: Option<Vec<u8>>,
    right: Option<Vec<u8>>,
    board: &dyn Board,
) -> Result<Plan, String> {
    let mut images = Vec::new();
    for (half, data) in [(Half::Left, left), (Half::Right, right)] {
        let Some(data) = data else { continue };
        let check = uf2::preflight(&data, half);
        if !check.ok() {
            return Err(format!(
                "The {} image can't be used: {}.",
                half.name(),
                check.reasons.join("; ")
            ));
        }
        images.push(Image { half, data, check });
    }
    if images.is_empty() {
        return Err("No firmware to write.".into());
    }
    match board.left_on_usb() {
        Some(true) => {}
        Some(false) => {
            return Err("The left half is connected over Bluetooth only. Plug its USB cable into this PC: an update needs the cable.".into())
        }
        None => return Err("Can't tell how the left half is connected, so nothing is written. Connect it by USB and try again.".into()),
    }
    if images.iter().any(|i| i.half == Half::Right) {
        // Restarted into its bootloader without a cable, it would wait there, not typing.
        if board.right_on_usb() == Some(false) {
            return Err("Plug the right half into this PC by USB too: its new firmware goes over its own cable.".into());
        }
        if let Some(b) = board.right_battery().filter(|b| *b < MIN_RIGHT_BATTERY) {
            return Err(format!(
                "The right half's battery is at {b}%. Charge it (or plug it in) before updating."
            ));
        }
    }
    let current = [board.build(Half::Left), board.build(Half::Right)];
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
            "This would leave the left {} and the right {}. Update both halves with the same kind of build.",
            after[0], after[1]
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

fn recovery(half: Half) -> String {
    format!(
        "If the {0} half doesn't type: double-tap its reset button, copy its firmware file onto the drive that appears, \
         and wait for the drive to go. Earlier firmware works the same way (RESCUE.md).",
        half.name()
    )
}

/// Whether `serial` may be `half`'s bootloader.
fn identify(
    half: Half,
    serial: &str,
    left_serial: Option<&str>,
    known: &Known,
) -> Result<(), String> {
    let is_left = left_serial == Some(serial) || known.left.as_deref() == Some(serial);
    let is_right = known.right.as_deref() == Some(serial);
    match half {
        Half::Left if is_right => Err("the bootloader that appeared is the right half's".into()),
        Half::Left if !is_left => Err("the bootloader that appeared isn't the left half's".into()),
        Half::Right if is_left => Err("the bootloader that appeared is the left half's".into()),
        Half::Right if known.right.is_some() && !is_right => {
            Err("the bootloader that appeared isn't the right half seen before".into())
        }
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
        one(image, board, known, left_serial.as_deref(), progress)?;
    }
    Ok(())
}

fn one(
    image: &Image,
    board: &dyn Board,
    known: &mut Known,
    left_serial: Option<&str>,
    progress: &mut dyn FnMut(Progress),
) -> Result<(), Failure> {
    let half = image.half;
    let sha = image.check.sha.as_str();
    let mut say = |text: String| progress(Progress { half, text });
    let fail = |message: String| Failure {
        half,
        message,
        recovery: recovery(half),
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
            message: "Can't identify the left half's USB connection, so nothing is written.".into(),
            recovery: String::new(),
        });
    }
    if half == Half::Right {
        say("Waiting for the link to the right half".into());
        if !board.wait_right_back(RELAY_WAIT) {
            say("The link to the right half isn't up; trying anyway".into());
        }
    }
    let hands_free = board.hands_free() && board.enter_bootloader(half).is_ok();
    say(if hands_free {
        format!("Restarting the {} half into its bootloader", half.name())
    } else {
        format!("Double-tap the reset button on the {} half (only that half), with its cable plugged in", half.name())
    });
    // Its bootloader drive: exactly one, and the right one.
    let deadline = board.now() + DRIVE_WAIT;
    let serial = loop {
        let present = board.bootloaders();
        match present.as_slice() {
            [one] => {
                identify(half, one, left_serial, known).map_err(|e| fail(format!("Stopped before writing: {e}.")))?;
                break one.clone();
            }
            [] => {}
            _ => say("Two bootloaders are connected. Unplug the one you're not updating; nothing is written meanwhile.".into()),
        }
        let left = deadline.saturating_duration_since(board.now());
        if left.is_zero() {
            return Err(fail(format!(
                "The {} half's bootloader drive didn't appear.",
                half.name()
            )));
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
    say(format!(
        "Found the {} half's bootloader ({id})",
        half.name()
    ));
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
        say(format!("Writing the {} half's firmware", half.name()));
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
    say(format!("Waiting for the {} half to restart", half.name()));
    let back = match half {
        Half::Left => board.wait_left_back(&serial, LEFT_BACK_WAIT),
        Half::Right => board.wait_right_back(RIGHT_BACK_WAIT),
    };
    if !back && half == Half::Left {
        board.record(half, sha, "not-back", "");
        return Err(fail(
            "The left half didn't come back as a keyboard; the right wasn't touched.".into(),
        ));
    }
    match board.build(half) {
        Some(b) if !sha.is_empty() && b.sha != sha => {
            board.record(half, sha, "verify-failed", &b.to_string());
            return Err(fail(format!(
                "The {} half runs {b}, not the new build.",
                half.name()
            )));
        }
        Some(b) if !sha.is_empty() => {
            board.record(half, sha, "verified", "");
            say(format!("The {} half runs {b}", half.name()));
        }
        _ if unconfirmed => {
            board.record(
                half,
                sha,
                "verify-failed",
                "no read-back after the drive went away",
            );
            return Err(fail(format!(
                "Couldn't read the {} half's build back, so it's unknown whether it was written.",
                half.name()
            )));
        }
        _ => {
            board.record(half, sha, "unverified", "");
            say(format!(
                "The {} half restarted; its build couldn't be read back",
                half.name()
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
