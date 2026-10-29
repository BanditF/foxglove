/*
 * Underglow idle auto-off, in place of ZMK's CONFIG_ZMK_RGB_UNDERGLOW_AUTO_OFF_IDLE.
 * SPDX-License-Identifier: MIT
 *
 * ZMK's auto-off turns the underglow off with zmk_rgb_underglow_off(), which saves "off" to flash
 * as if the user had chosen it. The right half runs on battery and deep-sleeps after an hour of no
 * key presses of its own (gaming: right hand on the mouse); a key press then reboots it, it loads
 * "off" and stays dark. Here the user's choice (wanted) is kept apart from the idle state and
 * saved on its own, so the underglow comes back after idle, sleep or a reboot.
 *
 * On a split keyboard the left (central) sees every key press, so its activity is the keyboard's.
 * With sync on (rgb_idle_set_sync, from the PC), the left sends its underglow state to the right
 * over the art relay link and the right follows it; with sync off, or while the link is down, the
 * right goes by its own activity.
 */

#include <string.h>

#include <zephyr/device.h>
#include <zephyr/drivers/led_strip.h>
#include <zephyr/kernel.h>
#include <zephyr/settings/settings.h>
#include <zephyr/spinlock.h>
#include <zephyr/sys/byteorder.h>

#include <drivers/ext_power.h>

#include <zmk/activity.h>
#include <zmk/event_manager.h>
#include <zmk/events/activity_state_changed.h>
#include <zmk/events/position_state_changed.h>
#include <zmk/events/sensor_event.h>
#include <zmk/keymap.h>
#include <zmk/sensors.h>
#include <zmk/rgb_underglow.h>

#include "rgb_idle.h"
#include "art_relay.h"
#include "host_link.h"
#if IS_ENABLED(CONFIG_NICE_VIEW_RGB_BATTERY_DIM)
#include "battery_dim.h"
#endif

BUILD_ASSERT(!IS_ENABLED(CONFIG_ZMK_RGB_UNDERGLOW_AUTO_OFF_IDLE),
             "NICE_VIEW_RGB_IDLE replaces ZMK_RGB_UNDERGLOW_AUTO_OFF_IDLE; turn that off");

#define CENTRAL (!IS_ENABLED(CONFIG_ZMK_SPLIT) || IS_ENABLED(CONFIG_ZMK_SPLIT_ROLE_CENTRAL))
#define RELAY_CENTRAL (CENTRAL && IS_ENABLED(CONFIG_NICE_VIEW_ART_RELAY))
#define HOST_CENTRAL (CENTRAL && IS_ENABLED(CONFIG_NICE_VIEW_HOST_LINK))
#define RIGHT_RELAY (!CENTRAL && IS_ENABLED(CONFIG_NICE_VIEW_ART_RELAY))

// On/off changes made with the rgb_ug behaviours are picked up this long after a key, an encoder
// step or a PC invoke, once the binding has run (#280: no free-running poll).
#define CHECK_AFTER K_MSEC(50)
// Saving waits a little, so a few quick toggles make one flash write.
#define SAVE_DELAY K_SECONDS(10)

static bool wanted;      // the user's on/off, saved
static bool loaded;      // wanted came from the settings
static bool sync = true; // left: the right follows the left, saved
static bool following;   // right: the left's state is in charge
static bool remote_on;   // right: the left's state
static atomic_t host_drawn; // the PC draws this half's LEDs (SET_FRAME, lamp_strip.c)
static atomic_t host_want;  // the last rgb_idle_host_control: hand-back pending or done

static bool active(void) { return zmk_activity_get_state() == ZMK_ACTIVITY_ACTIVE; }

/* What the LEDs show (#605). D7 83's on byte is ZMK's on flag (or, under the PC's frames, the
 * on/off hand-back will show): it read off while a scene lit the LEDs, and on while ext power or
 * a black frame kept them dark. Every frame goes through dim_strip.c, which hands each one's
 * summary here (rgb_idle_frame); with ext power and the owner (ZMK or the PC) that says whether
 * the LEDs are lit and what the brightest one shows. A board without dim_strip.c has no frames
 * here, and lit falls back to the owner's on/off. */

