//! "Update keyboard" (#382): both halves from the app, with the safe flasher's rules
//! (`foxglove_flash`, the same rules as `scripts/flash/foxglove_flash.py`).
//!
//! The Firmware card's button picks a folder with the new firmware (a release's
//! `foxglove-<version>-left.uf2` / `-right.uf2`, or CI's `foxglove-fw-…-left-dev.uf2`), checks it
//! against the keyboard, and shows what will happen before anything is touched. After the
//! confirm step the update runs on its own thread, left first, with each step listed.
//!
//! The Windows side ([`windows_board`]) waits on device events (bootloader drives and the
//! keyboard coming and going), each with a timeout. The one wait without an event, the left's
//! relay to the right coming back, asks HELLO every 2 s; it starts when the left is back and
//! stops when the relay is up or after a minute.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use foxglove_flash::{Half, Known, Plan};
use serde::{Deserialize, Serialize};

/// What the Firmware card shows of an update.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum View {
    #[default]
    Idle,
    /// Reading the folder and asking the keyboard.
    Checking,
    /// What will happen; nothing has been touched yet.
    Confirm {
        lines: Vec<String>,
        notes: Vec<String>,
        downgrade: bool,
        allow_downgrade: bool,
    },
    /// The folder or the keyboard isn't fit for an update; nothing was touched.
    Refused(String),
    Running(Vec<String>),
    Done(Vec<String>),
    Failed {
        steps: Vec<String>,
        message: String,
        recovery: String,
    },
}

/// A click in the update's part of the card.
#[derive(Clone, Debug, PartialEq)]
pub enum Intent {
    /// "Update keyboard…": pick the firmware folder.
    Start,
    AllowDowngrade(bool),
    Confirm,
    /// Cancel before it starts, or close the result.
    Close,
}

struct Inner {
    view: View,
    plan: Option<Plan>,
    wake: Option<crate::ui_wake::UiWake>,
}

static STATE: Mutex<Inner> = Mutex::new(Inner {
    view: View::Idle,
    plan: None,
    wake: None,
});

fn with<R>(f: impl FnOnce(&mut Inner) -> R) -> R {
    let mut s = STATE.lock().unwrap_or_else(|e| e.into_inner());
    let r = f(&mut s);
    if let Some(w) = &s.wake {
        w.request_repaint();
    }
    r
}

pub fn view() -> View {
    STATE.lock().unwrap_or_else(|e| e.into_inner()).view.clone()
}

/// The serials each half's bootloader had before (`[flash]` in the store).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct FlashSettings {
    left_serial: Option<String>,
    right_serial: Option<String>,
}

fn known() -> Known {
    let s = super::store::try_load::<FlashSettings>("flash").unwrap_or_default();
    Known {
        left: s.left_serial,
        right: s.right_serial,
    }
}

fn save_known(k: &Known) {
    let s = FlashSettings {
        left_serial: k.left.clone(),
        right_serial: k.right.clone(),
    };
    if let Err(e) = super::store::save("flash", &s) {
        eprintln!("Foxglove: can't save the halves' bootloader serials: {e}");
    }
}

