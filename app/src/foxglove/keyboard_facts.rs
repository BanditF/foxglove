//! The keyboard's state as short facts for plugins and the panel (#17), mirrored from the
//! device events the firmware already sends; nothing polls:
//!
//! - `keyboard.connected` (Bool)
//! - `keyboard.layer` (Int, the highest active layer) and `keyboard.layers` (Bytes, all of them)
//! - `keyboard.game_mode` (Bool, #1121): any active layer is named GAME... (GAME, GAME+), by the
//!   remembered layer names, as the wallpaper sync reads it. A shared fact only: nothing here
//!   acts on `gpu.reserved`, and a GPU reservation never follows it (contract §8.9)
//! - `keyboard.right.battery` (Int, percent; `keyboard.<part>.battery` for any part reported)
//! - `keyboard.link` (Text, `usb` or `ble`), `keyboard.profile` (Int, 1-based Bluetooth profile)
//!   and `keyboard.profile_connected` (Bool)
//! - `keyboard.lighting.on` (Bool), `keyboard.lighting.color` (Text, `#rrggbb`: the chosen hue
//!   and saturation at full brightness), `keyboard.lighting.brightness` (Int, 0-100),
//!   `keyboard.lighting.effect` (Text, the effect's name, or its id when unknown) and
//!   `keyboard.lighting.rainbow` (Bool, the effect makes its own colours), from the keyboard's
//!   report of its lights (#40: the live feed gives them to the wallpaper)
//!
//! While Windows Dynamic Lighting holds the keyboard and Foxglove draws through its own
//! LampArray (#789, `lights_owner::through_windows`), the keyboard's report doesn't change, so
//! the lighting facts follow what Foxglove draws instead ([`Sources`]): a running scene's look
//! first (its own colour, `scene_triggers::effect_colour`, set once per look, never per frame;
//! the effect is the look's name), else the Keyboard card's pick. When Windows lets go, the
//! keyboard's last report is the source again. All on events: the owner report, a card pick, a
//! scene's look.
//!
//! The same values stay under `device.<id>.*` for anything that needs a particular device. On a
//! disconnect everything but `keyboard.connected` goes, as it's unknown until the keyboard is
//! back; the link is read once when it reconnects, and the layer is set from what the device
//! last knew (the default layer when nothing, #1044), then both follow the firmware's events.

use foxglove_devices::facts::{Facts, Value};
use foxglove_devices::{DeviceEvent, LightingState};

/// The backend of the keyboard with Foxglove's host link (see `devices::device`).
const HOST_KEYBOARD: &str = "zmk";

pub fn start() {
    // Supervised (#587): the first attempt gets the subscription made here, a restart its own.
    let events = super::services::First::new(super::devices::bus().subscribe(false));
    let _ = super::services::spawn_supervised("keyboard-facts", move || {
        let events = events.take_or(|| super::devices::bus().subscribe(false));
        let facts = super::devices::bus().facts();
        for e in events {
            if e.device.backend.0 != HOST_KEYBOARD {
                continue;
            }
            let connected = e.event == DeviceEvent::Connected;
            publish(facts, &e.event);
            if connected {
                // #1044: the layer as it is on connect. The firmware only reports a layer change,
                // and the link drops a report that repeats the last one, so a keyboard back on
                // the layer it left (the base layer, mostly) never sends one: the device's last
                // known layers, else the default layer.
                let known = super::devices::device().and_then(|d| {
                    foxglove_devices::Device::layers(&*d).and_then(|l| l.active().ok())
                });
                publish(facts, &connect_layers(known));
                // One read when it connects; the firmware reports changes after that.
                if let Some(info) = super::devices::device()
                    .and_then(|d| foxglove_devices::Device::power(&*d).and_then(|p| p.info().ok()))
                {
                    if let (Some(output), Some(profile), Some(connected)) =
                        (info.output, info.profile, info.profile_connected)
                    {
                        publish(
                            facts,
                            &DeviceEvent::Link {
                                output,
                                profile,
                                connected,
                            },
                        );
                    }
                }
            }
        }
    });
}

