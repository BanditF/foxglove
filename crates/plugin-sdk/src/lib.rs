//! Write Foxglove plugins in Rust (docs/local-api.md).
//!
//! A Plugin is a program Foxglove starts; it talks to Foxglove on its stdin/stdout. [`Plugin::connect`]
//! says hello (`initialize`) and keeps the plugin alive with a `ping` every 10 s, so the runner
//! doesn't restart it. Calls are typed where it helps ([`Plugin::set_fact`], [`Plugin::set_lighting`],
//! ...) and generic otherwise ([`Plugin::call`]). Fact changes you subscribe to arrive on
//! [`Plugin::events`].
//!
//! ```no_run
//! use foxglove_plugin::{Plugin, Fact};
//! let plugin = Plugin::connect("weather", "1.0.0").unwrap();
//! plugin.set_fact("weather.temp", Some(Fact::Text("14°".into()))).unwrap();
//! ```

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub use foxglove_plugin_api::methods::NetKind;
pub use foxglove_plugin_api::methods::{AppWindows, LeaseParams, LeasePriority};
pub use foxglove_plugin_api::methods::{
    Device, Devices, Fact, FactChanged, FactList, Hsb, InitializeResult, LightingState,
    SetLightingParams,
};
use foxglove_plugin_api::rpc::{Error, Id, Outcome, Request, Response};

mod net;
pub use net::{NetEvent, TcpListener, TcpStream};
pub mod ws;

/// A watched app's window opening or closing (`Plugin::watch_app`): the program's file name,
/// and how many of its windows are open after it. `Closed { open: 0, .. }`: it has probably quit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AppWindowEvent {
    Appeared { exe: String, open: u32 },
    Closed { exe: String, open: u32 },
}
use serde::de::DeserializeOwned;
use serde_json::{json, Value};

/// Where a plugin draws on a keyboard (API 1.4, #560): one of its info screens, by id and size.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PanelTarget {
    /// `None`: the first info screen (an app before 1.4 takes no id).
    pub screen: Option<u8>,
    pub width: u16,
    pub height: u16,
}

impl PanelTarget {
    /// The one panel an app before API 1.4 draws: 64x52, no id.
    pub const BEFORE_1_4: PanelTarget = PanelTarget {
        screen: None,
        width: 64,
        height: 52,
    };

    /// A frame drawn at `w`x`h` (1 bpp, rows MSB first, padded to whole bytes), fitted to this
    /// screen: as it is at the same size, else scaled (nearest pixel, no stretching) to fit and
    /// centred, so a plugin's art shows on any info screen.
    pub fn fit(&self, frame: &[u8], w: u16, h: u16) -> Vec<u8> {
        let (tw, th) = (usize::from(self.width), usize::from(self.height));
        let (w, h) = (usize::from(w), usize::from(h));
        if (tw, th) == (w, h) {
            return frame.to_vec();
        }
        let (src_stride, stride) = (w.div_ceil(8), tw.div_ceil(8));
        let mut out = vec![0u8; stride * th];
        if w == 0 || h == 0 || tw == 0 || th == 0 {
            return out;
        }
        let scale = f64::min(tw as f64 / w as f64, th as f64 / h as f64);
        let (dw, dh) = (
            ((w as f64 * scale) as usize).clamp(1, tw),
            ((h as f64 * scale) as usize).clamp(1, th),
        );
        let (x0, y0) = ((tw - dw) / 2, (th - dh) / 2);
        for y in 0..dh {
            // Each target pixel takes the source pixel under its centre.
            let sy = ((2 * y + 1) * h / (2 * dh)).min(h - 1);
            for x in 0..dw {
                let sx = ((2 * x + 1) * w / (2 * dw)).min(w - 1);
                let ink = frame
                    .get(sy * src_stride + sx / 8)
                    .is_some_and(|b| b & (0x80 >> (sx % 8)) != 0);
                if ink {
                    let (ox, oy) = (x0 + x, y0 + y);
                    out[oy * stride + ox / 8] |= 0x80 >> (ox % 8);
                }
            }
        }
        out
    }
}

