//! The Scenes page (#487, epic #481): the room map with a live preview of the scene, the six
//! effects and their colour or palette, speed, direction and width, Start and Stop, and the
//! presets (built-in and the user's own).
//!
//! It is its own sidebar page, after Lights: the IA review keeps feature pages and organises
//! Lights by device (Keyboard, then Other devices), while a scene runs across every device at
//! once, the monitors included.
//!
//! [`page`] gathers the state and acts on clicks; [`draw`] paints a [`View`] and returns
//! [`Intent`]s, so the previews and tests draw it with no devices. Starting, stopping and a
//! running scene's look all go through `scene_run`, which owns the clock, the outputs' pacing
//! and who holds the lights (#485).
//!
//! Screen glow (#496, `screen_glow`) is the last effect. Its first Start shows the permission
//! card in place of the Effect card's controls; only "Allow and start" gives the yes, and no
//! other path asks or starts it without one. While it runs the map draws each screen's live edge
//! strips and a line from each lit device to the stretch it follows.
//!
//! The preview animates only while the page is drawn: each frame asks for the next one
//! [`PREVIEW_INTERVAL`] later (20 a second at most), and not while the window is minimised or
//! the PC is away (locked, display off). Another page, a closed window or a still effect asks
//! for nothing. The room is built again only when something it comes from changed
//! (`scene_room::generation`) or the page is shown again.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use egui::{Color32, Pos2, RichText, Sense, Stroke};
use foxglove_scene::room::{Placed, PlacedLight};
use foxglove_scene::{
    Ambient, Clock, DeviceKind, Effect, LightRole, Point, Rgb, RoomMap, SceneTime,
};

use super::lights_page::{full_slider, swatch, value_row, SWATCHES, TWO_COLUMNS};
use super::scene_looks::{
    self as looks, Background, Chosen, Direction, Kind, Look, Preset, Saved, WallpaperLook,
    PALETTES,
};
use super::screen_glow as glow;
use super::settings_ui::{card, split_row};
use super::theme;

/// The preview's frame interval: 20 frames a second at most.
pub const PREVIEW_INTERVAL: Duration = Duration::from_millis(50);

/// What the page shows.
#[derive(Clone, Debug, PartialEq)]
pub struct View {
    pub room: RoomMap,
    /// The keyboard answers, so a scene lights it.
    pub keyboard_lit: bool,
    /// It takes SET_FRAME frames (#529): each half (or LED) its own colour, not one for both.
    pub keyboard_frames: bool,
    /// How the keyboard takes a scene, from its zones (#611): "6 zones per half", "a colour
    /// per key"; `None` while they're unknown.
    pub keyboard_zones: Option<String>,
    /// The room ids of the other devices a scene lights (boards with a driver, Dynamic
    /// Lighting devices).
    pub lit: Vec<String>,
    pub look: Look,
    pub saved: Vec<Preset>,
    /// The running scene's name.
    pub running: Option<String>,
    /// Stop was pressed and the lights are being given back.
    pub stopping: bool,
    /// The keyboard lights self-heal's state (dark while the scene runs).
    pub heal: super::lights_heal::View,
    /// The user moved a device, so "Reset positions" shows.
    pub moved: bool,
    /// The moment the preview shows, on the running scene's clock (or the page's).
    pub t: SceneTime,
    /// "Art screen too": a wallpaper's look also shows its art-screen art (#488, #560).
    pub with_art: bool,
    /// The looks of the wallpapers that describe themselves (#562); none hides the group.
    pub wallpapers: Vec<WallpaperLook>,
    /// The wallpapers on screen now (ids), so their looks say "On screen".
    pub on_screen: Vec<String>,
    /// The Triggers card's boxes (#489), the active profile's.
    pub triggers: Triggers,
    /// The active profile's name, as the picker says it.
    pub profile: String,
    /// "Keep running on the keyboard" (#542): quitting leaves the scene's effect on it.
    pub keep_on_keyboard: bool,
    /// The keyboard's LEDs are placed (its profile's map is checked, or "Find my LEDs" saved
    /// one). A keyboard that takes frames without it gets the page's prompt.
    pub leds_placed: bool,
    /// Its profile ships a map no one has checked yet: scenes use it, the prompt asks for the
    /// dot test once.
    pub leds_shipped: bool,
    /// What Windows lets the scene do with Dynamic Lighting devices (#503), when it limits it:
    /// the text, and whether a device stopped (shown as a warning).
    pub lamp_note: Option<(String, bool)>,
    /// The running scene stepped aside for someone else's lights (#491): who, in words
    /// ("Windows Dynamic Lighting", "another app").
    pub yielded_to: Option<&'static str>,
    /// The room ids of the focused keyboard's devices, ringed on the map (#801); empty with
    /// one keyboard, where there is nothing to tell apart.
    pub focused: Vec<String>,
    /// Screen glow (#496).
    pub screen: ScreenView,
}

/// What the page knows of Screen glow (#496).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ScreenView {
    /// The user said yes ("Allow and start") and hasn't taken it back.
    pub allowed: bool,
    /// A start without the yes was refused (a profile, the last look after a restart).
    pub refused: bool,
    /// While Screen glow runs: reading, or what holds it.
    pub status: Option<glow::Status>,
    /// The screens' newest edge colours, while it reads.
    pub edges: Vec<foxglove_scene::Edges>,
}

/// How the status line reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Ok,
    Warn,
    Plain,
    Muted,
}

/// The scene's status line: stopping, paused for someone else's lights (#491), running, or
/// not running.
pub fn status(v: &View) -> (String, Tone) {
    let screen = v.screen.status.filter(|s| s.on);
    match (&v.running, v.stopping, v.yielded_to) {
        (_, true, _) => ("Stopping: putting the lights back…".into(), Tone::Plain),
        // Screen glow (#496): reading, or what holds it, in the design's words.
        (Some(_), false, who) if screen.is_some() => match screen.and_then(|s| s.hold) {
            Some(h) => (h.words(who), Tone::Warn),
            None => (glow::RUNNING.into(), Tone::Ok),
        },
        (None, false, _) if v.screen.refused && v.look.kind.reads_screen() => {
            (glow::NEEDS_YES.into(), Tone::Warn)
        }
        // No preview before the yes: the hint under the map says what shows once it runs.
        (None, false, _) if v.look.kind.reads_screen() => ("Not running.".into(), Tone::Muted),
        (Some(_), false, Some(who)) => (format!("Paused for {who}"), Tone::Warn),
        (Some(name), false, None) => match &v.heal.suffix {
            Some(s) => (format!("Running: {name} · {s}"), Tone::Warn),
            None => (format!("Running: {name}"), Tone::Ok),
        },
        (None, false, _) => ("Not running. The map shows a preview.".into(), Tone::Muted),
    }
}

/// The Triggers card's three boxes (#489).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Triggers {
    pub start_with_profile: bool,
    pub ripple_keys: bool,
    pub sweep_layers: bool,
}

impl Triggers {
    fn of(c: &Chosen) -> Triggers {
        Triggers {
            start_with_profile: c.start_with_profile,
            ripple_keys: c.ripple_keys,
            sweep_layers: c.sweep_layers,
        }
    }
}

/// One of the Triggers card's boxes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trigger {
    StartWithProfile,
    RippleKeys,
    SweepLayers,
}

impl View {
    fn is_lit(&self, id: &str) -> bool {
        if is_keyboard(id) {
            self.keyboard_lit
        } else {
            self.lit.iter().any(|l| l == id)
        }
    }
}

fn is_keyboard(id: &str) -> bool {
    id.starts_with(super::scene_output::KEYBOARD)
}

/// What the user is doing but hasn't finished.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Draft {
    /// The name typed for a new preset.
    pub name: String,
    /// A device being dragged on the map, and how far so far (mm).
    pub drag: Option<(String, Point)>,
    /// The preview's ambient lights, eased from frame to frame as the scene eases them.
    pub ambient: Ambient,
    /// Screen glow's permission card shows (Start without the yes, #496).
    pub asking: bool,
    /// The card just opened: focus goes to "Not now", so a stray Enter never agrees.
    pub ask_focus: bool,
}

/// What a click, a slider or a drag asks for.
#[derive(Clone, Debug, PartialEq)]
pub enum Intent {
    /// Another look; `save` when the change is done (not mid-drag of a slider).
    Look {
        look: Look,
        save: bool,
    },
    Start,
    Stop,
    /// Repaint now: send the scene's colours to the keyboard again.
    Repaint,
    SavePreset(String),
    RemovePreset(String),
    /// A device dragged on the map, by this much (mm).
    Move {
        id: String,
        by: Point,
    },
    ResetPositions,
    /// "Art screen too" switched on or off.
    WithArt(bool),
    /// A Triggers box switched on or off.
    Trigger(Trigger, bool),
    /// "Keep running on the keyboard" switched on or off.
    KeepOnKeyboard(bool),
    /// "Find my LEDs…": to the Lights page, with the wizard open.
    FindLeds,
    /// Screen glow's "Allow and start" (#496): the yes, then Start.
    AllowAndStart,
    /// Screen glow's "Stop allowing": forgets the yes and stops it.
    StopAllowing,
}

/// The page's own state between frames.
struct State {
    profile: Option<String>,
    chosen: Chosen,
    saved: Saved,
    room: RoomMap,
    lit: Vec<String>,
    /// The room ids of the Dynamic Lighting devices among `lit`.
    lamp_ids: Vec<String>,
    keyboard_lit: bool,
    keyboard_frames: bool,
    moved: bool,
    room_generation: u64,
    last_frame: u64,
    /// The profile changed: load its look again.
    reload: bool,
    /// The preview's clock while no scene runs.
    clock: Clock,
    /// "Keep running on the keyboard", as stored.
    keep_on_keyboard: bool,
    /// The focused keyboard's room ids (#801).
    focused: Vec<String>,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);
static DRAFT: Mutex<Draft> = Mutex::new(Draft {
    name: String::new(),
    drag: None,
    ambient: Ambient::new(),
    asking: false,
    ask_focus: false,
});
static STOPPING: AtomicBool = AtomicBool::new(false);
/// The running scene was started by a profile switch ("Start with this profile"), so a switch
/// to a profile without it stops it (#489).
static BY_PROFILE: AtomicBool = AtomicBool::new(false);
/// Foxglove has started its services, so a profile switch may start a scene.
static READY: AtomicBool = AtomicBool::new(false);
/// A profile switch came while a stop was ending a scene: decide again after it.
static RECHECK: AtomicBool = AtomicBool::new(false);
/// The chosen look is due to start without a profile's "Start with this profile": it ran when
/// Foxglove last closed (`[scene_resume]`), or a palette effect was clicked while a scene was
/// still ending. Cleared once it starts or on an explicit Stop.
static RESUME: AtomicBool = AtomicBool::new(false);

/// The store section that remembers whether a scene ran (not profiled: it describes the machine).
pub const RESUME_SECTION: &str = "scene_resume";

/// `[scene_resume]`: a look was started by hand (Start, a Lights-page palette effect) and not
/// stopped, so the next launch starts it again. `None` is a store from before this was kept.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Resume {
    pub running: Option<bool>,
}

fn save_resume(running: bool) {
    let r = Resume {
        running: Some(running),
    };
    if super::store::section(STORE_OWNER, RESUME_SECTION).try_load::<Resume>() == Some(r) {
        return;
    }
    if let Err(e) = super::store::section(STORE_OWNER, RESUME_SECTION).save(&r) {
        eprintln!("Foxglove: can't save [{RESUME_SECTION}]: {e}");
    }
}

/// Whether the launch starts the chosen look by itself (not by a profile's box): the saved
/// flag says so; a store from before the flag (`None`) resumes a look that was saved, since
/// choosing one used to start it and nothing remembered a Stop.
pub fn resume_at_launch(saved: Option<bool>, look_saved: bool) -> bool {
    saved.unwrap_or(look_saved)
}

/// The store has a chosen look (`[scenes] look`, the active profile's or the base's).
fn look_saved() -> bool {
    super::store::section(STORE_OWNER, looks::SECTION)
        .try_load::<toml::Table>()
        .is_some_and(|t| t.contains_key("look"))
}

