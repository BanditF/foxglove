//! device-layer device for a Foxglove link keyboard (feature `devices`): everything the host link can do, behind
//! the device layer's capability traits. The keymap is not here: ZMK keymaps come over Studio RPC, and the
//! app adds them on top of this device.
//!
//! Capabilities from HELLO's bits:
//!
//! | Device layer | from |
//! |---|---|
//! | `SCREENS` | `PANEL` or `ART_RELAY`, or no bits at all (firmware older than the bits) |
//! | `LIGHTING`, `SETTINGS`, `BEHAVIORS` | `SETTINGS_RGB` (the 0xD7 family) |
//! | `POWER` | always: HELLO carries the right half's battery; `CONN_STATE` adds output and profile |
//! | `LAYERS`, `KEY_EVENTS` | always: the overlay's 0xFF/0xF1 reports on the same interface |

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use foxglove_devices::calibration;
use foxglove_devices::{
    ArtStatus as DeviceArt, BehaviorInfo, Behaviors, Caps, Checked, DevSettings, Device,
    DeviceError, DeviceEvent, DeviceInfo, Effect, EventSink, Layers, Led, Lighting, LightingState,
    Power, PowerInfo, Screens, Value, Zone,
};

use crate::lamps::LampParts;
use crate::link::{Event, EventSubscription, Link};
use crate::protocol::{capabilities, Hello, InvokeStatus, RgbHalf, RgbState};
use crate::{art, panel, settings};

type PResult<T> = foxglove_devices::Result<T>;

/// First byte of the overlay's layer state report: size, default layers, active layers.
pub const LAYER_STATE: u8 = 0xFF;
/// First byte of a key event report: row, column, pressed.
pub const KEY_EVENT: u8 = 0xF1;
const MAX_LAYER_STATE_BYTES: usize = 4;

/// Active layers from a layer state report (the base layer always counts), or `None` for an
/// unrelated report that happens to start with 0xFF.
pub fn parse_layers(r: &[u8]) -> Option<Vec<u8>> {
    let size = *r.get(1)? as usize;
    if r[0] != LAYER_STATE || size == 0 || size > MAX_LAYER_STATE_BYTES || 2 + 2 * size > r.len() {
        return None;
    }
    let word = |b: &[u8]| {
        let mut w = [0u8; 4];
        w[..size].copy_from_slice(b);
        u32::from_le_bytes(w)
    };
    let active = word(&r[2..2 + size]) | word(&r[2 + size..2 + 2 * size]) | 1;
    Some((0..32).filter(|i| active & (1 << i) != 0).collect())
}

/// What device capabilities a keyboard has, from what it said to HELLO.
pub fn caps(hello: &Hello) -> Caps {
    let bits = hello.capabilities;
    let mut caps = Caps::LAYERS | Caps::KEY_EVENTS | Caps::POWER;
    if bits == 0 || bits & (capabilities::PANEL | capabilities::ART_RELAY) != 0 {
        caps |= Caps::SCREENS;
    }
    if bits & capabilities::SETTINGS_RGB != 0 {
        caps |= Caps::LIGHTING | Caps::SETTINGS | Caps::BEHAVIORS;
    }
    caps
}

/// What the keyboard can do with light and pictures (#423), for the Lights page's chips: the
/// underglow from HELLO's RGB bit (one colour at a time, ZMK's four effects, brightness), and a
/// screen per display the keyboard describes (D5 06), or the HELLO panel on older firmware.
pub fn light_caps(
    hello: &Hello,
    screens: &[crate::protocol::ScreenDescriptor],
) -> foxglove_devices::pc_lighting::LightCaps {
    use crate::protocol::{ScreenDescriptor as D, ScreenKind};
    use foxglove_devices::pc_lighting::{Color, LightCaps, Screen};
    let bits = hello.capabilities;
    let rgb = bits & capabilities::SETTINGS_RGB != 0;
    let mut out = LightCaps {
        color: if rgb { Color::Single } else { Color::None },
        effects: if rgb {
            ["Solid", "Breathe", "Spectrum", "Swirl"]
                .map(String::from)
                .to_vec()
        } else {
            Vec::new()
        },
        brightness: rgb,
        ..Default::default()
    };
    let displays: Vec<&D> = screens
        .iter()
        .filter(|s| s.kind == ScreenKind::Display)
        .collect();
    for d in &displays {
        // The display's own ops and those of the regions on it.
        let ops = screens
            .iter()
            .filter(|s| s.id == d.id || s.parent == Some(d.id))
            .fold(0, |ops, s| ops | s.ops);
        out.screens.push(Screen {
            width: Some(d.width),
            height: Some(d.height),
            depth: match d.format {
                D::FORMAT_1BIT => Some(1),
                D::FORMAT_GREY2 => Some(2),
                D::FORMAT_RGB565 => Some(16),
                _ => None,
            },
            fps: None,
            // Art is a set of frames with a rate: still pictures and animations.
            images: ops & D::OP_ART != 0,
            animations: ops & D::OP_ART != 0,
            stream: ops & (D::OP_ROWS | D::OP_ROWS_AT) != 0,
        });
    }
    let legacy = bits == 0 || bits & (capabilities::PANEL | capabilities::ART_RELAY) != 0;
    if displays.is_empty() && legacy && hello.width > 0 && hello.height > 0 {
        let art = bits == 0 || bits & capabilities::ART_RELAY != 0;
        out.screens.push(Screen {
            width: u16::try_from(hello.width).ok(),
            height: u16::try_from(hello.height).ok(),
            depth: Some(1),
            fps: None,
            images: art,
            animations: art,
            stream: bits == 0 || bits & capabilities::PANEL != 0,
        });
    }
    out
}

fn err(e: anyhow::Error) -> DeviceError {
    let text = e.to_string();
    if text.contains("not connected")
        || text.contains("disconnected")
        || text.contains("link stopped")
    {
        DeviceError::Disconnected
    } else if text.contains("no reply") {
        DeviceError::Timeout
    } else {
        DeviceError::Backend(text)
    }
}

fn lighting_state(h: &RgbHalf) -> LightingState {
    LightingState {
        on: h.on,
        hue: h.hue,
        sat: h.sat,
        brightness: h.brightness,
        effect: Effect {
            id: h.effect.to_string(),
            params: vec![],
        },
        speed: h.speed,
        settled: true,
    }
}

/// Whether the right half (as it last reported) shows what the left does; brightness aside,
/// since each half dims on its own battery.
fn halves_agree(rgb: &RgbState) -> bool {
    let Some((r, _age)) = &rgb.right else {
        return true;
    };
    let l = &rgb.left;
    let near = |a: u16, b: u16, wrap: u16| {
        let d = a.abs_diff(b);
        d.min(wrap.saturating_sub(d)) <= 2
    };
    l.on == r.on
        && near(l.hue, r.hue, 360)
        && near(l.sat.into(), r.sat.into(), 1000)
        && l.effect == r.effect
}

/// A Foxglove link keyboard as a device-layer device. Events reach the sink for as long as it lives.
pub struct FoxgloveLinkDevice {
    info: DeviceInfo,
    link: Arc<Link>,
    hello: Mutex<Hello>,
    /// The PC panel's size, once asked ([`FoxgloveLinkDevice::panel_size`]).
    panel_size: Mutex<Option<(usize, usize)>>,
    caps: Caps,
    sink: EventSink,
    layers: Arc<Mutex<Vec<u8>>>,
    behaviors: Mutex<Vec<BehaviorInfo>>,
    underglow: Mutex<Option<u16>>,
    /// Its key in the colour calibration registry: underglow colours go through that
    /// calibration (`calibration::send_hsb`).
    calibration_key: Mutex<String>,
    /// Its LEDs by part, in SET_FRAME's order (#561): what the firmware or the board profile
    /// says, else the old firmware's.
    lamp_parts: Mutex<LampParts>,
    _events: EventSubscription,
    _reports: EventSubscription,
}

