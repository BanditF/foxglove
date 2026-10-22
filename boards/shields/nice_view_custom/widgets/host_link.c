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
#include <zmk/display.h>
#include <zmk/event_manager.h>
#include <zmk/events/battery_state_changed.h>

#include "host_link.h"
#include "art_relay.h"

enum { PANEL_HELLO = 0x01, PANEL_FILL, PANEL_ROWS, PANEL_PRESENT, PANEL_RELEASE };
enum { ART_BEGIN = 0x01, ART_DATA, ART_COMMIT, ART_SELECT, ART_STATUS };

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
        uint8_t r[11 + 10] = {HOST_LINK_PANEL, 0x80 | PANEL_HELLO, HOST_LINK_VERSION, HOST_PANEL_W,
                              HOST_PANEL_H,    HOST_PANEL_X,       HOST_PANEL_Y,      right_link(),
                              right_battery,   (uint8_t)discovery, tries};
#if IS_ENABLED(CONFIG_NICE_VIEW_ART_RELAY)
        art_relay_debug(&r[11]);
#endif
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

static int raw_hid_received_listener(const zmk_event_t *eh) {
    const struct raw_hid_received_event *ev = as_raw_hid_received_event(eh);
    if (ev == NULL || ev->length < 2) {
        return ZMK_EV_EVENT_BUBBLE;
    }
    // Pad short reports so commands can read their fixed fields.
    uint8_t d[REPORT_SIZE] = {0};
    memcpy(d, ev->data, MIN(ev->length, REPORT_SIZE));

    switch (d[0]) {
    case HOST_LINK_PANEL:
        panel_command(d);
        break;
    case HOST_LINK_ART:
        art_command(d);
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
