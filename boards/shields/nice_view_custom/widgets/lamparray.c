/*
 * HID LampArray (usage page 0x59, #88): the reports without their transport (lamparray.h). One
 * LampArray of every lamp SET_FRAME has (the Eyelash Corne's 42: the left's 21, then the right's
 * 21 over the art relay), drawn through host_frames.c, so the LampArray, SET_FRAME and SET_EFFECT
 * share one lamp numbering, one owner and the right half's 10 frames a second.
 * SPDX-License-Identifier: MIT
 *
 * All six reports are feature reports (HID Usage Tables, Lighting and Illumination page): on USB
 * the control pipe (lamparray_usb.c), over Bluetooth a second HID service (lamparray_hog.c).
 * Autonomous mode off takes the lights for that transport (D7 8C event, owner 1 or 2); on hands
 * them back (owner 0). Updates land only while that transport has them.
 *
 * Positions come from SET_EFFECT's built-in lamp table (lamp_effect.c, the board profile's led
 * order: PROVISIONAL until the #90 row G sitting checks it on the board), placed in SET_EFFECT's
 * default room: two halves of 124 x 86 mm, 80 mm apart, the right mirrored.
 */

#include <errno.h>
#include <string.h>

#include <zephyr/kernel.h>
#include <zephyr/sys/byteorder.h>
#include <zephyr/sys/util.h>

#include "health.h"
#include "host_frames.h"
#include "lamp_effect.h"
#include "lamp_parts.h"
#include "lamparray.h"

#define LOGICAL_MAX_U16 0x27, 0xff, 0xff, 0x00, 0x00
#define LOGICAL_MAX_I32 0x27, 0xff, 0xff, 0xff, 0x7f
#define RGBI 0x09, 0x51, 0x09, 0x52, 0x09, 0x53, 0x09, 0x54

const uint8_t lamparray_report_desc[] = {
    0x05, 0x59, // Usage Page (Lighting and Illumination)
    0x09, 0x01, // Usage (LampArray)
    0xa1, 0x01, // Collection (Application)

    0x85, LAMPARRAY_REPORT_ATTRIBUTES, 0x09, 0x02, 0xa1, 0x02, // LampArrayAttributesReport
    0x09, 0x03, 0x15, 0x00, LOGICAL_MAX_U16, 0x75, 0x10, 0x95, 0x01, 0xb1, 0x03, // LampCount
    0x09, 0x04, 0x09, 0x05, 0x09, 0x06, 0x09, 0x07, 0x09, 0x08, // bounding box, kind, interval
    0x15, 0x00, LOGICAL_MAX_I32, 0x75, 0x20, 0x95, 0x05, 0xb1, 0x03, 0xc0,

    0x85, LAMPARRAY_REPORT_ATTR_REQUEST, 0x09, 0x20, 0xa1, 0x02, // LampAttributesRequestReport
    0x09, 0x21, 0x15, 0x00, LOGICAL_MAX_U16, 0x75, 0x10, 0x95, 0x01, 0xb1, 0x02, 0xc0,

    0x85, LAMPARRAY_REPORT_ATTR_RESPONSE, 0x09, 0x22, 0xa1, 0x02, // LampAttributesResponseReport
    0x09, 0x21, 0x15, 0x00, LOGICAL_MAX_U16, 0x75, 0x10, 0x95, 0x01, 0xb1, 0x02, // LampId
    0x09, 0x23, 0x09, 0x24, 0x09, 0x25, 0x09, 0x27, 0x09, 0x26, // x, y, z, latency, purposes
    0x15, 0x00, LOGICAL_MAX_I32, 0x75, 0x20, 0x95, 0x05, 0xb1, 0x02,
    0x09, 0x28, 0x09, 0x29, 0x09, 0x2a, 0x09, 0x2b, 0x09, 0x2c, 0x09, 0x2d, // level counts, ...
    0x15, 0x00, 0x26, 0xff, 0x00, 0x75, 0x08, 0x95, 0x06, 0xb1, 0x02, 0xc0,

    0x85, LAMPARRAY_REPORT_MULTI_UPDATE, 0x09, 0x50, 0xa1, 0x02, // LampMultiUpdateReport
    0x09, 0x03, 0x09, 0x55, 0x15, 0x00, 0x25, LAMPARRAY_MULTI_MAX, 0x75, 0x08, 0x95, 0x02, 0xb1,
    0x02, 0x09, 0x21, 0x09, 0x21, 0x09, 0x21, 0x09, 0x21, 0x09, 0x21, 0x09, 0x21, 0x09, 0x21, 0x09,
    0x21, 0x15, 0x00, LOGICAL_MAX_U16, 0x75, 0x10, 0x95, LAMPARRAY_MULTI_MAX, 0xb1, 0x02,
    RGBI, RGBI, RGBI, RGBI, RGBI, RGBI, RGBI, RGBI,
    0x15, 0x00, 0x26, 0xff, 0x00, 0x75, 0x08, 0x95, 4 * LAMPARRAY_MULTI_MAX, 0xb1, 0x02, 0xc0,

    0x85, LAMPARRAY_REPORT_RANGE_UPDATE, 0x09, 0x60, 0xa1, 0x02, // LampRangeUpdateReport
    0x09, 0x55, 0x15, 0x00, 0x25, 0x08, 0x75, 0x08, 0x95, 0x01, 0xb1, 0x02,
    0x09, 0x61, 0x09, 0x62, 0x15, 0x00, LOGICAL_MAX_U16, 0x75, 0x10, 0x95, 0x02, 0xb1, 0x02,
    RGBI, 0x15, 0x00, 0x26, 0xff, 0x00, 0x75, 0x08, 0x95, 0x04, 0xb1, 0x02, 0xc0,

    0x85, LAMPARRAY_REPORT_CONTROL, 0x09, 0x70, 0xa1, 0x02, // LampArrayControlReport
    0x09, 0x71, 0x15, 0x00, 0x25, 0x01, 0x75, 0x08, 0x95, 0x01, 0xb1, 0x02, 0xc0,

    0xc0,
};
const size_t lamparray_report_desc_len = sizeof(lamparray_report_desc);
BUILD_ASSERT(LAMPARRAY_MULTI_MAX == 8, "the descriptor lists 8 lamp ids and 8 RGBI");

