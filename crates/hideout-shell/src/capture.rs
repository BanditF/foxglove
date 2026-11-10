// SPDX-License-Identifier: MIT
//! Lab capture of the hub's own frames (#1002).
//!
//! Why: the hub draws with OpenGL (eframe's glow backend), and `PrintWindow` / `BitBlt` of its
//! window give a blank image, so screenshots for design reviews and lab proofs came out empty.
//! Here the hub reads back the frame it has just drawn and writes it as a PNG.
//!
//! How: `hideout.exe --capture <out.png>` (see [`request`]) connects to the lab hub's capture
//! pipe and sends one line. The pipe thread hands a [`Job`] to the hub and wakes it; the next
//! frame adds one paint callback on the topmost layer of the main window and, if it is open, of
//! the tray flyout. The callback runs after everything else is drawn, reads the back buffer
//! (`Painter::read_screen_rgba`, what eframe's own `ViewportCommand::Screenshot` does), and
//! sends the image to the pipe thread, which writes the PNGs ([`write_png`]) and answers.
//! `ViewportCommand::Screenshot` itself isn't used: eframe 0.33's glow backend drops it for
//! immediate viewports (the flyout), and its answer only arrives with a later frame.
//!
//! Cost: off unless the hub runs on a lab folder (`HIDEOUT_CONFIG_DIR` or `HIDEOUT_HOME`) with
//! `HIDEOUT_LAB_CAPTURE=1` ([`enabled`]); the user's hub starts no thread and no pipe. When on,
//! one thread waits in `ConnectNamedPipe` (a kernel wait), and a frame only checks an empty slot.
//! A capture costs exactly one frame.

use crate::paths::HubPaths;
use eframe::egui;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The switch, on top of a lab folder.
pub const ENV: &str = "HIDEOUT_LAB_CAPTURE";

/// How long the pipe thread waits for the frame that answers a capture (fallback only: the
/// frame normally comes within milliseconds of the wake).
pub const FRAME_TIMEOUT: Duration = Duration::from_secs(5);

/// The longest request line read.
const MAX_LINE: u64 = 4096;

/// Whether this hub serves captures: a lab folder and `HIDEOUT_LAB_CAPTURE=1`. Never for the
/// default folder, whatever the environment says.
pub fn enabled(paths: &HubPaths, env: impl Fn(&str) -> Option<std::ffi::OsString>) -> bool {
    paths.overridden && env(ENV).is_some_and(|v| v == "1")
}

/// `\\.\pipe\hideout-capture-<user><suffix>`: one per user and lab folder.
pub fn pipe_name(paths: &HubPaths) -> String {
    let user: String = std::env::var("USERNAME")
        .unwrap_or_default()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect();
    format!(
        r"\\.\pipe\hideout-capture-{user}{}",
        paths.instance_suffix()
    )
}

/// Which window a frame came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Window {
    Main,
    Flyout,
}

impl Window {
    pub fn name(self) -> &'static str {
        match self {
            Window::Main => "main",
            Window::Flyout => "flyout",
        }
    }
}

/// One read-back frame.
pub struct Shot {
    pub window: Window,
    pub image: egui::ColorImage,
}

/// A capture the hub's next frame answers.
pub struct Job {
    /// Read the main window back (it is on screen).
    pub main: bool,
    /// Read the flyout back (it is open).
    pub flyout: bool,
    pub shots: Sender<Shot>,
}

/// Where the pipe thread leaves a job for the next frame.
#[derive(Default)]
pub struct Slot(Mutex<Option<Job>>);

impl Slot {
    /// The waiting job, if any (once).
    pub fn take(&self) -> Option<Job> {
        self.0.lock().ok().and_then(|mut j| j.take())
    }

    /// Leaves `job` for the next frame (replacing one no frame took).
    pub fn put(&self, job: Job) {
        if let Ok(mut j) = self.0.lock() {
            *j = Some(job);
        }
    }
}