#define EXT_POWER                                                                                  \
    (IS_ENABLED(CONFIG_ZMK_RGB_UNDERGLOW_EXT_POWER) &&                                             \
     DT_HAS_COMPAT_STATUS_OKAY(zmk_ext_power_generic))
#if EXT_POWER
static const struct device *const ext_power = DEVICE_DT_GET(DT_INST(0, zmk_ext_power_generic));
#endif

static bool powered(void) {
#if EXT_POWER
    return !device_is_ready(ext_power) || ext_power_get(ext_power) > 0;
#else
    return true;
#endif
}

struct frame {
    bool seen;       // a frame from this owner went out since it last took the LEDs
    bool lit;        // any LED in it was lit
    uint8_t r, g, b; // its brightest LED, before the battery dim (the first of equals)
    uint8_t cap;     // the battery dim it went out with, percent
};
static struct frame frames[2]; // [0] ZMK's own, [1] the PC's
static struct k_spinlock frame_lock;

static struct frame frame_get(bool host) {
    k_spinlock_key_t key = k_spin_lock(&frame_lock);
    struct frame f = frames[host];
    k_spin_unlock(&frame_lock, key);
    return f;
}

static void frame_forget(bool host) {
    k_spinlock_key_t key = k_spin_lock(&frame_lock);
    frames[host].seen = false;
    k_spin_unlock(&frame_lock, key);
}

#define EFFECT_BREATHE 1 // ZMK's breathe passes through black every cycle: still on

bool rgb_idle_lit(void) {
    if (!powered()) {
        return false;
    }
    bool host = atomic_get(&host_drawn);
    struct frame f = frame_get(host);
    if (host) {
        return !f.seen || f.lit; // the PC's frames draw whatever the on/off (#542)
    }
    bool on;
    if (zmk_rgb_underglow_get_state(&on) != 0 || !on) {
        return false;
    }
    return !f.seen || f.lit || zmk_rgb_underglow_calc_effect(0) == EFFECT_BREATHE;
}

static void output_changed(void);

void rgb_idle_frame(bool host, const struct led_rgb *pixels, size_t n, unsigned int cap) {
    struct frame f = {.seen = true, .cap = MIN(cap, 100)};
    uint8_t best = 0;
    for (size_t i = 0; i < n; i++) {
        uint8_t m = MAX(pixels[i].r, MAX(pixels[i].g, pixels[i].b));
        if (m > best) {
            best = m;
            f.r = pixels[i].r;
            f.g = pixels[i].g;
            f.b = pixels[i].b;
        }
    }
    f.lit = best > 0 && f.cap > 0; // dim_strip keeps a lit channel at 1 or more
    k_spinlock_key_t key = k_spin_lock(&frame_lock);
    bool flipped = !frames[host].seen || frames[host].lit != f.lit;
    frames[host] = f;
    k_spin_unlock(&frame_lock, key);
    if (flipped) {
        output_changed(); // dark to lit or back: only then, not every frame
    }
}

static void to_hs(uint8_t r, uint8_t g, uint8_t b, uint16_t *hue, uint8_t *sat) {
    int mx = MAX(r, MAX(g, b)), mn = MIN(r, MIN(g, b)), d = mx - mn;
    *sat = mx == 0 ? 0 : (d * 100 + mx / 2) / mx;
    if (d == 0) {
        *hue = 0;
        return;
    }
    int h = mx == r ? 60 * (g - b) : mx == g ? 120 * d + 60 * (b - r) : 240 * d + 60 * (r - g);
    h = (h + (h < 0 ? -d / 2 : d / 2)) / d; // rounded
    *hue = (h + 360) % 360;
}

/* State for the PC */

static uint8_t speed = CONFIG_ZMK_RGB_UNDERGLOW_SPD_START;

// ZMK keeps the animation speed to itself, so it comes from what ZMK last saved: struct
// rgb_underglow_state in rgb_underglow.c (v0.3): h u16, s u8, b u8, speed u8, effect u8, ...
//
// Reading the settings is slow and takes the settings lock, which main holds for the whole boot
// time settings_load. Doing it on the system work queue from the settings commit is the likely
// cause of #45's boot hang (the left didn't even enumerate on USB). So it only happens when the PC
// asks, well after boot, at most every SPEED_EVERY_MS, on a thread of its own.
#define SPEED_AFTER_BOOT K_SECONDS(10)
#define SPEED_EVERY_MS 5000
#define SPEED_STACK_SIZE 2048

