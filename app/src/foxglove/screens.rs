//! The left screen's host panel, drawn by Foxglove: the clock, volume, media and per-game hints
//! from `foxglove_panel`, sent over the shared link. It runs on its own thread from start-up,
//! whether or not the overlay is connected, and only draws while it holds the panel lock that
//! the standalone hostpanel also takes.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use digitalis::link::{EventSubscription, Link};
use digitalis::panel;
use digitalis::protocol::Hello;
use digitalis::{protocol, Transport};
use foxglove_panel::panel_lock::PanelLock;
use foxglove_panel::render::{self, Panel, View};
use foxglove_panel::stats::{Sample, Sampler, Stats};
use foxglove_panel::Sources;

use super::screens_settings::ScreensSettings;

static STATUS: Mutex<String> = Mutex::new(String::new());
static PAUSED: AtomicBool = AtomicBool::new(false);

pub fn paused() -> bool {
    PAUSED.load(Ordering::Relaxed)
}
pub fn set_paused(value: bool) {
    PAUSED.store(value, Ordering::Relaxed);
    wake();
}

/// Foxglove paused or resumed (#404): the service hands the panel back or draws again.
pub fn pause_changed() {
    wake();
}

static STOP: AtomicBool = AtomicBool::new(false);
static STOPPED: AtomicBool = AtomicBool::new(false);

/// What the panel service is doing, for the UI.
pub fn status() -> String {
    STATUS.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// The last frame sent to the left panel, for the Screens settings' preview.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Frame {
    pub width: usize,
    pub height: usize,
    pub stride: usize,
    pub bits: Vec<u8>,
}

/// What the Screens settings show: the keyboard with the host link and what it said to HELLO,
/// the frame last sent, and the last program in front other than Foxglove.
#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub product: String,
    pub hello: Option<Hello>,
    pub frame: Option<Frame>,
    pub foreground: Option<String>,
    /// The screens the keyboard describes, when it does (#160).
    pub screens: Vec<protocol::ScreenDescriptor>,
}

static SNAPSHOT: Mutex<Snapshot> = Mutex::new(Snapshot {
    product: String::new(),
    hello: None,
    frame: None,
    foreground: None,
    screens: Vec::new(),
});

pub fn snapshot() -> Snapshot {
    SNAPSHOT.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

fn update_snapshot(f: impl FnOnce(&mut Snapshot)) {
    f(&mut SNAPSHOT.lock().unwrap_or_else(|e| e.into_inner()));
}

/// The settings window's panel preview, while it's drawn: a new frame repaints it once. The
/// preview asks again each time it's drawn, so a closed preview stops the repaints (#279).
static PREVIEW: Mutex<Option<crate::ui_wake::UiWake>> = Mutex::new(None);

pub fn want_preview(wake: crate::ui_wake::UiWake) {
    *PREVIEW.lock().unwrap_or_else(|e| e.into_inner()) = Some(wake);
}

fn new_frame_shown() {
    if let Some(wake) = PREVIEW.lock().unwrap_or_else(|e| e.into_inner()).take() {
        wake.request_repaint();
    }
}

/// Remembers the program in front unless it's Foxglove itself, so "Add foreground app" can
/// offer the game the user just left for the settings window.
fn note_foreground() {
    let Some(exe) = foxglove_panel::providers::foreground_exe().map(|e| e.to_lowercase()) else {
        return;
    };
    let own = std::env::current_exe()
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_lowercase()));
    if own.as_deref() != Some(exe.as_str()) {
        update_snapshot(|s| s.foreground = Some(exe));
    }
}

fn set_status(s: impl Into<String>) {
    let s = s.into();
    let mut cur = STATUS.lock().unwrap_or_else(|e| e.into_inner());
    if *cur != s {
        eprintln!("Foxglove screens: {s}");
        *cur = s;
    }
}

pub fn start() {
    let _ = super::services::spawn_supervised("screens", run);
}