/// The layers to publish when the keyboard connects (#1044): what the device knows, or the
/// default layer while it knows none.
fn connect_layers(known: Option<Vec<u8>>) -> DeviceEvent {
    DeviceEvent::LayerChanged {
        active: known.filter(|l| !l.is_empty()).unwrap_or_else(|| vec![0]),
    }
}

fn publish(facts: &Facts, event: &DeviceEvent) {
    publish_with(facts, &mut sources(), event);
}

fn publish_with(facts: &Facts, sources: &mut Sources, event: &DeviceEvent) {
    match event {
        DeviceEvent::Connected => {
            facts.set("keyboard.connected", Value::Bool(true));
            sources.connected = true;
            // The Scenes page's map lights the keyboard now (#487).
            super::scene_room::changed();
        }
        DeviceEvent::Disconnected { .. } => {
            sources.disconnected();
            facts.remove_prefix("keyboard.", &["keyboard.connected"]);
            facts.set("keyboard.connected", Value::Bool(false));
            super::scene_room::changed();
        }
        DeviceEvent::LayerChanged { active } => {
            layers(facts, active, &super::layer_names::remembered());
        }
        DeviceEvent::Battery { part, percent } => facts.set(
            format!("keyboard.{part}.battery"),
            Value::Int((*percent).into()),
        ),
        DeviceEvent::Link {
            output,
            profile,
            connected,
        } => {
            facts.set("keyboard.link", Value::Text(output.to_ascii_lowercase()));
            facts.set("keyboard.profile", Value::Int(i64::from(*profile) + 1));
            facts.set("keyboard.profile_connected", Value::Bool(*connected));
        }
        // The keyboard's report of its lights (every RGB_STATE read or event, #605): the
        // case's "Match keyboard" follows it (#616). Its facts are `device.<id>.lighting.*`.
        DeviceEvent::LightingChanged { state } => {
            sources.connected = true;
            sources.firmware = Some(state.clone());
            sources.publish(facts);
            super::pc_lights::keyboard_lights(state);
        }
        DeviceEvent::Key { .. } => {}
    }
}

/// The layer facts for `active`, with game mode read from the layers' `names`.
pub(super) fn layers(facts: &Facts, active: &[u8], names: &[Option<String>]) {
    facts.set("keyboard.layers", Value::Bytes(active.to_vec()));
    facts.set(
        "keyboard.layer",
        Value::Int(active.iter().copied().max().unwrap_or(0).into()),
    );
    facts.set(
        "keyboard.game_mode",
        Value::Bool(super::wallpaper_sync::game_mode(active, names)),
    );
}

/// The lights as the facts give them: what [`lighting`] sets.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Look {
    pub on: bool,
    /// The colour at full brightness.
    pub color: [u8; 3],
    /// 0-100.
    pub brightness: u8,
    /// The effect's name (a scene's: its look's name).
    pub effect: String,
    /// The effect makes its own colours.
    pub rainbow: bool,
}

impl Look {
    /// A keyboard's lighting state: its hue and saturation at full brightness, its effect by
    /// the underglow table's name (its id when unknown).
    pub fn of(state: &LightingState) -> Look {
        use super::effect_controls::{spec, EffectClass};
        let color = foxglove_devices::calibration::hsv_to_rgb(
            f32::from(state.hue),
            f32::from(state.sat.min(100)) / 100.0,
            1.0,
        )
        .map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8);
        let effect = spec(EffectClass::Underglow, &state.effect.id);
        Look {
            on: state.on,
            color,
            brightness: state.brightness.min(100),
            effect: effect.map_or_else(|| state.effect.id.clone(), |e| e.name.to_string()),
            rainbow: effect.is_some_and(|e| e.rainbow()),
        }
    }

    /// The Keyboard card's pick (#764, #768), lit: as the keyboard would report it.
    pub fn card(k: &super::pc_lights::KeyboardLights) -> Look {
        Look::of(&LightingState {
            on: true,
            hue: k.hue,
            sat: k.sat,
            brightness: k.brightness,
            effect: foxglove_devices::Effect {
                id: k.effect.to_string(),
                params: Vec::new(),
            },
            speed: 0,
            settled: true,
        })
    }

    /// A scene's look (#789): one stable colour for it, its own colour (a one-colour effect's
    /// colour, a palette's first: `scene_triggers::effect_colour`), at full brightness with its
    /// brightness apart; the effect is the look's name. Never a rainbow: the colour is there.
    pub fn scene(name: &str, effect: &foxglove_scene::Effect) -> Look {
        let [r, g, b] = super::scene_triggers::effect_colour(effect).0;
        let max = r.max(g).max(b);
        Look {
            on: max > 0,
            color: if max == 0 {
                [0, 0, 0]
            } else {
                super::scene_triggers::bright(r, g, b).0
            },
            brightness: ((u16::from(max) * 100 + 127) / 255) as u8,
            effect: name.to_string(),
            rainbow: false,
        }
    }
}