// SET_EFFECT's default room (host_effects.c, before any PLACE), in micrometres.
#define HALF_W_MM 124
#define HALF_H_MM 86
#define RIGHT_X_UM 204000
#define BOX_W_UM 328000
#define BOX_H_UM (HALF_H_MM * 1000)
#define BOX_D_UM 10000
#define LAMP_PURPOSE_ACCENT 0x02
#define LAMP_ARRAY_KIND_KEYBOARD 1
#define UPDATE_INTERVAL_US 33000
// A Bluetooth write takes a connection event and a frame is 6 of them, so hosts pace slower.
#define UPDATE_INTERVAL_BLE_US 100000
#define LAMP_UPDATE_COMPLETE BIT(0)

#define FLAG_USB BIT(0)
#define FLAG_BLE BIT(1)
#define FLAG_USB_READY BIT(2)
#define FLAG_BLE_READY BIT(3)

static atomic_t requested_lamp;
static atomic_t ready; // FLAG_*_READY
static atomic_t frames_drawn, refused;

static void bump(atomic_t *c) {
    if (atomic_get(c) < UINT16_MAX) {
        atomic_inc(c);
    }
}

static uint8_t owner_of(enum lamparray_transport t) {
    return t == LAMPARRAY_BLE ? HOST_FRAMES_OWNER_LAMPARRAY_BLE : HOST_FRAMES_OWNER_LAMPARRAY_USB;
}

static int lamp_count(void) { return host_frames_lamps(); }

// Lamp id's place in the room, micrometres from the top-left.
static void lamp_place(int id, int32_t *x, int32_t *y) {
    bool right = id >= LAMP_PARTS_OWN;
    uint16_t pm[2];
    lamp_effect_table(right ? id - LAMP_PARTS_OWN : id, right, pm);
    *x = (right ? RIGHT_X_UM : 0) + HALF_W_MM * pm[0]; // mm x thousandths = micrometres
    *y = HALF_H_MM * pm[1];
}

// Intensity scales the colour: the WS2812s have no separate brightness channel.
static void set_lamp(uint8_t who, int id, const uint8_t *rgbi) {
    host_frames_lamparray_set(who, id, rgbi[0] * rgbi[3] / 255, rgbi[1] * rgbi[3] / 255,
                              rgbi[2] * rgbi[3] / 255);
}

static void draw(uint8_t who) {
    if (host_frames_owner() == who) {
        host_frames_lamparray_show(who);
        bump(&frames_drawn);
        health_inc(HEALTH_LIGHT_APPLIES); // the PC's colours applied (#705)
    }
}

int lamparray_report_len(uint8_t id) {
    switch (id) {
    case LAMPARRAY_REPORT_ATTRIBUTES:
        return 22;
    case LAMPARRAY_REPORT_ATTR_REQUEST:
        return 2;
    case LAMPARRAY_REPORT_ATTR_RESPONSE:
        return 28;
    case LAMPARRAY_REPORT_MULTI_UPDATE:
        return LAMPARRAY_REPORT_MAX;
    case LAMPARRAY_REPORT_RANGE_UPDATE:
        return 9;
    case LAMPARRAY_REPORT_CONTROL:
        return 1;
    default:
        return 0;
    }
}