/// The firmware files in `dir` for each half: a release's `foxglove-<version>-<half>.uf2`, else
/// CI's `foxglove-fw-…-<half>-<profile>.uf2` for `profile`, else the older fixed names. More than
/// one candidate for a half is refused rather than guessed.
pub fn pick_files(dir: &Path, profile: &str) -> Result<(Option<PathBuf>, Option<PathBuf>), String> {
    let names: Vec<String> = std::fs::read_dir(dir)
        .map_err(|e| format!("Can't read {}: {e}", dir.display()))?
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter(|n| n.to_ascii_lowercase().ends_with(".uf2"))
        .collect();
    let one = |half: &str| -> Result<Option<PathBuf>, String> {
        let release: Vec<&String> = names
            .iter()
            .filter(|n| {
                n.starts_with("foxglove-")
                    && !n.starts_with("foxglove-fw-")
                    && n.ends_with(&format!("-{half}.uf2"))
            })
            .collect();
        let ci: Vec<&String> = names
            .iter()
            .filter(|n| {
                n.starts_with("foxglove-fw-") && n.ends_with(&format!("-{half}-{profile}.uf2"))
            })
            .collect();
        let legacy = match half {
            "left" => "eyelash_corne_studio_left.uf2",
            _ => "nice_view_custom-eyelash_corne_right-zmk.uf2",
        };
        for set in [release, ci] {
            match set.as_slice() {
                [] => continue,
                [one] => return Ok(Some(dir.join(one))),
                _ => {
                    return Err(format!(
                        "More than one {half} firmware file in {}; keep one.",
                        dir.display()
                    ))
                }
            }
        }
        Ok(names.iter().any(|n| n == legacy).then(|| dir.join(legacy)))
    };
    let (left, right) = (one("left")?, one("right")?);
    if left.is_none() && right.is_none() {
        return Err(format!(
            "No keyboard firmware (.uf2 for the left or right half) in {}.",
            dir.display()
        ));
    }
    Ok((left, right))
}

/// Notes for the confirm step, from the plan.
pub fn notes(plan: &Plan) -> Vec<String> {
    let mut out = vec![
        "Each half restarts into its bootloader, gets the new firmware and restarts. The left goes first; the right only after the left is back on the new build.".to_string(),
        "Keep both USB cables plugged in. Typing stops on each half for about a minute.".to_string(),
    ];
    if plan.hands_free {
        out.push("Your firmware can restart each half by itself: no buttons to press.".into());
    } else {
        out.push("When asked, double-tap the reset button on that half (GA builds can't restart themselves).".into());
    }
    if plan.images.iter().any(|i| i.half == Half::Right) {
        out.push(
            "Plug the right half into this PC too: its new firmware goes over its own cable."
                .into(),
        );
    }
    if plan.downgrade {
        out.push("This is an older version than the keyboard runs.".into());
    }
    out
}

/// Acts on a click; `ctx` wakes the page as the update moves on.
pub fn act(intent: Intent, ctx: &egui::Context) {
    with(|s| s.wake = Some(crate::ui_wake::UiWake::from_ctx(ctx)));
    match intent {
        Intent::Start => DIALOG.with_borrow_mut(|d| d.pick_directory()),
        Intent::AllowDowngrade(on) => with(|s| {
            if let View::Confirm {
                allow_downgrade, ..
            } = &mut s.view
            {
                *allow_downgrade = on;
            }
        }),
        Intent::Confirm => start_update(),
        Intent::Close => with(|s| {
            if !matches!(s.view, View::Running(_) | View::Checking) {
                s.view = View::Idle;
                s.plan = None;
            }
        }),
    }
}

thread_local! {
    static DIALOG: std::cell::RefCell<egui_file_dialog::FileDialog> = std::cell::RefCell::new(egui_file_dialog::FileDialog::new());
}

/// Keeps the folder picker going; a picked folder starts the checks.
pub fn update_dialog(ctx: &egui::Context) {
    let picked = DIALOG.with_borrow_mut(|d| {
        d.update(ctx);
        d.take_picked()
    });
    if let Some(dir) = picked {
        check(dir);
    }
}

#[cfg(not(windows))]
fn check(_dir: PathBuf) {
    with(|s| {
        s.view = View::Refused("Updating the keyboard from the app needs Windows for now.".into())
    });
}

#[cfg(not(windows))]
#[cfg(windows)]
fn start_update() {}

#[cfg(windows)]
fn check(dir: PathBuf) {
    with(|s| s.view = View::Checking);
    super::services::spawn_named("firmware-check", move || {
        let result = (|| -> Result<Plan, String> {
            let board = board()?;
            let profile = board.profile();
            let (left, right) = pick_files(&dir, &profile)?;
            let read = |p: Option<PathBuf>| {
                p.map(|p| std::fs::read(&p).map_err(|e| format!("Can't read {}: {e}", p.display())))
                    .transpose()
            };
            foxglove_flash::plan(read(left)?, read(right)?, &board)
        })();
        with(|s| match result {
            Ok(plan) if plan.images.is_empty() => s.view = View::Done(plan.lines()),
            Ok(plan) => {
                s.view = View::Confirm {
                    lines: plan.lines(),
                    notes: notes(&plan),
                    downgrade: plan.downgrade,
                    allow_downgrade: false,
                };
                s.plan = Some(plan);
            }
            Err(e) => s.view = View::Refused(e),
        });
    });
}