#[cfg(test)]
thread_local! {
    /// The previews' page, so they never touch a device.
    pub static PREVIEW: std::cell::RefCell<Option<(View, Draft)>> = const { std::cell::RefCell::new(None) };
    /// How many times the preview asked for a repaint on this thread.
    pub static REPAINTS: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

/// The Scenes page.
pub fn page(ui: &mut egui::Ui) {
    super::settings_ui::section(ui, "Scenes", show);
}

fn show(ui: &mut egui::Ui) {
    #[cfg(test)]
    if let Some((v, mut draft)) = PREVIEW.with_borrow(|v| v.clone()) {
        draw(ui, &v, &mut draft);
        animate(ui.ctx(), &v);
        return;
    }
    let ctx = ui.ctx().clone();
    wake_on_return(&ctx);
    let mut guard = STATE.lock().unwrap_or_else(|e| e.into_inner());
    let st = refresh(&mut guard, ctx.cumulative_frame_nr());
    let running = super::scene_run::running();
    let t = match &running {
        Some((_, clock)) => clock.now(),
        None => st.clock.now(),
    };
    let view = View {
        room: st.room.clone(),
        keyboard_lit: st.keyboard_lit,
        keyboard_frames: st.keyboard_frames,
        keyboard_zones: super::light_zones::scene_words(),
        lit: st.lit.clone(),
        look: st.chosen.look.clone(),
        saved: st.saved.presets.clone(),
        running: running.map(|(name, _)| name),
        stopping: STOPPING.load(Ordering::Relaxed),
        heal: super::lights_heal::view(),
        moved: st.moved,
        t,
        with_art: st.chosen.with_art,
        wallpapers: looks::scenes().looks.clone(),
        on_screen: super::wallpaper::on_screen_ids(),
        triggers: Triggers::of(&st.chosen),
        profile: st.profile.clone().unwrap_or_else(|| "Base settings".into()),
        keep_on_keyboard: st.keep_on_keyboard,
        leds_placed: super::led_map::checked(super::boards::current().as_ref()),
        leds_shipped: super::led_map::state(super::boards::current().as_ref())
            == super::led_map::MapState::Shipped,
        lamp_note: lamp_note(st),
        yielded_to: super::lights_owner::yielded_to(),
        focused: st.focused.clone(),
        screen: screen_view(),
    };
    let mut draft = DRAFT.lock().unwrap_or_else(|e| e.into_inner());
    let intents = draw(ui, &view, &mut draft);
    animate(&ctx, &view);
    for intent in intents {
        apply(st, &mut draft, intent, &ctx);
    }
}

/// Screen glow as it is now.
fn screen_view() -> ScreenView {
    let s = glow::status();
    ScreenView {
        allowed: glow::allowed(),
        refused: glow::refused(),
        status: s.on.then_some(s),
        edges: if s.reading {
            glow::live_edges()
        } else {
            Vec::new()
        },
    }
}

/// The Presets card's rows, as [`presets`] draws them: the wallpapers' looks (each with its
/// wallpaper's id and whether it's on screen), the effects' presets, and the user's own.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PresetsCard {
    pub wallpapers: Vec<(String, String, bool)>,
    pub effects: Vec<String>,
    pub yours: Vec<String>,
}

/// The card for `wallpapers`, `on_screen` and `saved`, in [`presets`]' order.
pub fn presets_card(
    wallpapers: &[WallpaperLook],
    on_screen: &[String],
    saved: &[Preset],
) -> PresetsCard {
    PresetsCard {
        wallpapers: wallpapers
            .iter()
            .map(|w| {
                let shown = on_screen.contains(&w.wallpaper);
                (w.preset.name.clone(), w.wallpaper.clone(), shown)
            })
            .collect(),
        effects: looks::effect_presets()
            .into_iter()
            .map(|p| p.name)
            .collect(),
        yours: saved.iter().map(|p| p.name.clone()).collect(),
    }
}

/// The Presets card the page shows now (the local API's `scenes.page`, #618 batch H), from the
/// same sources as the page: the installed wallpapers' manifests, what's on screen, and the
/// user's presets (the page's own copy once it has opened, else the store's).
pub fn presets_now() -> PresetsCard {
    let saved = STATE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .map(|st| st.saved.presets.clone())
        .unwrap_or_else(|| looks::load_saved().presets);
    presets_card(
        &looks::scenes().looks,
        &super::wallpaper::on_screen_ids(),
        &saved,
    )
}

/// The page's state, loaded on first use and brought up to date: the profile's look after a
/// profile switch, the room after a change or when the page is shown again.
fn refresh(slot: &mut Option<State>, frame: u64) -> &mut State {
    let profile = super::store::cached_profiles().1;
    let st = slot.get_or_insert_with(|| State {
        profile: profile.clone(),
        chosen: looks::load_chosen(),
        saved: looks::load_saved(),
        room: RoomMap::default(),
        lit: Vec::new(),
        lamp_ids: Vec::new(),
        keyboard_lit: false,
        keyboard_frames: false,
        moved: false,
        room_generation: u64::MAX,
        last_frame: 0,
        reload: false,
        clock: Clock::starting_now(),
        keep_on_keyboard: super::scene_run::keep().on_quit,
        focused: Vec::new(),
    });
    if st.profile != profile || std::mem::take(&mut st.reload) {
        st.profile = profile;
        st.chosen = looks::load_chosen();
        super::scene_triggers::set_flags(&st.chosen);
    }
    // Not drawn on the frame before (another page, or the window was closed and reopened).
    let shown_again = frame != st.last_frame.wrapping_add(1);
    if shown_again || st.room_generation != super::scene_room::generation() {
        rebuild_room(st);
    }
    st.last_frame = frame;
    st
}

fn rebuild_room(st: &mut State) {
    use foxglove_devices::facts::Value;
    st.room_generation = super::scene_room::generation();
    st.room = super::scene_room::room();
    st.moved = !super::scene_room::layout().offsets.is_empty();
    // #801: with more than one keyboard here, the focused one is ringed.
    let keyboards = super::keyboard_focus::keyboards();
    st.focused = if keyboards.iter().filter(|k| k.connected).count() > 1 {
        super::scene_room::focused_ids(&st.room, super::keyboard_focus::current().as_ref())
    } else {
        Vec::new()
    };
    st.keyboard_lit =
        super::devices::bus().facts().get("keyboard.connected") == Some(Value::Bool(true));
    // Never looks for a keyboard here (the UI thread): what the one found last said.
    st.keyboard_frames = st.keyboard_lit && super::devices::frames();
    use foxglove_devices::pc_lighting::Control;
    let rows = super::pc_lights::snapshot().devices;
    let ids = |lit: fn(&Control) -> bool| -> Vec<String> {
        rows.iter()
            .filter(|r| lit(&r.control))
            .map(|r| format!("light:{}", r.key))
            .collect()
    };
    st.lit = ids(|c| {
        matches!(
            c,
            Control::Driver(_) | Control::LampArray | Control::OpenRgb
        )
    });
    st.lamp_ids = ids(|c| *c == Control::LampArray);
}

/// The page's line about Dynamic Lighting devices: which ones Windows took back from the running
/// scene and why, or that they follow it only from the front without package identity.
fn lamp_note(st: &State) -> Option<(String, bool)> {
    let waiting: Vec<String> = super::scene_run::lamps()
        .waiting
        .iter()
        .map(|key| {
            let id = format!("light:{key}");
            st.room.device(&id).map_or_else(
                // The keyboard's own LampArray (#749 b) has no room device of its own: its name.
                || {
                    super::pc_lights::snapshot()
                        .keyboard_lamp_arrays
                        .iter()
                        .find(|a| key.starts_with(&format!("{:04x}:{:04x}", a.vid, a.pid)))
                        .map_or_else(|| key.clone(), |a| a.name.clone())
                },
                |d| d.name.clone(),
            )
        })
        .collect();
    let text = super::scene_lamps::note(
        &waiting,
        !st.lamp_ids.is_empty(),
        super::scene_lamps::identity(),
    )?;
    Some((text, !waiting.is_empty()))
}

/// The profile changed (a program rule or by hand), whatever page shows (or none); the page
/// loads it again when drawn. The new profile's "Start with this profile" starts its scene,
/// and a scene a profile started stops when the new one's is off; otherwise a running scene
/// takes the new profile's look (#489, `scene_triggers::on_profile`).
pub fn profile_changed() {
    if let Some(st) = STATE.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        st.reload = true;
    }
    let chosen = looks::load_chosen();
    super::scene_triggers::set_flags(&chosen);
    if !READY.load(Ordering::Relaxed) {
        return;
    }
    let running = super::scene_run::running().is_some();
    let by_profile = BY_PROFILE.load(Ordering::Relaxed);
    match super::scene_triggers::on_profile(running, by_profile, chosen.start_with_profile) {
        super::scene_triggers::ProfileAction::Start => {
            start_by_profile(&chosen);
            tell_unseen(&chosen);
        }
        super::scene_triggers::ProfileAction::Stop => {
            BY_PROFILE.store(false, Ordering::Relaxed);
            RECHECK.store(true, Ordering::Relaxed);
            stop_scene();
        }
        super::scene_triggers::ProfileAction::Restyle => {
            // A stop still on its way ends this scene: decide again once it has.
            if STOPPING.load(Ordering::Relaxed) {
                RECHECK.store(true, Ordering::Relaxed);
            }
            let saved = looks::load_saved().presets;
            let room = super::scene_room::room();
            super::scene_run::set_effect(
                &scene_name(&chosen.look, &saved),
                chosen.look.effect(&room),
            );
            show_art(&chosen);
            tell_unseen(&chosen);
        }
        super::scene_triggers::ProfileAction::Nothing if RESUME.load(Ordering::Relaxed) => {
            start_by_hand(&chosen);
            tell_unseen(&chosen);
        }
        super::scene_triggers::ProfileAction::Nothing => {}
    }
}

/// The wallpapers' looks changed (#562, a wallpaper found, added or removed). A wallpaper look
/// the Scenes page started before its wallpaper was found ran as "Custom scene" with the
/// fallback palette and no art: it takes its palette, name and right-screen art now.
pub fn wallpapers_changed() {
    let Some((running, _)) = super::scene_run::running() else {
        return;
    };
    // The page's look, unsaved edits included (a drag isn't undone); the saved one when the
    // page hasn't loaded it.
    let page = STATE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .filter(|st| !st.reload)
        .map(|st| st.chosen.clone());
    let chosen = page.unwrap_or_else(looks::load_chosen);
    if !restyle_on_wallpapers(&running, looks::wallpaper_look(&chosen.look).is_some()) {
        return;
    }
    let saved = looks::load_saved().presets;
    let room = super::scene_room::room();
    super::scene_run::set_effect(&scene_name(&chosen.look, &saved), chosen.look.effect(&room));
    show_art(&chosen);
}

/// The "Wallpaper" palette's colours changed (#40: another wallpaper on screen, or its picture
/// read for the first time). The running scene takes them when it is the chosen look and that
/// look follows the wallpaper; any other scene is left alone.
pub fn wallpaper_palette_changed() {
    let Some((running, _)) = super::scene_run::running() else {
        return;
    };
    let page = STATE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .filter(|st| !st.reload)
        .map(|st| st.chosen.clone());
    let chosen = page.unwrap_or_else(looks::load_chosen);
    let saved = looks::load_saved().presets;
    let name = scene_name(&chosen.look, &saved);
    if !chosen.look.follows_wallpaper() || running != name {
        return;
    }
    let room = super::scene_room::room();
    super::scene_run::set_effect(&name, chosen.look.effect(&room));
}

/// Only a scene that ran under the page's unnamed fallback while its look is a wallpaper's
/// is restyled: never the demo scene, a named preset or the user's own look.
fn restyle_on_wallpapers(running: &str, chosen_is_wallpaper_look: bool) -> bool {
    running == CUSTOM_SCENE && chosen_is_wallpaper_look
}

