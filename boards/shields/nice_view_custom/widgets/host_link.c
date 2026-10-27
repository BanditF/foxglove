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
#include "rgb_idle.h"
#include "battery_dim.h"
#include "behavior_safety.h"
#if IS_ENABLED(CONFIG_FOXGLOVE_HOST_FRAMES)
#include "host_frames.h"
#endif
#if IS_ENABLED(CONFIG_FOXGLOVE_HOST_EFFECTS)
#include "host_effects.h"
#endif
#if IS_ENABLED(CONFIG_FOXGLOVE_TEST_HOOKS)
#include "test_hooks.h"
#endif

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
enum { ART_BEGIN = 0x01, ART_DATA, ART_COMMIT, ART_SELECT, ART_STATUS, ART_SAVE, ART_SLOT, ART_ERASE };
enum { SETTING_RGB_SYNC = 0x01, SETTING_INVOKE, SETTING_RGB_STATE, SETTING_CONN_STATE, SETTING_POSITION,
       SETTING_SCREEN_SNAP, SETTING_SCREEN_READ, SETTING_BOOTLOADER,
       SETTING_BUILD_INFO, SETTING_GUARD_TEST = 0x0B, // 0x0A: LAMP_TEST (#115 branch)
       SETTING_FRAME = 0x0C, SETTING_ART_KEYS = 0x0D,
       SETTING_BATTERY_DIM = 0x0E, SETTING_EFFECT = 0x0F };
enum {
    INVOKE_OK, INVOKE_UNKNOWN, INVOKE_BUSY, INVOKE_FAILED, INVOKE_UNSUPPORTED,
    INVOKE_BAD_LENGTH, INVOKE_REFUSED
};

#define REPORT_SIZE CONFIG_RAW_HID_REPORT_SIZE
#define ALIVE_TIMEOUT K_SECONDS(CONFIG_NICE_VIEW_HOST_PANEL_TIMEOUT_S)

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

static void redraw_cb(struct k_work *work) { host_panel_redraw(); }
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
#endif
#if IS_ENABLED(CONFIG_NICE_VIEW_RGB_IDLE)
    caps |= HOST_LINK_CAP_SETTINGS_RGB | HOST_LINK_CAP_RGB_STATE_EVENT;
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
#endif
#if IS_ENABLED(CONFIG_NICE_VIEW_ART_RELAY) && IS_ENABLED(CONFIG_NICE_VIEW_RGB_IDLE)
    caps |= HOST_LINK_CAP_RIGHT_EVENTS;
#endif
#if IS_ENABLED(CONFIG_FOXGLOVE_HOST_FRAMES)
    caps |= HOST_LINK_CAP_FRAMES;
#endif
#if IS_ENABLED(CONFIG_FOXGLOVE_HOST_EFFECTS)
    caps |= HOST_LINK_CAP_EFFECTS | HOST_LINK_CAP_EFFECT_LAMPS;
#endif
#if IS_ENABLED(CONFIG_NICE_VIEW_ART_RELAY) && DT_HAS_COMPAT_STATUS_OKAY(foxglove_behavior_art_step)
    caps |= HOST_LINK_CAP_ART_KEYS;
#endif
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

/* D5 06 SCREENS (#160): the board's table (foxglove_screens[], e.g. screens_corne.c) as the PC
 * sees it. */

