/*
 * PC link over Raw HID: the host panel (0xD5) and right-hand art pushes (0xD6). Protocol in
 * host_link.h.
 * SPDX-License-Identifier: MIT
 */

#include <string.h>

#include <zephyr/device.h>
#include <zephyr/devicetree.h>
#include <zephyr/kernel.h>
#include <zephyr/spinlock.h>
#include <zephyr/sys/byteorder.h>

#include <zephyr/logging/log.h>
LOG_MODULE_DECLARE(zmk, CONFIG_ZMK_LOG_LEVEL);

#include <raw_hid/events.h>
#include <zmk/battery.h>
#include <zmk/behavior.h>
#include <zmk/ble.h>
#include <zmk/display.h>
#include <zmk/endpoints.h>
#include <zmk/event_manager.h>
#include <zmk/events/battery_state_changed.h>
#include <zmk/events/ble_active_profile_changed.h>
#include <zmk/events/endpoint_changed.h>
#include <zmk/events/usb_conn_state_changed.h>
#include <zmk/events/position_state_changed.h>
#include <zmk/keymap.h>
#include <zmk/usb.h>

#include "half_marker.h"
#include "host_link.h"
#include "screen_snap.h"
#include "art_relay.h"
#if IS_ENABLED(CONFIG_FOXGLOVE_ART_LOCAL)
#include "art_slots.h"
#include "art_store.h"
#endif
#include "rgb_idle.h"
#include "battery_dim.h"
#include "behavior_safety.h"
#include "colour_effects.h"
#if IS_ENABLED(CONFIG_FOXGLOVE_HOST_FRAMES)
#include "host_frames.h"
#endif
#if IS_ENABLED(CONFIG_FOXGLOVE_HOST_EFFECTS)
#include "host_effects.h"
#endif
#if IS_ENABLED(CONFIG_FOXGLOVE_LAMPARRAY)
#include "lamparray.h"
#endif
#if IS_ENABLED(CONFIG_FOXGLOVE_TEST_HOOKS)
#include "test_hooks.h"
#endif
#if IS_ENABLED(CONFIG_FOXGLOVE_PC_STATE)
#include "pc_state.h"
#endif
#include "panel_polarity.h"
#include "health.h"

#if IS_ENABLED(CONFIG_NICE_VIEW_RGB_IDLE)
#include <zmk/rgb_underglow.h>
#endif

// &rgb_ug commands from the PC whose right-half part goes over the relay (#266).
#define RGB_UG_RELAYED                                                                             \
    (IS_ENABLED(CONFIG_NICE_VIEW_RGB_IDLE) && IS_ENABLED(CONFIG_NICE_VIEW_ART_RELAY) &&            \
     DT_HAS_COMPAT_STATUS_OKAY(zmk_behavior_rgb_underglow))
#if RGB_UG_RELAYED
#include <dt-bindings/zmk/rgb.h>
#define RGB_UG_NAME DEVICE_DT_NAME(DT_INST(0, zmk_behavior_rgb_underglow))
#endif

enum { PANEL_HELLO = 0x01, PANEL_FILL, PANEL_ROWS, PANEL_PRESENT, PANEL_RELEASE, PANEL_SCREENS,
       PANEL_ROWS_AT };
enum {
    ART_BEGIN = 0x01,
    ART_DATA,
    ART_COMMIT,
    ART_SELECT,
    ART_STATUS,
    ART_SAVE,
    ART_SLOT,
    ART_ERASE,
    ART_STORE,   // 09 (#522)
    ART_ANIMATE, // 0A (#842): local art store only (HELLO v2 ART_PACE)
};
enum { SETTING_RGB_SYNC = 0x01, SETTING_INVOKE, SETTING_RGB_STATE, SETTING_CONN_STATE, SETTING_POSITION,
       SETTING_SCREEN_SNAP, SETTING_SCREEN_READ, SETTING_BOOTLOADER,
       SETTING_BUILD_INFO, SETTING_GUARD_TEST = 0x0B, // 0x0A: LAMP_TEST (#115 branch)
       SETTING_FRAME = 0x0C, SETTING_ART_KEYS = 0x0D,
       SETTING_BATTERY_DIM = 0x0E, SETTING_EFFECT = 0x0F, SETTING_LAMP_PARTS = 0x10,
       SETTING_PC_STATE = 0x11, SETTING_PANEL_POLARITY = 0x12, SETTING_HEALTH = 0x13,
       SETTING_KNOB = 0x14 };
enum {
    INVOKE_OK, INVOKE_UNKNOWN, INVOKE_BUSY, INVOKE_FAILED, INVOKE_UNSUPPORTED,
    INVOKE_BAD_LENGTH, INVOKE_REFUSED
};

#define REPORT_SIZE CONFIG_RAW_HID_REPORT_SIZE
#define ALIVE_TIMEOUT K_SECONDS(CONFIG_NICE_VIEW_HOST_PANEL_TIMEOUT_S)

// The knob behaviours this keymap has (#719; knob.c, built with CONFIG_FOXGLOVE_KNOB).
#if IS_ENABLED(CONFIG_FOXGLOVE_KNOB)
#define KNOB_PARTS                                                                                     ((DT_HAS_COMPAT_STATUS_OKAY(foxglove_behavior_knob) ? KNOB_HAS_TURN : 0) |                          (DT_HAS_COMPAT_STATUS_OKAY(foxglove_behavior_knob_press) ? KNOB_HAS_PRESS : 0))
#else
#define KNOB_PARTS 0
#endif

/*
 * Reports arrive on the USB thread. Rows go straight into the back buffer and PRESENT copies it
 * to the front one; the display thread only reads the front buffer (host_panel_get), so drawing
 * never waits on USB and USB never waits on drawing. The panel only takes the area at the first
 * PRESENT, so neither a half-written back buffer nor an empty front one is ever shown, and it keeps
 * the last presented frame when the PC goes quiet (CONFIG_NICE_VIEW_HOST_PANEL_TIMEOUT_S = 0).
 */
static struct k_spinlock lock;
static uint8_t back[HOST_PANEL_BYTES];
static uint8_t front[HOST_PANEL_BYTES];
static bool alive;

static atomic_t host_seen;

static uint8_t right_battery = 0xff; // unknown

bool host_panel_get(uint8_t buf[HOST_PANEL_BYTES]) {
    k_spinlock_key_t key = k_spin_lock(&lock);
    bool on = alive;
    if (on) {
        memcpy(buf, front, HOST_PANEL_BYTES);
    }
    k_spin_unlock(&lock, key);
    return on;
}

BUILD_ASSERT(HOST_PANEL_W % 8 == 0, "panel rows are whole bytes");

#if !defined(FOXGLOVE_HAS_WIDGETS) && !defined(FOXGLOVE_HAS_PANEL)
// A board without a panel drawer (status.c, foxglove_link/panel_lvgl.c) has nothing to redraw.
void host_panel_redraw(void) {}
#endif

static void redraw_cb(struct k_work *work) {
    health_inc(HEALTH_REDRAWS);
    host_panel_redraw();
}
static K_WORK_DEFINE(redraw_work, redraw_cb);

static void redraw(void) {
    if (zmk_display_is_initialized()) {
        k_work_submit_to_queue(zmk_display_work_q(), &redraw_work);
    }
}