/// A look started from elsewhere (the Lights page's palette effects, #871): it becomes the
/// chosen look, as the page's own pick would, and starts; a running scene takes it instead.
pub fn start_look(look: Look) {
    let mut chosen = looks::load_chosen();
    chosen.look = look.clamped();
    looks::save_chosen(&chosen);
    super::scene_triggers::set_flags(&chosen);
    if let Some(st) = STATE.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        st.reload = true;
    }
    BY_PROFILE.store(false, Ordering::Relaxed);
    let saved = looks::load_saved().presets;
    let room = super::scene_room::room();
    let name = scene_name(&chosen.look, &saved);
    if super::scene_run::running().is_some() && !STOPPING.load(Ordering::Relaxed) {
        super::scene_run::set_effect(&name, chosen.look.effect(&room));
        save_resume(true);
    } else {
        start_by_hand(&chosen);
    }
}

/// Stops the scene from elsewhere (the Lights page's palette effects, #871), as Stop does.
pub fn stop_look() {
    stop_by_hand();
}

/// The user's Stop (the page's button, the Lights page's): the next launch starts nothing, and
/// a start waiting for a stop to end is dropped.
fn stop_by_hand() {
    BY_PROFILE.store(false, Ordering::Relaxed);
    RESUME.store(false, Ordering::Relaxed);
    save_resume(false);
    stop_scene();
}

/// Starts the chosen look as the user's own scene (not a profile's): Start, a palette effect, or
/// the look that ran when Foxglove closed. A scene still ending makes it wait: the stop thread
/// decides again ([`after_start`]).
fn start_by_hand(chosen: &Chosen) {
    if needs_yes(chosen) {
        // Nothing to retry: only the user's click gives the yes.
        RESUME.store(false, Ordering::Relaxed);
        return;
    }
    let saved = looks::load_saved().presets;
    let room = super::scene_room::room();
    let name = scene_name(&chosen.look, &saved);
    let started = super::scene_run::start(&name, chosen.look.effect(&room), None);
    after_start(started);
    if started {
        BY_PROFILE.store(false, Ordering::Relaxed);
        save_resume(true);
        show_art(chosen);
    } else {
        eprintln!("Foxglove: a scene still ends; {name} starts after it");
    }
}

/// After a hand start: one that ran clears the pending start; one refused while a scene ends
/// keeps it pending and asks the stop thread to decide again (as `start_by_profile` does).
fn after_start(started: bool) {
    RESUME.store(!started, Ordering::Relaxed);
    if !started {
        RECHECK.store(true, Ordering::Relaxed);
    }
}

/// A profile's "Start with this profile" starts its scene.
fn start_by_profile(chosen: &Chosen) {
    if needs_yes(chosen) {
        return;
    }
    let saved = looks::load_saved().presets;
    let room = super::scene_room::room();
    let name = scene_name(&chosen.look, &saved);
    if super::scene_run::start(&name, chosen.look.effect(&room), None) {
        BY_PROFILE.store(true, Ordering::Relaxed);
        RESUME.store(false, Ordering::Relaxed);
        show_art(chosen);
    } else {
        // A scene is still ending: start once it has (the stop thread decides again).
        RECHECK.store(true, Ordering::Relaxed);
    }
}

/// Screen glow without the user's yes (#496): nothing starts, and the page says so.
fn needs_yes(chosen: &Chosen) -> bool {
    let refuse = !start_allowed(chosen.look.kind, glow::allowed());
    if refuse {
        eprintln!("Foxglove: Screen glow didn't start: it needs the user's yes");
        glow::set_refused(true);
        super::scene_run::repaint_page();
    }
    refuse
}

/// Whether a look may start without asking: anything but Screen glow, which needs the yes.
pub fn start_allowed(kind: Kind, allowed: bool) -> bool {
    !kind.reads_screen() || allowed
}

/// Screen glow started (or the look became it) without a click, while Foxglove's window is
/// closed: one Windows notification says so (#496).
fn tell_unseen(chosen: &Chosen) {
    let running_glow = glow::status().on || super::scene_run::running().is_some();
    if chosen.look.kind.reads_screen()
        && glow::allowed()
        && running_glow
        && !super::settings_open::window_shown()
    {
        glow::notify_started_unseen();
    }
}

/// Foxglove started its services: the trigger boxes take effect, and the active profile's
/// scene starts when it has "Start with this profile" on, or when a look the user started was
/// still running when Foxglove closed (`[scene_resume]`).
pub fn startup() {
    let saved = super::store::section(STORE_OWNER, RESUME_SECTION)
        .try_load::<Resume>()
        .unwrap_or_default();
    let resume = resume_at_launch(saved.running, look_saved());
    if resume {
        eprintln!("Foxglove: the last look ran when Foxglove closed; starting it again");
    }
    RESUME.store(resume, Ordering::Relaxed);
    READY.store(true, Ordering::Relaxed);
    // The same decision as a switch (it reads the active profile as it is now), unless the
    // demo scene already runs.
    if super::scene_run::running().is_none() {
        profile_changed();
    }
}

/// Quitting: profile switches no longer start scenes (called before the scene stops).
pub fn quitting() {
    READY.store(false, Ordering::Relaxed);
}

/// Repaints the page when the PC comes back (unlocked, display on), since the preview asks for
/// no frames while away. Subscribed once, on the page's first frame.
fn wake_on_return(ctx: &egui::Context) {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        super::scene_run::set_page(ctx);
        let ctx = ctx.clone();
        let sub = super::away::subscribe(move |away| {
            if !away {
                ctx.request_repaint();
            }
        });
        // For the app's life: one listener, woken only by lock and display events.
        std::mem::forget(sub);
    });
}

/// The name a look runs under when no preset has it.
const CUSTOM_SCENE: &str = "Custom scene";

/// The name a look runs under: its preset's, or "Custom scene".
fn scene_name(look: &Look, saved: &[Preset]) -> String {
    let fallback = if look.kind.reads_screen() {
        glow::NAME
    } else {
        CUSTOM_SCENE
    };
    looks::named(look, &looks::built_in(), saved)
        .unwrap_or(fallback)
        .to_string()
}

fn apply(st: &mut State, draft: &mut Draft, intent: Intent, ctx: &egui::Context) {
    match intent {
        Intent::Look { look, save } => {
            if !look.kind.reads_screen() {
                draft.asking = false;
            }
            st.chosen.look = look.clamped();
            if save {
                looks::save_chosen(&st.chosen);
            }
            resend(st);
        }
        Intent::Start => {
            BY_PROFILE.store(false, Ordering::Relaxed);
            looks::save_chosen(&st.chosen);
            start_by_hand(&st.chosen);
        }
        Intent::AllowAndStart => {
            draft.asking = false;
            glow::allow();
            BY_PROFILE.store(false, Ordering::Relaxed);
            looks::save_chosen(&st.chosen);
            start_by_hand(&st.chosen);
            ctx.request_repaint();
        }
        Intent::StopAllowing => {
            glow::forget();
            if glow::status().on {
                stop_by_hand();
            }
            ctx.request_repaint();
        }
        Intent::WithArt(on) => {
            st.chosen.with_art = on;
            looks::save_chosen(&st.chosen);
            show_art(&st.chosen);
        }
        Intent::Repaint => {
            if let Err(e) = super::lights_heal::user_repaint() {
                eprintln!("Foxglove: lights heal: Repaint now: {e}");
            }
            ctx.request_repaint();
        }
        Intent::Stop => {
            stop_by_hand();
            ctx.request_repaint();
        }
        Intent::Trigger(which, on) => {
            let c = &mut st.chosen;
            match which {
                Trigger::StartWithProfile => c.start_with_profile = on,
                Trigger::RippleKeys => c.ripple_keys = on,
                Trigger::SweepLayers => c.sweep_layers = on,
            }
            looks::save_chosen(&st.chosen);
            super::scene_triggers::set_flags(&st.chosen);
        }
        Intent::KeepOnKeyboard(on) => {
            st.keep_on_keyboard = on;
            super::scene_run::set_keep_on_quit(on);
        }
        Intent::FindLeds => {
            super::shell::set_page(super::shell::Page::Lights);
            super::led_map_ui::open(ctx);
            ctx.request_repaint();
        }
        Intent::SavePreset(name) => {
            if st.saved.save(&name, &st.chosen.look) {
                looks::save_saved(&st.saved);
                draft.name.clear();
            }
        }
        Intent::RemovePreset(name) => {
            st.saved.remove(&name);
            looks::save_saved(&st.saved);
        }
        Intent::Move { id, by } => {
            let mut layout = super::scene_room::layout();
            layout.nudge(&id, by);
            save_layout(st, &layout);
        }
        Intent::ResetPositions => {
            let mut layout = super::scene_room::layout();
            layout.offsets.clear();
            save_layout(st, &layout);
        }
    }
}

/// Stops the running scene off the calling thread: Stop waits (bounded) for the lights to be
/// given back. The scene's end repaints the page.
fn stop_scene() {
    if !STOPPING.swap(true, Ordering::Relaxed) {
        super::services::spawn_named("scene-stop", move || {
            super::scene_run::stop();
            STOPPING.store(false, Ordering::Relaxed);
            // A profile switch while this stop ran (quick switches back to a game) gets its
            // scene now that the lights are back.
            if super::scene_triggers::restart_after_stop(
                RECHECK.swap(false, Ordering::Relaxed),
                READY.load(Ordering::Relaxed),
            ) {
                profile_changed();
            }
            super::scene_run::repaint_page();
        });
    }
}

fn save_layout(st: &mut State, layout: &foxglove_scene::Layout) {
    if let Err(e) = super::store::section(STORE_OWNER, super::scene_room::SECTION).save(layout) {
        eprintln!("Foxglove: can't save the room map: {e}");
    }
    // A running scene places the devices again too.
    super::scene_run::devices_changed();
    rebuild_room(st);
    // A ripple starts at the keyboard, wherever it now is.
    resend(st);
}

/// A running scene takes the page's look (and its right-screen art).
fn resend(st: &State) {
    if super::scene_run::running().is_some() {
        let name = scene_name(&st.chosen.look, &st.saved.presets);
        super::scene_run::set_effect(&name, st.chosen.look.effect(&st.room));
        show_art(&st.chosen);
    }
}

/// The running scene's art-screen art: its wallpaper's with "Art screen too", else none
/// (the art from before the scene). Looked up only while a scene runs, since a bundled set is
/// copied into the art folder first.
fn show_art(chosen: &Chosen) {
    if super::scene_run::running().is_none() {
        return;
    }
    let art = chosen.art().and_then(|a| {
        a.item(
            |file| {
                super::art_library::folder()
                    .map(|f| f.join(file))
                    .filter(|p| p.is_file())
            },
            |file| super::art_pieces::bundled_file(file).ok(),
        )
    });
    super::scene_run::set_art(art);
}

/// When the preview wants its next frame: never while minimised or away, or for a still look.
pub fn next_frame(kind: Kind, minimised: bool, away: bool) -> Option<Duration> {
    (!minimised && !away && !matches!(kind, Kind::Gradient | Kind::ScreenGlow))
        .then_some(PREVIEW_INTERVAL)
}

fn animate(ctx: &egui::Context, v: &View) {
    let minimised = ctx.input(|i| i.viewport().minimized) == Some(true);
    if v.look.kind.reads_screen() {
        // Screen glow's map is a still drawing until it reads; then its live edge strips.
        if !v.screen.edges.is_empty() && !minimised && !super::away::away() {
            ctx.request_repaint_after(PREVIEW_INTERVAL);
        }
        return;
    }
    if let Some(after) = next_frame(v.look.kind, minimised, super::away::away()) {
        ctx.request_repaint_after(after);
        #[cfg(test)]
        REPAINTS.set(REPAINTS.get() + 1);
    }
}

