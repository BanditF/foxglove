//! hostpanel: drives the Eyelash Corne's screens from the PC.
//!
//! Without a command it sits in the tray and keeps the left screen's host panel up to date
//! (clock, volume, media, per-game hint). The commands talk to the keyboard once and exit.

#![cfg_attr(windows, windows_subsystem = "windows")]

mod config;
mod device;
mod panel_lock;
mod providers;
mod render;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use hidapi::HidApi;

use device::Keyboard;
use render::{Panel, View};

#[derive(Parser)]
#[command(version, about)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Run the panel (the default); --console keeps it in this window instead of the tray
    Run {
        #[arg(long)]
        console: bool,
    },
    /// Show what the keyboard reports: panel size, right half link and battery, art relay
    Status,
    /// Push an art set (.bin from scripts/flipper_art.py) to the right-hand screen
    PushArt { blob: PathBuf },
    /// Show a built-in right-hand art set again (-1 = random)
    Select {
        #[arg(allow_hyphen_values = true)]
        set: i8,
    },
    /// Draw the panel once into a .pbm image, without a keyboard
    Preview { out: PathBuf },
}

fn main() {
    let cli = Cli::parse_from(std::env::args_os());
    if cli.command.is_some() && !matches!(cli.command, Some(Command::Run { console: false })) {
        attach_console();
    }
    let result = match cli.command {
        None | Some(Command::Run { console: false }) => tray::run(),
        Some(Command::Run { console: true }) => {
            let shared = Shared::default();
            worker(shared.clone());
            Ok(())
        }
        Some(Command::Status) => status(),
        Some(Command::PushArt { blob }) => push_art(&blob),
        Some(Command::Select { set }) => select(set),
        Some(Command::Preview { out }) => preview(&out),
    };
    if let Err(e) = result {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

/// With the windows subsystem there is no console; borrow the one we were started from.
fn attach_console() {
    #[cfg(windows)]
    unsafe {
        use windows::Win32::System::Console::{AttachConsole, ATTACH_PARENT_PROCESS};
        let _ = AttachConsole(ATTACH_PARENT_PROCESS);
    }
}

fn open() -> Result<Keyboard> {
    let cfg = config::load();
    let mut api = HidApi::new()?;
    Keyboard::open(&mut api, cfg.product.as_deref())
}

fn status() -> Result<()> {
    let kb = open()?;
    let h = kb.hello;
    println!("{}: host link v{}, panel {}x{}", kb.product, h.version, h.width, h.height);
    println!(
        "right half: {}, battery {}",
        ["not connected", "connected (no art relay)", "connected"][h.right_link.min(2) as usize],
        h.right_battery.map(|b| format!("{b}%")).unwrap_or("unknown".into())
    );
    println!("relay discovery: {} after {} tries", h.discovery_text(), h.discovery_tries);
    let d = h.debug;
    println!(
        "relay debug: chrcs {} ends {} last ret {} att err {} timeouts {} answered {} in flight {} security {} mtu {}",
        d[0], d[1], d[2] as i8, d[3], d[4], d[5], d[6], d[7], u16::from_le_bytes([d[8], d[9]])
    );
    // The first STATUS starts a fresh read from the right half; the second returns it.
    kb.art_status()?;
    std::thread::sleep(Duration::from_millis(300));
    println!("art relay: {}", kb.art_status()?);
    Ok(())
}

fn push_art(path: &PathBuf) -> Result<()> {
    let blob = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    anyhow::ensure!(blob.starts_with(b"NVA1"), "{} is not an art blob", path.display());
    let kb = open()?;
    let start = Instant::now();
    println!("sending {} bytes to {}", blob.len(), kb.product);
    let st = kb.push_art(&blob, |s| {
        eprint!("\rpushing to the right half: {}/{} bytes", s.pushed, s.length);
    })?;
    eprintln!();
    println!("done in {:.1} s: {st}", start.elapsed().as_secs_f32());
    Ok(())
}

fn select(set: i8) -> Result<()> {
    let st = open()?.select_art(set)?;
    println!("{st}");
    Ok(())
}

fn sample_view() -> View {
    let now = chrono::Local::now();
    View {
        time: now.format("%H:%M").to_string(),
        date: now.format("%a %d %b").to_string(),
        volume: providers::volume().or(Some((40, false))),
        title: Some("A long song title that scrolls".into()),
        artist: Some("Some Artist".into()),
        playing: true,
        hint: Some("GAME: WASD".into()),
    }
}

fn preview(out: &PathBuf) -> Result<()> {
    providers::init_thread();
    let mut p = Panel::new(64, 52);
    render::draw(&mut p, &sample_view(), 0);
    let mut pbm = format!("P4\n{} {}\n", p.width, p.height).into_bytes();
    pbm.extend_from_slice(&p.bits);
    std::fs::write(out, pbm)?;
    println!("wrote {}", out.display());
    Ok(())
}

/// State shared between the worker and the tray.
#[derive(Clone, Default)]
pub struct Shared {
    pub status: Arc<Mutex<String>>,
    pub paused: Arc<AtomicBool>,
    pub quit: Arc<AtomicBool>,
}

impl Shared {
    fn set_status(&self, s: impl Into<String>) {
        let s = s.into();
        let mut cur = self.status.lock().unwrap();
        if *cur != s {
            eprintln!("{s}");
            *cur = s;
        }
    }
}

fn battery_log(battery: Option<u8>, last: &mut Option<u8>) {
    if battery.is_none() || battery == *last {
        return;
    }
    *last = battery;
    let p = config::path().with_file_name("right-battery.csv");
    let new = !p.exists();
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&p) {
        use std::io::Write;
        if new {
            let _ = writeln!(f, "time,right_battery_percent");
        }
        let _ = writeln!(f, "{},{}", chrono::Local::now().format("%Y-%m-%d %H:%M:%S"), battery.unwrap());
    }
}

/// Keeps the panel up to date until `quit`; reconnects whenever the keyboard goes away.
fn worker(shared: Shared) {
    providers::init_thread();
    let mut api = match HidApi::new() {
        Ok(api) => api,
        Err(e) => return shared.set_status(format!("HID unavailable: {e}")),
    };
    let mut cfg = config::load();
    let mut cfg_checked = Instant::now();
    let mut media = providers::Media::new();
    let mut view = View::default();
    let mut slow_at = Instant::now() - Duration::from_secs(10);
    let mut kb: Option<Keyboard> = None;
    let mut panel = Panel::new(64, 52);
    let mut last: Option<Vec<u8>> = None;
    let mut last_sent = Instant::now();
    let mut hello_at = Instant::now();
    let mut last_battery = None;
    let mut was_driving = false;
    let mut lock = panel_lock::PanelLock::new();
    let mut tick: u32 = 0;

    while !shared.quit.load(Ordering::Relaxed) {
        let frame_start = Instant::now();
        if cfg_checked.elapsed() > Duration::from_secs(5) {
            cfg = config::load();
            cfg_checked = Instant::now();
        }

        if kb.is_none() {
            match Keyboard::open(&mut api, cfg.product.as_deref()) {
                Ok(k) => {
                    panel = Panel::new(k.hello.width, k.hello.height);
                    battery_log(k.hello.right_battery, &mut last_battery);
                    shared.set_status(format!("Connected: {}", k.product));
                    last = None;
                    hello_at = Instant::now();
                    kb = Some(k);
                }
                Err(e) => {
                    shared.set_status(format!("Waiting for keyboard: {e}"));
                    std::thread::sleep(Duration::from_secs(3));
                    continue;
                }
            }
        }
        let k = kb.as_ref().unwrap();

        // Gather: the clock every frame, the rest once a second.
        let now = chrono::Local::now();
        view.time = now.format("%H:%M").to_string();
        view.date = now.format("%a %d %b").to_string();
        if slow_at.elapsed() >= Duration::from_secs(1) {
            slow_at = Instant::now();
            view.volume = providers::volume();
            match media.now_playing() {
                Some((title, artist, playing)) => {
                    view.title = Some(title);
                    view.artist = (!artist.is_empty()).then_some(artist);
                    view.playing = playing;
                }
                None => (view.title, view.artist) = (None, None),
            }
            view.hint = providers::foreground_exe()
                .and_then(|exe| cfg.games.get(&exe.to_lowercase()).cloned());
        }

        // Draw only while we hold the panel lock; paused gives it up so another app can draw.
        let paused = shared.paused.load(Ordering::Relaxed);
        if paused {
            lock.release();
        }
        let driving = !paused && lock.acquire();
        let mut result = Ok(());
        if driving {
            if !was_driving {
                shared.set_status(format!("Connected: {}", k.product));
            }
            render::draw(&mut panel, &view, tick);
            // Resend now and then even when nothing changed: the firmware drops the panel
            // after 65 s of silence.
            let stale = last_sent.elapsed() > Duration::from_secs(20);
            if last.as_deref() != Some(&panel.bits[..]) || stale {
                result = k.show(&panel.bits, last.as_deref(), panel.stride).map(|_| ());
                last = Some(panel.bits.clone());
                last_sent = Instant::now();
            }
        } else {
            if was_driving {
                result = k.release();
                last = None;
            }
            shared.set_status(if paused { "Paused".to_string() } else { "Panel driven by another app".to_string() });
        }
        was_driving = driving;

        // HELLO turns the panel back on in the firmware, so only the app drawing it asks.
        if driving && result.is_ok() && hello_at.elapsed() > Duration::from_secs(60) {
            hello_at = Instant::now();
            result = k.say_hello().map(|h| battery_log(h.right_battery, &mut last_battery));
        }
        if let Err(e) = result {
            shared.set_status(format!("Lost keyboard: {e}"));
            kb = None;
            continue;
        }

        tick = tick.wrapping_add(1);
        let period = Duration::from_millis(1000 / cfg.fps as u64);
        std::thread::sleep(period.saturating_sub(frame_start.elapsed()));
    }
    // Hand the panel back only if it was ours; another app may be drawing it.
    if let (Some(k), true) = (kb, was_driving) {
        let _ = k.release();
    }
}

#[cfg(windows)]
mod tray {
    use super::*;
    use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
    use tray_icon::{Icon, TrayIconBuilder};
    use windows::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, PeekMessageW, TranslateMessage, MSG, PM_REMOVE,
    };

    /// A small screen glyph: outline with a few "text" lines.
    fn icon() -> Icon {
        const N: usize = 32;
        let mut rgba = vec![0u8; N * N * 4];
        for y in 0..N {
            for x in 0..N {
                let border = (6..=25).contains(&x) && (2..=29).contains(&y)
                    && (x <= 7 || x >= 24 || y <= 3 || y >= 28);
                let line = (10..=21).contains(&x) && [8, 9, 14, 15, 20, 21].contains(&y);
                if border || line {
                    rgba[(y * N + x) * 4..][..4].copy_from_slice(&[255, 255, 255, 255]);
                }
            }
        }
        Icon::from_rgba(rgba, N as u32, N as u32).expect("icon")
    }

    pub fn run() -> Result<()> {
        let shared = Shared::default();
        let worker_shared = shared.clone();
        let handle = std::thread::spawn(move || worker(worker_shared));

        let status = MenuItem::new("Starting...", false, None);
        let pause = CheckMenuItem::new("Pause panel", true, false, None);
        let edit = MenuItem::new("Open config", true, None);
        let quit = MenuItem::new("Quit", true, None);
        let menu = Menu::new();
        menu.append_items(&[&status, &PredefinedMenuItem::separator(), &pause, &edit, &quit])?;
        let tray = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip("hostpanel")
            .with_icon(icon())
            .build()?;

        let mut shown = String::new();
        loop {
            unsafe {
                let mut msg = MSG::default();
                while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
            while let Ok(ev) = MenuEvent::receiver().try_recv() {
                if ev.id == quit.id() {
                    shared.quit.store(true, Ordering::Relaxed);
                } else if ev.id == pause.id() {
                    shared.paused.store(pause.is_checked(), Ordering::Relaxed);
                } else if ev.id == edit.id() {
                    let _ = std::process::Command::new("notepad").arg(config::path()).spawn();
                }
            }
            if shared.quit.load(Ordering::Relaxed) {
                break;
            }
            let s = shared.status.lock().unwrap().clone();
            if s != shown {
                let short: String = s.chars().take(100).collect();
                status.set_text(&short);
                let _ = tray.set_tooltip(Some(format!("hostpanel - {}", short.chars().take(60).collect::<String>())));
                shown = s;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = handle.join();
        Ok(())
    }
}

#[cfg(not(windows))]
mod tray {
    use super::*;
    pub fn run() -> Result<()> {
        worker(Shared::default());
        Ok(())
    }
}