/// Where the lighting facts come from (#789): the keyboard's own report, or, while Windows
/// Dynamic Lighting holds the keyboard and Foxglove draws through its LampArray, what Foxglove
/// draws there (a running scene's look first, then the Keyboard card's pick).
#[derive(Debug, Default)]
pub struct Sources {
    /// The keyboard's last report of its lights (RGB_STATE).
    firmware: Option<LightingState>,
    /// The Keyboard card's pick drawn through Windows, while held.
    card: Option<Look>,
    /// The running scene's look, while one runs.
    scene: Option<Look>,
    /// Windows holds the keyboard and Foxglove draws through its LampArray.
    held: bool,
    /// The keyboard is there (its facts are known).
    connected: bool,
}

impl Sources {
    /// What the facts say now; `None` while nothing is known.
    pub fn shown(&self) -> Option<Look> {
        if !self.connected {
            return None;
        }
        let firmware = || self.firmware.as_ref().map(Look::of);
        if self.held {
            self.scene
                .clone()
                .or_else(|| self.card.clone())
                .or_else(firmware)
        } else {
            firmware()
        }
    }

    /// Windows holds the keyboard (or let go). Letting go forgets the card's pick: the keyboard's
    /// own report is the source again.
    pub fn set_held(&mut self, held: bool) {
        if self.held && !held {
            self.card = None;
        }
        self.held = held;
    }

    /// The card drew `pick` through Windows (`None`: lights off, in the colour shown before).
    /// Only ever called while held.
    pub fn card_drawn(&mut self, pick: Option<&super::pc_lights::KeyboardLights>) {
        self.held = true;
        self.card = match pick {
            Some(k) => Some(Look::card(k)),
            None => self
                .card
                .clone()
                .or_else(|| self.firmware.as_ref().map(Look::of))
                .map(|l| Look { on: false, ..l }),
        };
    }

    /// A scene's look began (`None`: the scene ended).
    pub fn scene_look(&mut self, look: Option<Look>) {
        self.scene = look;
    }

    fn disconnected(&mut self) {
        self.connected = false;
        self.firmware = None;
        self.card = None;
        self.held = false;
    }

    /// Sets the facts to [`Sources::shown`]; only what changed is sent on.
    fn publish(&self, facts: &Facts) {
        if let Some(look) = self.shown() {
            set_look(facts, &look);
        }
    }
}

static SOURCES: std::sync::Mutex<Sources> = std::sync::Mutex::new(Sources {
    firmware: None,
    card: None,
    scene: None,
    held: false,
    connected: false,
});

fn sources() -> std::sync::MutexGuard<'static, Sources> {
    SOURCES.lock().unwrap_or_else(|e| e.into_inner())
}

/// Changes a source and sets the app's facts to match.
fn update(change: impl FnOnce(&mut Sources)) {
    let mut s = sources();
    change(&mut s);
    s.publish(super::devices::bus().facts());
}

/// Windows Dynamic Lighting holds the keyboard with Foxglove drawing through its LampArray, or
/// let go (`lights_owner`, on the owner report or a keyboard connect).
pub fn held(held: bool) {
    update(|s| s.set_held(held));
}

/// The Keyboard card drew `pick` through the keyboard's LampArray (`lights_page`, #764/#768).
pub fn card_drawn(pick: Option<super::pc_lights::KeyboardLights>) {
    update(|s| s.card_drawn(pick.as_ref()));
}

/// A scene's look began, `name` its name (`scene_run`: its start and each new look).
pub fn scene_look(name: &str, effect: &foxglove_scene::Effect) {
    let look = Look::scene(name, effect);
    update(|s| s.scene_look(Some(look)));
}