static void set_alive(bool on) {
    k_spinlock_key_t key = k_spin_lock(&lock);
    bool changed = alive != on;
    alive = on;
    k_spin_unlock(&lock, key);
    if (changed) {
        redraw();
    }
}

static void timeout_cb(struct k_work *work) { set_alive(false); }
static K_WORK_DELAYABLE_DEFINE(timeout_work, timeout_cb);

// Replies go out from the system work queue, not from inside the USB request that asked.
K_MSGQ_DEFINE(reply_msgq, REPORT_SIZE, 4, 1);

static void reply_cb(struct k_work *work) {
    static uint8_t report[REPORT_SIZE];
    while (k_msgq_get(&reply_msgq, report, K_NO_WAIT) == 0) {
        raise_raw_hid_sent_event((struct raw_hid_sent_event){.data = report, .length = REPORT_SIZE});
    }
}
static K_WORK_DEFINE(reply_work, reply_cb);

static void reply(const uint8_t *data, int len) {
    uint8_t report[REPORT_SIZE] = {0};
    memcpy(report, data, MIN(len, REPORT_SIZE));
    if (k_msgq_put(&reply_msgq, report, K_NO_WAIT) == 0) {
        k_work_submit(&reply_work);
    } else {
        health_inc(HEALTH_HID_REPLY_DROPS); // the queue is full: this reply is lost (#705)
    }
}

void host_link_screen_reply(const uint8_t *data, int len) { reply(data, len); }

#ifndef FOXGLOVE_HAS_WIDGETS
// Without our widgets there's no screen readback (screen_snap.c): D7 06/07 answer 4, not built in.
void screen_snap_command(const uint8_t *data, int len) {
    uint8_t r[7] = {HOST_LINK_SETTINGS, 0x80 | data[1], 4, len >= 3 ? data[2] : 0};
    reply(r, sizeof(r));
}
#endif

void host_link_send(const uint8_t *data, int len) { reply(data, len); }

void host_link_event(const uint8_t *data, int len) {
    if (atomic_get(&host_seen)) {
        reply(data, len);
    }
}

void host_link_build_info_reply(uint8_t status, uint8_t half,
                                const struct foxglove_build_info *build) {
    uint8_t r[4 + sizeof(struct foxglove_build_info)] = {HOST_LINK_SETTINGS,
                                                         0x80 | SETTING_BUILD_INFO, status, half};
    if (build != NULL) {
        memcpy(&r[4], build, sizeof(*build));
    }
    reply(r, sizeof(r));
}

static uint8_t right_link(void) {
#if IS_ENABLED(CONFIG_NICE_VIEW_ART_RELAY)
    return art_relay_link();
#else
    return 0;
#endif
}

static void hello_reply(void) {
    int8_t discovery = 0;
    uint8_t tries = 0;
#if IS_ENABLED(CONFIG_NICE_VIEW_ART_RELAY)
    art_relay_discovery(&discovery, &tries);
#endif
    uint8_t r[11 + 10 + 4 + 2 + 4 + 1] = {HOST_LINK_PANEL, 0x80 | PANEL_HELLO, HOST_LINK_VERSION, 0,
                          0,               0,                  0,                 right_link(),
                          right_battery,   (uint8_t)discovery, tries};
    // Bytes 3-6: the host panel (the first panel in the board's table) for apps without SCREENS.
    for (int i = 0; i < foxglove_screen_count; i++) {
        const struct screen_descriptor *p = &foxglove_screens[i];
        if (p->kind == SCREEN_PANEL) {
            r[3] = MIN(p->width, UINT8_MAX);
            r[4] = MIN(p->height, UINT8_MAX);
            r[5] = MIN(p->x, UINT8_MAX);
            r[6] = MIN(p->y, UINT8_MAX);
            break;
        }
    }
#if IS_ENABLED(CONFIG_NICE_VIEW_ART_RELAY)
    art_relay_debug(&r[11]);
#endif
    uint32_t caps = HOST_LINK_CAP_PANEL | HOST_LINK_CAP_CONN_STATE | HOST_LINK_CAP_SCREEN;
#if IS_ENABLED(CONFIG_NICE_VIEW_ART_RELAY)
    caps |= HOST_LINK_CAP_ART_RELAY | HOST_LINK_CAP_ART_STATUS_EVENT | HOST_LINK_CAP_ART_SLOTS;
    caps |= HOST_LINK_CAP_ACTIVITY_RELAY; // activity_relay.c is built with the relay
    caps |= HOST_LINK_CAP_ART_STORE;      // D6 09 (#522); the right's status version says too
#elif IS_ENABLED(CONFIG_FOXGLOVE_ART_LOCAL)
    // Bit 1 is "art push" (#834): the target is in SCREENS, here on this half (D5 08's word 2 has
    // ART_LOCAL). Slots when the board has any; STORE with them.
    caps |= HOST_LINK_CAP_ART_RELAY;
    if (art_slots_count() > 0) {
        caps |= HOST_LINK_CAP_ART_SLOTS | HOST_LINK_CAP_ART_STORE;
    }
#endif
#if IS_ENABLED(CONFIG_NICE_VIEW_RGB_IDLE)
    caps |= HOST_LINK_CAP_SETTINGS_RGB | HOST_LINK_CAP_RGB_STATE_EVENT | HOST_LINK_CAP_RGB_SHOWN;
#endif
#if IS_ENABLED(CONFIG_FOXGLOVE_TEST_HOOKS)
    caps |= HOST_LINK_CAP_POSITION | HOST_LINK_CAP_BOOTLOADER;
#endif
#if IS_ENABLED(CONFIG_ZMK_BEHAVIOR_LOCAL_IDS)
    caps |= HOST_LINK_CAP_INVOKE_REFUSES; // INVOKE exists, with the refuse list
#endif
    caps |= HOST_LINK_CAP_CONN_STATE_EVENT | HOST_LINK_CAP_BUILD_INFO | HOST_LINK_CAP_SCREENS;
#if IS_ENABLED(CONFIG_ZMK_BATTERY_REPORTING)
    caps |= HOST_LINK_CAP_LEFT_BATTERY;
#endif
#if IS_ENABLED(CONFIG_NICE_VIEW_RGB_BATTERY_DIM)
    caps |= HOST_LINK_CAP_BATTERY_DIM;
#if IS_ENABLED(CONFIG_NICE_VIEW_ART_RELAY)
    caps |= HOST_LINK_CAP_RIGHT_REDRAWS;
#endif
#endif
#if IS_ENABLED(CONFIG_NICE_VIEW_ART_RELAY) && IS_ENABLED(CONFIG_NICE_VIEW_RGB_IDLE)
    caps |= HOST_LINK_CAP_RIGHT_EVENTS;
#endif
#if IS_ENABLED(CONFIG_FOXGLOVE_HOST_FRAMES)
    caps |= HOST_LINK_CAP_FRAMES | HOST_LINK_CAP_LAMP_PARTS;
#endif
#if IS_ENABLED(CONFIG_FOXGLOVE_HOST_EFFECTS)
    caps |= HOST_LINK_CAP_EFFECTS | HOST_LINK_CAP_EFFECT_LAMPS;
#endif
#if IS_ENABLED(CONFIG_NICE_VIEW_ART_RELAY) && DT_HAS_COMPAT_STATUS_OKAY(foxglove_behavior_art_step)
    caps |= HOST_LINK_CAP_ART_KEYS;
#endif
#if IS_ENABLED(CONFIG_FOXGLOVE_PC_STATE)
    caps |= HOST_LINK_CAP_PC_STATE;
#endif
#if IS_ENABLED(CONFIG_FOXGLOVE_PANEL_POLARITY)
    caps |= HOST_LINK_CAP_PANEL_POLARITY;
#endif
#if IS_ENABLED(CONFIG_FOXGLOVE_HEALTH)
    caps |= HOST_LINK_CAP_HEALTH;
#endif
#if IS_ENABLED(CONFIG_FOXGLOVE_COLOUR_SWIRL) && IS_ENABLED(CONFIG_NICE_VIEW_RGB_IDLE)
    caps |= HOST_LINK_CAP_COLOUR_EFFECTS;
#endif
    if (KNOB_PARTS != 0) {
        caps |= HOST_LINK_CAP_KNOB; // D7 14 (#719): the keymap has &knob or &knob_press
    }
    if (CONFIG_NICE_VIEW_HOST_PANEL_TIMEOUT_S == 0) {
        caps |= HOST_LINK_CAP_PANEL_HOLD; // no keep-alive needed (#279)
    }
    sys_put_le32(caps, &r[21]);
    sys_put_le16(CONFIG_NICE_VIEW_HOST_PANEL_TIMEOUT_S, &r[25]);
    struct foxglove_build_info build;
    foxglove_build_info(&build);
    r[27] = build.major;
    r[28] = build.minor;
    r[29] = build.patch;
    r[30] = build.profile;
#if IS_ENABLED(CONFIG_NICE_VIEW_ART_RELAY)
    r[31] = art_relay_guard_drops();
#endif
    reply(r, sizeof(r));
}