/// Hands the panel back to the firmware before the app exits (waits up to half a second), so
/// the left screen doesn't keep Foxglove's last frame.
pub fn stop() {
    STOP.store(true, Ordering::Relaxed);
    wake();
    let end = Instant::now() + Duration::from_millis(500);
    while !STOPPED.load(Ordering::Relaxed) && Instant::now() < end {
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// The keyboard being drawn on and the panel's state there.
struct Drawing {
    link: Arc<Link>,
    hello: Hello,
    generation: u64,
    panel: Panel,
    last: Option<Vec<u8>>,
    last_sent: Instant,
}

impl Drawing {
    fn new(link: Arc<Link>, hello: Hello) -> Self {
        let generation = link.state().generation;
        // Keyboards that describe their screens (#160) say where the PC may draw; older ones
        // have the one panel HELLO reports.
        let screens = if hello.capabilities & protocol::capabilities::SCREENS != 0 {
            panel::screens(link.as_ref()).unwrap_or_default()
        } else {
            vec![]
        };
        // Its own descriptors first, then its board profile (#75), then HELLO.
        let profile = super::boards::current().and_then(|b| {
            b.panel()
                .map(|p| (usize::from(p.width), usize::from(p.height)))
        });
        let (width, height) = panel::drawable_panel(&screens)
            .map(|p| (usize::from(p.width), usize::from(p.height)))
            .or(profile.filter(|_| hello.width == 0 || hello.height == 0))
            .unwrap_or((hello.width, hello.height));
        update_snapshot(|s| s.screens = screens);
        Drawing {
            link,
            hello,
            generation,
            panel: Panel::new(width, height),
            last: None,
            last_sent: Instant::now(),
        }
    }
}

/// Hands the panel back to the firmware and lets another app have it.
fn stop_driving(lock: &mut PanelLock, drawing: &Option<Drawing>, was_driving: &mut bool) {
    if *was_driving {
        if let Some(d) = drawing {
            let _ = panel::release(d.link.as_ref());
        }
    }
    lock.release();
    *was_driving = false;
}

struct WatchedLink {
    link: Arc<Link>,
    _state: EventSubscription,
    _reports: EventSubscription,
    hello: Option<(u64, Hello)>,
    checked_generation: Option<u64>,
}

enum Event {
    Wake,
    Providers,
    PanelAvailable,
    Settings,
    Available(Arc<Link>),
    Report(usize, u64, digitalis::link::Report),
}
static EVENTS: OnceLock<Mutex<Option<mpsc::Sender<Event>>>> = OnceLock::new();

/// What the panel's providers read, as facts for plugins (`facts.read:media.*`,
/// `facts.read:volume.*`): one reader of the media sessions for the whole app. Plugins can't
/// read them themselves, as the sandbox denies SMTC (#215). Unchanged values aren't re-sent.
fn publish_media(view: &render::View) {
    use foxglove_devices::facts::Value;
    let facts = super::devices::bus().facts();
    let text = |key: &str, v: &Option<String>| match v {
        Some(t) => facts.set(key, Value::Text(t.clone())),
        None => {
            facts.remove(key);
        }
    };
    text("media.title", &view.title);
    text("media.artist", &view.artist);
    facts.set("media.playing", Value::Bool(view.playing));
    match view.volume {
        Some((level, muted)) => {
            facts.set("volume.level", Value::Int(i64::from(level)));
            facts.set("volume.muted", Value::Bool(muted));
        }
        None => facts.remove_prefix("volume.", &[]),
    }
}

static WALLPAPER_CHANGED: AtomicBool = AtomicBool::new(false);

/// Wallpaper Engine shows another wallpaper: reload its frames on the next draw.
pub fn wallpaper_changed() {
    WALLPAPER_CHANGED.store(true, Ordering::Relaxed);
    wake();
}

/// The current wallpaper's left frames (in playing order) and fps, for a panel of
/// `frame_bytes`; `None` falls back to the info layout.
fn load_wallpaper_frames(frame_bytes: usize) -> Option<(Vec<Vec<u8>>, u32)> {
    let monitor = super::art_cycle::Cycle::load().wallpaper_monitor;
    let w = super::wallpaper::current(monitor.as_deref())?;
    let left = w.left.as_ref()?;
    Some((super::wallpaper::left_frames(left, frame_bytes)?, left.fps))
}

/// A plugin's frame over the left panel (`screens.panel_override`, #209), until `until`.
struct Override {
    bits: Vec<u8>,
    /// `None`: held until replaced, released, or the connection closes.
    until: Option<Instant>,
    conn: u64,
}

static OVERRIDE: Mutex<Option<Override>> = Mutex::new(None);
/// The panel's size, once a keyboard with one has connected.
static PANEL_SIZE: Mutex<Option<(u16, u16)>> = Mutex::new(None);

/// The longest a plugin's timed frame lasts.
pub const MAX_OVERRIDE: Duration = Duration::from_secs(60);

/// Shows a plugin's frame (1 bpp, rows MSB first, padded to whole bytes) for `ttl`, or with no
/// `ttl` until it's replaced, released or the plugin's connection closes (#281).
pub fn set_override(
    conn: u64,
    width: u16,
    height: u16,
    bits: Vec<u8>,
    ttl: Option<Duration>,
) -> Result<(), String> {
    let size = *PANEL_SIZE.lock().unwrap_or_else(|e| e.into_inner());
    let Some((w, h)) = size else {
        return Err("no left panel connected".into());
    };
    if (width, height) != (w, h) {
        return Err(format!("the panel is {w}x{h}, not {width}x{height}"));
    }
    let want = usize::from(w).div_ceil(8) * usize::from(h);
    if bits.len() != want {
        return Err(format!(
            "a {w}x{h} frame is {want} bytes, not {}",
            bits.len()
        ));
    }
    let until = ttl.map(|t| Instant::now() + t.min(MAX_OVERRIDE));
    *OVERRIDE.lock().unwrap_or_else(|e| e.into_inner()) = Some(Override { bits, until, conn });
    wake();
    Ok(())
}

/// Drops a connection's frame (released, or the plugin stopped).
pub fn clear_override(conn: u64) {
    let mut o = OVERRIDE.lock().unwrap_or_else(|e| e.into_inner());
    if o.as_ref().is_some_and(|o| o.conn == conn) {
        *o = None;
        drop(o);
        wake();
    }
}

/// The current plugin frame and when it ends (`None`: held), if one is showing.
fn current_override(now: Instant) -> Option<(Vec<u8>, Option<Instant>)> {
    let mut o = OVERRIDE.lock().unwrap_or_else(|e| e.into_inner());
    if o.as_ref()
        .is_some_and(|o| o.until.is_some_and(|u| u <= now))
    {
        *o = None;
    }
    o.as_ref().map(|o| (o.bits.clone(), o.until))
}

fn wake() {
    if let Some(tx) = EVENTS.get().and_then(|s| s.lock().ok()) {
        if let Some(tx) = tx.as_ref() {
            let _ = tx.send(Event::Wake);
        }
    }
}

/// Zero means firmware keeps the frame forever. Leave queue/USB scheduling headroom.
/// How often the panel needs a PRESENT to stay ours: never when the firmware holds it
/// (PANEL_HOLD, or no timeout); otherwise just inside its timeout. For older firmware it starts
/// with drawing and stops with RELEASE or the link going (#279).
fn keep_alive_for(hello: &Hello) -> Option<Duration> {
    if hello.capabilities & protocol::capabilities::PANEL_HOLD != 0 {
        return None;
    }
    keep_alive(hello.panel_timeout_s)
}

fn keep_alive(timeout_s: u16) -> Option<Duration> {
    (timeout_s > 0).then(|| {
        let timeout = Duration::from_secs(u64::from(timeout_s));
        timeout - Duration::from_secs(1).min(timeout / 10)
    })
}

/// Replies that mean the keyboard (re)announced itself, after a replug or an endpoint change
/// (#350): HELLO, or the firmware's unsolicited CONN_STATE. What it shows may be gone or never
/// have reached the screen, so the whole panel is sent again.
fn reannounced(r: &[u8]) -> bool {
    protocol::is_reply(r, protocol::PANEL, protocol::panel::HELLO)
        || protocol::is_reply(r, protocol::SETTINGS, protocol::settings::CONN_STATE)
}

fn changed(last: Option<&[u8]>, frame: &[u8]) -> bool {
    last != Some(frame)
}

fn deadline(
    clock: Duration,
    animation: Option<Duration>,
    keep_alive: Option<Duration>,
) -> Duration {
    [Some(clock), animation, keep_alive]
        .into_iter()
        .flatten()
        .min()
        .unwrap()
}

/// The PC load layout's sampler (#506): made when the layout starts drawing (its first reading a
/// second later, as rates need two collections), read about once a second while drawn, and
/// dropped, closing its counters, the moment it isn't.
struct Load<S: Sample> {
    sampler: S,
    next: Instant,
    stats: Stats,
}

const LOAD_EVERY: Duration = Duration::from_secs(1);

impl<S: Sample> Load<S> {
    fn new(sampler: S, now: Instant) -> Self {
        Load {
            sampler,
            next: now + LOAD_EVERY,
            stats: Stats::default(),
        }
    }

    /// Samples when a second has passed; a late wake doesn't sample twice to catch up.
    fn tick(&mut self, now: Instant) {
        if now >= self.next {
            self.stats = self.sampler.sample();
            self.next = now + LOAD_EVERY;
        }
    }
}

/// The sampler while the PC load layout is `shown` (made on first need); dropped when it isn't.
fn load_shown<S: Sample>(
    load: &mut Option<Load<S>>,
    shown: bool,
    now: Instant,
    make: impl FnOnce() -> S,
) -> Option<&mut Load<S>> {
    if !shown {
        *load = None;
        return None;
    }
    Some(load.get_or_insert_with(|| Load::new(make(), now)))
}

fn run() {
    let (tx, rx) = mpsc::channel();
    *EVENTS.get_or_init(Default::default).lock().unwrap() = Some(tx.clone());
    STOPPED.store(false, Ordering::Relaxed);
    let mut sources = Sources::new();
    let providers_tx = tx.clone();
    // FOXGLOVE_DISABLE=providers: clock only, no Windows media/volume/foreground watchers.
    let mut changes = (!super::repaint_trace::disabled("providers")).then(|| {
        foxglove_panel::changes::Changes::new(Arc::new(move || {
            let _ = providers_tx.send(Event::Providers);
        }))
    });
    let mut providers_dirty = true;
    let settings_tx = tx.clone();
    let _settings_watch = ScreensSettings::path().and_then(|p| {
        super::screens_watch::settings(
            &p,
            Arc::new(move || {
                let _ = settings_tx.send(Event::Settings);
            }),
        )
    });
    let mut settings = ScreensSettings::load();
    // Locked or display off (#410): the panel goes back to the firmware until we're back.
    let away_tx = tx.clone();
    let _away = super::away::subscribe(move |_| {
        let _ = away_tx.send(Event::Wake);
    });
    let available = tx.clone();
    let _subscription = super::link::subscribe_links(move |link| {
        let _ = available.send(Event::Available(link));
    });
    let _ = super::link::discover_links();
    let mut links: Vec<WatchedLink> = Vec::new();
    let mut lock = PanelLock::new();
    let mut lock_watch = None;
    let mut view = View::default();
    let mut drawing: Option<Drawing> = None;
    let mut was_driving = false;
    let mut last_battery = None;
    let mut tick: u32 = 0;
    let mut wallpaper_frames: Option<Option<(Vec<Vec<u8>>, u32)>> = None;
    let mut wallpaper_tick: usize = 0;
    let mut next_wallpaper_frame = Instant::now();
    let mut next_frame = Instant::now();
    let mut wait = None;
    // The PC load layout's counters (#506), open only while it's drawn.
    let mut load: Option<Load<Sampler>> = None;

    while !STOP.load(Ordering::Relaxed) {
        let event = match wait {
            Some(delay) => rx.recv_timeout(delay).ok(),
            None => rx.recv().ok(),
        };
        if STOP.load(Ordering::Relaxed) {
            break;
        }
        match event {
            Some(Event::Providers) => providers_dirty = true,
            Some(Event::PanelAvailable) => lock_watch = None,
            Some(Event::Settings) => {
                // Keep current settings while an external writer has the file locked.
                if let Some(s) = ScreensSettings::try_load() {
                    providers_dirty |= s != settings;
                    settings = s;
                }
            }
            Some(Event::Available(link)) => {
                if links.iter().any(|l| Arc::ptr_eq(&l.link, &link)) {
                    continue;
                }
                let id = links.len();
                let events = tx.clone();
                let subscription = link.subscribe_events(move |_| {
                    let _ = events.send(Event::Wake);
                });
                let reports = tx.clone();
                let layer = std::cell::Cell::new(None);
                let tap = link.subscribe_reports(move |generation, r| {
                    if r[0] == 0xff {
                        if layer.get() == Some((generation, *r)) {
                            return;
                        }
                        layer.set(Some((generation, *r)));
                    }
                    if r[0] == 0xff
                        || reannounced(r)
                        || protocol::is_reply(r, protocol::ART, protocol::art::STATUS)
                    {
                        let _ = reports.send(Event::Report(id, generation, *r));
                    }
                });
                links.push(WatchedLink {
                    link,
                    _state: subscription,
                    _reports: tap,
                    hello: None,
                    checked_generation: None,
                });
            }
            Some(Event::Report(id, generation, report))
                if protocol::is_reply(&report, protocol::PANEL, protocol::panel::HELLO) =>
            {
                let hello = Hello::parse(&report);
                if let Some(watched) = links.get_mut(id) {
                    let state = watched.link.state();
                    if state.generation == generation && state.connected.is_some() {
                        watched.hello = Some((generation, hello));
                        if let Some(d) = drawing.as_mut() {
                            if Arc::ptr_eq(&watched.link, &d.link) && generation == d.generation {
                                d.hello = hello;
                                d.last = None; // #350: draw the whole panel again
                                if was_driving {
                                    battery_log(hello.right_battery, &mut last_battery);
                                }
                            }
                        }
                    }
                }
            }
            Some(Event::Report(id, generation, report)) if reannounced(&report) => {
                // CONN_STATE (HELLO is above): the keyboard changed endpoint or reconnected.
                if let (Some(watched), Some(d)) = (links.get(id), drawing.as_mut()) {
                    if Arc::ptr_eq(&watched.link, &d.link) && generation == d.generation {
                        d.last = None;
                    }
                }
            }
            _ => {}
        }
        wait = None;
        let shown = drawing
            .as_ref()
            .map(|d| (d.link.clone(), d.hello))
            .or_else(|| {
                links.iter().find_map(|w| {
                    let state = w.link.state();
                    let (generation, hello) = w.hello?;
                    (state.connected.is_some() && generation == state.generation)
                        .then(|| (w.link.clone(), hello))
                })
            });
        update_snapshot(|s| {
            s.product = shown
                .as_ref()
                .and_then(|(l, _)| l.state().connected.map(|c| c.product))
                .unwrap_or_default();
            s.hello = shown.map(|(_, h)| h);
        });
        let foxglove_paused = super::pause::paused();
        let away = super::away::away();
        if paused() || foxglove_paused || away || !settings.drive_left {
            // Not drawing: no PC load sampling either (#506).
            load = None;
            stop_driving(&mut lock, &drawing, &mut was_driving);
            if let Some(d) = drawing.as_mut() {
                d.last = None;
            }
            lock_watch = None;
            set_status(if foxglove_paused {
                "Foxglove paused"
            } else if away {
                "Away (locked or display off): the keyboard shows its own screen"
            } else if paused() {
                "Left screen paused"
            } else {
                "Left screen off"
            });
            continue;
        }
        if drawing.as_ref().is_some_and(|d| {
            let state = d.link.state();
            state.connected.is_none() || state.generation != d.generation
        }) {
            stop_driving(&mut lock, &drawing, &mut was_driving);
            drawing = None;
        }
        if drawing.is_none() {
            drawing = links.iter_mut().find_map(|watched| {
                let state = watched.link.state();
                state.connected.as_ref()?;
                let usable = |(generation, h): &(u64, Hello)| {
                    *generation == state.generation
                        && (1..=256).contains(&h.width)
                        && (1..=256).contains(&h.height)
                };
                if !watched.hello.as_ref().is_some_and(usable)
                    && watched.checked_generation != Some(state.generation)
                {
                    // Probe a missing capability once per connection, never on a retry timer.
                    watched.checked_generation = Some(state.generation);
                    watched.hello =
                        super::link::ask_hello(&watched.link).map(|h| (state.generation, h));
                }
                let (_, hello) = watched.hello.filter(usable)?;
                Some(Drawing::new(watched.link.clone(), hello))
            });
        }
        let Some(d) = drawing.as_mut() else {
            load = None;
            set_status("Waiting for a keyboard with the host link");
            continue;
        };
        if !lock.acquire() {
            load = None;
            set_status("Panel driven by another app");
            if lock_watch.is_none() {
                let available = tx.clone();
                lock_watch = super::screens_watch::panel(Arc::new(move || {
                    let _ = available.send(Event::PanelAvailable);
                }));
            }
            continue;
        }
        lock_watch = None;
        if !was_driving {
            d.last = None;
            set_status("Drawing the left screen");
            battery_log(d.hello.right_battery, &mut last_battery);
        }
        was_driving = true;
        let clock_at = if let Some(changes) = changes.as_mut().filter(|_| providers_dirty) {
            changes.refresh();
            providers_dirty = false;
            note_foreground();
            let next = sources.update(&mut view, |exe| settings.games.get(exe).cloned());
            publish_media(&view);
            next
        } else {
            Sources::update_clock(&mut view)
        };
        let now = Instant::now();
        let period = Duration::from_millis(1000 / settings.fps.clamp(1, 10) as u64);
        let animated = render::animated(&view, d.panel.width);
        if animated && now >= next_frame {
            tick = tick.wrapping_add(1);
            next_frame = now + period;
        }
        // PC load (#506): sampled only here, while that layout is drawn.
        let mut animated = animated;
        let shown = settings.layout == super::screens_settings::Layout::Stats;
        if let Some(l) = load_shown(&mut load, shown, now, Sampler::new) {
            l.tick(now);
            render::draw_stats(&mut d.panel, &l.stats);
            animated = false;
        } else {
            render::draw(&mut d.panel, &view, tick);
        }
        let clock_at = load.as_ref().map_or(clock_at, |l| clock_at.min(l.next));
        // The wallpaper's own frames, when that's the layout and the wallpaper has them.
        if settings.layout == super::screens_settings::Layout::Wallpaper {
            if wallpaper_frames.is_none() || WALLPAPER_CHANGED.swap(false, Ordering::Relaxed) {
                wallpaper_frames = Some(load_wallpaper_frames(d.panel.bits.len()));
            }
            if let Some(Some((frames, fps))) = &wallpaper_frames {
                if frames.len() > 1 && *fps > 0 {
                    animated = true;
                    if now >= next_wallpaper_frame {
                        wallpaper_tick = wallpaper_tick.wrapping_add(1);
                        next_wallpaper_frame = now + Duration::from_millis(1000 / u64::from(*fps));
                    }
                }
                d.panel
                    .bits
                    .clone_from(&frames[wallpaper_tick % frames.len()]);
            }
        } else {
            wallpaper_frames = None;
        }
        // A plugin's frame, over everything else, until it lapses.
        *PANEL_SIZE.lock().unwrap_or_else(|e| e.into_inner()) =
            Some((d.panel.width as u16, d.panel.height as u16));
        let overridden = current_override(now).filter(|(bits, _)| bits.len() == d.panel.bits.len());
        if let Some((bits, _)) = &overridden {
            d.panel.bits.clone_from(bits);
        }
        let clock_at = overridden
            .as_ref()
            .and_then(|(_, until)| *until)
            .map_or(clock_at, |until| clock_at.min(until));
        let interval = keep_alive_for(&d.hello);
        let result = if changed(d.last.as_deref(), &d.panel.bits) {
            panel::show(
                d.link.as_ref(),
                &d.panel.bits,
                d.last.as_deref(),
                d.panel.stride,
            )
            .map(|_| ())
        } else if interval.is_some_and(|i| d.last_sent.elapsed() >= i) {
            // PRESENT alone refreshes timeout_work without transferring any rows.
            d.link.send(&protocol::present())
        } else {
            wait = Some(deadline(
                clock_at.saturating_duration_since(Instant::now()),
                animated.then(|| {
                    next_frame
                        .min(next_wallpaper_frame)
                        .saturating_duration_since(Instant::now())
                }),
                interval.map(|i| i.saturating_sub(d.last_sent.elapsed())),
            ));
            continue;
        };
        match result {
            Ok(()) => {
                d.last = Some(d.panel.bits.clone());
                d.last_sent = Instant::now();
                let frame = Frame {
                    width: d.panel.width,
                    height: d.panel.height,
                    stride: d.panel.stride,
                    bits: d.panel.bits.clone(),
                };
                dump_panel(&frame);
                update_snapshot(|s| s.frame = Some(frame));
                new_frame_shown();
            }
            Err(e) => {
                set_status(format!("Lost keyboard: {e}"));
                lock.release();
                was_driving = false;
                d.last = None;
                continue;
            }
        }
        wait = Some(deadline(
            clock_at.saturating_duration_since(Instant::now()),
            animated.then(|| next_frame.saturating_duration_since(Instant::now())),
            interval,
        ));
    }
    stop_driving(&mut lock, &drawing, &mut was_driving);
    // RELEASE is queued to the shared writer; allow it to leave before app shutdown.
    std::thread::sleep(Duration::from_millis(50));
    *EVENTS.get().unwrap().lock().unwrap() = None;
    STOPPED.store(true, Ordering::Relaxed);
}

/// `FOXGLOVE_DUMP_PANEL=<path>`: writes every frame sent as a binary PBM (P4: 1 bit per pixel,
/// MSB first, rows padded to whole bytes, exactly the panel's own layout; 1 = ink), so the Test
/// lab can compare it with the keyboard's screen readback. Replaced atomically.
fn dump_panel(frame: &Frame) {
    static PATH: OnceLock<Option<std::path::PathBuf>> = OnceLock::new();
    let Some(path) = PATH.get_or_init(|| std::env::var_os("FOXGLOVE_DUMP_PANEL").map(Into::into))
    else {
        return;
    };
    let tmp = path.with_extension("tmp");
    let mut bytes = format!("P4\n{} {}\n", frame.width, frame.height).into_bytes();
    bytes.extend_from_slice(&frame.bits);
    if std::fs::write(&tmp, bytes)
        .and_then(|()| std::fs::rename(&tmp, path))
        .is_err()
    {
        eprintln!("Foxglove screens: can't write {}", path.display());
    }
}

/// Appends the right half's battery to `right-battery.csv` (next to the settings) when it changes.
fn battery_log(battery: Option<u8>, last: &mut Option<u8>) {
    use std::io::Write;

    let Some(percent) = battery.filter(|_| battery != *last) else {
        return;
    };
    let Some(path) = crate::settings::Settings::config_file_path()
        .map(|p| p.with_file_name("right-battery.csv"))
    else {
        return;
    };
    let new = !path.exists();
    let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    else {
        return;
    };
    let header_ok = !new || writeln!(f, "time,right_battery_percent").is_ok();
    if header_ok && writeln!(f, "{},{percent}", foxglove_panel::local_timestamp()).is_ok() {
        *last = battery;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn frames_change_only_after_pixels_change_or_ownership_resets() {
        assert!(changed(None, &[1, 2]));
        assert!(!changed(Some(&[1, 2]), &[1, 2]));
        assert!(changed(Some(&[1, 2]), &[1, 3]));
    }
    #[test]
    fn hello_and_conn_state_replies_redraw_the_whole_panel() {
        let mut hello = [0u8; 32];
        hello[..2].copy_from_slice(&[protocol::PANEL, 0x80 | protocol::panel::HELLO]);
        let mut conn = [0u8; 32];
        conn[..2].copy_from_slice(&[protocol::SETTINGS, 0x80 | protocol::settings::CONN_STATE]);
        assert!(reannounced(&hello) && reannounced(&conn));
        let mut layer = [0u8; 32];
        layer[0] = 0xff;
        assert!(
            !reannounced(&layer),
            "a layer report is not a reannouncement"
        );
        assert!(
            !reannounced(&protocol::hello()),
            "the request itself is not a reply"
        );
    }
    #[test]
    fn sleeps_until_the_first_visible_change_or_firmware_deadline() {
        let minute = Duration::from_secs(47);
        assert_eq!(keep_alive(0), None);
        let mut hello = Hello {
            panel_timeout_s: 10,
            ..Hello::default()
        };
        assert_eq!(
            keep_alive_for(&hello),
            Some(Duration::from_secs(9)),
            "older firmware"
        );
        hello.capabilities |= protocol::capabilities::PANEL_HOLD;
        assert_eq!(keep_alive_for(&hello), None, "the firmware holds the panel");
        assert_eq!(deadline(minute, None, None), minute);
        assert_eq!(
            deadline(minute, Some(Duration::from_millis(250)), None),
            Duration::from_millis(250)
        );
        assert_eq!(
            deadline(minute, None, keep_alive(10)),
            Duration::from_secs(9)
        );
        assert_eq!(keep_alive(1), Some(Duration::from_millis(900)));
        assert_eq!(keep_alive(3600), Some(Duration::from_secs(3599)));
        assert_eq!(
            deadline(Duration::ZERO, Some(Duration::from_secs(1)), None),
            Duration::ZERO
        );
    }

    /// A fake counter source: how often it was read.
    struct Counting(std::rc::Rc<std::cell::Cell<u32>>);

    impl Sample for Counting {
        fn sample(&mut self) -> Stats {
            self.0.set(self.0.get() + 1);
            Stats {
                cpu: Some(self.0.get() as u8),
                gpu: Some(1),
                ram: Some(50),
            }
        }
    }

    /// #506: nothing is opened or read while the PC load layout isn't on the screen; shown, a
    /// reading a second, never two to catch up; hidden again, the counters are closed.
    #[test]
    fn pc_load_is_sampled_only_while_shown() {
        let reads = std::rc::Rc::new(std::cell::Cell::new(0));
        let made = std::cell::Cell::new(0);
        let make = || {
            made.set(made.get() + 1);
            Counting(reads.clone())
        };
        let t = Instant::now();
        let mut load = None;
        // Another layout, for a long while: no sampler, no reads.
        for s in 0..100 {
            assert!(load_shown(&mut load, false, t + Duration::from_secs(s), make).is_none());
        }
        assert_eq!((made.get(), reads.get()), (0, 0));
        // Shown: made once, first read a second later.
        load_shown(&mut load, true, t, make).unwrap().tick(t);
        assert_eq!((made.get(), reads.get()), (1, 0));
        let half = t + Duration::from_millis(500);
        load_shown(&mut load, true, half, make).unwrap().tick(half);
        assert_eq!(reads.get(), 0);
        let one = t + LOAD_EVERY;
        let l = load_shown(&mut load, true, one, make).unwrap();
        l.tick(one);
        assert_eq!(reads.get(), 1);
        assert_eq!(l.stats.cpu, Some(1));
        assert_eq!(l.next, one + LOAD_EVERY);
        // Woken late (after a long frame): one read, not five.
        let late = one + Duration::from_secs(5);
        load_shown(&mut load, true, late, make).unwrap().tick(late);
        assert_eq!(reads.get(), 2);
        // Hidden (another layout, paused, away or no keyboard): dropped.
        assert!(load_shown(&mut load, false, late, make).is_none());
        assert!(load.is_none());
        assert_eq!(made.get(), 1);
    }
}