K_THREAD_STACK_DEFINE(speed_stack, SPEED_STACK_SIZE);
static struct k_work_q speed_q;
static atomic_t speed_ready;
static int64_t speed_read_at;
static int speed_cb(const char *key, size_t len, settings_read_cb read_cb, void *cb_arg,
                    void *param) {
    uint8_t buf[16];
    if (len >= 6 && len <= sizeof(buf) && read_cb(cb_arg, buf, len) == len && buf[4] >= 1 &&
        buf[4] <= 5) {
        speed = buf[4];
    }
    return 0;
}

static void speed_work_cb(struct k_work *work) {
    settings_load_subtree_direct("rgb/underglow/state", speed_cb, NULL);
}
static K_WORK_DEFINE(speed_work, speed_work_cb);

static void speed_ready_cb(struct k_work *work) {
    if (atomic_get(&speed_ready)) {
        return;
    }
    k_work_queue_start(&speed_q, speed_stack, K_THREAD_STACK_SIZEOF(speed_stack),
                       K_LOWEST_APPLICATION_THREAD_PRIO, NULL);
    atomic_set(&speed_ready, 1);
}
static K_WORK_DELAYABLE_DEFINE(speed_ready_work, speed_ready_cb);

int rgb_idle_get_on(bool *on) {
    if (atomic_get(&host_drawn)) {
        // ZMK's underglow is off under the PC's frames: say what hand-back will show.
        *on = following ? remote_on : wanted && active();
        return 0;
    }
    return zmk_rgb_underglow_get_state(on);
}

void rgb_idle_state(struct rgb_idle_wire *out) {
    bool on = false;
    rgb_idle_get_on(&on);
    struct zmk_led_hsb hsb = zmk_rgb_underglow_calc_hue(0);
    *out = (struct rgb_idle_wire){
        .on = on,
        .hue = hsb.h,
        .saturation = hsb.s,
        .brightness = hsb.b,
        .effect = zmk_rgb_underglow_calc_effect(0),
        .speed = speed,
    };
    int64_t now = k_uptime_get();
    if (atomic_get(&speed_ready) && now - speed_read_at >= SPEED_EVERY_MS) {
        speed_read_at = now;
        k_work_submit_to_queue(&speed_q, &speed_work); // fresher for next time
    }
}

void rgb_idle_shown(struct rgb_shown_wire *out) {
    bool host = atomic_get(&host_drawn);
    struct frame f = frame_get(host);
    bool lit = rgb_idle_lit();
    uint8_t flags = RGB_SHOWN_VALID;
    flags |= lit ? RGB_SHOWN_LIT : 0;
    flags |= wanted ? RGB_SHOWN_WANTED : 0;
    flags |= host ? RGB_SHOWN_HOST : 0;
    flags |= active() ? 0 : RGB_SHOWN_IDLE;
    flags |= powered() ? 0 : RGB_SHOWN_UNPOWERED;
    flags |= following ? RGB_SHOWN_FOLLOWING : 0;
#if IS_ENABLED(CONFIG_NICE_VIEW_RGB_BATTERY_DIM)
    uint8_t dim = battery_dim_level();
#else
    uint8_t dim = 100;
#endif
    *out = (struct rgb_shown_wire){.hue = 0xffff, .dim = dim};
    if (lit && f.seen && f.lit) {
        flags |= RGB_SHOWN_DRAWN;
        uint16_t hue;
        uint8_t sat;
        to_hs(f.r, f.g, f.b, &hue, &sat);
        int mx = MAX(f.r, MAX(f.g, f.b));
        int emitted = MAX(mx * f.cap / 100, 1); // as dim_strip.c scales it
        out->hue = hue;
        out->saturation = sat;
        out->value = (emitted * 100 + 127) / 255;
    }
    out->flags = flags;
}