/* D5 08 CAPS (HELLO v2): the capability words past HELLO's full word 1, from word 2 on. Bit n of
 * word 2 is capability 32 + n (HOST_LINK_CAPS2_*). */

static uint32_t caps_word2(void) {
    uint32_t caps = 0;
#if IS_ENABLED(CONFIG_FOXGLOVE_ART_LOCAL)
    caps |= HOST_LINK_CAPS2_ART_LOCAL; // D6 goes to the art store on this half (#834)
    caps |= HOST_LINK_CAPS2_ART_PACE;  // its art follows the target's pace; D6 0A (#842)
#endif
    return caps;
}

static void caps_reply(void) {
    uint8_t r[3 + 4 * HOST_LINK_CAPS_V2_WORDS] = {HOST_LINK_PANEL, 0x80 | HOST_LINK_PANEL_CAPS,
                                                  HOST_LINK_CAPS_V2_WORDS};
    BUILD_ASSERT(sizeof(r) <= 32, "D5 88 fits one report");
    sys_put_le32(caps_word2(), &r[3]);
    reply(r, sizeof(r));
}

/* D5 06 SCREENS (#160): the board's table (foxglove_screens[], e.g. screens_corne.c) as the PC
 * sees it. */

static void screens_reply(uint8_t index) {
    uint8_t r[5 + sizeof(struct screen_descriptor)] = {HOST_LINK_PANEL, 0x80 | PANEL_SCREENS, 5,
                                                       foxglove_screen_count, index};
    if (index < foxglove_screen_count) {
        struct screen_descriptor s = foxglove_screens[index];
        // v2 only for an art target that says something (#832): all 0 stays v1, as before.
        bool art_v2 = s.kind == SCREEN_ART &&
                      (s.stride || s.art_slots || s.art_refresh || s.art_min_frame_100ms);
        s.version = art_v2 ? SCREEN_DESCRIPTOR_VERSION_ART : SCREEN_DESCRIPTOR_VERSION;
        r[2] = 0;
        memcpy(&r[5], &s, sizeof(s));
    }
    reply(r, sizeof(r));
}

static void panel_command(const uint8_t *d) {
    if (CONFIG_NICE_VIEW_HOST_PANEL_TIMEOUT_S > 0 && d[1] != PANEL_RELEASE) {
        k_work_reschedule(&timeout_work, ALIVE_TIMEOUT);
    }

    switch (d[1]) {
    case PANEL_HELLO: {
        hello_reply();
        break;
    }
    case HOST_LINK_PANEL_CAPS:
        caps_reply();
        break;
    case PANEL_FILL: {
        k_spinlock_key_t key = k_spin_lock(&lock);
        memset(back, d[2] ? 0xff : 0x00, sizeof(back));
        k_spin_unlock(&lock, key);
        break;
    }
    case PANEL_ROWS: {
        int y = d[2], n = MIN(d[3], 3);
        if (y + n > HOST_PANEL_H || 4 + n * HOST_PANEL_STRIDE > REPORT_SIZE) {
            break;
        }
        k_spinlock_key_t key = k_spin_lock(&lock);
        memcpy(&back[y * HOST_PANEL_STRIDE], &d[4], n * HOST_PANEL_STRIDE);
        k_spin_unlock(&lock, key);
        break;
    }
    case PANEL_ROWS_AT: {
        // Byte-addressed (#289): rows wider than a report, panels taller than 255 rows.
        uint16_t y = sys_get_le16(&d[2]), offset = sys_get_le16(&d[4]);
        uint8_t n = d[6];
        size_t at = (size_t)y * HOST_PANEL_STRIDE + offset;
        if (y >= HOST_PANEL_H || offset >= HOST_PANEL_STRIDE || n > REPORT_SIZE - 7 ||
            at + n > HOST_PANEL_BYTES) {
            break;
        }
        k_spinlock_key_t key = k_spin_lock(&lock);
        memcpy(&back[at], &d[7], n);
        k_spin_unlock(&lock, key);
        break;
    }
    case PANEL_PRESENT: {
        k_spinlock_key_t key = k_spin_lock(&lock);
        memcpy(front, back, sizeof(front));
        k_spin_unlock(&lock, key);
        set_alive(true);
        redraw();
        break;
    }
    case PANEL_SCREENS:
        screens_reply(d[2]);
        break;
    case PANEL_RELEASE:
        k_work_cancel_delayable(&timeout_work);
        set_alive(false);
        break;
    }
}

/* D6: the art target's ops. A split board relays them to the far half (art_relay_central.c); a
 * board whose art target is on this half (CONFIG_FOXGLOVE_ART_LOCAL, #834) hands them straight to
 * its art store (art_local.c). Both answer with the same report. */
#if IS_ENABLED(CONFIG_NICE_VIEW_ART_RELAY)
#define ART_OPS(op) art_relay_##op
#elif IS_ENABLED(CONFIG_FOXGLOVE_ART_LOCAL)
#define ART_OPS(op) art_local_##op
#endif