/// Adds the read-back to the frame being built in `ctx` (the main window's, or the flyout's
/// inside its viewport). Drawn last, on the topmost layer, so it sees the finished frame.
/// Headless contexts (tests) never paint callbacks; there it is only a shape in the output.
pub fn queue_readback(ctx: &egui::Context, window: Window, tx: Sender<Shot>) {
    let tx = Mutex::new(tx);
    let read = eframe::egui_glow::CallbackFn::new(move |info, painter| {
        let image = painter.read_screen_rgba(info.screen_size_px);
        if let Ok(tx) = tx.lock() {
            let _ = tx.send(Shot { window, image });
        }
    });
    let layer = egui::LayerId::new(egui::Order::Debug, egui::Id::new("hideout.capture"));
    ctx.layer_painter(layer).add(egui::PaintCallback {
        rect: ctx.viewport_rect(),
        callback: Arc::new(read),
    });
}

/// Writes `image` as an opaque RGBA PNG (the window's alpha channel is meaningless), through a
/// temporary file, so a reader never sees half a PNG.
pub fn write_png(path: &Path, image: &egui::ColorImage) -> io::Result<()> {
    let [w, h] = image.size;
    let mut rgba = Vec::with_capacity(w * h * 4);
    for p in &image.pixels {
        rgba.extend_from_slice(&[p.r(), p.g(), p.b(), 255]);
    }
    let tmp = path.with_extension("png.tmp");
    {
        let file = io::BufWriter::new(std::fs::File::create(&tmp)?);
        let mut enc = png::Encoder::new(file, w as u32, h as u32);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut writer = enc.write_header().map_err(io::Error::other)?;
        writer.write_image_data(&rgba).map_err(io::Error::other)?;
        writer.finish().map_err(io::Error::other)?;
    }
    std::fs::rename(&tmp, path)
}

/// The file each window goes to: the main window to `out`, the flyout beside it as
/// `<stem>-flyout.png`.
pub fn out_path(out: &Path, window: Window) -> PathBuf {
    match window {
        Window::Main => out.to_path_buf(),
        Window::Flyout => {
            let stem = out.file_stem().unwrap_or_default().to_string_lossy();
            out.with_file_name(format!("{stem}-flyout.png"))
        }
    }
}

/// The request line's path: absolute and `.png`, else why not.
pub fn parse_request(line: &str) -> Result<PathBuf, String> {
    let path = line
        .trim_end_matches(['\r', '\n'])
        .strip_prefix("capture ")
        .ok_or("expected `capture <absolute path>.png`")?;
    let path = PathBuf::from(path);
    let png = path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("png"));
    if !path.is_absolute() || !png {
        return Err(format!("not an absolute .png path: {}", path.display()));
    }
    Ok(path)
}

/// Answers one capture request: queues the job, wakes the hub, writes what comes back. `on_screen`
/// says (main window on screen, flyout open). Every line of the answer starts `ok` or `error`.
pub fn answer(
    line: &str,
    slot: &Slot,
    on_screen: (bool, bool),
    wake: &dyn Fn(),
    timeout: Duration,
) -> String {
    let out = match parse_request(line) {
        Ok(p) => p,
        Err(e) => return format!("error {e}\n"),
    };
    let (main, flyout) = on_screen;
    if !main && !flyout {
        return "error nothing on screen: the window is hidden or minimised and the flyout is \
                closed (run hideout.exe on the same folder to show the window)\n"
            .into();
    }
    let (tx, rx) = mpsc::channel();
    slot.put(Job {
        main,
        flyout,
        shots: tx,
    });
    wake();
    let mut reply = collect(&rx, &out, main, flyout, timeout);
    // A job no frame took (the window went away) must not linger for a later frame.
    drop(slot.take());
    if reply.is_empty() {
        reply.push_str("error no frame\n");
    }
    reply
}