/// A call that failed: Foxglove's error (code and message), or the connection.
#[derive(Debug)]
pub enum PluginError {
    Api(Error),
    Closed,
    Decode(String),
    /// Foxglove didn't answer within [`CALL_TIMEOUT`].
    Timeout,
}

/// How long a call waits for Foxglove's answer; a one-shot deadline per call, not a timer.
pub const CALL_TIMEOUT: Duration = Duration::from_secs(30);

impl std::fmt::Display for PluginError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            PluginError::Api(e) => write!(f, "{} (code {})", e.message, e.code),
            PluginError::Closed => write!(f, "Foxglove closed the connection"),
            PluginError::Timeout => write!(f, "Foxglove didn't answer"),
            PluginError::Decode(e) => write!(f, "unexpected reply: {e}"),
        }
    }
}

impl std::error::Error for PluginError {}

pub type Result<T> = std::result::Result<T, PluginError>;

type Pending = Arc<Mutex<HashMap<i64, Sender<Outcome>>>>;

/// A connection to Foxglove.
pub struct Plugin {
    write: Mutex<Box<dyn Write + Send>>,
    next: AtomicI64,
    pending: Pending,
    events: Mutex<Option<Receiver<FactChanged>>>,
    actions: Mutex<Option<Receiver<String>>>,
    app_windows: Mutex<Option<Receiver<AppWindowEvent>>>,
    settings_changes: Mutex<Option<Receiver<serde_json::Map<String, Value>>>>,
    closed: Arc<std::sync::atomic::AtomicBool>,
    routes: net::Routes,
    /// What Foxglove granted this plugin.
    pub granted: Vec<String>,
}

impl Plugin {
    /// Connects over this process's stdin/stdout, as Foxglove starts plugins.
    pub fn connect(name: &str, version: &str) -> Result<Arc<Plugin>> {
        Plugin::connect_over(std::io::stdin(), std::io::stdout(), name, version)
    }