static void art_command(const uint8_t *d) {
    uint8_t r[2 + ART_RELAY_REPORT_SIZE] = {HOST_LINK_ART, 0x80 | d[1]};
#if defined(ART_OPS)
    int err = 0;
    switch (d[1]) {
    case ART_BEGIN:
        err = ART_OPS(begin)(sys_get_le16(&d[2]), sys_get_le32(&d[4]));
        break;
    case ART_DATA:
        err = ART_OPS(data)(sys_get_le16(&d[2]), &d[5], MIN(d[4], REPORT_SIZE - 5));
        if (err == 0) {
            return;
        }
        break;
    case ART_COMMIT:
        err = ART_OPS(commit)();
        break;
    case ART_SELECT:
        err = ART_OPS(select)((int8_t)d[2]);
        break;
    case ART_STATUS:
        break;
    case ART_SAVE:
        err = ART_OPS(save)(d[2], &d[3]);
        break;
    case ART_ERASE:
        err = ART_OPS(erase)(d[2]);
        break;
    case ART_STORE:
        err = ART_OPS(store)(d[2], &d[3]);
        break;
#if IS_ENABLED(CONFIG_FOXGLOVE_ART_LOCAL)
    case ART_ANIMATE:
        // The relay has no op for it yet: a relaying board doesn't answer, as before (#842).
        err = art_local_animate(d[2]);
        break;
#endif
    case ART_SLOT:
        if (ART_OPS(slot)(d[2]) == 0) {
            return; // D6 87 comes from the relay (or, local, has gone already)
        }
        r[2] = 1; // busy
        r[3] = d[2];
        reply(r, sizeof(r));
        return;
    default:
        return;
    }
    ARG_UNUSED(err); // the error is in the report
    ART_OPS(report)(&r[2]);
#else
    r[2] = ART_LEFT_FAILED;
    r[3] = ART_ERR_NO_LINK;
#endif
    reply(r, sizeof(r));
}

void host_link_art_status(void) {
    if (atomic_get(&host_seen)) {
        const uint8_t d[] = {HOST_LINK_ART, ART_STATUS};
        art_command(d);
    }
}

/* INVOKE: runs on the system work queue, like the keymap does for key presses */

#define INVOKE_MAX_HOLD_MS 10000
// No key sits here; behaviors that track positions (hold-tap, sticky keys) see a key of its own.
#define INVOKE_POSITION UINT32_MAX

static struct zmk_behavior_binding invoke_binding;
static struct zmk_behavior_binding_event invoke_event;
static uint16_t invoke_id; // of the invoke that holds invoke_busy
static uint16_t invoke_hold_ms;
static atomic_t invoke_busy;

static void invoke_reply(uint8_t status, uint16_t id) {
    uint8_t r[5] = {HOST_LINK_SETTINGS, 0x80 | SETTING_INVOKE, status};
    sys_put_le16(id, &r[3]);
    reply(r, sizeof(r));
}

static void invoke_release_cb(struct k_work *work) {
    invoke_event.timestamp = k_uptime_get();
    zmk_behavior_invoke_binding(&invoke_binding, invoke_event, false);
    atomic_set(&invoke_busy, 0);
}
static K_WORK_DELAYABLE_DEFINE(invoke_release_work, invoke_release_cb);

static void invoke_press_cb(struct k_work *work) {
    invoke_event = (struct zmk_behavior_binding_event){
        .layer = zmk_keymap_highest_layer_active(),
        .position = INVOKE_POSITION,
        .timestamp = k_uptime_get(),
#if IS_ENABLED(CONFIG_ZMK_SPLIT)
        .source = ZMK_POSITION_STATE_CHANGE_SOURCE_LOCAL,
#endif
    };
    int ret = zmk_behavior_invoke_binding(&invoke_binding, invoke_event, true);
    invoke_reply(ret < 0 ? INVOKE_FAILED : INVOKE_OK, invoke_id);
#if RGB_UG_RELAYED
    if (strcmp(invoke_binding.behavior_dev, RGB_UG_NAME) == 0) {
        art_relay_rgb_verify(); // went to the right through ZMK: read it back, then tell the PC
        rgb_idle_check();       // and an on/off toggle should be saved as the user's
    }
#endif
    k_work_schedule(&invoke_release_work, K_MSEC(invoke_hold_ms));
}
static K_WORK_DEFINE(invoke_press_work, invoke_press_cb);

// d starts at the local id.
static uint8_t invoke(const uint8_t *d, int len) {
#if IS_ENABLED(CONFIG_ZMK_BEHAVIOR_LOCAL_IDS)
    if (len < 10) {
        return INVOKE_BAD_LENGTH;
    }
    uint16_t id = sys_get_le16(&d[0]);
    const char *name = zmk_behavior_find_behavior_name_from_local_id(id);
    if (name == NULL) {
        return INVOKE_UNKNOWN;
    }
    if (foxglove_behavior_refused(name)) {
        return INVOKE_REFUSED;
    }
#if RGB_UG_RELAYED
    // The PC's colour (#266): set here at once and sent to the right as one acknowledged write,
    // not through ZMK's split queue, which delays and drops under streaming. Not busy-gated: the
    // newest colour wins.
    if (strcmp(name, RGB_UG_NAME) == 0 &&
        sys_get_le32(&d[2]) == RGB_COLOR_HSB_CMD) {
        uint32_t hsb = sys_get_le32(&d[6]);
        struct zmk_led_hsb color = {.h = hsb >> 16, .s = (hsb >> 8) & 0xff, .b = hsb & 0xff};
        if (color.h > 359 || color.s > 100 || color.b > 100) {
            return INVOKE_FAILED;
        }
        if (art_relay_rgb_hsb(color.h, color.s, color.b) == 0) {
            zmk_rgb_underglow_set_hsb(color);
            health_inc(HEALTH_LIGHT_APPLIES);
            invoke_reply(INVOKE_OK, id); // the caller only replies to errors
            return INVOKE_OK;
        }
    }
#endif
    if (!atomic_cas(&invoke_busy, 0, 1)) {
        return INVOKE_BUSY;
    }
    invoke_id = id;
    invoke_binding = (struct zmk_behavior_binding){
#if IS_ENABLED(CONFIG_ZMK_BEHAVIOR_LOCAL_IDS_IN_BINDINGS)
        .local_id = id,
#endif
        .behavior_dev = name,
        .param1 = sys_get_le32(&d[2]),
        .param2 = sys_get_le32(&d[6]),
    };
    invoke_hold_ms = MIN(len >= 12 ? sys_get_le16(&d[10]) : 0, INVOKE_MAX_HOLD_MS);
    k_work_submit(&invoke_press_work); // replies once pressed
    return INVOKE_OK;
#else
    return INVOKE_UNSUPPORTED;
#endif
}

void host_link_bootloader_reply(uint8_t status, uint8_t half) {
    const uint8_t r[] = {HOST_LINK_SETTINGS, 0x80 | SETTING_BOOTLOADER, status, half};
    reply(r, sizeof(r));
}

void host_link_position_reply(uint8_t status, uint8_t pos) {
    const uint8_t r[] = {HOST_LINK_SETTINGS, 0x80 | SETTING_POSITION, status, pos};
    reply(r, sizeof(r));
}

#define CONN_STATE_SIZE 9

static void conn_state(uint8_t r[CONN_STATE_SIZE]) {
    memset(r, 0, CONN_STATE_SIZE);
    r[0] = HOST_LINK_SETTINGS;
    r[1] = 0x80 | SETTING_CONN_STATE;
#if IS_ENABLED(CONFIG_ZMK_BLE)
    r[2] = zmk_ble_active_profile_index();
    r[3] = zmk_ble_active_profile_is_connected() ? 1 : 0;
    r[4] = zmk_ble_active_profile_is_open() ? 0 : 1;
    r[7] = ZMK_BLE_PROFILE_COUNT;
#endif
    r[5] = zmk_endpoints_selected().transport == ZMK_TRANSPORT_BLE ? 1 : 0;
#if IS_ENABLED(CONFIG_ZMK_USB)
    r[6] = zmk_usb_is_hid_ready() ? 1 : 0;
#endif
#if IS_ENABLED(CONFIG_ZMK_BATTERY_REPORTING)
    // The left's own level (#373): ZMK's last reading, no I/O here.
    r[8] = zmk_battery_state_of_charge();
#endif
}