#[cfg(windows)]
fn start_update() {
    let Some((plan, allow)) = with(|s| match (&s.view, s.plan.take()) {
        (
            View::Confirm {
                allow_downgrade, ..
            },
            Some(plan),
        ) => {
            let allow = *allow_downgrade;
            s.view = View::Running(vec![]);
            Some((plan, allow))
        }
        (_, plan) => {
            s.plan = plan;
            None
        }
    }) else {
        return;
    };
    super::services::spawn_named("firmware-update", move || {
        let mut steps: Vec<String> = Vec::new();
        let result = (|| -> Result<(), foxglove_flash::Failure> {
            let board = board().map_err(|message| foxglove_flash::Failure {
                half: Half::Left,
                message,
                recovery: String::new(),
            })?;
            let _lock = lock().map_err(|message| foxglove_flash::Failure {
                half: Half::Left,
                message,
                recovery: String::new(),
            })?;
            let mut known = known();
            let result = foxglove_flash::run(&plan, &board, &mut known, allow, &mut |p| {
                let line = p.text;
                with(|s| {
                    if let View::Running(list) = &mut s.view {
                        list.push(line.clone());
                    }
                });
            });
            save_known(&known);
            result
        })();
        with(|s| {
            if let View::Running(list) = &s.view {
                steps.clone_from(list);
            }
            s.view = match result {
                Ok(()) => {
                    steps.push("Both halves are up to date.".into());
                    View::Done(steps)
                }
                Err(f) => View::Failed {
                    steps,
                    message: f.message,
                    recovery: f.recovery,
                },
            };
        });
        super::keyboard_settings::read_firmware_again();
    });
}

#[cfg(windows)]
fn board() -> Result<windows_board::WindowsBoard, String> {
    windows_board::WindowsBoard::new()
}

/// The single-flasher lock the script uses too, so the app and the script never write at once.
#[derive(Debug)]
struct FlashLock(PathBuf);

impl Drop for FlashLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn lock() -> Result<FlashLock, String> {
    let path = std::env::var_os("FOXGLOVE_FLASH_LOCK")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("foxglove-flash.lock"));
    lock_at(&path, process_alive)
}

fn lock_at(path: &Path, alive: fn(u32) -> bool) -> Result<FlashLock, String> {
    use std::io::Write;
    for _ in 0..2 {
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
        {
            Ok(mut f) => {
                let _ = write!(f, "{}", std::process::id());
                return Ok(FlashLock(path.to_path_buf()));
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                let owner: u32 = std::fs::read_to_string(path)
                    .ok()
                    .and_then(|s| s.trim().parse().ok())
                    .unwrap_or(0);
                if owner != 0 && alive(owner) {
                    return Err(format!(
                        "Another flasher is running (process {owner}). Wait for it to finish."
                    ));
                }
                let _ = std::fs::remove_file(path); // its process is gone
            }
            Err(e) => return Err(format!("Can't take the flash lock {}: {e}", path.display())),
        }
    }
    Err(format!("Can't take the flash lock {}", path.display()))
}

#[cfg(windows)]
fn process_alive(pid: u32) -> bool {
    use windows::Win32::Foundation::{CloseHandle, STILL_ACTIVE};
    use windows::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    // SAFETY: plain Win32 calls; the handle is closed exactly once below.
    unsafe {
        let Ok(handle) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
            return false;
        };
        let mut code = 0u32;
        let alive = GetExitCodeProcess(handle, &mut code).is_ok() && code == STILL_ACTIVE.0 as u32;
        let _ = CloseHandle(handle);
        alive
    }
}

#[cfg(not(windows))]
fn process_alive(_pid: u32) -> bool {
    true
}