/// Paints the page; returns what was asked for this frame.
pub fn draw(ui: &mut egui::Ui, v: &View, draft: &mut Draft) -> Vec<Intent> {
    let mut out = Vec::new();
    let gap = 16.0;
    let width = ui.available_width();
    if width < TWO_COLUMNS {
        card(ui, "Room", |ui| room_card(ui, v, draft, &mut out));
        ui.add_space(10.0);
        card(ui, "Looks", |ui| presets(ui, v, &mut out));
        ui.add_space(10.0);
        card(ui, "Effect", |ui| effect_card(ui, v, draft, &mut out));
        ui.add_space(10.0);
        card(ui, "Triggers", |ui| triggers_card(ui, v, &mut out));
        ui.add_space(6.0);
        return out;
    }
    let left_w = ((width - gap) * 0.62).floor();
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = gap;
        ui.allocate_ui_with_layout(
            egui::vec2(left_w, 0.0),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                ui.set_width(left_w);
                card(ui, "Room", |ui| room_card(ui, v, draft, &mut out));
                ui.add_space(10.0);
                card(ui, "Effect", |ui| effect_card(ui, v, draft, &mut out));
                ui.add_space(10.0);
                card(ui, "Triggers", |ui| triggers_card(ui, v, &mut out));
            },
        );
        let right_w = width - left_w - gap;
        ui.allocate_ui_with_layout(
            egui::vec2(right_w, 0.0),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                ui.set_width(right_w);
                card(ui, "Looks", |ui| presets(ui, v, &mut out));
            },
        );
    });
    ui.add_space(6.0);
    out
}

/// What starts the scene or draws over it (#489): the active profile's three boxes.
fn triggers_card(ui: &mut egui::Ui, v: &View, out: &mut Vec<Intent>) {
    let t = v.triggers;
    let mut boxed = |ui: &mut egui::Ui, which: Trigger, on: bool, label: &str, hint: &str| {
        let mut now = on;
        if ui.checkbox(&mut now, label).on_hover_text(hint).changed() {
            out.push(Intent::Trigger(which, now));
        }
    };
    boxed(
        ui,
        Trigger::StartWithProfile,
        t.start_with_profile,
        "Start with this profile",
        "Switching to this profile starts the scene. Switching away stops it, unless you started it yourself.",
    );
    ui.weak(format!(
        "Profile: {}. Programs switch profiles under Settings, Profile.",
        v.profile
    ));
    ui.add_space(4.0);
    boxed(
        ui,
        Trigger::RippleKeys,
        t.ripple_keys,
        "Ripple on key press",
        "Each key you press sends one ring of the scene's colour across the room. Foxglove sees where the key is, never what it types.",
    );
    boxed(
        ui,
        Trigger::SweepLayers,
        t.sweep_layers,
        "Sweep on layer change",
        "Changing layer sends one sweep across the room in that layer's overlay colour.",
    );
    ui.weak("Ripples and sweeps show while a scene runs.");
    // With Foxglove closed nothing reads the screen: Screen glow can't keep running (#496).
    if v.look.kind.reads_screen() {
        return;
    }
    ui.add_space(4.0);
    let mut keep = v.keep_on_keyboard;
    if ui
        .checkbox(&mut keep, "Keep running on the keyboard")
        .on_hover_text("When you quit Foxglove, the keyboard keeps drawing the scene by itself. Needs keyboard firmware that draws effects (scanline, sweep, ripple, breathe). Off: quitting gives the keyboard its own lights back.")
        .changed()
    {
        out.push(Intent::KeepOnKeyboard(keep));
    }
}

/// Start or Stop with the scene's state, the map, and what the scene lights.
fn room_card(ui: &mut egui::Ui, v: &View, draft: &mut Draft, out: &mut Vec<Intent>) {
    let p = theme::palette(ui);
    split_row(
        ui,
        |ui| {
            if v.running.is_some() || v.stopping {
                // Only while the keyboard is dark (UX plan): first, so Tab reaches it before Stop.
                if v.heal.dark && !v.stopping {
                    let busy = v.heal.putting_back;
                    let label = if busy { "Repainting…" } else { "Repaint now" };
                    let first_try = v.heal.failed && v.heal.dark_since.is_some();
                    let b = ui.add_enabled(
                        !busy,
                        if first_try {
                            theme::primary_button(ui, label)
                        } else {
                            egui::Button::new(label)
                        },
                    );
                    let b = if busy {
                        b.on_disabled_hover_text("Foxglove is putting the lights back")
                    } else {
                        b.on_hover_text("Sends the scene's colours to the keyboard again")
                    };
                    b.widget_info(|| {
                        egui::WidgetInfo::labeled(
                            egui::WidgetType::Button,
                            true,
                            if busy {
                                "Repainting the keyboard lights"
                            } else {
                                "Repaint the keyboard lights now"
                            },
                        )
                    });
                    if b.clicked() {
                        out.push(Intent::Repaint);
                    }
                }
                let stop = ui.add_enabled(!v.stopping, egui::Button::new("Stop"));
                if stop
                    .on_hover_text("Stop the scene and put the lights back")
                    .clicked()
                {
                    out.push(Intent::Stop);
                }
            } else {
                // Screen glow needs a screen (#496), and its first Start asks.
                let screen = v.look.kind.reads_screen();
                let no_screen =
                    screen && !v.room.devices.iter().any(|d| d.kind == DeviceKind::Monitor);
                let start = ui
                    .add_enabled(!no_screen, theme::primary_button(ui, "Start"))
                    .on_hover_text("Run this scene on the lights")
                    .on_disabled_hover_text(glow::NO_SCREEN);
                if start.clicked() {
                    if start_allowed(v.look.kind, v.screen.allowed) {
                        out.push(Intent::Start);
                    } else {
                        draft.asking = true;
                        draft.ask_focus = true;
                    }
                }
            }
            if v.moved
                && ui
                    .button("Reset positions")
                    .on_hover_text("Put every device back where Foxglove placed it")
                    .clicked()
            {
                out.push(Intent::ResetPositions);
            }
        },
        |ui| {
            let (text, tone) = status(v);
            let color = match tone {
                Tone::Ok => p.ok,
                Tone::Warn => p.warn,
                Tone::Plain => p.text,
                Tone::Muted => p.muted,
            };
            // #496: the eye in front of "Running: Screen glow · reading your screen".
            if reads_now(v) {
                let (rect, _) = ui.allocate_exact_size(egui::vec2(22.0, 14.0), Sense::hover());
                super::tray_panel::eye(ui.painter(), rect.center(), 18.0, color);
            }
            ui.add(egui::Label::new(RichText::new(text).color(color)).wrap());
            if v.heal.failed {
                ui.add(
                    egui::Label::new(RichText::new(super::lights_heal::HINT).color(p.muted)).wrap(),
                );
            }
        },
    );
    ui.add_space(6.0);
    room_map(ui, v, draft, out);
    ui.add_space(6.0);
    if v.look.kind.reads_screen() {
        let line = if v.screen.status.is_some_and(|s| s.on) {
            glow::ROOM_RUNNING
        } else {
            glow::ROOM_STOPPED
        };
        ui.add(egui::Label::new(RichText::new(line).color(p.muted)).wrap());
        ui.add_space(4.0);
    }
    ui.add(egui::Label::new(RichText::new(legend(v)).color(p.muted)).wrap());
    if let Some((text, stopped)) = &v.lamp_note {
        let color = if *stopped { p.warn } else { p.muted };
        ui.add_space(4.0);
        ui.add(egui::Label::new(RichText::new(text).color(color)).wrap());
    }
    // Who drives the keyboard's lights (#491), while no scene runs: a running one's status says
    // it (running, or paused for Windows).
    if v.running.is_none() {
        super::lights_owner::show_line(ui, 4.0);
    }
    if v.keyboard_frames && !v.leds_placed {
        let (label, hint, text, color) = if v.leds_shipped {
            (
                "Check zones…",
                "A dot runs along the LEDs; say whether it follows the drawing: about 20 seconds",
                "Scenes use the LED map that comes with this keyboard, not yet checked on yours.",
                p.muted,
            )
        } else {
            (
                "Check zones…",
                "Show Foxglove where each LED is, so scenes light every LED in its place",
                if super::parts::Parts::current().is_split() {
                    "The keyboard lights a colour per half until Foxglove knows where its LEDs are."
                } else {
                    "The keyboard lights one colour until Foxglove knows where its LEDs are."
                },
                p.warn,
            )
        };
        ui.add_space(4.0);
        split_row(
            ui,
            |ui| {
                if ui.button(label).on_hover_text(hint).clicked() {
                    out.push(Intent::FindLeds);
                }
            },
            |ui| {
                ui.add(egui::Label::new(RichText::new(text).color(color)).wrap());
            },
        );
    }
}

/// Screen glow runs and reads the screen now (the status line's eye).
fn reads_now(v: &View) -> bool {
    !v.stopping
        && v.running.is_some()
        && v.screen
            .status
            .is_some_and(|s| s.on && s.reading && s.hold.is_none())
}

/// Which devices the scene lights, in words.
pub fn legend(v: &View) -> String {
    legend_for(v, &super::parts::Parts::current().seen(2))
}

/// [`legend`] with a split's parts named as `parts` names them (#563).
fn legend_for(v: &View, parts: &super::parts::Parts) -> String {
    let mut lit: Vec<String> = Vec::new();
    let mut shown: Vec<String> = Vec::new();
    for d in v
        .room
        .devices
        .iter()
        .filter(|d| d.kind != DeviceKind::Monitor)
    {
        let keyboard = is_keyboard(&d.id);
        let name = if keyboard {
            let n = d.name.trim_end_matches(" left").trim_end_matches(" right");
            // A split's halves are "keyboard:left" and "keyboard:right"; one piece is "keyboard".
            let split = d.id.contains(':');
            let how = if d.lights.len() > 1 && v.keyboard_frames {
                // Its real resolution (#611): "6 zones per half", never a colour per LED it
                // can't show.
                v.keyboard_zones
                    .clone()
                    .unwrap_or_else(|| "a colour per LED".to_string())
            } else if !split {
                "one colour".to_string()
            } else if !v.keyboard_frames {
                format!("one colour for {}", parts.all())
            } else {
                format!("a colour per {}", parts.noun())
            };
            format!("{n} ({how})")
        } else {
            d.name.clone()
        };
        let list = if v.is_lit(&d.id) {
            &mut lit
        } else {
            &mut shown
        };
        if !list.contains(&name) {
            list.push(name);
        }
    }
    let mut text = if lit.is_empty() {
        "Nothing to light yet: connect the keyboard or the case.".to_string()
    } else {
        format!("Lights: {}.", lit.join(", "))
    };
    if !shown.is_empty() {
        text.push_str(&format!(" Shown only: {}.", shown.join(", ")));
    }
    text.push_str(
        " Drag a device to where it stands; the monitors follow Windows' display layout.",
    );
    text
}

/// Where the room map assumes the primary monitor when Windows lists none (`foxglove_scene`'s
/// 27" default at the room's origin).
const STAND_IN_MONITOR: foxglove_scene::Rect = foxglove_scene::Rect::new(0.0, 0.0, 597.0, 336.0);

/// Room space (mm) onto the map's rectangle (points), keeping the room's shape.
struct Map {
    origin: Pos2,
    scale: f32,
    bounds: foxglove_scene::Rect,
}

impl Map {
    fn new(area: egui::Rect, bounds: foxglove_scene::Rect) -> Map {
        let (w, h) = (bounds.w.max(1.0), bounds.h.max(1.0));
        let scale = (area.width() / w).min(area.height() / h).max(0.001);
        let size = egui::vec2(w * scale, h * scale);
        Map {
            origin: area.center() - size / 2.0,
            scale,
            bounds,
        }
    }

    fn pos(&self, p: Point) -> Pos2 {
        self.origin
            + egui::vec2(
                (p.x - self.bounds.x) * self.scale,
                (p.y - self.bounds.y) * self.scale,
            )
    }

    fn rect(&self, r: &foxglove_scene::Rect) -> egui::Rect {
        egui::Rect::from_min_max(
            self.pos(Point::new(r.x, r.y)),
            self.pos(Point::new(r.right(), r.bottom())),
        )
    }

    fn mm(&self, d: egui::Vec2) -> Point {
        Point::new(d.x / self.scale, d.y / self.scale)
    }
}

/// The room as drawn this frame: with the drag in progress applied.
fn dragged_room(room: &RoomMap, drag: Option<&(String, Point)>) -> RoomMap {
    let mut room = room.clone();
    if let Some((id, by)) = drag {
        if let Some(d) = room.devices.iter_mut().find(|d| d.id == *id) {
            d.rect = d.rect.moved(*by);
            for l in &mut d.lights {
                l.at = l.at + *by;
            }
        }
    }
    room
}