// Unsolicited CONN_STATE: after host traffic, a change of the active BLE profile, its connection
// or bond, the endpoint, USB or the left's battery level sends the same reply on its own, once
// things have settled; an unchanged payload isn't sent again.
#define CONN_STATE_SETTLE K_MSEC(50)

static uint8_t conn_state_sent[CONN_STATE_SIZE];

static void conn_state_event_cb(struct k_work *work) {
    uint8_t r[CONN_STATE_SIZE];
    conn_state(r);
    if (memcmp(r, conn_state_sent, sizeof(r)) != 0) {
        memcpy(conn_state_sent, r, sizeof(r));
        reply(r, sizeof(r));
    }
}
static K_WORK_DELAYABLE_DEFINE(conn_state_event_work, conn_state_event_cb);

/* D7 0D ART_KEYS (#60): the app takes &art_next / &art_prev while subscribed */

static atomic_t art_keys;

int host_link_art_key(int8_t dir) {
    if (!atomic_get(&art_keys)) {
        return -ENOTCONN;
    }
    uint8_t r[4] = {HOST_LINK_SETTINGS, 0x80 | SETTING_ART_KEYS, 1, (uint8_t)dir};
    reply(r, sizeof(r));
    return 0;
}

/* D7 14 KNOB (#719, HELLO bit 31): the PC holds &knob / &knob_press (knob.c) */

// The state, one value so a target is never seen without its hold: bit 0 held, bit 1 an app.
#define KNOB_HELD BIT(0)
#define KNOB_ON_APP BIT(1)
static atomic_t knob_state;
static atomic_t knob_seq;

bool host_link_knob_held(void) { return (atomic_get(&knob_state) & KNOB_HELD) != 0; }

enum knob_target host_link_knob_target(void) {
    return (atomic_get(&knob_state) & KNOB_ON_APP) ? KNOB_APP : KNOB_MASTER;
}

static void knob_send(enum knob_event kind, int8_t steps, atomic_val_t state) {
    uint8_t r[7] = {HOST_LINK_SETTINGS, 0x80 | SETTING_KNOB, 1, kind, (uint8_t)steps,
                    (state & KNOB_ON_APP) ? KNOB_APP : KNOB_MASTER,
                    (uint8_t)atomic_inc(&knob_seq)};
    reply(r, sizeof(r));
    health_inc(HEALTH_KNOB_EVENTS);
    health_set(HEALTH_KNOB_LAST, kind);
}

int host_link_knob_event(enum knob_event kind, int8_t steps) {
    atomic_val_t state = atomic_get(&knob_state);
    if (!(state & KNOB_HELD)) {
        return -ENOTCONN;
    }
    knob_send(kind, steps, state);
    return 0;
}

static void knob_set(atomic_val_t state) {
    atomic_set(&knob_state, state);
    health_set(HEALTH_KNOB_STATE, (uint8_t)state);
}

// The PC may be gone (a USB disconnect, an endpoint change): the knob is its fallback keys again
// until the PC holds it anew. `tell`: say so (D7 94 01 05), for a PC that is still there.
static void knob_drop(bool tell) {
    atomic_val_t was = atomic_set(&knob_state, 0);
    if (!(was & KNOB_HELD)) {
        return;
    }
    health_set(HEALTH_KNOB_STATE, 0);
    health_inc(HEALTH_KNOB_DROPS);
    if (tell) {
        knob_send(KNOB_HANDED_BACK, 0, was);
    }
}

static void knob_command(const uint8_t *d, int len) {
    uint8_t status = 0;
    if (KNOB_PARTS == 0) {
        status = 4; // no &knob / &knob_press in this keymap (or CONFIG_FOXGLOVE_KNOB off)
    } else if (len >= 3 && d[2] != 0xff) {
        uint8_t hold = d[2], target = len >= 4 ? d[3] : KNOB_MASTER;
        if (hold > 1 || target > KNOB_APP) {
            status = 5;
        } else {
            knob_set(hold ? (KNOB_HELD | (target == KNOB_APP ? KNOB_ON_APP : 0)) : 0);
        }
    }
    atomic_val_t state = atomic_get(&knob_state);
    uint8_t r[7] = {HOST_LINK_SETTINGS, 0x80 | SETTING_KNOB, 0, status,
                    (state & KNOB_HELD) ? 1 : 0, (state & KNOB_ON_APP) ? KNOB_APP : KNOB_MASTER,
                    KNOB_PARTS};
    reply(r, sizeof(r));
}

static int conn_state_listener(const zmk_event_t *eh) {
    // The PC side may be gone: the keys step the art on the board again until it subscribes
    // anew, and the knob is its fallback keys until it is held anew (#719).
#if IS_ENABLED(CONFIG_ZMK_USB)
    if (!zmk_usb_is_hid_ready()) {
        atomic_set(&art_keys, 0);
        knob_drop(false); // no USB to say it on
    }
#endif
    if (as_zmk_endpoint_changed(eh) != NULL) {
        atomic_set(&art_keys, 0);
        knob_drop(true);
    }
    if (atomic_get(&host_seen)) {
        k_work_reschedule(&conn_state_event_work, CONN_STATE_SETTLE);
    }
    return ZMK_EV_EVENT_BUBBLE;
}

ZMK_LISTENER(host_link_conn, conn_state_listener);
ZMK_SUBSCRIPTION(host_link_conn, zmk_endpoint_changed);
#if IS_ENABLED(CONFIG_ZMK_BLE)
ZMK_SUBSCRIPTION(host_link_conn, zmk_ble_active_profile_changed);
#endif
#if IS_ENABLED(CONFIG_ZMK_USB)
ZMK_SUBSCRIPTION(host_link_conn, zmk_usb_conn_state_changed);
#endif
#if IS_ENABLED(CONFIG_ZMK_BATTERY_REPORTING)
ZMK_SUBSCRIPTION(host_link_conn, zmk_battery_state_changed);
#endif

#define RGB_SHOWN_AT (3 + 2 * sizeof(struct rgb_idle_wire))
#define RGB_COLOUR_EFFECTS_AT (RGB_SHOWN_AT + 2 * sizeof(struct rgb_shown_wire))
#define RGB_STATE_SIZE (RGB_COLOUR_EFFECTS_AT + 1)
BUILD_ASSERT(RGB_SHOWN_AT == 17 && RGB_STATE_SIZE == 30, "RGB_STATE's layout is the PC's");

// The RGB_STATE reply; asks the relay for a fresher right state on the way.
static void rgb_state(uint8_t r[RGB_STATE_SIZE]) {
    struct rgb_idle_wire left = {0}, right = {0};
    struct rgb_shown_wire left_shown = {0}, right_shown = {0};
    uint8_t age = 0xff;
#if IS_ENABLED(CONFIG_NICE_VIEW_RGB_IDLE)
    rgb_idle_state(&left);
    rgb_idle_shown(&left_shown);
#endif
#if IS_ENABLED(CONFIG_NICE_VIEW_ART_RELAY)
    age = art_relay_right_rgb(&right);
    if (age == 0xff || !art_relay_right_shown(&right_shown)) {
        memset(&right_shown, 0, sizeof(right_shown));
    }
#endif
    r[0] = HOST_LINK_SETTINGS;
    r[1] = 0x80 | SETTING_RGB_STATE;
    memcpy(&r[2], &left, sizeof(left));
    r[2 + sizeof(left)] = age;
    memcpy(&r[3 + sizeof(left)], &right, sizeof(right));
    memcpy(&r[RGB_SHOWN_AT], &left_shown, sizeof(left_shown));
    memcpy(&r[RGB_SHOWN_AT + sizeof(left_shown)], &right_shown, sizeof(right_shown));
    // The effects the left draws in the chosen colour (#626); 0, as before it, says nothing.
    r[RGB_COLOUR_EFFECTS_AT] = 0;
#if IS_ENABLED(CONFIG_FOXGLOVE_COLOUR_SWIRL) && IS_ENABLED(CONFIG_NICE_VIEW_RGB_IDLE)
    r[RGB_COLOUR_EFFECTS_AT] = colour_effects_mask();
#endif
}