/// `%LOCALAPPDATA%\Foxglove\flash-history.log` (or `FOXGLOVE_FLASH_HISTORY`), as the script writes it.
fn history_path() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("FOXGLOVE_FLASH_HISTORY") {
        return Some(PathBuf::from(p));
    }
    std::env::var_os("LOCALAPPDATA")
        .map(|d| PathBuf::from(d).join("Foxglove").join("flash-history.log"))
}

/// `secs` since 1970 as `YYYY-MM-DDTHH:MM:SSZ` (UTC), as the script writes it.
fn utc(secs: u64) -> String {
    let (days, rest) = ((secs / 86_400) as i64, secs % 86_400);
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rest / 3600,
        rest / 60 % 60,
        rest % 60
    )
}

/// One history line: "UTC time, half, image SHA, outcome, detail", tab-separated.
fn history_line(secs: u64, half: Half, sha: &str, outcome: &str, detail: &str) -> String {
    let sha = if sha.is_empty() { "?" } else { sha };
    format!(
        "{}\t{}\t{sha}\t{outcome}\t{detail}\n",
        utc(secs),
        half.name()
    )
}
fn record(half: Half, sha: &str, outcome: &str, detail: &str) {
    use std::io::Write;
    let line = history_line(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs()),
        half,
        sha,
        outcome,
        &format!("app: {detail}"),
    );
    let written = history_path().and_then(|p| {
        std::fs::create_dir_all(p.parent()?).ok()?;
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(p)
            .ok()?
            .write_all(line.as_bytes())
            .ok()
    });
    if written.is_none() {
        eprintln!("Foxglove: flash history not written: {}", line.trim());
    }
}

/// Writes `data` to `path` in 512-byte blocks and syncs it. An error after every byte was
/// accepted is the bootloader restarting, not a failure; one before is, unless the drive is gone.
fn write_image(path: &Path, data: &[u8]) -> Result<(), foxglove_flash::WriteError> {
    use foxglove_flash::WriteError;
    use std::io::Write;
    let mut written = 0usize;
    let result = (|| -> std::io::Result<()> {
        let mut f = std::fs::File::create(path)?;
        for block in data.chunks(512) {
            f.write_all(block)?;
            written += block.len();
        }
        f.flush()?;
        f.sync_all()
    })();
    match result {
        Ok(()) => Ok(()),
        Err(_) if written == data.len() => Ok(()),
        Err(e) if path.parent().is_some_and(|d| !d.exists()) => {
            Err(WriteError::DriveGone(format!(
                "the drive went away after {written}/{} bytes: {e}",
                data.len()
            )))
        }
        Err(e) => Err(WriteError::Failed(format!(
            "stopped after {written}/{} bytes: {e}",
            data.len()
        ))),
    }
}

/// The `Board-ID:` line of an INFO_UF2.TXT; exactly one.
fn board_id_line(text: &str) -> Result<String, String> {
    let lines: Vec<&str> = text
        .lines()
        .filter(|l| l.starts_with("Board-ID:"))
        .collect();
    match lines.as_slice() {
        [one] => Ok(one.trim().to_string()),
        _ => Err("INFO_UF2.TXT has no single Board-ID line".into()),
    }
}

#[cfg(windows)]
mod windows_board {
    use super::*;
    use digitalis::protocol;
    use foxglove_flash::{Board, Build, WriteError};
    use std::sync::mpsc;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    const BOOTLOADER: &str = r"USB\VID_239A&PID_00B3\";
    /// GUID_DEVINTERFACE_VOLUME: a bootloader drive mounting or going.
    const VOLUMES: u128 = 0x53f5630d_b6bf_11d0_94f2_00a0c91efb8b;

    pub struct WindowsBoard {
        link: Arc<digitalis::link::Link>,
        hello: protocol::Hello,
        /// The running keyboard's USB id, for finding the left by serial.
        id: (u16, u16),
        events: mpsc::Receiver<()>,
        _volumes: Option<super::super::device_watch::DeviceWatch>,
        _hid: Option<super::super::device_watch::DeviceWatch>,
    }