/// The device a drag on the map at `at` moves: the top one under it that isn't a monitor.
fn device_at<'a>(room: &'a RoomMap, map: &Map, at: Pos2) -> Option<&'a Placed> {
    room.devices
        .iter()
        .rev()
        .filter(|d| d.kind != DeviceKind::Monitor)
        .find(|d| map.rect(&d.rect).expand(3.0).contains(at))
}

fn color(c: Rgb) -> Color32 {
    let [r, g, b] = c.0;
    Color32::from_rgb(r, g, b)
}

/// The map: the scene's field across the room, faint, then each device and its lights.
fn room_map(ui: &mut egui::Ui, v: &View, draft: &mut Draft, out: &mut Vec<Intent>) {
    let p = theme::palette(ui);
    let width = ui.available_width();
    // With no display read, a monitor's outline where the room assumes one, for scale.
    let stand_in =
        (!v.room.devices.iter().any(|d| d.kind == DeviceKind::Monitor)).then_some(STAND_IN_MONITOR);
    let b = stand_in.map_or(v.room.bounds, |m| v.room.bounds.union(&m));
    let aspect = if b.w > 0.0 { b.h / b.w } else { 0.6 };
    let height = (width * aspect).clamp(150.0, 320.0);
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(width, height), Sense::click_and_drag());
    let map = Map::new(rect.shrink(12.0), b);
    handle_drag(&response, v, &map, draft, out);
    let room = dragged_room(&v.room, draft.drag.as_ref());
    let effect = v.look.effect(&room);
    let painter = ui.painter_at(rect);
    painter.rect(
        rect,
        6.0,
        p.well,
        Stroke::new(1.0, p.line),
        egui::StrokeKind::Inside,
    );
    field(&painter, &map, &effect, v.t, &room.bounds);
    if let Some(m) = stand_in {
        let r = map.rect(&m);
        painter.rect_stroke(r, 3.0, Stroke::new(1.0, p.line), egui::StrokeKind::Inside);
    }
    let live = v.look.kind.reads_screen() && !v.screen.edges.is_empty();
    let mut frame = if live {
        // The screen's colours as they are now (the lights ease towards them).
        foxglove_scene::Glow::new(foxglove_scene::Smoothing::Quick).render(
            &room,
            &v.screen.edges,
            v.t,
        )
    } else {
        effect.render(&room, v.t)
    };
    if v.look.kind == Kind::Gradient {
        // A still look asks for no more frames, so nothing would finish the fade: show where
        // it settles.
        draft.ambient.reset();
    }
    draft.ambient.apply(&mut frame, &room);
    let keyboard = super::scene_output::mixed(
        &frame
            .lights
            .iter()
            .filter(|l| is_keyboard(&l.device))
            .map(|l| l.colour)
            .collect::<Vec<_>>(),
    );
    let hovered = response
        .hover_pos()
        .and_then(|at| device_at(&room, &map, at))
        .map(|d| d.id.clone());
    for d in &room.devices {
        let r = map.rect(&d.rect);
        if d.kind == DeviceKind::Monitor {
            painter.rect_stroke(
                r,
                3.0,
                Stroke::new(1.5, p.control_line),
                egui::StrokeKind::Inside,
            );
            continue;
        }
        let lit = v.is_lit(&d.id);
        let active = hovered.as_deref() == Some(&d.id)
            || draft.drag.as_ref().is_some_and(|(id, _)| *id == d.id);
        let line = if active {
            p.accent
        } else if lit {
            p.control_line
        } else {
            p.line
        };
        painter.rect(
            r,
            4.0,
            p.surface_raised,
            Stroke::new(1.0, line),
            egui::StrokeKind::Inside,
        );
        if v.focused.contains(&d.id) {
            // #801: the keyboard shown in Foxglove, ringed a little outside its outline.
            painter.rect_stroke(
                r.expand(FOCUS_RING_GAP),
                4.0 + FOCUS_RING_GAP,
                Stroke::new(2.0, p.accent),
                egui::StrokeKind::Outside,
            );
        }
        let dim = |c: Color32| if lit { c } else { c.gamma_multiply(0.4) };
        if is_keyboard(&d.id) {
            // Without SET_FRAME the keyboard shows one colour for both halves (#485), so the
            // map does too; with it each half its own (#529), and its LEDs once placed.
            let fill = if v.keyboard_frames {
                super::scene_output::mixed(
                    &frame
                        .lights
                        .iter()
                        .filter(|l| l.device == d.id)
                        .map(|l| l.colour)
                        .collect::<Vec<_>>(),
                )
            } else {
                keyboard
            };
            if let Some(c) = fill {
                painter.rect_filled(r.shrink(3.0), 3.0, dim(color(c)));
            }
            if !(v.keyboard_frames && d.lights.len() > 1) {
                continue;
            }
        }
        let lights: Vec<(&PlacedLight, Rgb)> = frame
            .lights
            .iter()
            .filter(|l| l.device == d.id)
            .filter_map(|l| Some((d.lights.iter().find(|x| x.id == l.light)?, l.colour)))
            .collect();
        // Area lights (the case's interior light) as a soft glow, behind the point lights.
        for (x, c) in lights.iter().filter(|(x, _)| x.role == LightRole::Ambient) {
            let radius = (r.width().min(r.height()) * 0.6).max(8.0);
            glow(&painter, map.pos(x.at), radius, dim(color(*c)));
        }
        for (x, c) in lights.iter().filter(|(x, _)| x.role == LightRole::Point) {
            let radius = (r.width().min(r.height()) / 6.0).clamp(2.5, 7.0);
            painter.circle_filled(map.pos(x.at), radius, dim(color(*c)));
        }
    }
    if let Some(id) = hovered {
        if let Some(d) = room.devices.iter().find(|d| d.id == id) {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
            let lit = if v.is_lit(&d.id) {
                "lit by the scene"
            } else {
                "shown only"
            };
            let shown = if v.focused.contains(&d.id) {
                FOCUS_RING_WORDS
            } else {
                ""
            };
            response.on_hover_text_at_pointer(format!("{}{shown} · {lit} · drag to move", d.name));
        }
    }
    if live {
        screen_overlay(&painter, &map, &room, v);
    }
}

/// Screen glow's running map (#496): each screen's edge strips in their live colours, and a thin
/// line from each lit device to the stretch of edge it follows.
fn screen_overlay(painter: &egui::Painter, map: &Map, room: &RoomMap, v: &View) {
    use foxglove_scene::screen::{nearest_edge, Edge, MONITOR_PREFIX};
    let edges = &v.screen.edges;
    let find = |id: &str| {
        let name = id.strip_prefix(MONITOR_PREFIX)?;
        edges.iter().find(|e| e.monitor == name)
    };
    for d in room
        .devices
        .iter()
        .filter(|d| d.kind == DeviceKind::Monitor)
    {
        let Some(e) = find(&d.id) else { continue };
        let r = map.rect(&d.rect).shrink(1.5);
        let depth = (r.width().min(r.height()) * 0.1).clamp(3.0, 10.0);
        let sides = [
            (Edge::Top, &e.top),
            (Edge::Bottom, &e.bottom),
            (Edge::Left, &e.left),
            (Edge::Right, &e.right),
        ];
        for (side, colours) in sides {
            let n = colours.len().max(1) as f32;
            for (i, c) in colours.iter().enumerate() {
                let (a, b) = (i as f32 / n, (i + 1) as f32 / n);
                let seg = match side {
                    Edge::Top => egui::Rect::from_x_y_ranges(
                        r.left() + r.width() * a..=r.left() + r.width() * b,
                        r.top()..=r.top() + depth,
                    ),
                    Edge::Bottom => egui::Rect::from_x_y_ranges(
                        r.left() + r.width() * a..=r.left() + r.width() * b,
                        r.bottom() - depth..=r.bottom(),
                    ),
                    Edge::Left => egui::Rect::from_x_y_ranges(
                        r.left()..=r.left() + depth,
                        r.top() + r.height() * a..=r.top() + r.height() * b,
                    ),
                    Edge::Right => egui::Rect::from_x_y_ranges(
                        r.right() - depth..=r.right(),
                        r.top() + r.height() * a..=r.top() + r.height() * b,
                    ),
                };
                painter.rect_filled(seg, 0.0, color(*c));
            }
        }
    }
    for d in room
        .devices
        .iter()
        .filter(|d| d.kind != DeviceKind::Monitor && v.is_lit(&d.id))
    {
        let from = d.rect.centre();
        let Some(spot) = nearest_edge(room, from) else {
            continue;
        };
        let c = find(&spot.monitor).map_or(Rgb::WHITE, |e| e.at(spot.edge, spot.along));
        painter.line_segment(
            [map.pos(from), map.pos(spot.at)],
            Stroke::new(1.0, color(c).gamma_multiply(0.8)),
        );
        painter.circle_filled(map.pos(spot.at), 2.5, color(c));
    }
}

/// The permission card (#496), in place of the Effect card's controls after the first Start:
/// what Screen glow reads, the three points, Allow and start and Not now (focused; Esc too).
fn permission_card(ui: &mut egui::Ui, draft: &mut Draft, out: &mut Vec<Intent>) {
    let p = theme::palette(ui);
    egui::Frame::new()
        .fill(p.surface_raised)
        .stroke(Stroke::new(1.0, p.accent))
        .corner_radius(8)
        .inner_margin(egui::Margin::same(14))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.add(
                egui::Label::new(
                    RichText::new(glow::ASK_TITLE)
                        .size(16.0)
                        .strong()
                        .color(p.text_strong),
                )
                .wrap(),
            );
            ui.add_space(4.0);
            ui.add(egui::Label::new(RichText::new(glow::ASK_BODY).color(p.text)).wrap());
            ui.add_space(6.0);
            for point in glow::ASK_POINTS {
                ui.horizontal_top(|ui| {
                    ui.label(RichText::new("•").color(p.accent));
                    ui.add(egui::Label::new(RichText::new(point).color(p.text)).wrap());
                });
            }
            ui.add_space(8.0);
            ui.horizontal_wrapped(|ui| {
                // Reading order: Allow and start, Not now. A screen reader hears the question
                // with each button (the title and body as its description).
                let question = format!("{} {}", glow::ASK_TITLE, glow::ASK_BODY);
                let allow = ui.add(theme::primary_button(ui, glow::ALLOW));
                let not_now = ui.button(glow::NOT_NOW);
                for id in [allow.id, not_now.id] {
                    ui.ctx()
                        .accesskit_node_builder(id, |n| n.set_description(question.clone()));
                }
                if std::mem::take(&mut draft.ask_focus) {
                    not_now.request_focus();
                }
                if allow.clicked() {
                    draft.asking = false;
                    out.push(Intent::AllowAndStart);
                }
                if not_now.clicked() {
                    draft.asking = false;
                }
            });
            ui.add_space(6.0);
            ui.add(egui::Label::new(RichText::new(glow::ASK_FOOTNOTE).color(p.muted)).wrap());
        });
    if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        draft.asking = false;
    }
}

/// Screen glow's controls (#496): what it does, Smoothing, and whether Foxglove may read the
/// screen, with "Stop allowing".
fn screen_controls(ui: &mut egui::Ui, v: &View, out: &mut Vec<Intent>) {
    let p = theme::palette(ui);
    let look = &v.look;
    ui.add(egui::Label::new(RichText::new(glow::DESCRIPTION).color(p.text)).wrap());
    ui.add_space(6.0);
    ui.label(theme::caps_label(ui, glow::SMOOTHING));
    let now = glow::smoothing_for(look.smoothing);
    ui.horizontal_wrapped(|ui| {
        theme::pill_group(ui, glow::SMOOTHING, |ui, g| {
            for s in foxglove_scene::Smoothing::ALL {
                if g.pill(ui, now == s, glow::smoothing_label(s)).clicked() && now != s {
                    out.push(Intent::Look {
                        look: Look {
                            smoothing: Some(s),
                            ..look.clone()
                        },
                        save: true,
                    });
                }
            }
        })
    });
    ui.add(egui::Label::new(RichText::new(glow::SMOOTHING_HINT).color(p.muted)).wrap());
    ui.add_space(8.0);
    if v.screen.allowed {
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(glow::ALLOWED_LINE).color(p.muted));
            if ui.link(glow::STOP_ALLOWING).clicked() {
                out.push(Intent::StopAllowing);
            }
        });
    } else {
        ui.label(RichText::new(glow::NOT_ALLOWED_LINE).color(p.muted));
    }
}