fn collect(rx: &Receiver<Shot>, out: &Path, main: bool, flyout: bool, timeout: Duration) -> String {
    let mut want: Vec<Window> = Vec::new();
    if main {
        want.push(Window::Main);
    }
    if flyout {
        want.push(Window::Flyout);
    }
    let deadline = Instant::now() + timeout;
    let mut reply = String::new();
    while !want.is_empty() {
        let left = deadline.saturating_duration_since(Instant::now());
        let Ok(shot) = rx.recv_timeout(left) else {
            break;
        };
        let Some(i) = want.iter().position(|w| *w == shot.window) else {
            continue;
        };
        want.remove(i);
        let path = out_path(out, shot.window);
        let [w, h] = shot.image.size;
        let name = shot.window.name();
        match write_png(&path, &shot.image) {
            Ok(()) => {
                log::info!("capture: {name} {w}x{h} to {}", path.display());
                reply.push_str(&format!("ok {name} {w}x{h} {}\n", path.display()));
            }
            Err(e) => reply.push_str(&format!("error {name}: {}: {e}\n", path.display())),
        }
    }
    for w in want {
        let s = timeout.as_secs();
        reply.push_str(&format!("error {} was not drawn within {s} s\n", w.name()));
    }
    reply
}

/// Serves captures on `name` from a thread of its own: one request per connection, one at a
/// time. Blocks in `ConnectNamedPipe` between requests (no polling). The pipe is current-user
/// only and refuses remote clients (`ServerPipe`).
pub fn serve(
    name: &str,
    slot: Arc<Slot>,
    on_screen: impl Fn() -> (bool, bool) + Send + 'static,
    wake: impl Fn() + Send + 'static,
) -> io::Result<()> {
    use foxglove_plugin::channel::ServerPipe;
    let first = ServerPipe::create(name, true)?;
    let name = name.to_string();
    std::thread::Builder::new()
        .name("hideout-capture".into())
        .spawn(move || {
            let mut next = Some(first);
            loop {
                let pipe = match next.take() {
                    Some(p) => p,
                    None => match ServerPipe::create(&name, false) {
                        Ok(p) => p,
                        Err(e) => {
                            log::error!("capture: {name}: {e}");
                            return;
                        }
                    },
                };
                let mut conn = match pipe.accept() {
                    Ok(c) => c,
                    Err(e) => {
                        log::warn!("capture: {name}: accept: {e}");
                        continue;
                    }
                };
                let mut line = String::new();
                let read = BufReader::new(conn.clone())
                    .take(MAX_LINE)
                    .read_line(&mut line);
                let reply = match read {
                    Ok(_) => answer(&line, &slot, on_screen(), &wake, FRAME_TIMEOUT),
                    Err(e) => format!("error reading the request: {e}\n"),
                };
                let _ = conn.write_all(reply.as_bytes());
            }
        })?;
    Ok(())
}

/// The client side (`hideout.exe --capture <out.png>`): asks the hub on `pipe` for a capture to
/// `out` and returns its answer. Waits for the hub's answer, which comes within
/// [`FRAME_TIMEOUT`] or as a closed pipe.
pub fn request(pipe: &str, out: &Path) -> io::Result<String> {
    let mut conn = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(pipe)?;
    conn.write_all(format!("capture {}\n", out.display()).as_bytes())?;
    let mut reply = String::new();
    BufReader::new(conn).read_to_string(&mut reply)?;
    Ok(reply)
}