    impl WindowsBoard {
        pub fn new() -> Result<Self, String> {
            let (id, link, hello) = super::super::link::host_keyboard()
                .ok_or("No keyboard with Foxglove's firmware is connected.")?;
            // Device events wake the waits; nothing checks on a timer.
            let (tx, events) = mpsc::channel();
            let volumes_tx = tx.clone();
            let volumes = super::super::device_watch::DeviceWatch::class(
                VOLUMES,
                Box::new(move |_| {
                    let _ = volumes_tx.send(());
                }),
            );
            let hid = super::super::device_watch::DeviceWatch::interfaces(
                true,
                Box::new(move |_| {
                    let _ = tx.send(());
                }),
            );
            Ok(Self {
                link,
                hello,
                id,
                events,
                _volumes: volumes,
                _hid: hid,
            })
        }

        /// The left's running profile, for picking CI files ("dev" unless it says GA).
        pub fn profile(&self) -> String {
            self.build(Half::Left).map_or("dev".into(), |b| b.profile)
        }

        /// Waits for the next device event, up to `timeout`; then lets a burst settle.
        fn next_event(&self, timeout: Duration) -> bool {
            let got = self.events.recv_timeout(timeout).is_ok();
            while self.events.try_recv().is_ok() {}
            got
        }

        fn request(
            &self,
            report: &[u8],
            cmd: u8,
            timeout: Duration,
        ) -> Option<[u8; protocol::REPORT]> {
            use digitalis::Transport;
            self.link
                .request(report, protocol::SETTINGS, cmd, timeout)
                .ok()
        }

        fn present(&self, prefix: &str, ok_only: bool) -> Vec<String> {
            let filter = if ok_only {
                " -and $_.Status -eq 'OK'"
            } else {
                ""
            };
            let script = format!(
                "ConvertTo-Json -Compress -InputObject @(Get-PnpDevice -PresentOnly | Where-Object {{ $_.InstanceId -like '{}*'{filter} }} | ForEach-Object {{ $_.InstanceId.Split('\\')[-1] }})",
                prefix.replace('\'', "")
            );
            powershell(&script).map(|v| strings(&v)).unwrap_or_default()
        }

        fn keyboard_prefix(&self) -> String {
            format!(r"USB\VID_{:04X}&PID_{:04X}\", self.id.0, self.id.1)
        }
    }

    /// A JSON array of strings (or one string) as a list.
    pub(super) fn strings(v: &serde_json::Value) -> Vec<String> {
        match v {
            serde_json::Value::Array(a) => a
                .iter()
                .filter_map(|s| s.as_str().map(String::from))
                .collect(),
            serde_json::Value::String(s) => vec![s.clone()],
            _ => vec![],
        }
    }

