//! Right art on the shared host link. File I/O and all keyboard requests stay on the worker.
#![allow(dead_code)] // Handle consumed by the art settings and tray in #15/#16.

use std::io::Read;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use digitalis::art;
use digitalis::link::Link;
use digitalis::protocol::ArtStatus;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Choice {
    /// -1 chooses a random built-in.
    BuiltIn(i8),
    File(PathBuf),
}

#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub connected: bool,
    pub status: Option<ArtStatus>,
    pub pinned: Option<Choice>,
    pub busy: bool,
    pub cancelled: bool,
    pub push_progress: Option<(u16, u16)>,
    pub error: Option<String>,
}

impl Snapshot {
    /// Firmware supplies a count only; labels can later come from the art manifest.
    pub fn built_ins(&self) -> Vec<(i8, String)> {
        let count = self.status.map_or(0, |s| s.right_set_count).min(128);
        (0..count).map(|n| (n as i8, format!("Set {n}"))).collect()
    }

    pub fn progress(&self) -> Option<(u16, u16)> {
        self.push_progress
    }
}

enum Command {
    Status,
    Choose(Choice),
}

#[derive(Clone)]
pub struct Handle {
    commands: mpsc::SyncSender<Command>,
    state: Arc<Mutex<Snapshot>>,
    cancel: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
}

impl Handle {
    pub fn snapshot(&self) -> Snapshot {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn update(&self, f: impl FnOnce(&mut Snapshot)) {
        f(&mut self.state.lock().unwrap_or_else(|e| e.into_inner()));
    }

    fn submit(&self, command: Command) -> bool {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.busy || self.stop.load(Ordering::Relaxed) {
            return false;
        }
        self.cancel.store(false, Ordering::Relaxed);
        if self.commands.try_send(command).is_err() {
            return false;
        }
        state.busy = true;
        state.cancelled = false;
        state.error = None;
        state.push_progress = None;
        true
    }

    pub fn refresh(&self) -> bool {
        self.submit(Command::Status)
    }

    pub fn select(&self, set: i8) -> bool {
        set >= -1 && self.submit(Command::Choose(Choice::BuiltIn(set)))
    }

    pub fn push(&self, path: PathBuf) -> bool {
        self.submit(Command::Choose(Choice::File(path)))
    }

    /// Stops further writes/polls; a relay already committed may finish on the keyboard.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed) || self.stop.load(Ordering::Relaxed)
    }
}

static SERVICE: OnceLock<Handle> = OnceLock::new();

pub fn handle() -> Option<Handle> {
    SERVICE.get().cloned()
}

pub fn start() {
    SERVICE.get_or_init(|| {
        let (commands, receiver) = mpsc::sync_channel(1);
        let h = Handle {
            commands,
            state: Arc::new(Mutex::new(Snapshot::default())),
            cancel: Arc::new(AtomicBool::new(false)),
            stop: Arc::new(AtomicBool::new(false)),
        };
        let worker = h.clone();
        if let Err(e) = std::thread::Builder::new()
            .name("foxglove-art".into())
            .spawn(move || {
                while !worker.stop.load(Ordering::Relaxed) {
                    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        run(&worker, &receiver)
                    }))
                    .is_ok()
                    {
                        break;
                    }
                    worker.update(|s| {
                        s.busy = false;
                        s.connected = false;
                        s.error = Some("Art worker panicked; restarting".into());
                    });
                    std::thread::sleep(Duration::from_secs(3));
                }
            })
        {
            h.update(|s| s.error = Some(e.to_string()));
        }
        h
    });
}

pub fn stop() {
    if let Some(h) = SERVICE.get() {
        h.stop.store(true, Ordering::Relaxed);
        h.cancel();
    }
}

fn settings_path() -> Option<PathBuf> {
    crate::settings::Settings::config_file_path().map(|p| p.with_file_name("art.json"))
}

fn restore(pinned: &Option<Choice>, status: ArtStatus) -> Option<Choice> {
    // Only link=2 has a ready relay; 1 is connected but still discovering it.
    pinned
        .clone()
        .filter(|_| status.link == 2 && status.right_state == 0)
}

fn fresh_status(link: &Link, h: &Handle) -> Result<ArtStatus, String> {
    art::status(link).map_err(|e| e.to_string())?;
    std::thread::sleep(Duration::from_millis(300));
    if h.cancelled() {
        return Err("cancelled".into());
    }
    art::status(link).map_err(|e| e.to_string())
}

fn choose(link: &Link, h: &Handle, choice: &Choice) -> Result<ArtStatus, String> {
    if h.cancelled() {
        return Err("cancelled".into());
    }
    match choice {
        Choice::BuiltIn(set) => art::select(link, *set).map_err(|e| e.to_string()),
        Choice::File(path) => {
            let mut blob = Vec::new();
            std::fs::File::open(path)
                .and_then(|f| f.take(65536).read_to_end(&mut blob))
                .map_err(|e| format!("reading {}: {e}", path.display()))?;
            if !art::is_blob(&blob) {
                return Err(format!("{} is not an art blob", path.display()));
            }
            if blob.len() <= u16::MAX as usize {
                h.update(|s| s.push_progress = Some((0, blob.len() as u16)));
            }
            art::push_cancellable(
                link,
                &blob,
                |st| {
                    h.update(|s| {
                        s.status = Some(*st);
                        s.push_progress = Some((st.pushed, st.length));
                    })
                },
                || h.cancelled(),
            )
            .map_err(|e| e.to_string())
        }
    }
}