// Unsolicited RGB_STATE: after host traffic, the left's underglow switching on or off (idle,
// wake, a key, sync) sends the same reply on its own. The right's half of it is the relay's
// freshest read, taken RGB_STATE_SETTLE after the change so the right has followed by then.
#define RGB_STATE_SETTLE K_MSEC(600)

// left on | right on << 1 | right known << 2 | left lit << 3 | right lit << 4, last sent
static uint8_t rgb_state_sent = 0xff;

static void rgb_state_event_cb(struct k_work *work) {
    uint8_t r[RGB_STATE_SIZE];
    rgb_state(r);
    uint8_t age = r[2 + sizeof(struct rgb_idle_wire)];
    bool left_lit = r[RGB_SHOWN_AT] & RGB_SHOWN_LIT;
    bool right_lit = r[RGB_SHOWN_AT + sizeof(struct rgb_shown_wire)] & RGB_SHOWN_LIT;
    uint8_t now = r[2] | r[3 + sizeof(struct rgb_idle_wire)] << 1 | (age != 0xff) << 2 |
                  left_lit << 3 | right_lit << 4;
    if (now != rgb_state_sent) {
        rgb_state_sent = now;
        reply(r, sizeof(r));
    }
}
static K_WORK_DELAYABLE_DEFINE(rgb_state_event_work, rgb_state_event_cb);

// The right has followed (#266): the PC hears at once, with the right's colour as acknowledged.
static void rgb_followed_cb(struct k_work *work) {
    uint8_t r[RGB_STATE_SIZE];
    rgb_state(r);
    reply(r, sizeof(r));
}
static K_WORK_DEFINE(rgb_followed_work, rgb_followed_cb);

void host_link_rgb_right_followed(void) {
    if (atomic_get(&host_seen)) {
        k_work_submit(&rgb_followed_work);
    }
}

void host_link_rgb_right_changed(void) {
    if (atomic_get(&host_seen)) {
        k_work_reschedule(&rgb_state_event_work, K_NO_WAIT); // deduped against the last one sent
    }
}

void host_link_rgb_changed(void) {
    if (atomic_get(&host_seen)) {
#if IS_ENABLED(CONFIG_NICE_VIEW_ART_RELAY)
        struct rgb_idle_wire right;
        art_relay_right_rgb(&right); // starts the status read the event will use
#endif
        k_work_reschedule(&rgb_state_event_work, RGB_STATE_SETTLE);
    }
}

/* D7 0E BATTERY_DIM: the underglow's levels by power state, per half (battery_dim.c) */

#define BATTERY_DIM_SIZE 21
#define BATTERY_DIM_CMP 16 // the unsolicited reply goes by these bytes (not the redraw count)

// The D7 8E reply. refresh: also ask the right for a fresher state (a PC request); the unsolicited
// one is the answer to such a read, so it doesn't ask again.
static void battery_dim_fill(uint8_t r[BATTERY_DIM_SIZE], uint8_t status, bool refresh) {
    memset(r, 0, BATTERY_DIM_SIZE);
    r[0] = HOST_LINK_SETTINGS;
    r[1] = 0x80 | SETTING_BATTERY_DIM;
    r[2] = status;
#if IS_ENABLED(CONFIG_NICE_VIEW_RGB_BATTERY_DIM)
    struct battery_dim_wire left, right = {0};
    uint8_t age = 0xff;
    battery_dim_state(&left);
#if IS_ENABLED(CONFIG_NICE_VIEW_ART_RELAY)
    age = art_relay_right_dim(&right, refresh);
#endif
    memcpy(&r[3], &left.config, sizeof(left.config));
    r[8] = left.flags;
    r[9] = left.level;
    r[10] = left.soc;
    r[11] = age;
    if (age != 0xff) {
        r[12] = right.flags;
        r[13] = right.level;
        r[14] = right.soc;
        r[15] = memcmp(&right.config, &left.config, sizeof(left.config)) == 0;
    }
#if IS_ENABLED(CONFIG_NICE_VIEW_ART_RELAY)
    uint32_t redraws;
    r[16] = art_relay_right_redraws(&redraws); // #618, HELLO bit 25
    sys_put_le32(redraws, &r[17]);
#endif
#endif
}

#if IS_ENABLED(CONFIG_NICE_VIEW_RGB_BATTERY_DIM)
// Unsolicited D7 8E: the state changed on either half; coalesced, and only sent if the payload
// (apart from the right's age) differs from the last one sent this way.
#define BATTERY_DIM_SETTLE K_MSEC(50)

static uint8_t battery_dim_sent[BATTERY_DIM_SIZE];

static void battery_dim_event_cb(struct k_work *work) {
    uint8_t r[BATTERY_DIM_SIZE], cmp[BATTERY_DIM_SIZE];
    battery_dim_fill(r, 0, false);
    memcpy(cmp, r, sizeof(cmp));
    cmp[11] = cmp[11] == 0xff ? 0xff : 0;
    if (memcmp(cmp, battery_dim_sent, BATTERY_DIM_CMP) != 0) {
        memcpy(battery_dim_sent, cmp, sizeof(cmp));
        reply(r, sizeof(r));
    }
}
static K_WORK_DELAYABLE_DEFINE(battery_dim_event_work, battery_dim_event_cb);
#endif

void host_link_battery_dim_changed(void) {
#if IS_ENABLED(CONFIG_NICE_VIEW_RGB_BATTERY_DIM)
    if (atomic_get(&host_seen)) {
        k_work_reschedule(&battery_dim_event_work, BATTERY_DIM_SETTLE);
    }
#endif
}

/* D7 11 PC_STATE (#609, #604): the PC's state, its lease's asks (pc_state.c) */

#define PC_STATE_SIZE (2 + 7)

static void pc_state_fill(uint8_t r[PC_STATE_SIZE], const uint8_t *d, int len) {
    r[0] = HOST_LINK_SETTINGS;
    r[1] = 0x80 | SETTING_PC_STATE;
    r[2] = 4; // not built in
#if IS_ENABLED(CONFIG_FOXGLOVE_PC_STATE)
    BUILD_ASSERT(sizeof(struct pc_state_wire) == PC_STATE_SIZE - 2, "the D7 91 reply");
    struct pc_state_wire w;
    if (d == NULL) {
        pc_state_report(&w);
        w.status = 0;
        w.event = 1;
    } else {
        // A report cut short is a read; d is padded to the report size.
        uint8_t req = len >= 3 ? d[2] : PC_STATE_REQ_READ;
        pc_state_host(req, d[3], sys_get_le16(&d[4]), &w);
    }
    memcpy(&r[2], &w, sizeof(w));
#else
    ARG_UNUSED(d);
    ARG_UNUSED(len);
#endif
}