    /// Runs a PowerShell query with no window and returns its JSON output.
    fn powershell(script: &str) -> Result<serde_json::Value, String> {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let out = std::process::Command::new("powershell")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                &format!("$ErrorActionPreference='Stop'; {script}"),
            ])
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .map_err(|e| e.to_string())?;
        if !out.status.success() {
            return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
        }
        let text = String::from_utf8_lossy(&out.stdout);
        serde_json::from_str(if text.trim().is_empty() {
            "[]"
        } else {
            text.trim()
        })
        .map_err(|e| e.to_string())
    }

    impl Board for WindowsBoard {
        fn left_on_usb(&self) -> Option<bool> {
            let r = self.request(
                &protocol::conn_state(),
                protocol::settings::CONN_STATE,
                Duration::from_millis(800),
            )?;
            Some(protocol::ConnState::parse(&r).usb_ready)
        }

        fn right_battery(&self) -> Option<u8> {
            digitalis::panel::hello(self.link.as_ref())
                .ok()
                .and_then(|h| h.right_battery)
                .or(self.hello.right_battery)
        }

        fn left_serial(&self) -> Option<String> {
            // The right half on USB power shows up with the same id but not OK (Windows problem 28).
            match self.present(&self.keyboard_prefix(), true).as_slice() {
                [one] => Some(one.clone()),
                _ => None,
            }
        }

        fn build(&self, half: Half) -> Option<Build> {
            let timeout = Duration::from_secs(if half == Half::Right { 6 } else { 1 });
            let r = self.request(
                &protocol::build_info(half.index()),
                protocol::settings::BUILD_INFO,
                timeout,
            )?;
            let b = protocol::BuildInfo::parse(&r).ok()?;
            let v = b.version;
            Some(Build {
                version: format!("{}.{}.{}", v.major, v.minor, v.patch),
                profile: if v.profile == 1 { "ga" } else { "dev" }.into(),
                sha: b.sha,
            })
        }

        fn hands_free(&self) -> bool {
            self.hello.capabilities & protocol::capabilities::BOOTLOADER != 0
        }

        fn enter_bootloader(&self, half: Half) -> Result<(), String> {
            let cmd = protocol::settings::BOOTLOADER;
            let arm = self
                .request(&protocol::bootloader_arm(), cmd, Duration::from_secs(1))
                .ok_or("no reply to ARM")?;
            if arm[2] != 0 {
                return Err(format!("ARM status {}", arm[2]));
            }
            let nonce = u32::from_le_bytes([arm[3], arm[4], arm[5], arm[6]]);
            let r = self
                .request(
                    &protocol::bootloader_trigger(half.index(), nonce),
                    cmd,
                    Duration::from_secs(5),
                )
                .ok_or("no reply to TRIGGER")?;
            if r[2] != 0 {
                return Err(format!("TRIGGER status {}", r[2]));
            }
            Ok(())
        }

        fn bootloaders(&self) -> Vec<String> {
            self.present(BOOTLOADER, false)
        }

        fn wait_bootloaders(&self, timeout: Duration) {
            self.next_event(timeout);
        }

        fn volume(&self, serial: &str) -> Result<PathBuf, String> {
            if !serial.chars().all(|c| c.is_ascii_alphanumeric()) {
                return Err("an unexpected bootloader serial".into());
            }
            let script = format!(
                "$disks = @(Get-CimInstance Win32_DiskDrive | Where-Object {{ $_.PNPDeviceID -match '{serial}' }}); ConvertTo-Json -Compress -InputObject @($disks | Where-Object {{ $disks.Count -eq 1 }} | ForEach-Object {{ Get-CimAssociatedInstance -InputObject $_ -Association Win32_DiskDriveToDiskPartition | ForEach-Object {{ Get-CimAssociatedInstance -InputObject $_ -Association Win32_LogicalDiskToPartition | ForEach-Object {{ $_.DeviceID }} }} }})"
            );
            let rows = strings(&powershell(&script)?);
            let [letter] = rows.as_slice() else {
                return Err("its drive can't be told apart from other drives; unplug other keyboards in bootloader mode".into());
            };
            let volume = PathBuf::from(format!("{letter}\\"));
            if !volume.join("INFO_UF2.TXT").exists() {
                return Err("its drive has no INFO_UF2.TXT".into());
            }
            Ok(volume)
        }

        fn board_id(&self, volume: &Path) -> Result<String, String> {
            let text =
                std::fs::read_to_string(volume.join("INFO_UF2.TXT")).map_err(|e| e.to_string())?;
            board_id_line(&text)
        }

        fn settle(&self) {
            std::thread::sleep(Duration::from_secs(3)); // one-shot, after the drive appears
        }

        fn write(&self, volume: &Path, data: &[u8]) -> Result<(), WriteError> {
            write_image(&volume.join("flash.uf2"), data)
        }

        fn wait_drive_gone(&self, volume: &Path, timeout: Duration) -> bool {
            let end = Instant::now() + timeout;
            loop {
                if !volume.join("INFO_UF2.TXT").exists() {
                    return true;
                }
                let left = end.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    return false;
                }
                self.next_event(left);
            }
        }

        fn wait_left_back(&self, serial: &str, timeout: Duration) -> bool {
            let end = Instant::now() + timeout;
            let instance = format!("{}{serial}", self.keyboard_prefix());
            loop {
                let back = self.present(&instance, true).len() == 1;
                if back && self.link.wait_connected(Duration::from_secs(5)).is_ok() {
                    return true;
                }
                let left = end.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    return false;
                }
                self.next_event(left);
            }
        }

        fn wait_right_back(&self, timeout: Duration) -> bool {
            // No event says the relay is up: ask HELLO every 2 s, from now (the left is back) until
            // it is or `timeout` (#122: 20-40 s).
            let end = Instant::now() + timeout;
            loop {
                if digitalis::panel::hello(self.link.as_ref()).is_ok_and(|h| h.right_link == 2) {
                    return true;
                }
                if Instant::now() >= end {
                    return false;
                }
                std::thread::sleep(
                    Duration::from_secs(2).min(end.saturating_duration_since(Instant::now())),
                );
            }
        }

        fn record(&self, half: Half, sha: &str, outcome: &str, detail: &str) {
            super::record(half, sha, outcome, detail);
        }
    }
}