/// The scene ended.
pub fn scene_ended() {
    update(|s| s.scene_look(None));
}

/// The keyboard's lights as `keyboard.lighting.*` (tests: straight from a report).
#[cfg(test)]
fn lighting(facts: &Facts, state: &LightingState) {
    set_look(facts, &Look::of(state));
}

/// `look` as `keyboard.lighting.*`. Only what changed is sent on, so a new colour is one change.
fn set_look(facts: &Facts, look: &Look) {
    let [r, g, b] = look.color;
    facts.set("keyboard.lighting.on", Value::Bool(look.on));
    facts.set(
        "keyboard.lighting.color",
        Value::Text(format!("#{r:02x}{g:02x}{b:02x}")),
    );
    facts.set(
        "keyboard.lighting.brightness",
        Value::Int(look.brightness.min(100).into()),
    );
    facts.set("keyboard.lighting.effect", Value::Text(look.effect.clone()));
    facts.set("keyboard.lighting.rainbow", Value::Bool(look.rainbow));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #1121: game mode follows the GAME layers by name, is cleared on a disconnect with the
    /// other layer facts, and is only a fact (nothing here reaches a GPU reservation).
    #[test]
    fn game_mode_is_a_fact_of_the_game_layers() {
        let facts = Facts::default();
        let names: Vec<Option<String>> = ["QWERTY", "NUMBER", "SYMBOL", "GAME", "GAME+"]
            .iter()
            .map(|n| Some(n.to_string()))
            .collect();
        layers(&facts, &[0], &names);
        assert_eq!(facts.get("keyboard.game_mode"), Some(Value::Bool(false)));
        layers(&facts, &[0, 3], &names);
        assert_eq!(facts.get("keyboard.game_mode"), Some(Value::Bool(true)));
        layers(&facts, &[0, 3, 4], &names);
        assert_eq!(facts.get("keyboard.game_mode"), Some(Value::Bool(true)));
        layers(&facts, &[0, 1], &names);
        assert_eq!(facts.get("keyboard.game_mode"), Some(Value::Bool(false)));
        // Unnamed layers are not game mode.
        layers(&facts, &[0, 3], &[]);
        assert_eq!(facts.get("keyboard.game_mode"), Some(Value::Bool(false)));
        assert_eq!(facts.get("gpu.reserved"), None);
        // A disconnect forgets it with the other layer facts: unknown, not "off".
        let mut s = Sources::default();
        publish_with(
            &facts,
            &mut s,
            &DeviceEvent::Disconnected {
                reason: "unplugged".into(),
            },
        );
        assert_eq!(facts.get("keyboard.game_mode"), None);
    }

    #[test]
    fn events_become_keyboard_facts_and_a_disconnect_clears_them() {
        let facts = Facts::default();
        let mut s = Sources::default();
        let mut publish = |f: &Facts, e: &DeviceEvent| publish_with(f, &mut s, e);
        publish(&facts, &DeviceEvent::Connected);
        publish(
            &facts,
            &DeviceEvent::LayerChanged {
                active: vec![0, 2, 3],
            },
        );
        publish(
            &facts,
            &DeviceEvent::Battery {
                part: "right".into(),
                percent: 61,
            },
        );
        publish(
            &facts,
            &DeviceEvent::Link {
                output: "BLE".into(),
                profile: 1,
                connected: true,
            },
        );
        assert_eq!(facts.get("keyboard.layer"), Some(Value::Int(3)));
        assert_eq!(
            facts.get("keyboard.layers"),
            Some(Value::Bytes(vec![0, 2, 3]))
        );
        assert_eq!(facts.get("keyboard.right.battery"), Some(Value::Int(61)));
        assert_eq!(facts.get("keyboard.link"), Some(Value::Text("ble".into())));
        assert_eq!(
            facts.get("keyboard.profile"),
            Some(Value::Int(2)),
            "1-based, as the app shows it"
        );
        publish(
            &facts,
            &DeviceEvent::Disconnected {
                reason: "unplugged".into(),
            },
        );
        assert_eq!(facts.get("keyboard.connected"), Some(Value::Bool(false)));
        assert_eq!(facts.get("keyboard.layer"), None);
        assert_eq!(facts.get("keyboard.right.battery"), None);
        assert_eq!(facts.get("keyboard.link"), None);
    }

    /// #1044: a connect, and a reconnect after a disconnect wiped the facts, publish the layer
    /// at once, with no layer report from the keyboard: the device's last known layers, else
    /// the default layer.
    #[test]
    fn a_connect_and_a_reconnect_publish_the_layer() {
        let facts = Facts::default();
        let mut s = Sources::default();
        fn connect(f: &Facts, s: &mut Sources, known: Option<Vec<u8>>) {
            publish_with(f, s, &DeviceEvent::Connected);
            publish_with(f, s, &connect_layers(known));
        }
        // First connect, nothing known: the default layer.
        connect(&facts, &mut s, None);
        assert_eq!(facts.get("keyboard.layer"), Some(Value::Int(0)));
        assert_eq!(facts.get("keyboard.layers"), Some(Value::Bytes(vec![0])));
        connect(&facts, &mut s, Some(Vec::new()));
        assert_eq!(facts.get("keyboard.layer"), Some(Value::Int(0)));
        // Unplugged: the layer goes with the rest.
        publish_with(
            &facts,
            &mut s,
            &DeviceEvent::Disconnected {
                reason: "unplugged".into(),
            },
        );
        assert_eq!(facts.get("keyboard.layer"), None);
        // Back on the layer it left: no report comes, the known layers do.
        connect(&facts, &mut s, Some(vec![0, 3]));
        assert_eq!(facts.get("keyboard.connected"), Some(Value::Bool(true)));
        assert_eq!(facts.get("keyboard.layer"), Some(Value::Int(3)));
        assert_eq!(facts.get("keyboard.layers"), Some(Value::Bytes(vec![0, 3])));
    }

    fn lights(hue: u16, sat: u8, effect: &str) -> LightingState {
        LightingState {
            on: true,
            hue,
            sat,
            brightness: 60,
            effect: foxglove_devices::Effect {
                id: effect.into(),
                params: Vec::new(),
            },
            speed: 2,
            settled: true,
        }
    }

    #[test]
    fn the_lights_become_facts_with_the_colour_at_full_brightness() {
        let facts = Facts::default();
        lighting(&facts, &lights(25, 100, "0"));
        let text = |k: &str| match facts.get(k) {
            Some(Value::Text(t)) => t,
            other => panic!("{k}: {other:?}"),
        };
        assert_eq!(text("keyboard.lighting.color"), "#ff6a00");
        assert_eq!(text("keyboard.lighting.effect"), "Solid");
        assert_eq!(facts.get("keyboard.lighting.on"), Some(Value::Bool(true)));
        assert_eq!(
            facts.get("keyboard.lighting.brightness"),
            Some(Value::Int(60))
        );
        assert_eq!(
            facts.get("keyboard.lighting.rainbow"),
            Some(Value::Bool(false))
        );

        // White, a rainbow effect, and an effect the table doesn't know.
        lighting(&facts, &lights(200, 0, "2"));
        assert_eq!(text("keyboard.lighting.color"), "#ffffff");
        assert_eq!(text("keyboard.lighting.effect"), "Spectrum");
        assert_eq!(
            facts.get("keyboard.lighting.rainbow"),
            Some(Value::Bool(true))
        );
        lighting(&facts, &lights(0, 100, "42"));
        assert_eq!(text("keyboard.lighting.color"), "#ff0000");
        assert_eq!(text("keyboard.lighting.effect"), "42");
    }

    #[test]
    fn a_new_colour_is_one_change_and_a_disconnect_clears_the_lights() {
        let facts = Facts::default();
        lighting(&facts, &lights(25, 100, "0"));
        let changes = facts.subscribe("keyboard.*");
        lighting(&facts, &lights(120, 100, "0"));
        let got: Vec<_> = changes.try_iter().map(|c| c.key).collect();
        assert_eq!(got, ["keyboard.lighting.color"]);
        publish_with(
            &facts,
            &mut Sources::default(),
            &DeviceEvent::Disconnected {
                reason: "unplugged".into(),
            },
        );
        assert_eq!(facts.get("keyboard.lighting.color"), None);
        assert_eq!(facts.get("keyboard.lighting.on"), None);
    }

    fn text(facts: &Facts, k: &str) -> String {
        match facts.get(k) {
            Some(Value::Text(t)) => t,
            other => panic!("{k}: {other:?}"),
        }
    }

    /// The keyboard there, reporting orange Solid.
    fn orange(facts: &Facts) -> Sources {
        let mut s = Sources::default();
        publish_with(facts, &mut s, &DeviceEvent::Connected);
        publish_with(
            facts,
            &mut s,
            &DeviceEvent::LightingChanged {
                state: lights(25, 100, "0"),
            },
        );
        assert_eq!(text(facts, "keyboard.lighting.color"), "#ff6a00");
        s
    }

    fn pick(
        hue: u16,
        sat: u8,
        brightness: u8,
        effect: u8,
    ) -> super::super::pc_lights::KeyboardLights {
        super::super::pc_lights::KeyboardLights {
            hue,
            sat,
            brightness,
            effect,
        }
    }

    /// #789: while Windows holds the keyboard, the Keyboard card's pick drawn through its
    /// LampArray is what the facts say: colour at full brightness, brightness, effect, rainbow
    /// and on/off; a report from the keyboard meanwhile doesn't undo it. Only changes go on.
    #[test]
    fn held_the_facts_follow_the_cards_pick() {
        let facts = Facts::default();
        let mut s = orange(&facts);
        s.set_held(true);
        s.publish(&facts);
        assert_eq!(
            text(&facts, "keyboard.lighting.color"),
            "#ff6a00",
            "nothing drawn yet: the keyboard's report"
        );
        let changes = facts.subscribe("keyboard.*");
        // Solid blue, Apply colour (the sitting's case).
        s.card_drawn(Some(&pick(240, 100, 40, 0)));
        s.publish(&facts);
        assert_eq!(text(&facts, "keyboard.lighting.color"), "#0000ff");
        assert_eq!(
            facts.get("keyboard.lighting.brightness"),
            Some(Value::Int(40))
        );
        let got: Vec<_> = changes.try_iter().map(|c| c.key).collect();
        assert_eq!(
            got,
            ["keyboard.lighting.color", "keyboard.lighting.brightness"],
            "only what changed"
        );
        // Spectrum and Swirl make their own colours.
        for (effect, name) in [(2, "Spectrum"), (3, "Swirl")] {
            s.card_drawn(Some(&pick(240, 100, 40, effect)));
            s.publish(&facts);
            assert_eq!(text(&facts, "keyboard.lighting.effect"), name);
            assert_eq!(
                facts.get("keyboard.lighting.rainbow"),
                Some(Value::Bool(true))
            );
        }
        s.card_drawn(Some(&pick(240, 100, 40, 1)));
        s.publish(&facts);
        assert_eq!(text(&facts, "keyboard.lighting.effect"), "Breathe");
        assert_eq!(
            facts.get("keyboard.lighting.rainbow"),
            Some(Value::Bool(false))
        );
        // The keyboard reports its own (unchanged) lights meanwhile: the pick stays.
        publish_with(
            &facts,
            &mut s,
            &DeviceEvent::LightingChanged {
                state: lights(25, 100, "0"),
            },
        );
        assert_eq!(text(&facts, "keyboard.lighting.color"), "#0000ff");
        // Lights off keeps the colour.
        s.card_drawn(None);
        s.publish(&facts);
        assert_eq!(facts.get("keyboard.lighting.on"), Some(Value::Bool(false)));
        assert_eq!(text(&facts, "keyboard.lighting.color"), "#0000ff");
    }

    /// #789: a scene drawn through the held keyboard's LampArray sets the facts from its look:
    /// its own colour (a palette's first), at full brightness, the look's name as the effect;
    /// once per look, not per frame. It comes before the card's pick; its end gives the card's
    /// pick back.
    #[test]
    fn held_the_facts_follow_the_scenes_look() {
        use foxglove_scene::{Effect, Rgb};
        let facts = Facts::default();
        let mut s = orange(&facts);
        s.card_drawn(Some(&pick(240, 100, 40, 0)));
        s.publish(&facts);
        let changes = facts.subscribe("keyboard.*");
        let scanline = Effect::Scanline {
            colour: Rgb([0, 128, 128]),
            background: Rgb::BLACK,
            angle_deg: 0.0,
            width_mm: 80.0,
            period_ms: 4000,
        };
        s.scene_look(Some(Look::scene("Bandit scanline", &scanline)));
        s.publish(&facts);
        assert_eq!(text(&facts, "keyboard.lighting.color"), "#00ffff");
        assert_eq!(
            facts.get("keyboard.lighting.brightness"),
            Some(Value::Int(50))
        );
        assert_eq!(text(&facts, "keyboard.lighting.effect"), "Bandit scanline");
        assert_eq!(
            facts.get("keyboard.lighting.rainbow"),
            Some(Value::Bool(false))
        );
        assert_eq!(facts.get("keyboard.lighting.on"), Some(Value::Bool(true)));
        let first = changes.try_iter().count();
        assert!(first > 0);
        // The same look again (a frame, a repeat): nothing changes.
        s.scene_look(Some(Look::scene("Bandit scanline", &scanline)));
        s.publish(&facts);
        assert_eq!(changes.try_iter().count(), 0, "not per frame");
        // A new look: a palette's first colour.
        let sweep = Effect::Sweep {
            stops: vec![Rgb([255, 0, 0]), Rgb([0, 255, 0])],
            angle_deg: 0.0,
            period_ms: 3000,
        };
        s.scene_look(Some(Look::scene("Ember", &sweep)));
        s.publish(&facts);
        assert_eq!(text(&facts, "keyboard.lighting.color"), "#ff0000");
        assert_eq!(text(&facts, "keyboard.lighting.effect"), "Ember");
        // The scene ends: the card's pick again.
        s.scene_look(None);
        s.publish(&facts);
        assert_eq!(text(&facts, "keyboard.lighting.color"), "#0000ff");
        assert_eq!(text(&facts, "keyboard.lighting.effect"), "Solid");
    }

    /// #789: not held, a running scene leaves the facts to the keyboard's report; when Windows
    /// lets go, the keyboard's last report wins again and the card's pick is forgotten; a
    /// disconnect forgets it all.
    #[test]
    fn released_the_keyboards_report_wins() {
        use foxglove_scene::{Effect, Rgb};
        let facts = Facts::default();
        let mut s = orange(&facts);
        let breathe = Effect::Breathe {
            from: Rgb::BLACK,
            to: Rgb([0, 255, 0]),
            period_ms: 4000,
        };
        s.scene_look(Some(Look::scene("Calm", &breathe)));
        s.publish(&facts);
        assert_eq!(
            text(&facts, "keyboard.lighting.color"),
            "#ff6a00",
            "the keyboard draws the scene itself: its report says"
        );
        s.scene_look(None);
        s.card_drawn(Some(&pick(240, 100, 40, 2)));
        s.publish(&facts);
        assert_eq!(text(&facts, "keyboard.lighting.effect"), "Spectrum");
        let changes = facts.subscribe("keyboard.*");
        s.set_held(false);
        s.publish(&facts);
        assert_eq!(text(&facts, "keyboard.lighting.color"), "#ff6a00");
        assert_eq!(text(&facts, "keyboard.lighting.effect"), "Solid");
        assert_eq!(
            facts.get("keyboard.lighting.brightness"),
            Some(Value::Int(60))
        );
        assert_eq!(
            facts.get("keyboard.lighting.rainbow"),
            Some(Value::Bool(false))
        );
        assert!(changes.try_iter().count() > 0);
        // Held again, nothing drawn yet: still the keyboard's report, not the old pick.
        s.set_held(true);
        s.publish(&facts);
        assert_eq!(text(&facts, "keyboard.lighting.effect"), "Solid");
        // The keyboard's next report goes through as before.
        s.set_held(false);
        publish_with(
            &facts,
            &mut s,
            &DeviceEvent::LightingChanged {
                state: lights(120, 100, "0"),
            },
        );
        assert_eq!(text(&facts, "keyboard.lighting.color"), "#00ff00");
        publish_with(
            &facts,
            &mut s,
            &DeviceEvent::Disconnected {
                reason: "unplugged".into(),
            },
        );
        s.publish(&facts);
        assert_eq!(facts.get("keyboard.lighting.color"), None);
    }
}