impl FoxgloveLinkDevice {
    /// Wraps a connected link; `hello` is what the keyboard answered when it connected.
    pub fn new(info: DeviceInfo, link: Arc<Link>, hello: Hello, sink: EventSink) -> Self {
        let events_sink = sink.clone();
        let events = link.subscribe_events(move |event| {
            if let Event::State(state) = event {
                events_sink.emit(match state.connected {
                    Some(_) => DeviceEvent::Connected,
                    None => DeviceEvent::Disconnected {
                        reason: state.error.unwrap_or_else(|| "not connected".into()),
                    },
                });
            }
        });
        let layers = Arc::new(Mutex::new(vec![0]));
        let (report_sink, report_layers) = (sink.clone(), layers.clone());
        let capabilities = hello.capabilities;
        // The right's level from the last HELLO seen, so a repeat isn't news.
        let right_seen = Mutex::new(hello.right_battery);
        // Runs on the link's reader thread: emitting only updates facts and queues events.
        let reports = link.subscribe_reports(move |_, r| {
            if r[0] == KEY_EVENT {
                report_sink.emit(DeviceEvent::Key {
                    row: r[1].into(),
                    col: r[2].into(),
                    pressed: r[3] != 0,
                });
            } else if crate::protocol::is_reply(
                r,
                crate::protocol::SETTINGS,
                crate::protocol::settings::CONN_STATE,
            ) {
                // The firmware says when the connection changes (CONN_STATE_EVENT), and answers
                // a read the same way (#17).
                let c = crate::protocol::ConnState::parse(r);
                report_sink.emit(DeviceEvent::Link {
                    output: if c.endpoint == 0 { "USB" } else { "BLE" }.into(),
                    profile: c.active_profile,
                    connected: c.connected,
                });
                // The left's own battery rides on CONN_STATE (#373).
                if let Some(percent) = crate::protocol::left_battery(r, capabilities) {
                    report_sink.emit(DeviceEvent::Battery {
                        part: "left".into(),
                        percent,
                    });
                }
            } else if crate::protocol::is_reply(
                r,
                crate::protocol::PANEL,
                crate::protocol::panel::HELLO,
            ) {
                // The firmware sends HELLO again when the right's battery changes (after host
                // traffic), and its first reading only comes once the right has joined, often
                // after the connect-time HELLO: without this the right's level never showed.
                let right = crate::protocol::Hello::parse(r).right_battery;
                let mut seen = right_seen.lock().unwrap_or_else(|e| e.into_inner());
                if right != *seen {
                    *seen = right;
                    if let Some(percent) = right {
                        report_sink.emit(DeviceEvent::Battery {
                            part: "right".into(),
                            percent,
                        });
                    }
                }
            } else if let Some(active) = parse_layers(r) {
                let mut current = report_layers.lock().unwrap_or_else(|e| e.into_inner());
                if *current != active {
                    current.clone_from(&active);
                    report_sink.emit(DeviceEvent::LayerChanged { active });
                }
            }
        });
        if let Some(percent) = hello.right_battery {
            sink.emit(DeviceEvent::Battery {
                part: "right".into(),
                percent,
            });
        }
        FoxgloveLinkDevice {
            calibration_key: Mutex::new(calibration::key_for(&info.id)),
            info,
            link,
            caps: caps(&hello),
            hello: Mutex::new(hello),
            panel_size: Mutex::new(None),
            lamp_parts: Mutex::new(LampParts::old_firmware()),
            sink,
            layers,
            behaviors: Mutex::new(Vec::new()),
            underglow: Mutex::new(None),
            _events: events,
            _reports: reports,
        }
    }

    /// The behaviors [`Behaviors::list`] offers; the app fills it from the keymap it reads.
    pub fn set_behaviors(&self, list: Vec<BehaviorInfo>) {
        *self.behaviors.lock().unwrap_or_else(|e| e.into_inner()) = list;
    }