/// The update's part of the Firmware card; returns the click, if any.
pub fn draw(ui: &mut egui::Ui, v: &View) -> Option<Intent> {
    use egui::RichText;
    let p = super::theme::palette(ui);
    let mut out = None;
    let list = |ui: &mut egui::Ui, steps: &[String]| {
        for s in steps {
            ui.label(RichText::new(format!("• {s}")).color(p.muted));
        }
    };
    match v {
        View::Idle => {
            if ui
                .button("Update keyboard…")
                .on_hover_text("Pick a folder with new firmware for both halves")
                .clicked()
            {
                out = Some(Intent::Start);
            }
        }
        View::Checking => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Checking the firmware and the keyboard…");
            });
        }
        View::Confirm {
            lines,
            notes,
            downgrade,
            allow_downgrade,
        } => {
            ui.label(RichText::new("Update keyboard").strong());
            for l in lines {
                ui.label(RichText::new(l).font(super::theme::mono(12.5)));
            }
            for n in notes {
                ui.label(RichText::new(n).color(p.muted));
            }
            let mut allow = *allow_downgrade;
            if *downgrade
                && ui
                    .checkbox(&mut allow, "Install the older version anyway")
                    .changed()
            {
                out = Some(Intent::AllowDowngrade(allow));
            }
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(!*downgrade || *allow_downgrade, egui::Button::new("Update"))
                    .clicked()
                {
                    out = Some(Intent::Confirm);
                }
                if ui.button("Cancel").clicked() {
                    out = Some(Intent::Close);
                }
            });
        }
        View::Refused(why) => {
            ui.label(RichText::new(why).color(p.warn));
            ui.label(RichText::new("Nothing was written.").color(p.muted));
            if ui.button("OK").clicked() {
                out = Some(Intent::Close);
            }
        }
        View::Running(steps) => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(RichText::new("Updating the keyboard").strong());
            });
            list(ui, steps);
        }
        View::Done(steps) => {
            list(ui, steps);
            if ui.button("OK").clicked() {
                out = Some(Intent::Close);
            }
        }
        View::Failed {
            steps,
            message,
            recovery,
        } => {
            list(ui, steps);
            ui.label(RichText::new(message).color(p.danger));
            if !recovery.is_empty() {
                ui.label(recovery);
            }
            if ui.button("OK").clicked() {
                out = Some(Intent::Close);
            }
        }
    }
    out
}