/// Whether an answer reports every window written.
pub fn answer_ok(reply: &str) -> bool {
    !reply.is_empty() && reply.lines().all(|l| l.starts_with("ok "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;

    fn lab() -> HubPaths {
        HubPaths::resolve(|k| (k == "HIDEOUT_CONFIG_DIR").then(|| OsString::from(r"C:\lab\h")))
            .unwrap()
    }

    #[test]
    fn only_a_lab_folder_with_the_switch_serves_captures() {
        let on = |k: &str| (k == ENV).then(|| OsString::from("1"));
        let off = |_: &str| None;
        assert!(enabled(&lab(), on));
        assert!(!enabled(&lab(), off));
        assert!(!enabled(&lab(), |k: &str| (k == ENV).then(|| OsString::from("0"))));
        // The user's own hub never does, whatever the environment says.
        let user =
            HubPaths::resolve(|k| (k == "LOCALAPPDATA").then(|| OsString::from(r"C:\u"))).unwrap();
        assert!(!enabled(&user, on));
        assert!(pipe_name(&lab()).ends_with(&lab().instance_suffix()));
    }

    #[test]
    fn requests_need_an_absolute_png_path() {
        assert_eq!(
            parse_request("capture C:\\lab\\a.png\r\n").unwrap(),
            PathBuf::from(r"C:\lab\a.png")
        );
        assert!(parse_request("capture a.png\n").is_err());
        assert!(parse_request("capture C:\\lab\\a.bmp\n").is_err());
        assert!(parse_request("grab C:\\lab\\a.png\n").is_err());
        assert_eq!(
            out_path(Path::new(r"C:\lab\hub.png"), Window::Flyout),
            PathBuf::from(r"C:\lab\hub-flyout.png")
        );
    }

    #[test]
    fn nothing_on_screen_answers_at_once_without_a_frame() {
        let slot = Slot::default();
        let woke = std::cell::Cell::new(false);
        let r = answer(
            "capture C:\\x\\a.png\n",
            &slot,
            (false, false),
            &|| woke.set(true),
            FRAME_TIMEOUT,
        );
        assert!(r.starts_with("error nothing on screen"), "{r}");
        assert!(!woke.get() && slot.take().is_none());
    }

    #[test]
    fn a_window_that_never_draws_times_out_and_leaves_no_job() {
        let slot = Slot::default();
        let r = answer(
            "capture C:\\x\\a.png\n",
            &slot,
            (true, false),
            &|| {},
            Duration::from_millis(50),
        );
        assert_eq!(r, "error main was not drawn within 0 s\n");
        assert!(slot.take().is_none());
        assert!(!answer_ok(&r));
    }

    /// The whole path, window-free: the client, the current-user pipe, the job, a "frame" that
    /// answers with an image (as the paint callback does), the PNGs on disk.
    #[test]
    fn a_capture_round_trips_over_the_pipe() {
        let dir = std::env::temp_dir().join(format!("hideout-capture-rt-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let pipe = format!(r"\\.\pipe\hideout-capture-test-{}", std::process::id());
        let slot = Arc::new(Slot::default());
        let (wake_tx, wake_rx) = mpsc::channel::<()>();
        serve(
            &pipe,
            slot.clone(),
            || (true, true),
            move || {
                let _ = wake_tx.send(());
            },
        )
        .unwrap();
        // The hub's side: woken, it takes the job and its frame sends both windows back.
        let frame_slot = slot.clone();
        let frame = std::thread::spawn(move || {
            wake_rx.recv().unwrap();
            let job = frame_slot.take().expect("woken without a job");
            assert!(job.main && job.flyout);
            for (window, size, colour) in [
                (
                    Window::Main,
                    [6, 4],
                    egui::Color32::from_rgb(0xff, 0x8a, 0x2a),
                ),
                (Window::Flyout, [3, 5], egui::Color32::from_rgb(10, 20, 30)),
            ] {
                let image = egui::ColorImage::new(size, vec![colour; size[0] * size[1]]);
                job.shots.send(Shot { window, image }).unwrap();
            }
        });
        let out = dir.join("hub.png");
        let reply = request(&pipe, &out).unwrap();
        frame.join().unwrap();
        assert!(answer_ok(&reply), "{reply}");
        assert!(
            reply.contains("ok main 6x4") && reply.contains("ok flyout 3x5"),
            "{reply}"
        );
        let main = image::open(&out).unwrap().to_rgba8();
        assert_eq!(main.dimensions(), (6, 4));
        assert_eq!(main.get_pixel(5, 3).0, [0xff, 0x8a, 0x2a, 255]);
        let fly = image::open(dir.join("hub-flyout.png")).unwrap().to_rgba8();
        assert_eq!(fly.dimensions(), (3, 5));
        // A bad request is answered too, and the server keeps serving.
        let reply = request(&pipe, Path::new("relative.png")).unwrap();
        assert!(reply.starts_with("error not an absolute"), "{reply}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