    /// Connects over any pair of streams (tests, other transports).
    pub fn connect_over(
        read: impl Read + Send + 'static,
        write: impl Write + Send + 'static,
        name: &str,
        version: &str,
    ) -> Result<Arc<Plugin>> {
        let pending: Pending = Arc::default();
        let (events_tx, events_rx) = mpsc::channel();
        let (actions_tx, actions_rx) = mpsc::channel::<String>();
        let (app_tx, app_rx) = mpsc::channel::<AppWindowEvent>();
        let (settings_tx, settings_rx) = mpsc::channel::<serde_json::Map<String, Value>>();
        let reader_pending = pending.clone();
        let closed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let reader_closed = closed.clone();
        let routes = net::Routes::default();
        let reader_routes = routes.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(read).lines() {
                let Ok(line) = line else { break };
                if let Ok(r) = serde_json::from_str::<Response>(&line) {
                    if let Id::Number(n) = r.id {
                        if let Some(tx) = reader_pending
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .remove(&n)
                        {
                            let _ = tx.send(r.outcome);
                        }
                    }
                } else if let Ok(n) = serde_json::from_str::<Request>(&line) {
                    if n.method == "facts.changed" {
                        if let Ok(ev) = serde_json::from_value(n.params) {
                            let _ = events_tx.send(ev);
                        }
                    } else if n.method == "action.invoked" {
                        if let Some(a) = n.params.get("action").and_then(Value::as_str) {
                            let _ = actions_tx.send(a.to_string());
                        }
                    } else if n.method == "settings.changed" {
                        if let Ok(s) = serde_json::from_value::<
                            foxglove_plugin_api::methods::SettingValues,
                        >(n.params)
                        {
                            let _ = settings_tx.send(s.values);
                        }
                    } else if n.method == "app.window.appeared" || n.method == "app.window.closed" {
                        if let Ok(w) = serde_json::from_value::<AppWindows>(n.params) {
                            let (exe, open) = (w.exe, w.open);
                            let _ = app_tx.send(if n.method == "app.window.appeared" {
                                AppWindowEvent::Appeared { exe, open }
                            } else {
                                AppWindowEvent::Closed { exe, open }
                            });
                        }
                    } else {
                        reader_routes.handle(&n.method, n.params);
                    }
                }
            }
            // Fail every waiting call once Foxglove is gone, and end every stream, so plugins
            // blocked on one wake up and can exit.
            reader_closed.store(true, Ordering::Relaxed);
            reader_pending
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clear();
            reader_routes.close_all("Foxglove went away");
        });
        let mut plugin = Plugin {
            write: Mutex::new(Box::new(write)),
            next: AtomicI64::new(0),
            pending,
            events: Mutex::new(Some(events_rx)),
            actions: Mutex::new(Some(actions_rx)),
            app_windows: Mutex::new(Some(app_rx)),
            settings_changes: Mutex::new(Some(settings_rx)),
            closed,
            routes,
            granted: vec![],
        };
        let init: InitializeResult = plugin.call_as(
            "initialize",
            json!({"api": foxglove_plugin_api::API_VERSION.to_string(), "client": {"name": name, "version": version}}),
        )?;
        plugin.granted = init.granted.into_iter().map(String::from).collect();
        // No heartbeat (#281): Foxglove sees an exit from the process itself, and the plugin
        // sees Foxglove go when its stdin ends.
        Ok(Arc::new(plugin))
    }

    /// Calls `method` and returns its result.
    pub fn call(&self, method: &str, params: Value) -> Result<Value> {
        let id = self.next.fetch_add(1, Ordering::Relaxed) + 1;
        let (tx, rx) = mpsc::channel();
        self.pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id, tx);
        let line = serde_json::to_string(&Request::call(id, method, params)).expect("serializable");
        {
            let mut w = self.write.lock().unwrap_or_else(|e| e.into_inner());
            writeln!(w, "{line}")
                .and_then(|()| w.flush())
                .map_err(|_| PluginError::Closed)?;
        }
        let outcome = match rx.recv_timeout(CALL_TIMEOUT) {
            Ok(o) => o,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                self.pending
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&id);
                return Err(PluginError::Timeout);
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => return Err(PluginError::Closed),
        };
        match outcome {
            Outcome::Result(v) => Ok(v),
            Outcome::Error(e) => Err(PluginError::Api(e)),
        }
    }

    fn call_as<T: DeserializeOwned>(&self, method: &str, params: Value) -> Result<T> {
        serde_json::from_value(self.call(method, params)?)
            .map_err(|e| PluginError::Decode(e.to_string()))
    }

    pub fn fact(&self, key: &str) -> Result<Option<Fact>> {
        let r: foxglove_plugin_api::methods::FactResult =
            self.call_as("facts.get", json!({ "key": key }))?;
        Ok(r.value)
    }

    /// Publishes a fact (`None` removes it). Needs `facts.write` covering the key.
    pub fn set_fact(&self, key: &str, value: Option<Fact>) -> Result<()> {
        self.call("facts.set", json!({ "key": key, "value": value }))
            .map(|_| ())
    }

    /// Subscribes to fact changes; they arrive on [`Plugin::events`].
    pub fn subscribe(&self, pattern: &str) -> Result<u64> {
        let s: foxglove_plugin_api::methods::Subscription =
            self.call_as("facts.subscribe", json!({ "pattern": pattern }))?;
        Ok(s.subscription)
    }

    /// The fact changes of every subscription (take it once).
    pub fn events(&self) -> Option<Receiver<FactChanged>> {
        self.events.lock().unwrap_or_else(|e| e.into_inner()).take()
    }

    /// Holds `device`'s lights at `color` until the returned lease is dropped or released, or
    /// the plugin stops (#236; needs `lighting.write`). The highest-priority lease shows; when
    /// the last one ends the lights go back as they were.
    pub fn lease(
        self: &Arc<Self>,
        device: &str,
        color: Hsb,
        priority: LeasePriority,
    ) -> Result<Lease> {
        let p = LeaseParams {
            device: device.into(),
            color,
            effect: None,
            priority,
        };
        let id: foxglove_plugin_api::methods::Lease = self.call_as(
            "lighting.lease",
            serde_json::to_value(p).expect("serializable"),
        )?;
        Ok(Lease {
            plugin: self.clone(),
            id: id.lease,
        })
    }

    /// A device's screens by role, id and size (API 1.4, #560). Needs `devices.read`.
    pub fn screens(&self, device: &str) -> Result<Vec<foxglove_plugin_api::methods::ScreenInfo>> {
        let s: foxglove_plugin_api::methods::Screens =
            self.call_as("screens.list", json!({ "device": device }))?;
        Ok(s.screens)
    }

    /// Where to draw on a device: its first info screen (API 1.4). An app before 1.4 has no
    /// `screens.list` and one 64x52 panel ([`PanelTarget::BEFORE_1_4`]); `None` when the device
    /// has no info screen, or it can't be asked.
    pub fn panel_target(&self, device: &str) -> Option<PanelTarget> {
        match self.screens(device) {
            Ok(screens) => screens
                .iter()
                .find(|s| s.role == "info" && s.format == "1bit")
                .map(|s| PanelTarget {
                    screen: Some(s.id),
                    width: s.width,
                    height: s.height,
                }),
            Err(PluginError::Api(e))
                if e.code == foxglove_plugin_api::rpc::code::METHOD_NOT_FOUND
                    || e.code == foxglove_plugin_api::rpc::code::UNSUPPORTED =>
            {
                Some(PanelTarget::BEFORE_1_4)
            }
            Err(_) => None,
        }
    }

    /// Shows `frame` (1 bpp at the target's size) on `target`, for `ttl` (60 s at most), or with
    /// no `ttl` until it's replaced, released or this plugin stops. Needs `screens.write`.
    pub fn panel_show(
        &self,
        device: &str,
        target: &PanelTarget,
        frame: &[u8],
        ttl: Option<Duration>,
    ) -> Result<()> {
        use base64::Engine;
        let bitmap = base64::engine::general_purpose::STANDARD.encode(frame);
        let ttl_ms = ttl.map_or(0, |t| {
            u32::try_from(t.as_millis()).unwrap_or(u32::MAX).max(1)
        });
        let mut params = json!({"device": device, "width": target.width, "height": target.height, "bitmap": bitmap, "ttl_ms": ttl_ms});
        if let Some(id) = target.screen {
            params["screen"] = json!(id);
        }
        self.call("screens.panel_override", params).map(|_| ())
    }

    /// Shows `frame` (1 bpp, rows MSB first, padded to whole bytes) on a keyboard's first info
    /// screen for `ttl` (60 s at most; send it again to keep it). Needs `screens.write`.
    pub fn panel_override(
        &self,
        device: &str,
        width: u16,
        height: u16,
        frame: &[u8],
        ttl: Duration,
    ) -> Result<()> {
        use base64::Engine;
        let bitmap = base64::engine::general_purpose::STANDARD.encode(frame);
        let ttl_ms = u32::try_from(ttl.as_millis()).unwrap_or(u32::MAX);
        self.call("screens.panel_override", json!({"device": device, "width": width, "height": height, "bitmap": bitmap, "ttl_ms": ttl_ms.max(1)}))
            .map(|_| ())
    }

    /// Shows a frame on the first info screen until it's replaced, released ([`Plugin::panel_release`])
    /// or this plugin stops: a steady screen without re-sending it (API 1.3, #281).
    pub fn panel_hold(&self, device: &str, width: u16, height: u16, frame: &[u8]) -> Result<()> {
        use base64::Engine;
        let bitmap = base64::engine::general_purpose::STANDARD.encode(frame);
        self.call("screens.panel_override", json!({"device": device, "width": width, "height": height, "bitmap": bitmap, "ttl_ms": 0}))
            .map(|_| ())
    }

    /// Gives the info screen back to Foxglove.
    pub fn panel_release(&self, device: &str) -> Result<()> {
        self.call("screens.panel_release", json!({ "device": device }))
            .map(|_| ())
    }

    /// One of this plugin's secrets (#213; needs `secrets`): what the user entered in the
    /// Plugins tab, or what the plugin saved. `None` when it isn't set.
    pub fn secret(&self, name: &str) -> Result<Option<String>> {
        Ok(self
            .call_as::<foxglove_plugin_api::methods::SecretValue>(
                "secrets.get",
                json!({ "name": name }),
            )?
            .value)
    }

    /// Saves (or with `None` removes) one of this plugin's secrets, such as a refreshed token.
    pub fn set_secret(&self, name: &str, value: Option<&str>) -> Result<()> {
        self.call("secrets.set", json!({ "name": name, "value": value }))
            .map(|_| ())
    }

    /// The plugin's own folder for state between runs (`data/` in its folder, writable even in
    /// the sandbox). Foxglove creates it before starting the plugin.
    pub fn data_dir() -> std::path::PathBuf {
        std::env::current_dir().unwrap_or_default().join("data")
    }

    /// Whether Foxglove has gone (its end of the connection closed): a plugin that holds
    /// something, such as a port, should stop.
    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Relaxed)
    }

    /// The ids of this plugin's actions (`[[actions]]` in `plugin.toml`) as the user presses
    /// their keys (#212). Taken once.
    pub fn actions(&self) -> Option<Receiver<String>> {
        self.actions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
    }

    /// Asks to be told when `exe`'s (such as `obs64.exe`) windows open and close; needs
    /// `apps.watch:<exe>` (API 1.3). How many of its windows are open now. Wait for an app this
    /// way instead of polling for it.
    pub fn watch_app(&self, exe: &str) -> Result<u32> {
        self.call_as::<AppWindows>("apps.watch", json!({ "exe": exe }))
            .map(|w| w.open)
    }

    /// This plugin's settings (`[[settings]]` in `plugin.toml`), as the user set them in the
    /// Plugins tab, or their defaults (API 1.3).
    pub fn settings(&self) -> Result<serde_json::Map<String, Value>> {
        self.call_as::<foxglove_plugin_api::methods::SettingValues>("settings.get", json!({}))
            .map(|s| s.values)
    }

    /// All the settings again each time the user changes one. Taken once.
    pub fn settings_changes(&self) -> Option<Receiver<serde_json::Map<String, Value>>> {
        self.settings_changes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
    }

    /// The watched apps' windows opening and closing. It ends when Foxglove goes away. Taken
    /// once.
    pub fn app_windows(&self) -> Option<Receiver<AppWindowEvent>> {
        self.app_windows
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
    }

    pub fn devices(&self) -> Result<Vec<Device>> {
        Ok(self.call_as::<Devices>("devices.list", json!({}))?.devices)
    }

    /// A device's lights as they are. Needs `devices.read`.
    pub fn lighting_state(&self, device: &str) -> Result<LightingState> {
        self.call_as("lighting.state", json!({ "device": device }))
    }

    /// Changes a device's lights (only what's given). Needs `lighting.write`.
    pub fn set_lighting(&self, params: &SetLightingParams) -> Result<()> {
        self.call(
            "lighting.set",
            serde_json::to_value(params).expect("serializable"),
        )
        .map(|_| ())
    }
}

