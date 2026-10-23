/*
 * PC link over Raw HID: the host panel (0xD5) and right-hand art pushes (0xD6). Protocol in
 * host_link.h.
 * SPDX-License-Identifier: MIT
 */

#include <string.h>

#include <zephyr/kernel.h>
#include <zephyr/spinlock.h>
#include <zephyr/sys/byteorder.h>

#include <zephyr/logging/log.h>
LOG_MODULE_DECLARE(zmk, CONFIG_ZMK_LOG_LEVEL);

#include <raw_hid/events.h>
#include <zmk/behavior.h>
#include <zmk/display.h>
#include <zmk/event_manager.h>
#include <zmk/events/battery_state_changed.h>
#include <zmk/events/position_state_changed.h>
#include <zmk/keymap.h>

#include "host_link.h"
#include "art_relay.h"
#include "rgb_idle.h"

#if IS_ENABLED(CONFIG_NICE_VIEW_RGB_IDLE)
#include <zmk/rgb_underglow.h>
#endif

enum { PANEL_HELLO = 0x01, PANEL_FILL, PANEL_ROWS, PANEL_PRESENT, PANEL_RELEASE };
enum { ART_BEGIN = 0x01, ART_DATA, ART_COMMIT, ART_SELECT, ART_STATUS };
enum { SETTING_RGB_SYNC = 0x01, SETTING_INVOKE, SETTING_RGB_STATE };
enum { INVOKE_OK, INVOKE_UNKNOWN, INVOKE_BUSY, INVOKE_FAILED, INVOKE_UNSUPPORTED, INVOKE_BAD_LENGTH };

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

static uint8_t right_link(void) {
#if IS_ENABLED(CONFIG_NICE_VIEW_ART_RELAY)
    return art_relay_link();
#else
    return 0;
#endif
}

static void panel_command(const uint8_t *d) {
    if (CONFIG_NICE_VIEW_HOST_PANEL_TIMEOUT_S > 0 && d[1] != PANEL_RELEASE) {
        k_work_reschedule(&timeout_work, ALIVE_TIMEOUT);
    }

    switch (d[1]) {
    case PANEL_HELLO: {
        int8_t discovery = 0;
        uint8_t tries = 0;
#if IS_ENABLED(CONFIG_NICE_VIEW_ART_RELAY)
        art_relay_discovery(&discovery, &tries);
#endif
        uint8_t r[11 + 10 + 4] = {HOST_LINK_PANEL, 0x80 | PANEL_HELLO, HOST_LINK_VERSION, HOST_PANEL_W,
                              HOST_PANEL_H,    HOST_PANEL_X,       HOST_PANEL_Y,      right_link(),
                              right_battery,   (uint8_t)discovery, tries};
#if IS_ENABLED(CONFIG_NICE_VIEW_ART_RELAY)
        art_relay_debug(&r[11]);
#endif
        uint32_t caps = HOST_LINK_CAP_PANEL;
#if IS_ENABLED(CONFIG_NICE_VIEW_ART_RELAY)
        caps |= HOST_LINK_CAP_ART_RELAY | HOST_LINK_CAP_ART_STATUS_EVENT;
#endif
#if IS_ENABLED(CONFIG_NICE_VIEW_RGB_IDLE)
        caps |= HOST_LINK_CAP_SETTINGS_RGB;
#endif
        sys_put_le32(caps, &r[21]);
        reply(r, sizeof(r));
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
    case PANEL_PRESENT: {
        k_spinlock_key_t key = k_spin_lock(&lock);
        memcpy(front, back, sizeof(front));
        k_spin_unlock(&lock, key);
        set_alive(true);
        redraw();
        break;
    }
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

static void settings_command(const uint8_t *d, int len) {
    switch (d[1]) {
    case SETTING_RGB_SYNC: {
        uint8_t r[5] = {HOST_LINK_SETTINGS, 0x80 | SETTING_RGB_SYNC, 0xff, 0, right_link()};
#if IS_ENABLED(CONFIG_NICE_VIEW_RGB_IDLE)
        if (len >= 3 && d[2] <= 1) { // a report cut short is a read
            rgb_idle_set_sync(d[2]);
        }
        bool on = false;
        zmk_rgb_underglow_get_state(&on);
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
        uint8_t r[2 + 2 * sizeof(struct rgb_idle_wire) + 1] = {HOST_LINK_SETTINGS,
                                                                0x80 | SETTING_RGB_STATE};
        struct rgb_idle_wire left = {0}, right = {0};
        uint8_t age = 0xff;
#if IS_ENABLED(CONFIG_NICE_VIEW_RGB_IDLE)
        rgb_idle_state(&left);
#endif
#if IS_ENABLED(CONFIG_NICE_VIEW_ART_RELAY)
        age = art_relay_right_rgb(&right);
#endif
        memcpy(&r[2], &left, sizeof(left));
        r[2 + sizeof(left)] = age;
        memcpy(&r[3 + sizeof(left)], &right, sizeof(right));
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
    if (ev != NULL && ev->source == 0) {
        right_battery = ev->state_of_charge;
    }
    return ZMK_EV_EVENT_BUBBLE;
}

ZMK_LISTENER(host_link_battery, peripheral_battery_listener);
ZMK_SUBSCRIPTION(host_link_battery, zmk_peripheral_battery_state_changed);
#endif