fn run(h: &Handle, receiver: &mpsc::Receiver<Command>) {
    let pinned = settings_path()
        .and_then(|p| std::fs::read(p).ok())
        .and_then(|b| serde_json::from_slice::<Choice>(&b).ok());
    h.update(|s| s.pinned = pinned);
    let mut current: Option<(Arc<Link>, u64)> = None;
    let mut retry = Instant::now();
    let mut check_restore = false;
    let mut restore_delay = Duration::from_secs(1);
    while !h.stop.load(Ordering::Relaxed) {
        if let Some((link, generation)) = &current {
            let state = link.state();
            if state.connected.is_none() || state.generation != *generation {
                current = None;
                h.update(|s| {
                    s.connected = false;
                    s.status = None;
                });
                retry = Instant::now();
            }
        }
        if current.is_none() && Instant::now() >= retry {
            if let Some((link, _)) = super::link::host_link() {
                let generation = link.state().generation;
                current = Some((link, generation));
                check_restore = true;
                restore_delay = Duration::from_secs(1);
                h.update(|s| s.connected = true);
            }
            retry = Instant::now() + Duration::from_secs(3);
        }
        let command = match receiver.recv_timeout(Duration::from_secs(1)) {
            Ok(c) => Some(c),
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => None,
        };
        let mut automatic = false;
        if command.is_none() && check_restore && current.is_some() {
            h.update(|s| {
                if !s.busy {
                    h.cancel.store(false, Ordering::Relaxed);
                    s.busy = true;
                    s.error = None;
                    s.cancelled = false;
                    s.push_progress = None;
                    automatic = true;
                }
            });
        }
        if command.is_none() && !automatic {
            continue;
        }
        let result = if let Some((link, _)) = &current {
            if automatic {
                let result = fresh_status(link, h).and_then(|st| {
                    h.update(|s| s.status = Some(st));
                    check_restore = st.link != 2;
                    if let Some(choice) = restore(&h.snapshot().pinned, st) {
                        choose(link, h, &choice)
                    } else {
                        Ok(st)
                    }
                });
                // Retry a missing right half or failed STATUS, backing off to once a minute so
                // a sleeping right half doesn't cost the left half's battery.
                if check_restore {
                    std::thread::sleep(restore_delay);
                    restore_delay = (restore_delay * 2).min(Duration::from_secs(60));
                }
                result
            } else {
                match command.unwrap() {
                    Command::Status => fresh_status(link, h),
                    Command::Choose(choice) => choose(link, h, &choice).and_then(|st| {
                        check_restore = false;
                        h.update(|s| {
                            s.status = Some(st);
                            s.pinned = Some(choice.clone());
                        });
                        if let Some(path) = settings_path() {
                            if let Some(parent) = path.parent() {
                                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
                            }
                            let bytes = serde_json::to_vec(&choice).map_err(|e| e.to_string())?;
                            std::fs::write(path, bytes)
                                .map_err(|e| format!("saving pinned art: {e}"))?;
                        }
                        Ok(st)
                    }),
                }
            }
        } else {
            Err("Waiting for a keyboard with the host link".into())
        };
        h.update(|s| {
            s.busy = false;
            s.cancelled = h.cancelled();
            match result {
                Ok(st) => {
                    if s.push_progress.is_some() {
                        s.push_progress = Some((st.pushed, st.length));
                    }
                    s.status = Some(st);
                    s.error = None;
                }
                Err(e) => s.error = Some(e),
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handle_rejects_overlapping_commands_and_cancel_is_immediate() {
        let (commands, receiver) = mpsc::sync_channel(1);
        let h = Handle {
            commands,
            state: Arc::new(Mutex::new(Snapshot::default())),
            cancel: Arc::new(AtomicBool::new(false)),
            stop: Arc::new(AtomicBool::new(false)),
        };
        assert!(!h.select(-2));
        assert!(h.select(-1));
        assert!(h.snapshot().busy);
        assert!(!h.refresh());
        h.cancel();
        assert!(h.cancelled());
        assert!(matches!(
            receiver.try_recv(),
            Ok(Command::Choose(Choice::BuiltIn(-1)))
        ));
        h.update(|s| s.busy = false);
        assert!(h.refresh());
        assert!(!h.cancelled());
        h.stop.store(true, Ordering::Relaxed);
        h.update(|s| s.busy = false);
        assert!(!h.refresh());
    }

    #[test]
    fn restores_only_a_live_right_half_on_a_builtin() {
        let pinned = Some(Choice::File("art.bin".into()));
        let mut st = ArtStatus::default();
        assert_eq!(restore(&pinned, st), None);
        st.link = 1;
        assert_eq!(restore(&pinned, st), None);
        st.link = 2;
        assert_eq!(restore(&pinned, st), pinned);
        st.right_state = 2;
        assert_eq!(restore(&pinned, st), None);
        st.right_state = 0;
        assert_eq!(restore(&None, st), None);
    }

    #[test]
    fn choices_round_trip_and_builtin_list_uses_firmware_count() {
        for choice in [
            Choice::BuiltIn(-1),
            Choice::BuiltIn(4),
            Choice::File(PathBuf::from(r"C:\art\fox.bin")),
        ] {
            assert_eq!(
                serde_json::from_str::<Choice>(&serde_json::to_string(&choice).unwrap()).unwrap(),
                choice
            );
        }
        let s = Snapshot {
            status: Some(ArtStatus {
                right_set_count: 3,
                ..Default::default()
            }),
            ..Default::default()
        };
        assert_eq!(
            s.built_ins(),
            vec![
                (0, "Set 0".into()),
                (1, "Set 1".into()),
                (2, "Set 2".into())
            ]
        );
    }
}