#[cfg(test)]
pub mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let d =
            std::env::temp_dir().join(format!("foxglove-fw-update-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn picks_release_then_ci_names_and_refuses_doubles() {
        let d = temp("pick");
        for n in [
            "foxglove-0.2.0-left.uf2",
            "foxglove-0.2.0-right.uf2",
            "foxglove-0.2.0-settings-reset.uf2",
            "SHA256SUMS",
        ] {
            std::fs::write(d.join(n), b"x").unwrap();
        }
        let (l, r) = pick_files(&d, "dev").unwrap();
        assert_eq!(l.unwrap().file_name().unwrap(), "foxglove-0.2.0-left.uf2");
        assert_eq!(r.unwrap().file_name().unwrap(), "foxglove-0.2.0-right.uf2");
        let d = temp("ci");
        for n in [
            "foxglove-fw-v0.2.0-1a2b3c4-left-dev.uf2",
            "foxglove-fw-v0.2.0-1a2b3c4-left-ga.uf2",
            "foxglove-fw-v0.2.0-1a2b3c4-right-dev.uf2",
        ] {
            std::fs::write(d.join(n), b"x").unwrap();
        }
        let (l, r) = pick_files(&d, "ga").unwrap();
        assert!(l.unwrap().to_string_lossy().ends_with("left-ga.uf2"));
        assert!(r.is_none(), "no right GA build: the right is left as it is");
        std::fs::write(d.join("foxglove-fw-v0.2.0-9z9z9z9-left-dev.uf2"), b"x").unwrap();
        assert!(pick_files(&d, "dev")
            .unwrap_err()
            .contains("More than one left"));
        assert!(pick_files(&temp("empty"), "dev")
            .unwrap_err()
            .contains("No keyboard firmware"));
    }

    #[test]
    fn the_history_line_matches_the_scripts() {
        assert_eq!(utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(utc(1709251199), "2024-02-29T23:59:59Z");
        assert_eq!(
            history_line(1792976523, Half::Right, "", "written", "app: x"),
            "2026-10-26T01:02:03Z\tright\t?\twritten\tapp: x\n"
        );
    }

    #[test]
    fn a_full_write_counts_even_when_the_drive_goes_and_a_missing_drive_is_gone() {
        let d = temp("write");
        assert_eq!(write_image(&d.join("flash.uf2"), &[1u8; 1024]), Ok(()));
        assert_eq!(std::fs::read(d.join("flash.uf2")).unwrap().len(), 1024);
        let gone = d.join("no-such-drive").join("flash.uf2");
        assert!(matches!(
            write_image(&gone, &[1u8; 512]),
            Err(foxglove_flash::WriteError::DriveGone(_))
        ));
    }

    #[test]
    fn board_id_needs_exactly_one_line() {
        assert_eq!(
            board_id_line("UF2 Bootloader 0.6\nModel: nice!nano\nBoard-ID: nRF52840-nicenano\n")
                .unwrap(),
            "Board-ID: nRF52840-nicenano"
        );
        assert!(board_id_line("Model: x\n").is_err());
        assert!(board_id_line("Board-ID: a\nBoard-ID: b\n").is_err());
    }

    #[test]
    fn one_flasher_at_a_time_and_a_dead_owner_is_taken_over() {
        let path = temp("lock").join("foxglove-flash.lock");
        let held = lock_at(&path, |_| true).unwrap();
        assert!(lock_at(&path, |_| true)
            .unwrap_err()
            .contains("Another flasher"));
        drop(held);
        assert!(!path.exists(), "released on drop");
        std::fs::write(&path, "999999").unwrap();
        let taken = lock_at(&path, |_| false).unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            std::process::id().to_string()
        );
        drop(taken);
    }

    #[test]
    fn confirm_notes_say_what_will_happen() {
        let plan = Plan {
            hands_free: false,
            downgrade: true,
            ..Default::default()
        };
        let n = notes(&plan);
        assert!(n[0].contains("left goes first"));
        assert!(n.iter().any(|l| l.contains("double-tap the reset button")));
        assert!(n.iter().any(|l| l.contains("older version")));
    }

    #[cfg(windows)]
    #[test]
    fn powershell_lists_parse_one_or_many() {
        use serde_json::json;
        assert_eq!(windows_board::strings(&json!(["A", "B"])), ["A", "B"]);
        assert_eq!(windows_board::strings(&json!("A")), ["A"]);
        assert!(windows_board::strings(&json!(null)).is_empty());
    }

    /// The confirm step as the previews draw it.
    pub fn sample() -> View {
        View::Confirm {
            lines: vec![
                "Left: 0.1.0 dev e255d32 -> 0.2.0 dev 1a2b3c4".into(),
                "Right: 0.1.0 dev e255d32 -> 0.2.0 dev 1a2b3c4".into(),
            ],
            notes: notes(&Plan {
                hands_free: true,
                ..Default::default()
            }),
            downgrade: false,
            allow_downgrade: false,
        }
    }
}
