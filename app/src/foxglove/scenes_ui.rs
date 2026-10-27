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
    /// The room ids of the other devices a scene lights (boards with a driver, Dynamic
    /// Lighting devices).
    pub lit: Vec<String>,
    pub look: Look,
    pub saved: Vec<Preset>,
    /// The running scene's name.
    pub running: Option<String>,
    /// Stop was pressed and the lights are being given back.
    pub stopping: bool,
    /// The user moved a device, so "Reset positions" shows.
    pub moved: bool,
    /// The moment the preview shows, on the running scene's clock (or the page's).
    pub t: SceneTime,
    /// "Right screen too": a wallpaper's look also shows its right-screen art (#488).
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
    SavePreset(String),
    RemovePreset(String),
    /// A device dragged on the map, by this much (mm).
    Move {
        id: String,
        by: Point,
    },
    ResetPositions,
    /// "Right screen too" switched on or off.
    WithArt(bool),
    /// A Triggers box switched on or off.
    Trigger(Trigger, bool),
    /// "Keep running on the keyboard" switched on or off.
    KeepOnKeyboard(bool),
    /// "Find my LEDs…": to the Lights page, with the wizard open.
    FindLeds,
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
}

static STATE: Mutex<Option<State>> = Mutex::new(None);
static DRAFT: Mutex<Draft> = Mutex::new(Draft {
    name: String::new(),
    drag: None,
    ambient: Ambient::new(),
});
static STOPPING: AtomicBool = AtomicBool::new(false);
/// The running scene was started by a profile switch ("Start with this profile"), so a switch
/// to a profile without it stops it (#489).
static BY_PROFILE: AtomicBool = AtomicBool::new(false);
/// Foxglove has started its services, so a profile switch may start a scene.
static READY: AtomicBool = AtomicBool::new(false);
/// A profile switch came while a stop was ending a scene: decide again after it.
static RECHECK: AtomicBool = AtomicBool::new(false);

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
        lit: st.lit.clone(),
        look: st.chosen.look.clone(),
        saved: st.saved.presets.clone(),
        running: running.map(|(name, _)| name),
        stopping: STOPPING.load(Ordering::Relaxed),
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
    };
    let mut draft = DRAFT.lock().unwrap_or_else(|e| e.into_inner());
    let intents = draw(ui, &view, &mut draft);
    animate(&ctx, &view);
    for intent in intents {
        apply(st, &mut draft, intent, &ctx);
    }
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
    st.lit = ids(|c| matches!(c, Control::Driver(_) | Control::LampArray));
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
            st.room
                .device(&id)
                .map_or_else(|| key.clone(), |d| d.name.clone())
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
        super::scene_triggers::ProfileAction::Start => start_by_profile(&chosen),
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
        }
        super::scene_triggers::ProfileAction::Nothing => {}
    }
}

/// The wallpapers' looks changed (#562, the wallpaper service's first gather or a wallpaper
/// added or removed): a running scene takes the chosen look again, so a wallpaper look started
/// before its wallpaper was found gets its palette, name and right-screen art.
pub fn wallpapers_changed() {
    if super::scene_run::running().is_none() {
        return;
    }
    let chosen = looks::load_chosen();
    let saved = looks::load_saved().presets;
    let room = super::scene_room::room();
    super::scene_run::set_effect(&scene_name(&chosen.look, &saved), chosen.look.effect(&room));
    show_art(&chosen);
}

/// A profile's "Start with this profile" starts its scene.
fn start_by_profile(chosen: &Chosen) {
    let saved = looks::load_saved().presets;
    let room = super::scene_room::room();
    let name = scene_name(&chosen.look, &saved);
    if super::scene_run::start(&name, chosen.look.effect(&room), None) {
        BY_PROFILE.store(true, Ordering::Relaxed);
        show_art(chosen);
    } else {
        // A scene is still ending: start once it has (the stop thread decides again).
        RECHECK.store(true, Ordering::Relaxed);
    }
}