    /// Its key in the colour calibration registry (from its id: VID:PID, plus a trusted
    /// serial).
    pub fn calibration_key(&self) -> String {
        self.calibration_key
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Its LEDs by part, in SET_FRAME's order (#561).
    pub fn lamp_parts(&self) -> LampParts {
        self.lamp_parts
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// The board's LEDs by part, from its firmware or profile: frames are split (and calibrated)
    /// by them.
    pub fn set_lamp_parts(&self, parts: LampParts) {
        *self.lamp_parts.lock().unwrap_or_else(|e| e.into_inner()) = parts;
    }

    /// Asks the keyboard for its LEDs per part (LAMP_PARTS, HELLO bit 22, #561). Unsupported on
    /// firmware without it, whose parts are the profile's (or the old firmware's).
    pub fn read_lamp_parts(&self) -> PResult<LampParts> {
        if !self.has(capabilities::LAMP_PARTS) {
            return Err(DeviceError::Unsupported);
        }
        settings::lamp_parts(&*self.link).map_err(err)
    }

    /// Stores its calibration under another key.
    pub fn set_calibration_key(&self, key: String) {
        *self
            .calibration_key
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = key;
    }

    /// A half's colour as the keyboard reports it, turned back into the colour Foxglove asked
    /// for (before calibration), so the app shows and resends the palette's colour.
    pub fn asked_half(&self, half: &mut RgbHalf) {
        (half.hue, half.sat, half.brightness) = calibration::asked_hsb(
            &self.calibration_key(),
            (half.hue, half.sat, half.brightness),
        );
    }

    /// Both halves' state as [`Self::asked_half`] reports them.
    pub fn asked_state(&self, state: &mut RgbState) {
        self.asked_half(&mut state.left);
        if let Some((right, _)) = &mut state.right {
            self.asked_half(right);
        }
    }

    /// The keymap's `&rgb_ug` behavior id, which lighting changes go through (the firmware has
    /// no direct colour command yet).
    pub fn set_underglow(&self, id: Option<u16>) {
        *self.underglow.lock().unwrap_or_else(|e| e.into_inner()) = id;
    }

    /// Sends one `&rgb_ug` command. Lighting commands are always safe, so they skip the
    /// action list.
    ///
    /// The firmware answers Busy while it's still releasing the previous command, so a quick
    /// run of changes (a plugin streaming colours) waits briefly and tries again instead of
    /// dropping them.
    fn underglow(&self, command: u32, param: u32) -> PResult<()> {
        let id = self
            .underglow
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .ok_or(DeviceError::Unsupported)?;
        let mut tries = 0;
        loop {
            match status(settings::invoke(&*self.link, id, command, param, 0).map_err(err)?) {
                Err(DeviceError::Busy(_)) if tries < UNDERGLOW_BUSY_TRIES => {
                    tries += 1;
                    std::thread::sleep(UNDERGLOW_BUSY_WAIT);
                }
                other => return other,
            }
        }
    }

    pub fn link(&self) -> &Arc<Link> {
        &self.link
    }

    /// The underglow's levels by power state and both halves' state (D7 0E); `set` changes the
    /// levels first. Unsupported on firmware without [`capabilities::BATTERY_DIM`].
    pub fn battery_dim(
        &self,
        set: Option<crate::protocol::BatteryDimConfig>,
    ) -> PResult<crate::protocol::BatteryDim> {
        if !self.has(capabilities::BATTERY_DIM) {
            return Err(DeviceError::Unsupported);
        }
        let d = settings::battery_dim(&*self.link, set).map_err(err)?;
        match d.status {
            0 => Ok(d),
            4 => Err(DeviceError::Unsupported),
            s => Err(err(anyhow::anyhow!(
                "the keyboard refused the levels (status {s})"
            ))),
        }
    }

    /// SET_FRAME (#272): per-LED colours from lamp `start` on, part by part in each strip's chain
    /// order ([`Self::lamp_parts`]). The first frame takes both halves' lights from the keyboard's own
    /// effect until [`Self::release_frame`] or the keyboard's timeout (the reply says how long).
    /// The colours go as they are, without the colour calibration. Unsupported on firmware
    /// without [`capabilities::FRAMES`] (GA builds keep [`Lighting::set_color`]).
    pub fn set_frame(
        &self,
        start: u8,
        colours: &[[u8; 3]],
    ) -> PResult<crate::protocol::FrameReply> {
        if !self.has(capabilities::FRAMES) {
            return Err(DeviceError::Unsupported);
        }
        frame_status(settings::set_frame(&*self.link, start, colours).map_err(err)?)
    }

    /// Whether the firmware takes [`Self::set_frame`] (HELLO bit 14).
    pub fn frames(&self) -> bool {
        self.has(capabilities::FRAMES)
    }

    /// Both halves back to their own effect after [`Self::set_frame`].
    pub fn release_frame(&self) -> PResult<crate::protocol::FrameReply> {
        if !self.has(capabilities::FRAMES) {
            return Err(DeviceError::Unsupported);
        }
        frame_status(settings::release_frame(&*self.link).map_err(err)?)
    }

    /// Whether the firmware takes [`Self::set_effect`] (HELLO bit 19).
    pub fn effects(&self) -> bool {
        self.has(capabilities::EFFECTS)
    }

    /// Both halves draw `e` on their own (#490); Unsupported without
    /// [`capabilities::EFFECTS`] (GA builds keep frames or one colour).
    pub fn set_effect(&self, e: &crate::protocol::Effect) -> PResult<crate::protocol::EffectReply> {
        if !self.effects() {
            return Err(DeviceError::Unsupported);
        }
        effect_status(settings::set_effect(&*self.link, e).map_err(err)?)
    }

    /// [`Self::set_effect`] with the keyboard's colour calibration on each colour: one set for
    /// both halves, since the wire carries one (per-half calibration is the frames' only).
    pub fn show_effect(
        &self,
        e: &crate::protocol::Effect,
    ) -> PResult<crate::protocol::EffectReply> {
        let key = self.calibration_key();
        let colours = e
            .colours
            .iter()
            .map(|&[r, g, b]| {
                let c =
                    calibration::global().rgb(&key, None, None, foxglove_devices::Rgb { r, g, b });
                [c.r, c.g, c.b]
            })
            .collect();
        self.set_effect(&crate::protocol::Effect {
            colours,
            ..e.clone()
        })
    }

    /// Where the halves sit in the room, for [`Self::set_effect`].
    pub fn place_effect(
        &self,
        room: crate::protocol::MmRect,
        left: crate::protocol::MmRect,
        right: crate::protocol::MmRect,
    ) -> PResult<crate::protocol::EffectReply> {
        if !self.effects() {
            return Err(DeviceError::Unsupported);
        }
        effect_status(settings::place_effect(&*self.link, room, left, right).map_err(err)?)
    }

    /// Whether the firmware takes LED points for effects (HELLO bit 21, #550).
    pub fn effect_lamps(&self) -> bool {
        self.has(capabilities::EFFECT_LAMPS)
    }

    /// Where each LED of `half` (0 left, 1 right) sits for [`Self::set_effect`], in thousandths
    /// of the half (see [`crate::protocol::effect_lamps`]); empty puts the half back on the
    /// firmware's table. Unsupported without [`capabilities::EFFECT_LAMPS`].
    pub fn place_effect_lamps(
        &self,
        half: u8,
        points: &[[u16; 2]],
    ) -> PResult<crate::protocol::EffectReply> {
        if !self.effect_lamps() {
            return Err(DeviceError::Unsupported);
        }
        effect_status(settings::lamps_effect(&*self.link, half, points).map_err(err)?)
    }

    /// Both halves back to their own effect after [`Self::set_effect`].
    pub fn release_effect(&self) -> PResult<crate::protocol::EffectReply> {
        if !self.effects() {
            return Err(DeviceError::Unsupported);
        }
        effect_status(settings::release_effect(&*self.link).map_err(err)?)
    }

    /// Whether the right half's on/off and power source arrive as RGB_STATE / BATTERY_DIM
    /// events (HELLO bit 20, #543), so a stale right half needs no second read.
    pub fn right_events(&self) -> bool {
        self.has(capabilities::RIGHT_EVENTS)
    }

    /// The PC panel's size: from the keyboard's screen descriptors when it has them (#289),
    /// read once, else from HELLO.
    fn panel_size(&self) -> (usize, usize) {
        let hello = *self.hello.lock().unwrap_or_else(|e| e.into_inner());
        let mut cached = self.panel_size.lock().unwrap_or_else(|e| e.into_inner());
        *cached.get_or_insert_with(|| {
            let screens = if hello.capabilities & capabilities::SCREENS != 0 {
                panel::screens(&*self.link).unwrap_or_default()
            } else {
                vec![]
            };
            panel::drawable_panel(&screens).map_or((hello.width, hello.height), |p| {
                (usize::from(p.width), usize::from(p.height))
            })
        })
    }

    fn has(&self, bit: u32) -> bool {
        self.hello
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .capabilities
            & bit
            != 0
    }
}

impl Device for FoxgloveLinkDevice {
    fn info(&self) -> &DeviceInfo {
        &self.info
    }

    fn caps(&self) -> Caps {
        self.caps
    }

    fn layers(&self) -> Option<&dyn Layers> {
        Some(self)
    }

    fn behaviors(&self) -> Option<&dyn Behaviors> {
        self.caps
            .contains(Caps::BEHAVIORS)
            .then_some(self as &dyn Behaviors)
    }

    fn lighting(&self) -> Option<&dyn Lighting> {
        self.caps
            .contains(Caps::LIGHTING)
            .then_some(self as &dyn Lighting)
    }

    fn screens(&self) -> Option<&dyn Screens> {
        self.caps
            .contains(Caps::SCREENS)
            .then_some(self as &dyn Screens)
    }

    fn power(&self) -> Option<&dyn Power> {
        Some(self)
    }

    fn settings(&self) -> Option<&dyn DevSettings> {
        self.caps
            .contains(Caps::SETTINGS)
            .then_some(self as &dyn DevSettings)
    }
}

impl Layers for FoxgloveLinkDevice {
    fn active(&self) -> PResult<Vec<u8>> {
        Ok(self
            .layers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone())
    }
}

impl Behaviors for FoxgloveLinkDevice {
    fn list(&self) -> PResult<Vec<BehaviorInfo>> {
        Ok(self
            .behaviors
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone())
    }

    fn invoke_checked(&self, checked: Checked) -> PResult<()> {
        let a = checked.action();
        let id = u16::try_from(a.id).map_err(|_| DeviceError::Unsupported)?;
        status(settings::invoke(&*self.link, id, a.param1, a.param2, a.hold_ms).map_err(err)?)
    }
}

/// The keyboard's answer to FRAME as a device-layer result.
fn frame_status(r: crate::protocol::FrameReply) -> PResult<crate::protocol::FrameReply> {
    match r.status {
        0 => Ok(r),
        4 => Err(DeviceError::Unsupported),
        s => Err(err(anyhow::anyhow!(
            "the keyboard refused the frame (status {s})"
        ))),
    }
}

fn effect_status(r: crate::protocol::EffectReply) -> PResult<crate::protocol::EffectReply> {
    match r.status {
        0 => Ok(r),
        4 => Err(DeviceError::Unsupported),
        s => Err(err(anyhow::anyhow!(
            "the keyboard refused the effect (status {s})"
        ))),
    }
}

/// The keyboard's answer to INVOKE as a device-layer result; the keyboard's own words are kept.
fn status(s: InvokeStatus) -> PResult<()> {
    match s {
        InvokeStatus::Ok => Ok(()),
        InvokeStatus::Busy => Err(DeviceError::Busy(s.to_string())),
        InvokeStatus::Refused => Err(DeviceError::Denied(s.to_string())),
        other => Err(DeviceError::Backend(other.to_string())),
    }
}

/// Underglow commands that find the firmware busy are retried this many times, this far apart.
const UNDERGLOW_BUSY_TRIES: u32 = 10;
const UNDERGLOW_BUSY_WAIT: std::time::Duration = std::time::Duration::from_millis(20);

// `&rgb_ug` commands (ZMK's dt-bindings/zmk/rgb.h).
const RGB_ON: u32 = 1;
const RGB_OFF: u32 = 2;
const RGB_SPEED_UP: u32 = 9;
const RGB_SPEED_DOWN: u32 = 10;
const RGB_EFFECT_SET: u32 = 13;
/// ZMK's underglow speeds run 1-5, so this many steps reach any of them.
const SPEED_STEPS: u32 = 5;
const RGB_COLOR_HSB: u32 = 14;

impl Lighting for FoxgloveLinkDevice {
    fn layout(&self) -> PResult<Vec<Zone>> {
        let rgb = settings::rgb_state(&*self.link).map_err(err)?;
        let zone = |name: &str, id| Zone {
            name: name.into(),
            leds: vec![Led { id, position: None }],
        };
        let mut zones = vec![zone("left", 0)];
        if rgb.right.is_some() {
            zones.push(zone("right", 1));
        }
        Ok(zones)
    }

    fn state(&self) -> PResult<LightingState> {
        let mut rgb = settings::rgb_state(&*self.link).map_err(err)?;
        self.asked_state(&mut rgb);
        let state = LightingState {
            settled: halves_agree(&rgb),
            ..lighting_state(&rgb.left)
        };
        self.sink.emit(DeviceEvent::LightingChanged {
            state: state.clone(),
        });
        Ok(state)
    }

    /// Effects by the firmware's number: 0 solid, 1 breathe, 2 spectrum, 3 swirl.
    fn set_effect(&self, effect: Effect) -> PResult<()> {
        let n: u32 = effect.id.parse().map_err(|_| DeviceError::Unsupported)?;
        self.underglow(RGB_EFFECT_SET, n)
    }

    fn set_on(&self, on: bool) -> PResult<()> {
        self.underglow(if on { RGB_ON } else { RGB_OFF }, 0)
    }

    /// Through the keyboard's colour calibration: the firmware gets the calibrated colour.
    fn set_color(&self, hue: u16, sat: u8, brightness: u8) -> PResult<()> {
        let key = self.calibration_key();
        calibration::send_hsb(&key, (hue, sat, brightness), |(hue, sat, brightness)| {
            let hsb = (u32::from(hue) << 16) | (u32::from(sat) << 8) | u32::from(brightness);
            self.underglow(RGB_COLOR_HSB, hsb)
        })
    }

    /// ZMK has no "set speed", only one step up or down: steps toward `speed`, reading it back
    /// each time, at most [`SPEED_STEPS`] times (#293).
    fn set_speed(&self, speed: u8) -> PResult<()> {
        for _ in 0..SPEED_STEPS {
            let now = settings::rgb_state(&*self.link).map_err(err)?.left.speed;
            if now == speed {
                return Ok(());
            }
            self.underglow(
                if now < speed {
                    RGB_SPEED_UP
                } else {
                    RGB_SPEED_DOWN
                },
                0,
            )?;
        }
        Ok(())
    }

    /// SET_FRAME (#272): every part's LEDs in strip order ([`FoxgloveLinkDevice::lamp_parts`]); a
    /// split board's halves each through their own colour calibration (the zone `left` or
    /// `right`), a one-piece board's through the keyboard's. A frame past what a frame
    /// addresses is refused.
    fn set_frame(&self, frame: &[foxglove_devices::Rgb]) -> PResult<()> {
        self.show_frame(frame).map(|_| ())
    }

    /// Hands frames and a SET_EFFECT effect back to the keyboard's own effect (a frame RELEASE
    /// leaves an effect running, so both go). Colours, effects and on/off go through the
    /// keymap's own behavior, so without frames the keyboard always keeps control.
    fn release(&self) -> PResult<()> {
        let effect = if self.effects() {
            self.release_effect().map(|_| ())
        } else {
            Ok(())
        };
        let frames = if self.frames() {
            self.release_frame().map(|_| ())
        } else {
            Ok(())
        };
        effect.and(frames)
    }
}

impl FoxgloveLinkDevice {
    /// [`Lighting::set_frame`] with the keyboard's reply: how long it holds a frame without
    /// another ([`crate::protocol::FrameReply::timeout_s`]) and whether the right half draws
    /// it. Every part's LEDs in strip order ([`FoxgloveLinkDevice::lamp_parts`]); a split board's
    /// halves each through their own colour calibration (the zone `left` or `right`), a
    /// one-piece board's through the keyboard's. A frame past what a frame addresses is
    /// refused.
    pub fn show_frame(
        &self,
        frame: &[foxglove_devices::Rgb],
    ) -> PResult<crate::protocol::FrameReply> {
        if frame.len() > crate::lamps::MAX_LAMPS {
            return Err(err(crate::lamps::LampsError::TooMany(frame.len()).into()));
        }
        let key = self.calibration_key();
        let parts = self.lamp_parts();
        let colours: Vec<[u8; 3]> = frame
            .iter()
            .enumerate()
            .map(|(i, &c)| {
                let part = parts.part_of(i).map(|(p, _)| p);
                let zone = match (parts.len(), part) {
                    (2, Some(0)) => Some("left"),
                    (2, _) => Some("right"),
                    _ => None,
                };
                let c = calibration::global().rgb(&key, zone, None, c);
                [c.r, c.g, c.b]
            })
            .collect();
        FoxgloveLinkDevice::set_frame(self, 0, &colours)
    }
}

impl Screens for FoxgloveLinkDevice {
    fn send_panel(&self, bitmap: &[u8], w: u16, h: u16) -> PResult<()> {
        let (width, height) = self.panel_size();
        let stride = (w as usize).div_ceil(8);
        if (w as usize, h as usize) != (width, height) || bitmap.len() != stride * h as usize {
            return Err(DeviceError::Backend(format!(
                "the panel is {width}x{height}; got {w}x{h} with {} bytes",
                bitmap.len()
            )));
        }
        panel::show(&*self.link, bitmap, None, stride)
            .map(|_| ())
            .map_err(err)
    }

    fn art_status(&self) -> PResult<DeviceArt> {
        let st = art::status(&*self.link).map_err(err)?;
        Ok(DeviceArt {
            selected: u8::try_from(st.right_set).ok(),
            bytes: st.length.into(),
            receiving: st.right_state == 1,
        })
    }

    fn push_art(
        &self,
        bytes: &[u8],
        progress: &mut dyn FnMut(usize, usize),
        cancel: &AtomicBool,
    ) -> PResult<()> {
        art::push_cancellable(
            &*self.link,
            bytes,
            |st| progress(st.pushed.into(), st.length.into()),
            || cancel.load(Ordering::Relaxed),
        )
        .map(|_| ())
        .map_err(err)
    }

    fn select_art(&self, index: Option<u8>) -> PResult<()> {
        let set = match index {
            None => -1,
            Some(n) => i8::try_from(n).map_err(|_| DeviceError::Unsupported)?,
        };
        art::select(&*self.link, set).map(|_| ()).map_err(err)
    }
}

impl Power for FoxgloveLinkDevice {
    fn batteries(&self) -> PResult<Vec<(String, u8)>> {
        let fresh = panel::hello(&*self.link).map_err(err)?;
        let mut hello = self.hello.lock().unwrap_or_else(|e| e.into_inner());
        if fresh.right_battery != hello.right_battery {
            if let Some(percent) = fresh.right_battery {
                self.sink.emit(DeviceEvent::Battery {
                    part: "right".into(),
                    percent,
                });
            }
        }
        *hello = fresh;
        Ok(fresh
            .right_battery
            .map(|p| ("right".to_string(), p))
            .into_iter()
            .collect())
    }

    fn info(&self) -> PResult<PowerInfo> {
        if !self.has(capabilities::CONN_STATE) {
            return Ok(PowerInfo::default());
        }
        let c = settings::conn_state(&*self.link).map_err(err)?;
        Ok(PowerInfo {
            output: Some(if c.endpoint == 0 { "USB" } else { "BLE" }.into()),
            profile: Some(c.active_profile),
            profile_connected: Some(c.connected),
        })
    }
}

impl DevSettings for FoxgloveLinkDevice {
    fn get(&self, key: &str) -> PResult<Option<Value>> {
        match key {
            "rgb_sync" => Ok(settings::rgb_sync(&*self.link, None)
                .map_err(err)?
                .sync
                .map(Value::Bool)),
            _ => Ok(None),
        }
    }

    fn set(&self, key: &str, value: Value) -> PResult<()> {
        match (key, value) {
            ("rgb_sync", Value::Bool(on)) => settings::rgb_sync(&*self.link, Some(on))
                .map(|_| ())
                .map_err(err),
            _ => Err(DeviceError::Unsupported),
        }
    }
}

#[cfg(feature = "hid")]
mod backend {
    use super::*;
    use crate::hid::Match;
    use crate::protocol;
    use foxglove_devices::{Backend, BackendId, DeviceId, DeviceKind};
    use std::time::Duration;

    /// Finds Foxglove link keyboards over hidapi; backend id `zmk`.
    #[derive(Default)]
    pub struct FoxgloveLinkBackend {
        /// Narrows discovery (product name, VID/PID); the default takes any Raw HID keyboard.
        pub filter: Match,
    }

    impl Backend for FoxgloveLinkBackend {
        fn id(&self) -> BackendId {
            BackendId("zmk".into())
        }

        fn discover(&self) -> PResult<Vec<DeviceInfo>> {
            let api = hidapi::HidApi::new().map_err(|e| DeviceError::Backend(e.to_string()))?;
            let mut found: Vec<DeviceInfo> = Vec::new();
            for d in api.device_list() {
                if d.usage_page() != protocol::USAGE_PAGE
                    || d.usage() != protocol::USAGE
                    || !self.filter.accepts(d)
                {
                    continue;
                }
                let path = d.path().to_string_lossy();
                let id = DeviceId::new(
                    self.id(),
                    d.vendor_id(),
                    d.product_id(),
                    d.serial_number(),
                    &path,
                );
                // A keyboard on USB and Bluetooth at once shows up twice; keep the first.
                if found.iter().all(|f| f.id != id) {
                    found.push(DeviceInfo {
                        id,
                        backend: self.id(),
                        name: d.product_string().unwrap_or("Keyboard").into(),
                        kind: DeviceKind::Keyboard,
                        firmware: None,
                    });
                }
            }
            Ok(found)
        }

        /// Opens the first interface with this VID/PID that answers HELLO; two identical
        /// keyboards at once can't be told apart yet.
        fn open(&self, info: &DeviceInfo, sink: EventSink) -> PResult<Box<dyn Device>> {
            let m = Match {
                vid_pid: Some((info.id.vid, info.id.pid)),
                ..Match::default()
            };
            let link = Arc::new(Link::hid(m).map_err(err)?);
            let connected = link.wait_connected(Duration::from_secs(3)).map_err(err)?;
            let hello = connected.hello.ok_or(DeviceError::Unsupported)?;
            Ok(Box::new(FoxgloveLinkDevice::new(
                info.clone(),
                link,
                hello,
                sink,
            )))
        }
    }
}

#[cfg(feature = "hid")]
pub use backend::FoxgloveLinkBackend;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::link::{Connected, RawHid, Report};
    use crate::protocol::{self, REPORT};
    use foxglove_devices::{BackendId, DeviceId, DeviceKind, EventBus, Risk};
    use std::collections::VecDeque;
    use std::time::{Duration, Instant};

    const WAIT: Duration = Duration::from_secs(2);

    /// A keyboard in memory that answers the host link's requests with canned replies.
    #[derive(Clone, Default)]
    struct Fake {
        inbox: Arc<(Mutex<VecDeque<Report>>, std::sync::Condvar)>,
        interrupted: Arc<AtomicBool>,
        written: Arc<Mutex<Vec<Vec<u8>>>>,
        /// INVOKEs to answer "busy" before answering OK.
        busy: Arc<std::sync::atomic::AtomicU32>,
    }

    fn report(bytes: &[u8]) -> Report {
        let mut r = [0u8; REPORT];
        r[..bytes.len()].copy_from_slice(bytes);
        r
    }

    impl Fake {
        fn push(&self, bytes: &[u8]) {
            self.inbox.0.lock().unwrap().push_back(report(bytes));
            self.inbox.1.notify_all();
        }
    }

    impl RawHid for Fake {
        fn read(&self, buf: &mut Report) -> anyhow::Result<usize> {
            let mut inbox = self.inbox.0.lock().unwrap();
            loop {
                if self.interrupted.load(Ordering::Relaxed) {
                    return Ok(0);
                }
                if let Some(r) = inbox.pop_front() {
                    *buf = r;
                    return Ok(REPORT);
                }
                inbox = self.inbox.1.wait(inbox).unwrap();
            }
        }

        fn interrupt(&self) {
            let _inbox = self.inbox.0.lock().unwrap();
            self.interrupted.store(true, Ordering::Relaxed);
            self.inbox.1.notify_all();
        }

        fn write(&self, out: &[u8]) -> anyhow::Result<()> {
            self.written.lock().unwrap().push(out.to_vec());
            let reply = match (out[0], out[1]) {
                (protocol::SETTINGS, protocol::settings::RGB_STATE) => {
                    report(&[0xD7, 0x83, 1, 0x68, 0x01, 80, 50, 3, 2, 0xff])
                }
                (protocol::SETTINGS, protocol::settings::INVOKE) => {
                    let busy = self
                        .busy
                        .try_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_sub(1))
                        .is_ok();
                    report(&[0xD7, 0x82, if busy { 2 } else { 0 }])
                }
                (protocol::SETTINGS, protocol::settings::RGB_SYNC) => {
                    report(&[0xD7, 0x81, 1, 1, 2])
                }
                (protocol::PANEL, protocol::panel::HELLO) => {
                    report(&[0xD5, 0x81, 1, 64, 52, 0, 0, 1, 55])
                }
                // Frames: the last report of one (SHOW) and RELEASE get an answer.
                (protocol::SETTINGS, protocol::settings::FRAME)
                    if out[2] & (protocol::frame::SHOW | protocol::frame::RELEASE) != 0 =>
                {
                    let owner = if out[2] & protocol::frame::RELEASE != 0 {
                        0
                    } else {
                        3
                    };
                    report(&[0xD7, 0x8C, 0, owner, 30, 1, 0])
                }
                // A split board of 30 + 24 LEDs.
                (protocol::SETTINGS, protocol::settings::LAMP_PARTS) => {
                    report(&[0xD7, 0x90, 0, 2, 30, 24])
                }
                // Effects: every op answers; EFFECT runs its kind, RELEASE and PLACE report.
                (protocol::SETTINGS, protocol::settings::EFFECT) => {
                    let (owner, kind) = match out[2] {
                        protocol::effect::EFFECT => (3, out[3]),
                        _ => (0, 0),
                    };
                    report(&[0xD7, 0x8F, 0, owner, kind, 1, 0])
                }
                _ => return Ok(()),
            };
            self.push(&reply);
            Ok(())
        }
    }