void rgb_idle_look(uint8_t out[RGB_IDLE_LOOK_SIZE]) {
    struct zmk_led_hsb hsb = zmk_rgb_underglow_calc_hue(0);
    sys_put_le16(hsb.h, out);
    out[2] = hsb.s;
    out[3] = hsb.b;
    out[4] = zmk_rgb_underglow_calc_effect(0);
}

void rgb_idle_remote_look(const uint8_t look[RGB_IDLE_LOOK_SIZE]) {
    struct zmk_led_hsb want = {.h = sys_get_le16(look), .s = look[2], .b = look[3]};
    struct zmk_led_hsb hsb = zmk_rgb_underglow_calc_hue(0);
    if (want.h < 360 && want.s <= 100 && want.b <= 100 &&
        (hsb.h != want.h || hsb.s != want.s || hsb.b != want.b)) {
        zmk_rgb_underglow_set_hsb(want);
    }
    if (look[4] <= 3 && zmk_rgb_underglow_calc_effect(0) != look[4]) {
        zmk_rgb_underglow_select_effect(look[4]);
    }
}

static void save_cb(struct k_work *work) {
    settings_save_one("nvc_rgb/wanted", &wanted, sizeof(wanted));
#if CENTRAL
    settings_save_one("nvc_rgb/sync", &sync, sizeof(sync));
#endif
}
static K_WORK_DELAYABLE_DEFINE(save_work, save_cb);

static void save(void) { k_work_reschedule(&save_work, SAVE_DELAY); }

// The work items below all run on the system work queue, one at a time.

static int8_t told = -1; // the on/off last told to the PC (left) or the left (right), -1 none

// The on/off may have changed: the PC hears (left), the left reads it (right). Under a scene too,
// where it's the on/off hand-back will show (#542).
static void tell(bool on) {
    if (told == on) {
        return;
    }
    told = on;
#if HOST_CENTRAL
    host_link_rgb_changed();
#endif
#if RIGHT_RELAY
    art_relay_right_rang(ART_RELAY_RANG_RGB); // the left reads it and tells the PC (#478)
#endif
}

// The LEDs went lit or dark (#605): the PC hears (left), the left hears with the state (right).
// Dark to lit and back only: a breathe passing through black changes nothing here.
static int8_t told_lit = -1;

static void lit_cb(struct k_work *work) {
    bool lit = rgb_idle_lit();
    if (told_lit == lit) {
        return;
    }
    told_lit = lit;
#if HOST_CENTRAL
    host_link_rgb_changed();
#endif
#if RIGHT_RELAY
    art_relay_right_rang(ART_RELAY_RANG_RGB); // the ring carries the state (#605)
#endif
}
static K_WORK_DEFINE(lit_work, lit_cb);

static void output_changed(void) { k_work_submit(&lit_work); }

static void apply_cb(struct k_work *work) {
    bool target = following ? remote_on : wanted && active();
    // Under the PC's scene (SET_FRAME, SET_EFFECT) ZMK's underglow stays off and the scene draws
    // whatever the on/off (#542); this runs again on hand-back, which puts target back.
    if (!atomic_get(&host_drawn)) {
        bool on;
        if (zmk_rgb_underglow_get_state(&on) == 0 && on != target) {
            if (target) {
                zmk_rgb_underglow_on();
            } else {
                zmk_rgb_underglow_off();
            }
        }
    }
    tell(target);
    output_changed();
#if RELAY_CENTRAL
    // The right follows the left's on/off under a scene as well, so it's right on hand-back
    // (#542). The relay sends only what the right hasn't confirmed.
    art_relay_rgb_kick();
#endif
}
static K_WORK_DEFINE(apply_work, apply_cb);

#if RELAY_CENTRAL
// The look last handed to the relay: a key or an invoke that changed the colour or the effect
// sends it, so the right shows the left's (#624).
static uint8_t look_kicked[RGB_IDLE_LOOK_SIZE];
static bool look_kicked_valid;

static void kick_if_look_changed(void) {
    uint8_t look[RGB_IDLE_LOOK_SIZE];
    rgb_idle_look(look);
    if (!look_kicked_valid || memcmp(look, look_kicked, sizeof(look)) != 0) {
        memcpy(look_kicked, look, sizeof(look));
        look_kicked_valid = true;
        art_relay_rgb_kick();
    }
}
#endif