/// How far outside a focused keyboard's outline its ring sits (#801).
const FOCUS_RING_GAP: f32 = 3.0;

/// After a focused keyboard's name in the map's hover.
pub const FOCUS_RING_WORDS: &str = " · shown in Foxglove";

/// A soft round glow: `c` in the middle, fading to nothing at `radius`.
fn glow(painter: &egui::Painter, centre: Pos2, radius: f32, c: Color32) {
    const SEGMENTS: u32 = 32;
    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(centre, c.gamma_multiply(0.85));
    // A middle ring keeps the falloff soft rather than a cone.
    for (k, (f, a)) in [(0.45, 0.45), (1.0, 0.0)].into_iter().enumerate() {
        for i in 0..SEGMENTS {
            let angle = std::f32::consts::TAU * i as f32 / SEGMENTS as f32;
            let at = centre + egui::vec2(angle.cos(), angle.sin()) * radius * f;
            mesh.colored_vertex(at, c.gamma_multiply(a));
        }
        let ring = 1 + k as u32 * SEGMENTS;
        for i in 0..SEGMENTS {
            let j = (i + 1) % SEGMENTS;
            if k == 0 {
                mesh.add_triangle(0, ring + i, ring + j);
            } else {
                let inner = ring - SEGMENTS;
                mesh.add_triangle(inner + i, ring + i, ring + j);
                mesh.add_triangle(inner + i, ring + j, inner + j);
            }
        }
    }
    painter.add(egui::Shape::mesh(mesh));
}

/// A drag on the map moves the device it started on; the move is kept when it ends.
fn handle_drag(
    response: &egui::Response,
    v: &View,
    map: &Map,
    draft: &mut Draft,
    out: &mut Vec<Intent>,
) {
    if response.drag_started() {
        let start = response.interact_pointer_pos();
        draft.drag = start
            .and_then(|at| device_at(&v.room, map, at))
            .map(|d| (d.id.clone(), Point::default()));
    }
    if response.dragged() {
        if let Some((_, by)) = draft.drag.as_mut() {
            *by = *by + map.mm(response.drag_delta());
        }
    }
    if response.drag_stopped() {
        if let Some((id, by)) = draft.drag.take() {
            if by.x.abs() >= 1.0 || by.y.abs() >= 1.0 {
                out.push(Intent::Move { id, by });
            }
        }
    }
}

/// The scene across the whole room, faint: what a wallpaper would draw on the monitors, and
/// how the light travels between the devices. One mesh, its corners coloured by the effect.
fn field(
    painter: &egui::Painter,
    map: &Map,
    effect: &Effect,
    t: SceneTime,
    bounds: &foxglove_scene::Rect,
) {
    let area = map.rect(bounds);
    let nx = ((area.width() / 14.0).ceil() as usize).clamp(2, 48);
    let ny = ((area.height() / 14.0).ceil() as usize).clamp(2, 32);
    // The grid as one device's lights, so the effect renders it in one pass.
    let mut lights = Vec::with_capacity((nx + 1) * (ny + 1));
    for j in 0..=ny {
        for i in 0..=nx {
            let (fx, fy) = (i as f32 / nx as f32, j as f32 / ny as f32);
            lights.push(PlacedLight::point(
                String::new(),
                Point::new(bounds.x + fx * bounds.w, bounds.y + fy * bounds.h),
            ));
        }
    }
    let grid = RoomMap {
        devices: vec![Placed {
            id: String::new(),
            name: String::new(),
            kind: DeviceKind::Other,
            rect: *bounds,
            lights,
        }],
        bounds: *bounds,
    };
    let frame = effect.render(&grid, t);
    let mut mesh = egui::Mesh::default();
    for (k, l) in frame.lights.iter().enumerate() {
        let (i, j) = (k % (nx + 1), k / (nx + 1));
        let at = area.min
            + egui::vec2(
                area.width() * i as f32 / nx as f32,
                area.height() * j as f32 / ny as f32,
            );
        mesh.colored_vertex(at, color(l.colour).gamma_multiply(0.45));
    }
    let row = (nx + 1) as u32;
    for j in 0..ny as u32 {
        for i in 0..nx as u32 {
            let a = j * row + i;
            mesh.add_triangle(a, a + 1, a + row);
            mesh.add_triangle(a + 1, a + row + 1, a + row);
        }
    }
    painter.add(egui::Shape::mesh(mesh));
}

/// A strip of colours left to right, as one mesh.
fn strip(painter: &egui::Painter, rect: egui::Rect, stops: &[Rgb]) {
    let stops: Vec<Rgb> = if stops.is_empty() {
        vec![Rgb::BLACK]
    } else {
        stops.to_vec()
    };
    let n = stops.len().max(2) - 1;
    let mut mesh = egui::Mesh::default();
    for k in 0..=n {
        let x = rect.left() + rect.width() * k as f32 / n as f32;
        let c = color(stops[k.min(stops.len() - 1)]);
        mesh.colored_vertex(egui::pos2(x, rect.top()), c);
        mesh.colored_vertex(egui::pos2(x, rect.bottom()), c);
    }
    for k in 0..n as u32 {
        let a = k * 2;
        mesh.add_triangle(a, a + 1, a + 2);
        mesh.add_triangle(a + 1, a + 3, a + 2);
    }
    painter.add(egui::Shape::mesh(mesh));
}

/// A look's colours, for its chip in the preset list.
fn look_stops(look: &Look) -> Vec<Rgb> {
    if look.kind.reads_screen() {
        // Screen glow has no colours of its own: a screen's grey.
        return vec![
            foxglove_scene::effects::SCREEN_IDLE,
            Rgb([0x9a, 0xa0, 0xa8]),
        ];
    }
    if look.kind.uses_colour() {
        vec![Rgb(look.colour), look.background.rgb()]
    } else {
        looks::palette(&look.palette)
    }
}

/// The effect and its parameters, and saving the look as a preset.
fn effect_card(ui: &mut egui::Ui, v: &View, draft: &mut Draft, out: &mut Vec<Intent>) {
    let look = &v.look;
    // Screen glow's first Start: the permission card instead of the controls (#496).
    if look.kind.reads_screen() && draft.asking {
        permission_card(ui, draft, out);
        return;
    }
    let mut set = |look: Look, save: bool| out.push(Intent::Look { look, save });
    ui.horizontal_wrapped(|ui| {
        theme::pill_group(ui, "Effect", |ui, g| {
            for k in Kind::ALL {
                if g.pill(ui, look.kind == k, k.label())
                    .on_hover_text(k.hint())
                    .clicked()
                    && look.kind != k
                {
                    set(
                        Look {
                            kind: k,
                            ..look.clone()
                        },
                        true,
                    );
                }
            }
        })
    });
    ui.add_space(4.0);
    if look.kind.reads_screen() {
        screen_controls(ui, v, out);
        ui.add_space(6.0);
        save_row(ui, draft, out);
        return;
    }
    if look.kind.uses_colour() {
        ui.label(theme::caps_label(ui, "Colour"));
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            for (name, rgb) in SWATCHES {
                if swatch(ui, color(Rgb(rgb)), look.colour == rgb)
                    .on_hover_text(name)
                    .clicked()
                {
                    set(
                        Look {
                            colour: rgb,
                            ..look.clone()
                        },
                        true,
                    );
                }
            }
        });
        ui.label(theme::caps_label(ui, "Background"));
        ui.horizontal_wrapped(|ui| {
            theme::pill_group(ui, "Background", |ui, g| {
                for b in Background::ALL {
                    if g.pill(ui, look.background == b, b.label()).clicked() {
                        set(
                            Look {
                                background: b,
                                ..look.clone()
                            },
                            true,
                        );
                    }
                }
            })
        });
    } else {
        ui.label(theme::caps_label(ui, "Palette"));
        ui.horizontal_wrapped(|ui| {
            theme::pill_group(ui, "Palette", |ui, g| {
                // A wallpaper's palette shows only while its look has it and its wallpaper is
                // here (#488, #562).
                let own = (!PALETTES.iter().any(|(n, _)| *n == look.palette)
                    && looks::scenes()
                        .palettes
                        .iter()
                        .any(|p| p.name == look.palette))
                .then_some(look.palette.as_str());
                // Then the palette that follows the wallpaper on screen (#40).
                let names = PALETTES
                    .iter()
                    .map(|(n, _)| *n)
                    .chain([looks::WALLPAPER])
                    .chain(own);
                for name in names {
                    let label = if name == looks::WALLPAPER {
                        looks::WALLPAPER_LABEL
                    } else {
                        name
                    };
                    let mut pill = g.pill(ui, look.palette == name, label);
                    if name == looks::WALLPAPER {
                        pill = pill.on_hover_text(looks::WALLPAPER_HINT);
                    }
                    if pill.clicked() {
                        set(
                            Look {
                                palette: name.into(),
                                ..look.clone()
                            },
                            true,
                        );
                    }
                }
            })
        });
        let (rect, _) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), 10.0), Sense::hover());
        strip(ui.painter(), rect, &looks::palette(&look.palette));
    }
    ui.add_space(4.0);
    if look.kind.uses_speed() {
        let mut speed = look.speed;
        value_row(ui, "Speed", format!("{speed} of 10"));
        let r = full_slider(ui, egui::Slider::new(&mut speed, looks::SPEEDS));
        slid(
            &r,
            Look {
                speed,
                ..look.clone()
            },
            look,
            &mut set,
        );
    }
    if look.kind.uses_direction() {
        ui.label(theme::caps_label(ui, "Direction"));
        ui.horizontal_wrapped(|ui| {
            theme::pill_group(ui, "Direction", |ui, g| {
                for d in Direction::ALL {
                    if g.pill(ui, look.direction == d, d.label()).clicked() {
                        set(
                            Look {
                                direction: d,
                                ..look.clone()
                            },
                            true,
                        );
                    }
                }
            })
        });
    }
    if look.kind.uses_width() {
        let mut width = look.width_mm;
        value_row(ui, look.kind.width_label(), format!("{width} mm"));
        let r = full_slider(ui, egui::Slider::new(&mut width, looks::WIDTHS));
        slid(
            &r,
            Look {
                width_mm: width,
                ..look.clone()
            },
            look,
            &mut set,
        );
    }
    ui.add_space(6.0);
    save_row(ui, draft, out);
}

/// A slider's change: shown at once, saved when the drag ends (or on a key press).
fn slid(r: &egui::Response, new: Look, old: &Look, set: &mut impl FnMut(Look, bool)) {
    if r.changed() && new != *old {
        set(new, !r.dragged());
    } else if r.drag_stopped() {
        set(new, true);
    }
}

fn save_row(ui: &mut egui::Ui, draft: &mut Draft, out: &mut Vec<Intent>) {
    ui.label(theme::caps_label(ui, "Save as look"));
    let problem = looks::name_problem(&draft.name);
    let save = std::cell::Cell::new(false);
    split_row(
        ui,
        |ui| {
            let clicked = ui
                .add_enabled(problem.is_none(), egui::Button::new("Save"))
                .on_hover_text("Keep this look under Yours")
                .clicked();
            save.set(save.get() || clicked);
        },
        |ui| {
            let edit = egui::TextEdit::singleline(&mut draft.name)
                .hint_text("Name")
                .char_limit(looks::NAME_MAX)
                .desired_width(ui.available_width().max(60.0));
            let r = ui.add(edit);
            if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) && problem.is_none()
            {
                save.set(true);
            }
        },
    );
    if let Some(why) = problem.filter(|_| !draft.name.trim().is_empty()) {
        ui.weak(why);
    }
    if save.get() {
        out.push(Intent::SavePreset(draft.name.trim().to_string()));
    }
}