    fn hello(capabilities: u32) -> Hello {
        Hello {
            version: 1,
            width: 64,
            height: 52,
            right_link: 1,
            right_battery: Some(80),
            capabilities,
            ..Hello::default()
        }
    }

    fn device(capabilities: u32) -> (FoxgloveLinkDevice, Fake, EventBus) {
        let fake = Fake::default();
        let opener_fake = fake.clone();
        let link = Arc::new(Link::start(Box::new(move || {
            let connected = Connected {
                product: "Fake".into(),
                hello: Some(hello(capabilities)),
                bus: crate::link::Bus::Unknown,
            };
            Ok((Box::new(opener_fake.clone()) as Box<dyn RawHid>, connected))
        })));
        link.wait_connected(WAIT).unwrap();
        let id = DeviceId::new(BackendId("zmk".into()), 0x1d50, 0x615e, Some("1"), "p");
        let info = DeviceInfo {
            id: id.clone(),
            backend: id.backend.clone(),
            name: "Fake".into(),
            kind: DeviceKind::Keyboard,
            firmware: None,
        };
        let bus = EventBus::default();
        let dev = FoxgloveLinkDevice::new(info, link, hello(capabilities), bus.sink(id));
        (dev, fake, bus)
    }

    fn wait_for(mut done: impl FnMut() -> bool) {
        let end = Instant::now() + WAIT;
        while !done() && Instant::now() < end {
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn frames_go_through_the_lighting_trait_only_with_bit_14() {
        let (dev, fake, _bus) = device(capabilities::SETTINGS_RGB);
        let light = dev.lighting().unwrap();
        let red = foxglove_devices::Rgb { r: 255, g: 0, b: 0 };
        assert!(!dev.frames());
        assert!(matches!(
            light.set_frame(&[red; 42]),
            Err(DeviceError::Unsupported)
        ));
        light.release().unwrap();
        assert!(fake.written.lock().unwrap().is_empty(), "nothing sent");

        let (dev, fake, _bus) = device(capabilities::SETTINGS_RGB | capabilities::FRAMES);
        let light = dev.lighting().unwrap();
        assert!(dev.frames());
        let blue = foxglove_devices::Rgb { r: 0, g: 0, b: 255 };
        let mut frame = vec![red; 21];
        frame.extend([blue; 21]);
        light.set_frame(&frame).unwrap();
        light.release().unwrap();
        let sent: Vec<(u8, u8, Vec<u8>)> = fake
            .written
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r[..2] == [protocol::SETTINGS, protocol::settings::FRAME])
            .map(|r| {
                let at = r.get(3).copied().unwrap_or(0);
                (r[2], at, r.get(5..8).unwrap_or(&[]).to_vec())
            })
            .collect();
        assert_eq!(
            sent,
            [
                (0, 0, vec![255, 0, 0]),
                (0, 9, vec![255, 0, 0]),
                // Lamps 18-26: three of the left's, then the right's blue.
                (0, 18, vec![255, 0, 0]),
                (0, 27, vec![0, 0, 255]),
                (protocol::frame::SHOW, 36, vec![0, 0, 255]),
                (protocol::frame::RELEASE, 0, vec![]),
            ]
        );
    }

    #[test]
    fn the_lamp_parts_come_from_the_firmware_with_bit_22() {
        let (dev, fake, _bus) = device(capabilities::SETTINGS_RGB | capabilities::FRAMES);
        assert!(matches!(
            dev.read_lamp_parts(),
            Err(DeviceError::Unsupported)
        ));
        assert!(fake.written.lock().unwrap().is_empty(), "nothing asked");
        let (dev, _fake, _bus) = device(capabilities::FRAMES | capabilities::LAMP_PARTS);
        let parts = dev.read_lamp_parts().unwrap();
        assert_eq!(parts.counts().collect::<Vec<_>>(), [30, 24]);
    }

    #[test]
    fn a_frame_takes_the_boards_own_lamp_count() {
        // Not the Corne: a split board of 30 + 24 LEDs, then a one-piece board of 61.
        let (dev, fake, _bus) = device(capabilities::SETTINGS_RGB | capabilities::FRAMES);
        assert_eq!(dev.lamp_parts(), LampParts::old_firmware());
        dev.set_lamp_parts(LampParts::new(&[30, 24]).unwrap());
        let light = dev.lighting().unwrap();
        let red = foxglove_devices::Rgb { r: 255, g: 0, b: 0 };
        light.set_frame(&[red; 54]).unwrap();
        dev.set_lamp_parts(LampParts::new(&[61]).unwrap());
        light.set_frame(&[red; 61]).unwrap();
        let starts: Vec<(u8, u8, u8)> = fake
            .written
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r[..2] == [protocol::SETTINGS, protocol::settings::FRAME])
            .map(|r| (r[2], r[3], r[4]))
            .collect();
        let show = protocol::frame::SHOW;
        assert_eq!(starts[5], (show, 45, 9), "54 lamps: 6 reports");
        assert_eq!(starts[12], (show, 54, 7), "61 lamps: 7 reports");
        // Past one byte of lamp index: refused, nothing sent.
        let before = fake.written.lock().unwrap().len();
        assert!(light.set_frame(&[red; 257]).is_err());
        assert_eq!(fake.written.lock().unwrap().len(), before);
    }