static void screens_reply(uint8_t index) {
    uint8_t r[5 + sizeof(struct screen_descriptor)] = {HOST_LINK_PANEL, 0x80 | PANEL_SCREENS, 5,
                                                       foxglove_screen_count, index};
    if (index < foxglove_screen_count) {
        struct screen_descriptor s = foxglove_screens[index];
        s.version = SCREEN_DESCRIPTOR_VERSION;
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

static void art_command(const uint8_t *d) {
    uint8_t r[2 + ART_RELAY_REPORT_SIZE] = {HOST_LINK_ART, 0x80 | d[1]};
#if IS_ENABLED(CONFIG_NICE_VIEW_ART_RELAY)
    int err = 0;
    switch (d[1]) {
    case ART_BEGIN:
        err = art_relay_begin(sys_get_le16(&d[2]), sys_get_le32(&d[4]));
        break;
    case ART_DATA:
        err = art_relay_data(sys_get_le16(&d[2]), &d[5], MIN(d[4], REPORT_SIZE - 5));
        if (err == 0) {
            return;
        }
        break;
    case ART_COMMIT:
        err = art_relay_commit();
        break;
    case ART_SELECT:
        err = art_relay_select((int8_t)d[2]);
        break;
    case ART_STATUS:
        break;
    case ART_SAVE:
        err = art_relay_save(d[2], &d[3]);
        break;
    case ART_ERASE:
        err = art_relay_erase(d[2]);
        break;
    case ART_SLOT:
        if (art_relay_slot(d[2]) == 0) {
            return; // D6 87 comes from the relay
        }
        r[2] = 1; // busy
        r[3] = d[2];
        reply(r, sizeof(r));
        return;
    default:
        return;
    }
    ARG_UNUSED(err); // the error is in the report
    art_relay_report(&r[2]);
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

static int conn_state_listener(const zmk_event_t *eh) {
    // The PC side may be gone: the keys step the art on the board again until it subscribes anew.
#if IS_ENABLED(CONFIG_ZMK_USB)
    if (!zmk_usb_is_hid_ready()) {
        atomic_set(&art_keys, 0);
    }
#endif
    if (as_zmk_endpoint_changed(eh) != NULL) {
        atomic_set(&art_keys, 0);
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

#define RGB_STATE_SIZE (2 + 2 * sizeof(struct rgb_idle_wire) + 1)

// The RGB_STATE reply; asks the relay for a fresher right state on the way.
static void rgb_state(uint8_t r[RGB_STATE_SIZE]) {
    struct rgb_idle_wire left = {0}, right = {0};
    uint8_t age = 0xff;
#if IS_ENABLED(CONFIG_NICE_VIEW_RGB_IDLE)
    rgb_idle_state(&left);
#endif
#if IS_ENABLED(CONFIG_NICE_VIEW_ART_RELAY)
    age = art_relay_right_rgb(&right);
#endif
    r[0] = HOST_LINK_SETTINGS;
    r[1] = 0x80 | SETTING_RGB_STATE;
    memcpy(&r[2], &left, sizeof(left));
    r[2 + sizeof(left)] = age;
    memcpy(&r[3 + sizeof(left)], &right, sizeof(right));
}

// Unsolicited RGB_STATE: after host traffic, the left's underglow switching on or off (idle,
// wake, a key, sync) sends the same reply on its own. The right's half of it is the relay's
// freshest read, taken RGB_STATE_SETTLE after the change so the right has followed by then.
#define RGB_STATE_SETTLE K_MSEC(600)

static uint8_t rgb_state_sent = 0xff; // left on | right on << 1 | right known << 2, last sent

static void rgb_state_event_cb(struct k_work *work) {
    uint8_t r[RGB_STATE_SIZE];
    rgb_state(r);
    uint8_t age = r[2 + sizeof(struct rgb_idle_wire)];
    uint8_t now = r[2] | r[3 + sizeof(struct rgb_idle_wire)] << 1 | (age != 0xff) << 2;
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

#define BATTERY_DIM_SIZE 16

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
    if (memcmp(cmp, battery_dim_sent, sizeof(cmp)) != 0) {
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
#endif
        reply(r, sizeof(r));
        break;
    }
    case SETTING_EFFECT: {
        uint8_t r[7] = {HOST_LINK_SETTINGS, 0x80 | SETTING_EFFECT, 4}; // not built in
#if IS_ENABLED(CONFIG_FOXGLOVE_HOST_EFFECTS)
        BUILD_ASSERT(sizeof(r) == HOST_EFFECTS_REPLY_SIZE, "the D7 8F reply");
        host_effects_command(d, len, r);
#endif
        reply(r, sizeof(r));
        break;
    }
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