void host_link_pc_state_event(void) {
    if (atomic_get(&host_seen)) {
        uint8_t r[PC_STATE_SIZE];
        pc_state_fill(r, NULL, 0);
        reply(r, sizeof(r));
    }
}

/* D7 13 HEALTH (#705, HELLO bit 28): each half's self-report block (health.h, docs/health.md), in
 * HEALTH_CHUNKS chunks. Chunk 0 takes a new copy (the left's at once; the right's over the relay,
 * replied when it is in) and the other chunks come from that copy, so one read is one moment. */

#define HEALTH_REPLY_SIZE (8 + 24)

#if IS_ENABLED(CONFIG_FOXGLOVE_HEALTH)
BUILD_ASSERT(HEALTH_REPLY_SIZE == 8 + HEALTH_CHUNK, "D7 93 bytes 8-31");
static uint8_t health_copy[2][HEALTH_SIZE];
static bool health_have[2];
static uint8_t health_seq[2];
static struct k_spinlock health_lock;
#endif

// D7 93: status, half, copy number, chunk, chunks, block length, then the chunk's bytes.
static void health_reply(uint8_t status, uint8_t half, uint8_t chunk) {
    uint8_t r[HEALTH_REPLY_SIZE] = {HOST_LINK_SETTINGS, 0x80 | SETTING_HEALTH, status, half};
    r[5] = chunk;
#if IS_ENABLED(CONFIG_FOXGLOVE_HEALTH)
    r[6] = HEALTH_CHUNKS;
    r[7] = HEALTH_SIZE;
    if (status == 0 && half <= 1 && chunk < HEALTH_CHUNKS) {
        k_spinlock_key_t key = k_spin_lock(&health_lock);
        r[4] = health_seq[half];
        memcpy(&r[8], &health_copy[half][chunk * HEALTH_CHUNK],
               MIN(HEALTH_CHUNK, HEALTH_SIZE - chunk * HEALTH_CHUNK));
        k_spin_unlock(&health_lock, key);
    }
#endif
    reply(r, sizeof(r));
}

void host_link_health_right(uint8_t status) {
#if IS_ENABLED(CONFIG_FOXGLOVE_HEALTH) && IS_ENABLED(CONFIG_NICE_VIEW_ART_RELAY)
    if (status == 0) {
        k_spinlock_key_t key = k_spin_lock(&health_lock);
        if (art_relay_right_health(health_copy[1], HEALTH_SIZE)) {
            health_have[1] = true;
            health_seq[1]++;
        } else {
            status = 3;
        }
        k_spin_unlock(&health_lock, key);
    }
    health_reply(status, 1, 0);
#else
    ARG_UNUSED(status);
#endif
}

// Status: 0 ok, 1 busy (a read of the right runs), 2 no relay link, 3 the right didn't answer,
// 4 not built in (or the right's firmware has no block), 5 bad request, 6 no copy yet (chunk 0
// first).
static void health_command(const uint8_t *d) {
    uint8_t half = d[2], chunk = d[3]; // d is padded: a report cut short reads half 0, chunk 0
#if IS_ENABLED(CONFIG_FOXGLOVE_HEALTH)
    if (half > 1 || chunk >= HEALTH_CHUNKS) {
        health_reply(5, half, chunk);
        return;
    }
    if (chunk == 0 && half == 0) {
        struct health_wire h;
        health_fill(&h);
        k_spinlock_key_t key = k_spin_lock(&health_lock);
        memcpy(health_copy[0], &h, sizeof(h));
        health_have[0] = true;
        health_seq[0]++;
        k_spin_unlock(&health_lock, key);
        health_reply(0, 0, 0);
        return;
    }
    if (chunk == 0) {
#if IS_ENABLED(CONFIG_NICE_VIEW_ART_RELAY)
        if (art_relay_health_read() != 0) {
            health_reply(1, 1, 0); // one is on its way; its reply comes
        }
#else
        health_reply(2, 1, 0);
#endif
        return;
    }
    health_reply(health_have[half] ? 0 : 6, half, chunk);
#else
    health_reply(4, half, chunk);
#endif
}