    #[test]
    fn effects_only_with_bit_19() {
        let scan = protocol::Effect {
            kind: protocol::EffectKind::Scanline,
            colours: vec![[255, 0, 0], [0, 0, 0]],
            clock_ms: 0,
            period_ms: 4000,
            angle_deg: 90.0,
            width_mm: 80.0,
            origin_mm: [0, 0],
        };
        let (dev, fake, _bus) = device(capabilities::FRAMES);
        assert!(!dev.effects());
        assert!(matches!(
            dev.set_effect(&scan),
            Err(DeviceError::Unsupported)
        ));
        assert!(matches!(
            dev.release_effect(),
            Err(DeviceError::Unsupported)
        ));
        assert!(fake.written.lock().unwrap().is_empty(), "nothing sent");

        let (dev, fake, _bus) = device(capabilities::FRAMES | capabilities::EFFECTS);
        let placed = dev
            .place_effect([0, 0, 600, 700], [0, 500, 124, 86], [204, 500, 124, 86])
            .unwrap();
        assert!(!placed.running());
        let reply = dev.set_effect(&scan).unwrap();
        assert!(reply.running());
        assert_eq!(reply.kind, protocol::EffectKind::Scanline as u8);
        assert!(!dev.release_effect().unwrap().running());
        let ops: Vec<u8> = fake
            .written
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r[..2] == [protocol::SETTINGS, protocol::settings::EFFECT])
            .map(|r| r[2])
            .collect();
        assert_eq!(
            ops,
            [
                protocol::effect::PLACE,
                protocol::effect::EFFECT,
                protocol::effect::RELEASE
            ]
        );
        // One that doesn't fit the wire never leaves the PC.
        let three = protocol::Effect {
            colours: vec![[0; 3]; 3],
            ..scan.clone()
        };
        assert!(dev.set_effect(&three).is_err());