/// Foxglove started its services: the trigger boxes take effect, and the active profile's
/// scene starts when it has "Start with this profile" on.
pub fn startup() {
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

/// The name a look runs under: its preset's, or "Custom scene".
fn scene_name(look: &Look, saved: &[Preset]) -> String {
    looks::named(look, &looks::built_in(), saved)
        .unwrap_or("Custom scene")
        .to_string()
}

fn apply(st: &mut State, draft: &mut Draft, intent: Intent, ctx: &egui::Context) {
    match intent {
        Intent::Look { look, save } => {
            st.chosen.look = look.clamped();
            if save {
                looks::save_chosen(&st.chosen);
            }
            resend(st);
        }
        Intent::Start => {
            BY_PROFILE.store(false, Ordering::Relaxed);
            looks::save_chosen(&st.chosen);
            let name = scene_name(&st.chosen.look, &st.saved.presets);
            if super::scene_run::start(&name, st.chosen.look.effect(&st.room), None) {
                show_art(&st.chosen);
            } else {
                eprintln!("Foxglove: a scene still runs; Start waits for it to end");
            }
        }
        Intent::WithArt(on) => {
            st.chosen.with_art = on;
            looks::save_chosen(&st.chosen);
            show_art(&st.chosen);
        }
        Intent::Stop => {
            BY_PROFILE.store(false, Ordering::Relaxed);
            stop_scene();
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
    if let Err(e) = super::store::save(super::scene_room::SECTION, layout) {
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

/// The running scene's right-screen art: its wallpaper's with "Right screen too", else none
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
    (!minimised && !away && kind != Kind::Gradient).then_some(PREVIEW_INTERVAL)
}

fn animate(ctx: &egui::Context, v: &View) {
    let minimised = ctx.input(|i| i.viewport().minimized) == Some(true);
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
        card(ui, "Presets", |ui| presets(ui, v, &mut out));
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
                card(ui, "Presets", |ui| presets(ui, v, &mut out));
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
                let stop = ui.add_enabled(!v.stopping, egui::Button::new("Stop"));
                if stop
                    .on_hover_text("Stop the scene and put the lights back")
                    .clicked()
                {
                    out.push(Intent::Stop);
                }
            } else if ui
                .add(theme::primary_button(ui, "Start"))
                .on_hover_text("Run this scene on the lights")
                .clicked()
            {
                out.push(Intent::Start);
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
            let (text, color) = match (&v.running, v.stopping) {
                (_, true) => ("Stopping: putting the lights back…".to_string(), p.text),
                (Some(name), false) => (format!("Running: {name}"), p.ok),
                (None, false) => ("Not running. The map shows a preview.".to_string(), p.muted),
            };
            ui.add(egui::Label::new(RichText::new(text).color(color)).wrap());
        },
    );
    ui.add_space(6.0);
    room_map(ui, v, draft, out);
    ui.add_space(6.0);
    ui.add(egui::Label::new(RichText::new(legend(v)).color(p.muted)).wrap());
    if let Some((text, stopped)) = &v.lamp_note {
        let color = if *stopped { p.warn } else { p.muted };
        ui.add_space(4.0);
        ui.add(egui::Label::new(RichText::new(text).color(color)).wrap());
    }
    if v.keyboard_frames && !v.leds_placed {
        let (label, hint, text, color) = if v.leds_shipped {
            (
                "Check LED map…",
                "A dot runs along the LEDs; say whether it follows the drawing: about 20 seconds",
                "Scenes use the LED map that comes with this keyboard, not yet checked on yours.",
                p.muted,
            )
        } else {
            (
                "Find my LEDs…",
                "Show Foxglove where each LED is, so scenes light every LED in its place",
                "The keyboard lights a colour per half until Foxglove knows where its LEDs are.",
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

/// Which devices the scene lights, in words.
pub fn legend(v: &View) -> String {
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
            let how = if !v.keyboard_frames {
                "one colour for both halves"
            } else if d.lights.len() > 1 {
                "a colour per LED"
            } else {
                "a colour per half"
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
    let mut frame = effect.render(&room, v.t);
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
            response.on_hover_text_at_pointer(format!("{} · {lit} · drag to move", d.name));
        }
    }
}

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
    if look.kind.uses_colour() {
        vec![Rgb(look.colour), look.background.rgb()]
    } else {
        looks::palette(&look.palette)
    }
}

/// The effect and its parameters, and saving the look as a preset.
fn effect_card(ui: &mut egui::Ui, v: &View, draft: &mut Draft, out: &mut Vec<Intent>) {
    let look = &v.look;
    let mut set = |look: Look, save: bool| out.push(Intent::Look { look, save });
    ui.horizontal_wrapped(|ui| {
        for k in Kind::ALL {
            if theme::pill(ui, look.kind == k, k.label())
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
    });
    ui.add_space(4.0);
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
            for b in Background::ALL {
                if theme::pill(ui, look.background == b, b.label()).clicked() {
                    set(
                        Look {
                            background: b,
                            ..look.clone()
                        },
                        true,
                    );
                }
            }
        });
    } else {
        ui.label(theme::caps_label(ui, "Palette"));
        ui.horizontal_wrapped(|ui| {
            // A wallpaper's palette shows only while its look has it and its wallpaper is
            // here (#488, #562).
            let own = (!PALETTES.iter().any(|(n, _)| *n == look.palette)
                && looks::scenes()
                    .palettes
                    .iter()
                    .any(|p| p.name == look.palette))
            .then_some(look.palette.as_str());
            for name in PALETTES.iter().map(|(n, _)| *n).chain(own) {
                if theme::pill(ui, look.palette == name, name).clicked() {
                    set(
                        Look {
                            palette: name.into(),
                            ..look.clone()
                        },
                        true,
                    );
                }
            }
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
            for d in Direction::ALL {
                if theme::pill(ui, look.direction == d, d.label()).clicked() {
                    set(
                        Look {
                            direction: d,
                            ..look.clone()
                        },
                        true,
                    );
                }
            }
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
    ui.label(theme::caps_label(ui, "Save as preset"));
    let problem = looks::name_problem(&draft.name);
    let save = std::cell::Cell::new(false);
    split_row(
        ui,
        |ui| {
            let clicked = ui
                .add_enabled(problem.is_none(), egui::Button::new("Save"))
                .on_hover_text("Keep this look in your presets")
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
        if ui
            .checkbox(&mut with_art, "Right screen too")
            .on_hover_text("While one of these runs, the right screen shows its wallpaper's art")
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
        ui.weak("Presets you save show here.");
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
                    .on_hover_text("Remove this preset")
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
        }
    }

    /// No devices found and nothing running.
    pub(crate) fn sample_none() -> View {
        View {
            room: sample_room_empty(),
            keyboard_lit: false,
            keyboard_frames: false,
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
        }
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
        // Firmware with SET_FRAME (#529): with the profile's shipped LED map, each LED its own
        // colour.
        let frames = View {
            keyboard_frames: true,
            ..sample()
        };
        let text = legend(&frames);
        assert!(
            text.starts_with("Lights: Eyelash Corne (a colour per LED), HP OMEN 45L"),
            "{text}"
        );
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

    /// With no wallpaper that describes itself, only the effects show (#562).
    #[test]
    fn no_wallpapers_no_wallpaper_group() {
        let none = View {
            wallpapers: vec![],
            ..sample()
        };
        assert!(shows(&sample(), "WALLPAPERS"));
        assert!(shows(&sample(), "Right screen too"));
        assert!(!shows(&none, "WALLPAPERS"));
        assert!(!shows(&none, "Right screen too"));
        assert!(!shows(&none, "Reef drift"));
        assert!(shows(&none, "Steel sweep"), "the effects stay");
    }

    /// A wallpaper's looks (#488, #562; here a fake third-party pack): a row picks the look,
    /// and "Right screen too" switches their art on and off.
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
            click(&sample(), "Right screen too"),
            vec![Intent::WithArt(true)]
        );
        let on = View {
            with_art: true,
            ..sample()
        };
        assert_eq!(click(&on, "Right screen too"), vec![Intent::WithArt(false)]);
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
}