static void check_cb(struct k_work *work) {
    output_changed(); // an &ext_power key, or ZMK's own on/off under a scene
#if RELAY_CENTRAL
    kick_if_look_changed();
#endif
    bool on;
    // Under the PC's frames ZMK's underglow is off on purpose: that's not the user's choice.
    if (!following && !atomic_get(&host_drawn) && zmk_rgb_underglow_get_state(&on) == 0 &&
        on != wanted) {
        wanted = on; // toggled with a key
        save();
        tell(on);
#if RELAY_CENTRAL
        art_relay_rgb_kick();
#endif
    }
}
static K_WORK_DELAYABLE_DEFINE(check_work, check_cb);

void rgb_idle_check(void) { k_work_reschedule(&check_work, CHECK_AFTER); }

static int activity_listener(const zmk_event_t *eh) {
    k_work_submit(&apply_work);
    return ZMK_EV_EVENT_BUBBLE;
}

ZMK_LISTENER(rgb_idle, activity_listener);
ZMK_SUBSCRIPTION(rgb_idle, zmk_activity_state_changed);

// A key or encoder step may have run an rgb_ug binding: look once, shortly after.
static int binding_listener(const zmk_event_t *eh) {
    rgb_idle_check();
    return ZMK_EV_EVENT_BUBBLE;
}

ZMK_LISTENER(rgb_idle_bindings, binding_listener);
ZMK_SUBSCRIPTION(rgb_idle_bindings, zmk_position_state_changed);
#if ZMK_KEYMAP_HAS_SENSORS
ZMK_SUBSCRIPTION(rgb_idle_bindings, zmk_sensor_event);
#endif

bool rgb_idle_sync(void) { return sync; }

void rgb_idle_set_sync(bool on) {
    if (sync != on) {
        sync = on;
        save();
    }
#if RELAY_CENTRAL
    art_relay_rgb_kick();
#endif
}

void rgb_idle_remote(bool on, bool follow) {
    remote_on = on;
    following = follow;
    k_work_submit(&apply_work);
}

// Hand-back clears the flag on the system queue, where check_cb and apply_cb run, so a key's
// check can't see ZMK still off without the flag and save that as the user's choice.
// A take-over that comes before this runs wins: the scene keeps the lights.
static void host_back_cb(struct k_work *work) {
    if (atomic_get(&host_want)) {
        return;
    }
    atomic_set(&host_drawn, 0);
    frame_forget(false); // ZMK's frames from before the scene are gone
    apply_cb(work);
}
static K_WORK_DEFINE(host_back_work, host_back_cb);

void rgb_idle_host_control(bool on) {
    atomic_set(&host_want, on);
    if (on) {
        atomic_set(&host_drawn, 1); // at once: nothing may turn ZMK's underglow back on
        frame_forget(true);         // the last scene's frames are gone too
        output_changed();
        return;
    }
    k_work_submit(&host_back_work);
}

void rgb_idle_remote_lost(void) {
    following = false;
    k_work_submit(&apply_work);
}

static int rgb_settings_set(const char *name, size_t len, settings_read_cb read_cb, void *cb_arg) {
    if (settings_name_steq(name, "wanted", NULL) && len == sizeof(wanted)) {
        loaded = read_cb(cb_arg, &wanted, sizeof(wanted)) == sizeof(wanted);
    } else if (settings_name_steq(name, "sync", NULL) && len == sizeof(sync)) {
        read_cb(cb_arg, &sync, sizeof(sync));
    }
    return 0;
}

// After all settings are loaded (ZMK's own underglow state too).
static int rgb_settings_commit(void) {
    if (!loaded) {
        // First boot with this firmware. ZMK's saved state can't tell the user's "off" from its
        // idle auto-off's, and after any idle it says off, so start lit; a user who wants it off
        // turns it off again and that sticks.
        wanted = true;
        loaded = true;
        save();
    }
    k_work_submit(&apply_work);
    rgb_idle_check();
    k_work_schedule(&speed_ready_work, SPEED_AFTER_BOOT); // never during settings_load
    return 0;
}

SETTINGS_STATIC_HANDLER_DEFINE(nvc_rgb, "nvc_rgb", NULL, rgb_settings_set, rgb_settings_commit, NULL);