/// The presets: the wallpapers' looks (only when some wallpaper describes itself, #562), one per
/// effect, then the user's own.
fn presets(ui: &mut egui::Ui, v: &View, out: &mut Vec<Intent>) {
    let wallpapers = &v.wallpapers;
    if !wallpapers.is_empty() {
        ui.label(theme::caps_label(ui, "Wallpapers"));
        let mut with_art = v.with_art;
        // Only a board with an art screen has art to show (#560).
        if super::screen_roles::has(super::screen_roles::Role::Art)
            && ui
                .checkbox(
                    &mut with_art,
                    format!(
                        "{} too",
                        super::screen_roles::name(super::screen_roles::Role::Art)
                    ),
                )
                .on_hover_text(format!(
                    "While one of these runs, {} shows its wallpaper's art",
                    super::screen_roles::the(super::screen_roles::Role::Art)
                ))
                .changed()
        {
            out.push(Intent::WithArt(with_art));
        }
        for w in wallpapers {
            let shown = v.on_screen.contains(&w.wallpaper);
            let tag = shown.then_some("On screen");
            preset_row(
                ui,
                &w.preset,
                v.look == w.preset.look,
                false,
                tag,
                &w.hint(),
                out,
            );
        }
        ui.add_space(6.0);
        ui.label(theme::caps_label(ui, "Effects"));
    }
    for p in &looks::effect_presets() {
        preset_row(
            ui,
            p,
            v.look == p.look,
            false,
            None,
            p.look.kind.hint(),
            out,
        );
    }
    ui.add_space(6.0);
    ui.label(theme::caps_label(ui, "Yours"));
    if v.saved.is_empty() {
        ui.weak("Looks you save show here.");
    }
    for p in &v.saved {
        preset_row(ui, p, v.look == p.look, true, None, p.look.kind.hint(), out);
    }
}

/// One preset: its colours, its name, and Remove (the user's) or a tag ("On screen").
fn preset_row(
    ui: &mut egui::Ui,
    p: &Preset,
    selected: bool,
    removable: bool,
    tag: Option<&str>,
    hint: &str,
    out: &mut Vec<Intent>,
) {
    let mut remove = false;
    split_row(
        ui,
        |ui| {
            remove = removable
                && ui
                    .small_button("Remove")
                    .on_hover_text("Remove this look")
                    .clicked();
            if let Some(tag) = tag {
                ui.weak(tag)
                    .on_hover_text("Wallpaper Engine shows this wallpaper now");
            }
        },
        |ui| {
            let (chip, _) = ui.allocate_exact_size(egui::vec2(26.0, 12.0), Sense::hover());
            strip(ui.painter(), chip, &look_stops(&p.look));
            let label = ui.selectable_label(selected, &p.name).on_hover_text(hint);
            if label.clicked() && !selected {
                out.push(Intent::Look {
                    look: p.look.clone(),
                    save: true,
                });
            }
        },
    );
    if remove {
        out.push(Intent::RemovePreset(p.name.clone()));
    }
}