int lamparray_get_report(enum lamparray_transport t, uint8_t id, uint8_t *p) {
    uint32_t interval = t == LAMPARRAY_BLE ? UPDATE_INTERVAL_BLE_US : UPDATE_INTERVAL_US;
    memset(p, 0, LAMPARRAY_REPORT_MAX);
    switch (id) {
    case LAMPARRAY_REPORT_ATTRIBUTES:
        sys_put_le16(lamp_count(), &p[0]);
        sys_put_le32(BOX_W_UM, &p[2]);
        sys_put_le32(BOX_H_UM, &p[6]);
        sys_put_le32(BOX_D_UM, &p[10]);
        sys_put_le32(LAMP_ARRAY_KIND_KEYBOARD, &p[14]);
        sys_put_le32(interval, &p[18]);
        return 22;
    case LAMPARRAY_REPORT_ATTR_RESPONSE: {
        int lamp = (int)atomic_get(&requested_lamp) % lamp_count();
        int32_t x, y;
        lamp_place(lamp, &x, &y);
        sys_put_le16(lamp, &p[0]);
        sys_put_le32(x, &p[2]);
        sys_put_le32(y, &p[6]);
        sys_put_le32(0, &p[10]);
        sys_put_le32(interval, &p[14]);
        sys_put_le32(LAMP_PURPOSE_ACCENT, &p[18]);
        p[22] = p[23] = p[24] = p[25] = 255; // red, green, blue, intensity levels
        p[26] = 1;                           // programmable
        p[27] = 0;                           // no key bound
        atomic_set(&requested_lamp, (lamp + 1) % lamp_count()); // hosts walk by reading again
        return 28;
    }
    default:
        return -ENOTSUP;
    }
}

static int set_report(enum lamparray_transport t, uint8_t id, const uint8_t *d, int n) {
    uint8_t who = owner_of(t);
    switch (id) {
    case LAMPARRAY_REPORT_ATTR_REQUEST:
        if (n < 2) {
            return -EINVAL;
        }
        atomic_set(&requested_lamp, MIN(sys_get_le16(&d[0]), lamp_count() - 1));
        return 0;
    case LAMPARRAY_REPORT_MULTI_UPDATE: {
        if (n < LAMPARRAY_REPORT_MAX) {
            return -EINVAL;
        }
        int count = MIN(d[0], LAMPARRAY_MULTI_MAX);
        for (int i = 0; i < count; i++) {
            set_lamp(who, sys_get_le16(&d[2 + 2 * i]), &d[2 + 2 * LAMPARRAY_MULTI_MAX + 4 * i]);
        }
        if (d[1] & LAMP_UPDATE_COMPLETE) {
            draw(who);
        }
        return 0;
    }
    case LAMPARRAY_REPORT_RANGE_UPDATE: {
        if (n < 9) {
            return -EINVAL;
        }
        int start = sys_get_le16(&d[1]), end = MIN(sys_get_le16(&d[3]), lamp_count() - 1);
        for (int lamp = start; lamp <= end; lamp++) {
            set_lamp(who, lamp, &d[5]);
        }
        if (d[0] & LAMP_UPDATE_COMPLETE) {
            draw(who);
        }
        return 0;
    }
    case LAMPARRAY_REPORT_CONTROL:
        if (n < 1) {
            return -EINVAL;
        }
        host_frames_lamparray(who, d[0] == 0); // autonomous mode off: this host draws
        return 0;
    default:
        return -ENOTSUP;
    }
}

int lamparray_set_report(enum lamparray_transport t, uint8_t id, const uint8_t *d, int n) {
    int err = set_report(t, id, d, n);
    if (err) {
        bump(&refused);
    }
    return err;
}

void lamparray_transport_ready(enum lamparray_transport t) {
    atomic_or(&ready, t == LAMPARRAY_BLE ? FLAG_BLE_READY : FLAG_USB_READY);
}

void lamparray_state(uint8_t out[LAMPARRAY_STATE_SIZE]) {
    uint16_t takes, hand_backs, host_refused;
    host_frames_lamparray_counts(&takes, &hand_backs, &host_refused);
    out[0] = (IS_ENABLED(CONFIG_FOXGLOVE_LAMPARRAY_USB) ? FLAG_USB : 0) |
             (IS_ENABLED(CONFIG_FOXGLOVE_LAMPARRAY_BLE) ? FLAG_BLE : 0) |
             (uint8_t)atomic_get(&ready);
    out[1] = host_frames_owner();
    sys_put_le16(takes, &out[2]);
    sys_put_le16(hand_backs, &out[4]);
    sys_put_le16((uint16_t)atomic_get(&frames_drawn), &out[6]);
    sys_put_le16(host_refused, &out[8]);
    sys_put_le16((uint16_t)atomic_get(&refused), &out[10]);
}