/// A hold on a device's lights ([`Plugin::lease`]); dropping it gives them back.
pub struct Lease {
    plugin: Arc<Plugin>,
    id: u64,
}

impl Lease {
    /// Changes the leased colour in place (API 1.3): for a stream of colours, one lease for the
    /// whole stream.
    pub fn set(&self, color: Hsb) -> Result<()> {
        self.plugin
            .call(
                "lighting.update",
                json!({ "lease": self.id, "color": color }),
            )
            .map(|_| ())
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        let _ = self
            .plugin
            .call("lighting.release", json!({ "lease": self.id }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::Receiver as Rx;

    /// A pipe for tests: what one side writes, the other reads.
    struct PipeRead(Rx<Vec<u8>>, Vec<u8>);
    impl Read for PipeRead {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if self.1.is_empty() {
                match self.0.recv() {
                    Ok(b) => self.1 = b,
                    Err(_) => return Ok(0),
                }
            }
            let n = buf.len().min(self.1.len());
            buf[..n].copy_from_slice(&self.1[..n]);
            self.1.drain(..n);
            Ok(n)
        }
    }
    struct PipeWrite(Sender<Vec<u8>>);
    impl Write for PipeWrite {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            self.0
                .send(b.to_vec())
                .map_err(|_| std::io::ErrorKind::BrokenPipe)?;
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// A fake Foxglove: answers initialize, facts.set and facts.subscribe, then sends an event.
    fn fake() -> (PipeRead, PipeWrite) {
        let (to_plugin, plugin_reads) = mpsc::channel::<Vec<u8>>();
        let (plugin_writes, from_plugin) = mpsc::channel::<Vec<u8>>();
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            for chunk in from_plugin {
                buf.extend(chunk);
                while let Some(i) = buf.iter().position(|&b| b == b'\n') {
                    let line: Vec<u8> = buf.drain(..=i).collect();
                    let req: Request = serde_json::from_slice(&line).unwrap();
                    let id = req.id.clone().unwrap();
                    let result = match req.method.as_str() {
                        "initialize" => {
                            json!({"api":"1.1","app":"test","granted":["facts.write:focus.*"]})
                        }
                        "facts.subscribe" => json!({"subscription": 7}),
                        _ => json!({}),
                    };
                    let mut out = serde_json::to_vec(&Response::ok(id, result)).unwrap();
                    out.push(b'\n');
                    to_plugin.send(out).unwrap();
                    if req.method == "facts.subscribe" {
                        let ev = Request::notification(
                            "facts.changed",
                            json!({"subscription":7,"key":"clock.time","value":"12:00"}),
                        );
                        let mut out = serde_json::to_vec(&ev).unwrap();
                        out.push(b'\n');
                        to_plugin.send(out).unwrap();
                    }
                }
            }
        });
        (PipeRead(plugin_reads, Vec::new()), PipeWrite(plugin_writes))
    }

    #[test]
    fn connects_calls_and_receives_events() {
        let (r, w) = fake();
        let plugin = Plugin::connect_over(r, w, "test", "1").unwrap();
        assert_eq!(plugin.granted, vec!["facts.write:focus.*".to_string()]);
        plugin
            .set_fact("focus.state", Some(Fact::Text("focus".into())))
            .unwrap();
        let events = plugin.events().unwrap();
        assert_eq!(plugin.subscribe("clock.*").unwrap(), 7);
        let ev = events.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(
            (ev.key.as_str(), ev.value),
            ("clock.time", Some(Fact::Text("12:00".into())))
        );
    }
}

#[cfg(test)]
mod fit_tests {
    use super::PanelTarget;

    /// #560: the same size is untouched (byte for byte); another size gets the frame scaled to
    /// fit and centred.
    #[test]
    fn a_frame_fits_any_info_screen() {
        let mut frame = vec![0u8; 8 * 52];
        frame[0] = 0x80; // top-left
        frame[8 * 51 + 7] = 0x01; // bottom-right
        assert_eq!(PanelTarget::BEFORE_1_4.fit(&frame, 64, 52), frame);
        let wide = PanelTarget {
            screen: Some(0),
            width: 128,
            height: 32,
        };
        let out = wide.fit(&frame, 64, 52);
        assert_eq!(out.len(), 16 * 32);
        // 64x52 at 32/52 is 39x32, centred: x from 44.
        let at = |x: usize, y: usize| out[y * 16 + x / 8] & (0x80 >> (x % 8)) != 0;
        assert!(
            at(44, 0),
            "top-left lands at the left edge of the centred art"
        );
        assert!(at(44 + 38, 31), "bottom-right at its bottom-right");
        assert_eq!(
            out.iter().map(|b| b.count_ones()).sum::<u32>(),
            2,
            "nothing else"
        );
    }
}