static void settings_command(const uint8_t *d, int len) {
    if (d[1] == SETTING_SCREEN_SNAP || d[1] == SETTING_SCREEN_READ) {
        screen_snap_command(d, len);
        return;
    }
    switch (d[1]) {
    case SETTING_FRAME: {
        uint8_t r[7] = {HOST_LINK_SETTINGS, 0x80 | SETTING_FRAME, 4}; // not built in
#if IS_ENABLED(CONFIG_FOXGLOVE_HOST_FRAMES)
        BUILD_ASSERT(sizeof(r) == HOST_FRAMES_REPLY_SIZE, "the D7 8C reply");
        if (!host_frames_command(d, len, r)) {
            break;
        }
        if (r[2] == 0 && (d[2] & 1)) {
            health_inc(HEALTH_LIGHT_APPLIES); // a frame shown (SHOW)
        }
#endif
        reply(r, sizeof(r));
        break;
    }
    case SETTING_LAMP_PARTS: {
        // Parts at bytes 4-11, the LampArray's state at 12-23 (host_link.h).
        uint8_t r[24] = {HOST_LINK_SETTINGS, 0x80 | SETTING_LAMP_PARTS, 4}; // not built in
#if IS_ENABLED(CONFIG_FOXGLOVE_HOST_FRAMES)
        r[3] = host_frames_parts(&r[4], 8);
        r[2] = 0;
#endif
#if IS_ENABLED(CONFIG_FOXGLOVE_LAMPARRAY)
        BUILD_ASSERT(LAMPARRAY_STATE_AT + LAMPARRAY_STATE_SIZE <= sizeof(r), "D7 90's tail");
        lamparray_state(&r[LAMPARRAY_STATE_AT]);
#endif
        reply(r, sizeof(r));
        break;
    }
    case SETTING_EFFECT: {
        uint8_t r[7] = {HOST_LINK_SETTINGS, 0x80 | SETTING_EFFECT, 4}; // not built in
#if IS_ENABLED(CONFIG_FOXGLOVE_HOST_EFFECTS)
        BUILD_ASSERT(sizeof(r) == HOST_EFFECTS_REPLY_SIZE, "the D7 8F reply");
        host_effects_command(d, len, r);
        if (r[2] == 0 && d[2] == 2) { // op EFFECT: kind in byte 3
            health_inc(HEALTH_LIGHT_APPLIES);
            health_set(HEALTH_LAST_EFFECT, d[3]);
        } else if (r[2] == 0 && d[2] == 0) { // op RELEASE
            health_set(HEALTH_LAST_EFFECT, 0);
        }
#endif
        reply(r, sizeof(r));
        break;
    }
    case SETTING_PC_STATE: {
        uint8_t r[PC_STATE_SIZE];
        pc_state_fill(r, d, len);
        reply(r, sizeof(r));
        break;
    }
    case SETTING_HEALTH:
        health_command(d);
        break;
    case SETTING_PANEL_POLARITY: {
        // D7 12 (#666): a report cut short is a read; d is padded to the report size.
        uint8_t r[6] = {HOST_LINK_SETTINGS, 0x80 | SETTING_PANEL_POLARITY, 4,
                        PANEL_POLARITY_DEFAULT, PANEL_POLARITY_DEFAULT, PANEL_POLARITY_UNKNOWN};
#if IS_ENABLED(CONFIG_FOXGLOVE_PANEL_POLARITY)
        uint8_t want = len >= 3 ? d[2] : PANEL_POLARITY_READ;
        r[2] = want == PANEL_POLARITY_READ || panel_polarity_set(want) == 0 ? 0 : 5;
        r[3] = panel_polarity_get();
#if IS_ENABLED(CONFIG_NICE_VIEW_ART_RELAY)
        r[5] = art_relay_right_polarity();
#endif
#endif
        reply(r, sizeof(r));
        break;
    }
    case SETTING_KNOB:
        knob_command(d, len);
        break;
    case SETTING_ART_KEYS: {
        atomic_set(&art_keys, len >= 3 && d[2] != 0);
        uint8_t r[4] = {HOST_LINK_SETTINGS, 0x80 | SETTING_ART_KEYS, 0, atomic_get(&art_keys)};
        reply(r, sizeof(r));
        break;
    }
    case SETTING_GUARD_TEST: {
        uint8_t r[3] = {HOST_LINK_SETTINGS, 0x80 | SETTING_GUARD_TEST, 4};
#if IS_ENABLED(CONFIG_FOXGLOVE_TEST_HOOKS) && IS_ENABLED(CONFIG_NICE_VIEW_ART_RELAY)
        int err = art_relay_guard_test(len >= 4 ? sys_get_le16(&d[2]) : 2000);
        r[2] = err == 0 ? 0 : err == -ENOTCONN ? 2 : 4;
#endif
        reply(r, sizeof(r));
        break;
    }
    case SETTING_BUILD_INFO: {
        uint8_t half = len >= 3 ? d[2] : 0;
        if (half == 0) {
            struct foxglove_build_info build;
            foxglove_build_info(&build);
            host_link_build_info_reply(0, 0, &build);
            break;
        }
#if IS_ENABLED(CONFIG_NICE_VIEW_ART_RELAY)
        if (half == 1) {
            if (art_relay_build_info()) {
                host_link_build_info_reply(1, 1, NULL); // one is on its way
            }
            break;
        }
#endif
        host_link_build_info_reply(half == 1 ? 2 : 5, half, NULL);
        break;
    }
    case SETTING_BOOTLOADER: {
        uint8_t r[7] = {HOST_LINK_SETTINGS, 0x80 | SETTING_BOOTLOADER, 4};
#if IS_ENABLED(CONFIG_FOXGLOVE_TEST_HOOKS)
        if (test_hooks_bootloader(d, len, r)) {
            break; // the reply comes from the relay
        }
#endif
        reply(r, sizeof(r));
        break;
    }
    case SETTING_POSITION: {
        uint8_t status = 4;
#if IS_ENABLED(CONFIG_FOXGLOVE_TEST_HOOKS)
        status = len < 4 ? 5 : test_hooks_press(d[2], d[3]);
#endif
        if (status != 0) {
            host_link_position_reply(status, d[2]);
        }
        break;
    }
    case SETTING_CONN_STATE: {
        uint8_t r[CONN_STATE_SIZE];
        conn_state(r);
        reply(r, sizeof(r));
        break;
    }
    case SETTING_RGB_SYNC: {
        uint8_t r[5] = {HOST_LINK_SETTINGS, 0x80 | SETTING_RGB_SYNC, 0xff, 0, right_link()};
#if IS_ENABLED(CONFIG_NICE_VIEW_RGB_IDLE)
        if (len >= 3 && d[2] <= 1) { // a report cut short is a read
            rgb_idle_set_sync(d[2]);
        }
        bool on = false;
        rgb_idle_get_on(&on); // the user's on/off: ZMK's is off under a scene (#542)
        r[2] = rgb_idle_sync();
        r[3] = on;
#endif
        reply(r, sizeof(r));
        break;
    }
    case SETTING_INVOKE: {
        uint8_t status = invoke(&d[2], len - 2);
        if (status != INVOKE_OK) {
            invoke_reply(status, sys_get_le16(&d[2]));
        }
        break;
    }
    case SETTING_RGB_STATE: {
        uint8_t r[RGB_STATE_SIZE];
        rgb_state(r);
        reply(r, sizeof(r));
        break;
    }
    case SETTING_BATTERY_DIM: {
        uint8_t status = 4;
#if IS_ENABLED(CONFIG_NICE_VIEW_RGB_BATTERY_DIM)
        status = 0;
        if (len >= 3 && d[2] == 1) {
            struct battery_dim_config c;
            memcpy(&c, &d[3], sizeof(c)); // d is padded to the report size
            status = len < 3 + (int)sizeof(c) || battery_dim_set_config(&c) != 0 ? 5 : 0;
        }
#endif
        uint8_t r[BATTERY_DIM_SIZE];
        battery_dim_fill(r, status, true);
#if IS_ENABLED(CONFIG_NICE_VIEW_RGB_BATTERY_DIM)
        // The PC has this now: an unsolicited one only comes if something changes from here.
        memcpy(battery_dim_sent, r, sizeof(r));
        battery_dim_sent[2] = 0;
        battery_dim_sent[11] = r[11] == 0xff ? 0xff : 0;
#endif
        reply(r, sizeof(r));
        break;
    }
    }
}

static int raw_hid_received_listener(const zmk_event_t *eh) {
    const struct raw_hid_received_event *ev = as_raw_hid_received_event(eh);
    if (ev == NULL || ev->length < 2) {
        return ZMK_EV_EVENT_BUBBLE;
    }
    // Pad short reports so commands can read their fixed fields.
    uint8_t d[REPORT_SIZE] = {0};
    memcpy(d, ev->data, MIN(ev->length, REPORT_SIZE));

    if (d[0] == HOST_LINK_PANEL || d[0] == HOST_LINK_ART || d[0] == HOST_LINK_SETTINGS) {
        atomic_set(&host_seen, 1);
#if IS_ENABLED(CONFIG_FOXGLOVE_TEST_HOOKS)
        test_hooks_host_seen();
#endif
    }
    switch (d[0]) {
    case HOST_LINK_PANEL:
        panel_command(d);
        break;
    case HOST_LINK_ART:
        art_command(d);
        break;
    case HOST_LINK_SETTINGS:
        settings_command(d, MIN(ev->length, REPORT_SIZE));
        break;
    }
    return ZMK_EV_EVENT_BUBBLE;
}

ZMK_LISTENER(host_link, raw_hid_received_listener);
ZMK_SUBSCRIPTION(host_link, raw_hid_received_event);

#if IS_ENABLED(CONFIG_ZMK_SPLIT_BLE_CENTRAL_BATTERY_LEVEL_FETCHING)
static int peripheral_battery_listener(const zmk_event_t *eh) {
    const struct zmk_peripheral_battery_state_changed *ev =
        as_zmk_peripheral_battery_state_changed(eh);
    if (ev != NULL && ev->source == 0 && right_battery != ev->state_of_charge) {
        right_battery = ev->state_of_charge;
        if (atomic_get(&host_seen)) {
            hello_reply();
        }
    }
    return ZMK_EV_EVENT_BUBBLE;
}

ZMK_LISTENER(host_link_battery, peripheral_battery_listener);
ZMK_SUBSCRIPTION(host_link_battery, zmk_peripheral_battery_state_changed);
#endif