        // The lease arbiter's hand-back (`Lighting::release`) ends an effect as well as frames:
        // a frame RELEASE alone leaves an effect running.
        fake.written.lock().unwrap().clear();
        assert!(dev.show_effect(&scan).unwrap().running());
        Lighting::release(&dev).unwrap();
        let ops: Vec<(u8, u8)> = fake
            .written
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r[0] == protocol::SETTINGS)
            .map(|r| (r[1], r[2]))
            .collect();
        assert_eq!(
            ops,
            [
                (protocol::settings::EFFECT, protocol::effect::EFFECT),
                (protocol::settings::EFFECT, protocol::effect::RELEASE),
                (protocol::settings::FRAME, protocol::frame::RELEASE),
            ]
        );
        // Without bit 19 the hand-back is the frame RELEASE alone.
        let (dev, fake, _bus) = device(capabilities::FRAMES);
        Lighting::release(&dev).unwrap();
        let effect_ops = fake
            .written
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r[..2] == [protocol::SETTINGS, protocol::settings::EFFECT])
            .count();
        assert_eq!(effect_ops, 0);
    }

    #[test]
    fn caps_follow_hello_bits() {
        let legacy = caps(&hello(0));
        assert!(legacy.contains(Caps::SCREENS) && !legacy.contains(Caps::LIGHTING));
        let rgb_only = caps(&hello(capabilities::SETTINGS_RGB));
        assert!(rgb_only.contains(Caps::LIGHTING | Caps::SETTINGS | Caps::BEHAVIORS));
        assert!(!rgb_only.contains(Caps::SCREENS));
        assert!(
            caps(&hello(capabilities::PANEL)).contains(Caps::SCREENS | Caps::POWER | Caps::LAYERS)
        );
    }

    #[test]
    fn light_caps_from_hello_and_screens() {
        use crate::protocol::{ScreenDescriptor as D, ScreenKind};
        let screen = |id, kind, parent, ops| D {
            count: 2,
            index: 0,
            id,
            kind,
            half: 0,
            parent,
            width: 160,
            height: 68,
            x: 0,
            y: 0,
            rotation: 0,
            format: D::FORMAT_1BIT,
            ops,
            stride: 20,
            rows_per_report: 1,
            readback_half: None,
            version: 1,
        };
        let h = Hello {
            width: 160,
            height: 68,
            capabilities: capabilities::SETTINGS_RGB | capabilities::SCREENS,
            ..Hello::default()
        };
        let caps = light_caps(
            &h,
            &[
                screen(0, ScreenKind::Display, None, 0),
                screen(1, ScreenKind::Art, Some(0), D::OP_ART | D::OP_SLOTS),
            ],
        );
        assert_eq!(
            caps.chips(true),
            vec![
                "Single-colour RGB",
                "4 onboard effects",
                "Brightness",
                "Screen 160×68 · 1-bit · images & animations"
            ]
        );
        // Older firmware: the HELLO panel, live frames only.
        let old = Hello {
            width: 160,
            height: 68,
            capabilities: capabilities::PANEL,
            ..Hello::default()
        };
        assert_eq!(
            light_caps(&old, &[]).chips(true),
            vec!["Screen 160×68 · 1-bit · live"]
        );
    }

    #[test]
    fn layer_reports_parse_like_the_overlay() {
        // size 4, default layer 0, layers 2 and 3 active
        assert_eq!(
            parse_layers(&report(&[0xFF, 4, 1, 0, 0, 0, 0b1100, 0, 0, 0])),
            Some(vec![0, 2, 3])
        );
        assert_eq!(parse_layers(&report(&[0xFF, 0])), None);
        assert_eq!(parse_layers(&report(&[0xFF, 9])), None);
    }

    #[test]
    fn passes_the_device_contract() {
        let all = capabilities::PANEL | capabilities::ART_RELAY | capabilities::SETTINGS_RGB;
        let (dev, fake, _bus) = device(all);
        dev.set_behaviors(vec![
            BehaviorInfo {
                id: 3,
                param1: 0,
                param2: 0,
                name: "Bootloader".into(),
                hold_ms: 0,
                risk: Risk::Confirm,
            },
            BehaviorInfo {
                id: 4,
                param1: 1,
                param2: 0,
                name: "RGB on".into(),
                hold_ms: 0,
                risk: Risk::Safe,
            },
        ]);
        foxglove_devices::contract::check_device(&dev).unwrap();
        let behaviors = dev.behaviors().unwrap();
        assert!(matches!(
            behaviors.invoke(3, 0, 0, false),
            Err(DeviceError::Denied(_))
        ));
        behaviors.invoke(4, 1, 0, false).unwrap();
        assert_eq!(
            behaviors.invoke(4, 2, 0, false),
            Err(DeviceError::Unsupported)
        );
        let lighting = dev.lighting().unwrap();
        assert_eq!(lighting.state().unwrap().hue, 0x168);
        assert_eq!(
            lighting.set_on(true),
            Err(DeviceError::Unsupported),
            "no &rgb_ug known yet"
        );
        dev.set_underglow(Some(7));
        lighting.set_color(400, 50, 100).unwrap();
        let sent = fake.written.lock().unwrap().last().unwrap().clone();
        assert_eq!(
            &sent[..12],
            &protocol::invoke(7, RGB_COLOR_HSB, (359 << 16) | (50 << 8) | 100, 0)[..12]
        );
        assert_eq!(
            dev.settings().unwrap().get("rgb_sync").unwrap(),
            Some(Value::Bool(true))
        );
        assert_eq!(
            dev.power().unwrap().batteries().unwrap(),
            vec![("right".to_string(), 55)]
        );
    }

    #[test]
    fn halves_agree_on_all_but_brightness() {
        let left = RgbHalf {
            on: true,
            hue: 0,
            sat: 100,
            brightness: 80,
            effect: 0,
            speed: 1,
        };
        assert!(
            halves_agree(&RgbState { left, right: None }),
            "no right half: nothing to wait for"
        );
        let dim = RgbHalf {
            brightness: 30,
            ..left
        };
        assert!(
            halves_agree(&RgbState {
                left,
                right: Some((dim, 1))
            }),
            "a half dimmed on battery still agrees"
        );
        let behind = RgbHalf { hue: 240, ..left };
        assert!(!halves_agree(&RgbState {
            left,
            right: Some((behind, 2))
        }));
        assert!(!halves_agree(&RgbState {
            left,
            right: Some((RgbHalf { on: false, ..left }, 0))
        }));
        assert!(!halves_agree(&RgbState {
            left,
            right: Some((RgbHalf { effect: 3, ..left }, 0))
        }));
    }

    #[test]
    fn underglow_colours_go_through_the_keyboards_calibration() {
        use foxglove_devices::calibration::{Calibration, DeviceCalibration};
        let (dev, fake, _bus) = device(capabilities::PANEL | capabilities::SETTINGS_RGB);
        assert_eq!(
            dev.calibration_key(),
            "1d50:615e",
            "an untrusted serial isn't used"
        );
        let key = "foxglove-link-calibration-test".to_string();
        dev.set_calibration_key(key.clone());
        dev.set_underglow(Some(7));
        let warm = Calibration {
            gain: [1.0, 1.0, 0.5],
            ..Default::default()
        };
        calibration::global().set_device(
            &key,
            Some(DeviceCalibration {
                device: warm.clone(),
                ..Default::default()
            }),
        );
        let lighting = dev.lighting().unwrap();
        lighting.set_color(200, 40, 80).unwrap();
        let (h, s, b) = warm.apply_hsb(200, 40, 80);
        let sent = fake.written.lock().unwrap().last().unwrap().clone();
        assert_eq!(
            &sent[..12],
            &protocol::invoke(
                7,
                RGB_COLOR_HSB,
                (u32::from(h) << 16) | (u32::from(s) << 8) | u32::from(b),
                0
            )[..12]
        );
        // The fake reports hue 0x168, sat 80, brightness 50, not what was sent: it reads back
        // through the calibration's inverse, never as if it were the palette's colour.
        let shown = lighting.state().unwrap();
        assert_eq!(
            (shown.hue, shown.sat, shown.brightness),
            warm.invert_hsb((0x168, 80, 50))
        );
        let mut state = RgbState {
            left: RgbHalf {
                on: true,
                hue: h,
                sat: s,
                brightness: b,
                effect: 0,
                speed: 1,
            },
            right: None,
        };
        dev.asked_state(&mut state);
        assert_eq!(
            (state.left.hue, state.left.sat, state.left.brightness),
            (200, 40, 80),
            "the keyboard's calibrated colour reads back as the palette's"
        );
        calibration::global().set_device(&key, None);
    }

    #[test]
    fn underglow_waits_out_a_busy_keyboard() {
        let (dev, fake, _bus) = device(capabilities::PANEL | capabilities::SETTINGS_RGB);
        dev.set_underglow(Some(7));
        let lighting = dev.lighting().unwrap();
        fake.busy.store(3, Ordering::Relaxed);
        lighting.set_on(true).unwrap();
        let invokes = |f: &Fake| {
            f.written
                .lock()
                .unwrap()
                .iter()
                .filter(|w| w[1] == protocol::settings::INVOKE)
                .count()
        };
        assert_eq!(invokes(&fake), 4, "three busy answers, then OK");
        fake.busy.store(UNDERGLOW_BUSY_TRIES + 1, Ordering::Relaxed);
        assert!(
            matches!(lighting.set_on(false), Err(DeviceError::Busy(_))),
            "gives up after the retries"
        );
    }

    #[test]
    fn panel_size_is_checked() {
        let (dev, _fake, _bus) = device(capabilities::PANEL);
        let screens = dev.screens().unwrap();
        assert!(screens.send_panel(&[0; 8 * 52], 64, 52).is_ok());
        assert!(screens.send_panel(&[0; 10], 64, 52).is_err());
        assert!(screens.send_panel(&[0; 8 * 50], 64, 50).is_err());
    }

    /// #373: the left's own battery rides on CONN_STATE, only with the capability bit.
    #[test]
    fn conn_state_carries_the_left_battery_when_the_firmware_says_so() {
        let left = |caps: u32| {
            let (_dev, fake, bus) = device(capabilities::PANEL | caps);
            let events = bus.subscribe(false);
            fake.push(&[0xD7, 0x84, 0, 1, 1, 0, 1, 3, 72]);
            let end = Instant::now() + WAIT;
            let mut seen = None;
            while Instant::now() < end {
                match events.recv_timeout(Duration::from_millis(50)) {
                    Ok(e) => match e.event {
                        DeviceEvent::Battery { part, percent } if part == "left" => {
                            seen = Some(percent)
                        }
                        DeviceEvent::Link { .. } => {}
                        _ => continue,
                    },
                    Err(_) => continue,
                }
                if seen.is_some() || caps == 0 {
                    // Without the bit, the Link event is the last one this report gives.
                    break;
                }
            }
            seen
        };
        assert_eq!(left(capabilities::LEFT_BATTERY), Some(72));
        assert_eq!(
            left(0),
            None,
            "older firmware: byte 8 isn't a battery level"
        );
    }

    /// The right's level arrives in later HELLOs (its first reading comes once it has joined,
    /// and each change sends HELLO again): each new level is a battery event, a repeat or "no
    /// reading" (0xff, or 0 after the right drops) isn't.
    #[test]
    fn unsolicited_hellos_carry_the_right_battery() {
        let (_dev, fake, bus) = device(capabilities::PANEL);
        let events = bus.subscribe(false);
        for level in [80, 0xff, 66, 66, 0, 55] {
            fake.push(&[0xD5, 0x81, 1, 64, 52, 0, 0, 2, level]);
        }
        let mut seen = Vec::new();
        let end = Instant::now() + WAIT;
        while seen.len() < 2 && Instant::now() < end {
            if let Ok(e) = events.recv_timeout(Duration::from_millis(50)) {
                if let DeviceEvent::Battery { part, percent } = e.event {
                    if part == "right" {
                        seen.push(percent);
                    }
                }
            }
        }
        assert_eq!(seen, vec![66, 55], "80 was already known from the connect");
    }

    #[test]
    fn reports_become_events_and_keys_need_opt_in() {
        let (dev, fake, bus) = device(capabilities::PANEL);
        let plain = bus.subscribe(false);
        let keys = bus.subscribe(true);
        fake.push(&[0xFF, 4, 1, 0, 0, 0, 0b10, 0, 0, 0]);
        fake.push(&[0xF1, 2, 3, 1]);
        wait_for(|| dev.layers().unwrap().active().unwrap() == vec![0, 1]);
        assert_eq!(dev.layers().unwrap().active().unwrap(), vec![0, 1]);
        let mut saw_key = false;
        let end = Instant::now() + WAIT;
        while !saw_key && Instant::now() < end {
            if let Ok(e) = keys.recv_timeout(Duration::from_millis(50)) {
                saw_key = e.event
                    == DeviceEvent::Key {
                        row: 2,
                        col: 3,
                        pressed: true,
                    };
            }
        }
        assert!(saw_key);
        assert!(plain
            .try_iter()
            .all(|e| !matches!(e.event, DeviceEvent::Key { .. })));
        let fact = format!("device.{}.layers", Device::info(&dev).id);
        assert_eq!(bus.facts().get(&fact), Some(Value::Bytes(vec![0, 1])));
    }
}