/// The feature that owns this file's store sections.
const STORE_OWNER: &str = "hideout.scenes";

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::foxglove::scene_room::tests::{sample_room, sample_room_empty, SAMPLE_CASE};

    /// The user's desk: the room with the case and the keyboard lit, the scanline over the
    /// case's middle fans.
    pub(crate) fn sample() -> View {
        View {
            room: sample_room(),
            keyboard_lit: true,
            keyboard_frames: false,
            keyboard_zones: Some("6 zones per half".into()),
            lit: vec![SAMPLE_CASE.into()],
            look: Look::default(),
            saved: vec![Preset {
                name: "Desk evening".into(),
                look: Look {
                    kind: Kind::Breathe,
                    colour: [0x7f, 0xb0, 0xe0],
                    ..Look::default()
                },
            }],
            running: Some("Bandit scanline".into()),
            stopping: false,
            heal: Default::default(),
            moved: true,
            // The band on the case's middle fans, on its way to the keys.
            t: SceneTime(2300),
            with_art: false,
            wallpapers: looks::scenes().looks.clone(),
            on_screen: vec!["tidepool.lighthouse".into()],
            triggers: Triggers {
                start_with_profile: true,
                ripple_keys: true,
                sweep_layers: false,
            },
            profile: "Game".into(),
            keep_on_keyboard: false,
            leds_placed: true,
            leds_shipped: false,
            lamp_note: None,
            yielded_to: None,
            focused: Vec::new(),
            screen: ScreenView::default(),
        }
    }

    /// `scenes.page` (#618 batch H, the sitting's old H4): the Presets card's rows in the order
    /// the card draws them, each wallpaper's look tagged on screen as the card tags it.
    #[test]
    fn the_presets_card_is_the_pages_rows() {
        let v = sample();
        let card = presets_card(&v.wallpapers, &v.on_screen, &v.saved);
        let names: Vec<_> = v.wallpapers.iter().map(|w| w.preset.name.clone()).collect();
        assert_eq!(
            card.wallpapers
                .iter()
                .map(|w| w.0.clone())
                .collect::<Vec<_>>(),
            names
        );
        for (w, (_, id, shown)) in v.wallpapers.iter().zip(&card.wallpapers) {
            assert_eq!(id, &w.wallpaper);
            assert_eq!(*shown, w.wallpaper == "tidepool.lighthouse", "{id}");
        }
        assert_eq!(
            card.effects,
            looks::effect_presets()
                .into_iter()
                .map(|p| p.name)
                .collect::<Vec<_>>()
        );
        assert_eq!(card.yours, ["Desk evening"]);
        assert!(presets_card(&[], &[], &[]).wallpapers.is_empty());
    }

    /// No devices found and nothing running.
    pub(crate) fn sample_none() -> View {
        View {
            room: sample_room_empty(),
            keyboard_lit: false,
            keyboard_frames: false,
            keyboard_zones: None,
            lit: vec![],
            look: Look {
                kind: Kind::Drift,
                palette: "Forest".into(),
                width_mm: 250,
                ..Look::default()
            },
            saved: vec![],
            running: None,
            stopping: false,
            heal: Default::default(),
            moved: false,
            t: SceneTime(5000),
            with_art: false,
            wallpapers: looks::scenes().looks.clone(),
            on_screen: vec![],
            triggers: Triggers::default(),
            profile: "Base settings".into(),
            keep_on_keyboard: false,
            leds_placed: false,
            leds_shipped: false,
            lamp_note: None,
            yielded_to: None,
            focused: Vec::new(),
            screen: ScreenView::default(),
        }
    }

    /// #496: Screen glow never starts without the yes, whoever asks: the page's Start shows the
    /// permission card instead, and a profile, the last look or the API is refused.
    #[test]
    fn screen_glow_needs_the_yes() {
        assert!(!start_allowed(Kind::ScreenGlow, false));
        assert!(start_allowed(Kind::ScreenGlow, true));
        for k in Kind::ALL.into_iter().filter(|k| *k != Kind::ScreenGlow) {
            assert!(start_allowed(k, false), "{k:?} needs no yes");
        }
        let glow = Effect::Screen {
            smoothing: foxglove_scene::Smoothing::Even,
        };
        // The one gate every start goes through (`scene_run::start`, `set_effect`).
        assert!(!crate::foxglove::scene_run::may_run(&glow, false));
        assert!(crate::foxglove::scene_run::may_run(&glow, true));
        assert!(crate::foxglove::scene_run::may_run(
            &crate::foxglove::scene_run::demo_effect(),
            false
        ));
        // Its map is a still drawing until it reads.
        assert_eq!(next_frame(Kind::ScreenGlow, false, false), None);
        // Its scene's name, even with a smoothing picked.
        let look = Look {
            kind: Kind::ScreenGlow,
            smoothing: Some(foxglove_scene::Smoothing::Calm),
            ..Look::default()
        };
        assert_eq!(scene_name(&look, &[]), "Screen glow");
        assert_eq!(
            look.effect(&RoomMap::default()),
            Effect::Screen {
                smoothing: foxglove_scene::Smoothing::Calm
            }
        );
    }

    /// #496: the status line while Screen glow reads, while each gate holds it, and after a
    /// start without the yes was refused.
    #[test]
    fn the_status_says_what_screen_glow_is_doing() {
        use crate::foxglove::screen_glow::{preview_status, Hold};
        let glow = Look {
            kind: Kind::ScreenGlow,
            ..Look::default()
        };
        let running = |status, yielded_to| View {
            look: glow.clone(),
            running: Some("Screen glow".into()),
            yielded_to,
            screen: ScreenView {
                allowed: true,
                status: Some(status),
                ..ScreenView::default()
            },
            ..sample()
        };
        assert_eq!(
            status(&running(preview_status(true, None), None)),
            (
                "Running: Screen glow · reading your screen".to_string(),
                Tone::Ok
            )
        );
        for (hold, words) in [
            (Hold::Game, "Paused while a game is full screen"),
            (Hold::Locked, "Paused while the PC is locked"),
            (Hold::DisplayOff, "Paused while the display is off"),
            (
                Hold::Paused,
                "Paused: Foxglove is paused. Resume it from the tray.",
            ),
            (
                Hold::Failed,
                "Can't read the screen right now. Foxglove tries again when the display changes.",
            ),
        ] {
            assert_eq!(
                status(&running(preview_status(false, Some(hold)), None)),
                (words.to_string(), Tone::Warn)
            );
        }
        assert_eq!(
            status(&running(
                preview_status(false, Some(Hold::Yielded)),
                Some("Windows Dynamic Lighting")
            ))
            .0,
            "Paused for Windows Dynamic Lighting"
        );
        // Picked, not running: no preview before the yes, so no "The map shows a preview".
        let stopped = View {
            look: glow.clone(),
            running: None,
            ..sample()
        };
        assert_eq!(status(&stopped), ("Not running.".to_string(), Tone::Muted));
        let refused = View {
            look: glow.clone(),
            running: None,
            screen: ScreenView {
                refused: true,
                ..ScreenView::default()
            },
            ..sample()
        };
        assert_eq!(
            status(&refused),
            (
                "Screen glow didn't start: it needs your yes. Pick it and press Start.".to_string(),
                Tone::Warn
            )
        );
    }

    /// #491: while the scene steps aside the status says for whom, in the warning tone; after
    /// the release it runs again.
    #[test]
    fn the_status_says_paused_while_the_scene_steps_aside() {
        use crate::foxglove::lights_owner::{yielded_words, State};
        let mut owner = State::default();
        let again = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let say = |s: &mut State, o: u8| {
            let mut r = [0u8; 32];
            r[..7].copy_from_slice(&[0xD7, 0x8C, 0, o, 30, 1, 1]);
            s.observe(&r);
        };
        let status_now = |s: &State| {
            status(&View {
                yielded_to: yielded_words(&s.now()),
                ..sample()
            })
        };
        say(&mut owner, 3);
        assert!(!owner.yield_to(&again));
        let running = ("Running: Bandit scanline".to_string(), Tone::Ok);
        assert_eq!(status_now(&owner), running);
        say(&mut owner, 1);
        assert!(owner.yield_to(&again));
        assert_eq!(
            status_now(&owner),
            ("Paused for Windows Dynamic Lighting".into(), Tone::Warn)
        );
        say(&mut owner, 0);
        assert_eq!(status_now(&owner), running, "released: running again");
        say(&mut owner, 9);
        assert!(owner.yield_to(&again));
        assert_eq!(
            status_now(&owner),
            ("Paused for another app".into(), Tone::Warn)
        );
        let none = View {
            yielded_to: Some("another app"),
            ..sample_none()
        };
        assert_eq!(status(&none).1, Tone::Muted, "no scene, nothing paused");
    }

    #[test]
    fn the_preview_waits_while_minimised_away_or_still() {
        assert_eq!(
            next_frame(Kind::Scanline, false, false),
            Some(PREVIEW_INTERVAL)
        );
        assert!(
            PREVIEW_INTERVAL >= Duration::from_millis(50),
            "20 frames a second at most"
        );
        assert_eq!(next_frame(Kind::Scanline, true, false), None);
        assert_eq!(next_frame(Kind::Ripple, false, true), None);
        assert_eq!(
            next_frame(Kind::Gradient, false, false),
            None,
            "a still look"
        );
    }

    #[test]
    fn the_legend_names_what_the_scene_lights() {
        let text = legend(&sample());
        assert!(
            text.starts_with("Lights: Eyelash Corne (one colour for both halves), HP OMEN 45L"),
            "{text}"
        );
        assert!(
            text.contains("Shown only: G502 Proteus Spectrum."),
            "{text}"
        );
        let none = legend(&sample_none());
        assert!(none.starts_with("Nothing to light yet"), "{none}");
        assert!(none.contains("Shown only: Keyboard (one colour"), "{none}");
        // Firmware with SET_FRAME (#529): with the profile's shipped LED map, each zone its own
        // colour, in the zones' words (#611), never a colour per LED it can't show.
        let frames = View {
            keyboard_frames: true,
            ..sample()
        };
        let text = legend(&frames);
        assert!(
            text.starts_with("Lights: Eyelash Corne (6 zones per half), HP OMEN 45L"),
            "{text}"
        );
        // Zones not known: the old words.
        let unknown = View {
            keyboard_zones: None,
            ..frames
        };
        assert!(
            legend(&unknown).starts_with("Lights: Eyelash Corne (a colour per LED)"),
            "{}",
            legend(&unknown)
        );
        // #563: a split whose parts aren't halves reads in its own words.
        let wedge = crate::foxglove::fixtures::wedge_parts();
        let text = legend_for(&sample(), &wedge);
        assert!(text.contains("(one colour for both parts)"), "{text}");
        assert!(!text.contains("halves"), "{text}");
    }

    #[test]
    fn a_drag_moves_the_device_under_it_but_never_a_monitor() {
        let v = sample();
        let map = Map::new(
            egui::Rect::from_min_size(Pos2::ZERO, egui::vec2(500.0, 300.0)),
            v.room.bounds,
        );
        let case = v.room.device(SAMPLE_CASE).unwrap();
        let at = map.rect(&case.rect).center();
        assert_eq!(
            device_at(&v.room, &map, at).map(|d| d.id.as_str()),
            Some(SAMPLE_CASE)
        );
        let monitor = v
            .room
            .devices
            .iter()
            .find(|d| d.kind == DeviceKind::Monitor)
            .unwrap();
        let on_monitor = map.rect(&monitor.rect).center();
        assert!(device_at(&v.room, &map, on_monitor).is_none());
        // A drag of 50 points is 50 / scale mm, and the drawn room moves with it.
        let by = map.mm(egui::vec2(50.0, 0.0));
        assert!((by.x * map.scale - 50.0).abs() < 1e-3);
        let moved = dragged_room(&v.room, Some(&(SAMPLE_CASE.into(), by)));
        let after = moved.device(SAMPLE_CASE).unwrap();
        assert_eq!(after.rect.x, case.rect.x + by.x);
        assert_eq!(after.lights[0].at.x, case.lights[0].at.x + by.x);
    }

    /// Where `label` was drawn in a frame's shapes.
    fn text_at(shapes: &[egui::epaint::ClippedShape], label: &str) -> Option<Pos2> {
        fn find(shape: &egui::Shape, label: &str) -> Option<Pos2> {
            match shape {
                egui::Shape::Vec(v) => v.iter().find_map(|s| find(s, label)),
                egui::Shape::Text(t) if t.galley.job.text == label => {
                    Some(t.pos + t.galley.rect.center().to_vec2())
                }
                _ => None,
            }
        }
        shapes.iter().find_map(|s| find(&s.shape, label))
    }

    /// The intents a click on `label` gives, through the real draw code.
    fn click(v: &View, label: &str) -> Vec<Intent> {
        let ctx = egui::Context::default();
        let mut fonts = egui::FontDefinitions::default();
        crate::platform::add_phosphor_to_fonts(&mut fonts);
        theme::add_fonts(&mut fonts);
        ctx.set_fonts(fonts);
        let mut found = Vec::new();
        let mut at = None;
        for _ in 0..3 {
            let mut raw = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    Pos2::ZERO,
                    egui::vec2(900.0, 1100.0),
                )),
                ..Default::default()
            };
            if let Some(at) = at.take() {
                raw.events = vec![
                    egui::Event::PointerMoved(at),
                    egui::Event::PointerButton {
                        pos: at,
                        button: egui::PointerButton::Primary,
                        pressed: true,
                        modifiers: Default::default(),
                    },
                    egui::Event::PointerButton {
                        pos: at,
                        button: egui::PointerButton::Primary,
                        pressed: false,
                        modifiers: Default::default(),
                    },
                ];
            }
            let mut out = ctx.run_ui(raw, |ui| {
                found.extend(draw(ui, v, &mut Draft::default()));
            });
            out.textures_delta.clear();
            if found.is_empty() {
                at = text_at(&out.shapes, label);
            }
        }
        found
    }

    #[test]
    fn a_running_scene_shows_stop_and_an_idle_one_start() {
        assert_eq!(click(&sample(), "Stop"), vec![Intent::Stop]);
        assert_eq!(click(&sample_none(), "Start"), vec![Intent::Start]);
        assert_eq!(
            click(&sample(), "Reset positions"),
            vec![Intent::ResetPositions]
        );
        assert!(
            click(&sample(), "Start").is_empty(),
            "no Start while one runs"
        );
        // A preset row picks its look; an effect pill keeps the other parameters.
        let picked = click(&sample_none(), "Steel sweep");
        assert!(
            matches!(&picked[..], [Intent::Look { look, save: true }] if look.kind == Kind::Sweep && look.palette == "Steel"),
            "{picked:?}"
        );
        let pill = click(&sample(), "Ripple");
        assert!(
            matches!(&pill[..], [Intent::Look { look, save: true }] if look.kind == Kind::Ripple && look.colour == sample().look.colour),
            "{pill:?}"
        );
        assert_eq!(
            click(&sample(), "Remove"),
            vec![Intent::RemovePreset("Desk evening".into())]
        );
    }

    /// The Triggers card (#489): each box sends its own switch, from the active profile's state.
    #[test]
    fn the_trigger_boxes_switch_their_own_trigger() {
        assert_eq!(
            click(&sample(), "Start with this profile"),
            vec![Intent::Trigger(Trigger::StartWithProfile, false)]
        );
        assert_eq!(
            click(&sample(), "Ripple on key press"),
            vec![Intent::Trigger(Trigger::RippleKeys, false)]
        );
        assert_eq!(
            click(&sample_none(), "Sweep on layer change"),
            vec![Intent::Trigger(Trigger::SweepLayers, true)]
        );
    }

    /// Whether the page draws `label`.
    fn shows(v: &View, label: &str) -> bool {
        let ctx = egui::Context::default();
        let mut fonts = egui::FontDefinitions::default();
        crate::platform::add_phosphor_to_fonts(&mut fonts);
        theme::add_fonts(&mut fonts);
        ctx.set_fonts(fonts);
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                Pos2::ZERO,
                egui::vec2(900.0, 1100.0),
            )),
            ..Default::default()
        };
        let mut out = ctx.run_ui(raw, |ui| {
            draw(ui, v, &mut Draft::default());
        });
        out.textures_delta.clear();
        text_at(&out.shapes, label).is_some()
    }

    /// Wallpapers found late restyle only a page scene that ran unnamed with a wallpaper look
    /// saved: never the demo, a preset or the user's own look (#562).
    #[test]
    fn only_an_unnamed_wallpaper_look_is_restyled_when_wallpapers_change() {
        assert!(restyle_on_wallpapers(CUSTOM_SCENE, true));
        assert!(
            !restyle_on_wallpapers(CUSTOM_SCENE, false),
            "the user's own look"
        );
        assert!(
            !restyle_on_wallpapers("demo scanline", true),
            "the demo scene"
        );
        assert!(!restyle_on_wallpapers("Reef drift", true), "already named");
    }

    /// With no wallpaper that describes itself, only the effects show (#562).
    #[test]
    fn no_wallpapers_no_wallpaper_group() {
        let none = View {
            wallpapers: vec![],
            ..sample()
        };
        assert!(shows(&sample(), "WALLPAPERS"));
        assert!(shows(&sample(), "Art screen too"));
        assert!(!shows(&none, "WALLPAPERS"));
        assert!(!shows(&none, "Art screen too"));
        assert!(!shows(&none, "Reef drift"));
        assert!(shows(&none, "Steel sweep"), "the effects stay");
    }

    /// A wallpaper's looks (#488, #562; here a fake third-party pack): a row picks the look,
    /// and "Art screen too" switches their art on and off.
    #[test]
    fn a_wallpaper_look_is_picked_and_its_art_switched() {
        let v = sample_none();
        let reef = v
            .wallpapers
            .iter()
            .find(|w| w.wallpaper == "tidepool.reef")
            .unwrap();
        let picked = click(&v, "Reef drift");
        assert_eq!(
            picked,
            vec![Intent::Look {
                look: reef.preset.look.clone(),
                save: true
            }]
        );
        assert_eq!(
            click(&sample(), "Art screen too"),
            vec![Intent::WithArt(true)]
        );
        let on = View {
            with_art: true,
            ..sample()
        };
        assert_eq!(click(&on, "Art screen too"), vec![Intent::WithArt(false)]);
        // The wallpaper's own palette is a pill while its look is chosen, and only then.
        let v = View {
            look: reef.preset.look.clone(),
            ..sample_none()
        };
        let forest = click(&v, "Forest");
        assert!(
            matches!(&forest[..], [Intent::Look { look, .. }] if look.palette == "Forest"),
            "{forest:?}"
        );
        assert!(click(&sample_none(), "Reef").is_empty());
    }

    /// #40: every palette effect offers the "Wallpaper" palette after Foxglove's own; a
    /// one-colour effect doesn't.
    #[test]
    fn the_wallpaper_palette_is_a_pill() {
        let v = View {
            look: Look {
                kind: Kind::Gradient,
                ..Look::default()
            },
            ..sample_none()
        };
        let picked = click(&v, looks::WALLPAPER_LABEL);
        assert!(
            matches!(&picked[..], [Intent::Look { look, save: true }] if look.follows_wallpaper()),
            "{picked:?}"
        );
        let scan = View {
            look: Look::default(),
            ..sample_none()
        };
        assert!(click(&scan, looks::WALLPAPER_LABEL).is_empty());
    }

    /// A look the user started comes back after a restart; one they stopped doesn't; a store
    /// from before the flag resumes a saved look (choosing one used to start it).
    #[test]
    fn the_launch_resumes_a_look_that_ran() {
        assert!(resume_at_launch(Some(true), false));
        assert!(resume_at_launch(Some(true), true));
        assert!(!resume_at_launch(Some(false), true));
        assert!(!resume_at_launch(Some(false), false));
        assert!(resume_at_launch(None, true));
        assert!(!resume_at_launch(None, false));
    }

    /// `[scene_resume]` round-trips through the (sandboxed) store; Start writes true, Stop false.
    #[test]
    fn the_resume_flag_is_kept_in_the_store() {
        crate::foxglove::store::test_config_dir();
        assert!(!crate::foxglove::store::PROFILED.contains(&RESUME_SECTION));
        save_resume(true);
        let r: Resume = crate::foxglove::store::section(STORE_OWNER, RESUME_SECTION)
            .try_load()
            .unwrap();
        assert_eq!(r.running, Some(true));
        save_resume(false);
        let r: Resume = crate::foxglove::store::section(STORE_OWNER, RESUME_SECTION)
            .try_load()
            .unwrap();
        assert_eq!(r.running, Some(false));
        // An old store has no key: `None`, so the look decides.
        let old: Resume = toml::from_str("").unwrap();
        assert_eq!(old.running, None);
    }

    /// A palette effect clicked while a scene still ends (start refused) waits: the stop thread
    /// decides again and the look is still due; a start that ran clears it.
    #[test]
    fn a_start_refused_during_a_stop_waits_for_it() {
        RECHECK.store(false, Ordering::Relaxed);
        after_start(false);
        assert!(RESUME.load(Ordering::Relaxed));
        assert!(RECHECK.load(Ordering::Relaxed));
        assert!(crate::foxglove::scene_triggers::restart_after_stop(
            RECHECK.swap(false, Ordering::Relaxed),
            true
        ));
        // After the stop: no profile box, but the pending look starts.
        assert_eq!(
            crate::foxglove::scene_triggers::on_profile(false, false, false),
            crate::foxglove::scene_triggers::ProfileAction::Nothing
        );
        after_start(true);
        assert!(!RESUME.load(Ordering::Relaxed));
        assert!(!RECHECK.load(Ordering::Relaxed));
    }
}
