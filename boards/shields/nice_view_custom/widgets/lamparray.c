/*
 * HID LampArray (usage page 0x59) on the left's USB, so Windows Dynamic Lighting can drive the
 * underglow. Prototype for #88: the left's own strip only; the right half comes later over the
 * art relay.
 * SPDX-License-Identifier: MIT
 *
 * All six reports are feature reports on the control pipe (HID Usage Tables, Lighting and
 * Illumination page). While the host is in charge (autonomous mode off), ZMK's underglow and
 * rgb_idle step aside and this file writes the strip; autonomous mode on hands it back.
 */

#include <string.h>

#include <zephyr/device.h>
#include <zephyr/drivers/led_strip.h>
#include <zephyr/init.h>
#include <zephyr/kernel.h>
#include <zephyr/sys/byteorder.h>
#include <zephyr/usb/class/usb_hid.h>

#include <drivers/ext_power.h>
#include <zmk/rgb_underglow.h>
#include <zmk/workqueue.h>

#include <zephyr/logging/log.h>
LOG_MODULE_DECLARE(zmk, CONFIG_ZMK_LOG_LEVEL);

#include "rgb_idle.h"

#define STRIP DT_CHOSEN(zmk_underglow)
#define LAMPS DT_PROP(STRIP, chain_length)

enum {
    REPORT_ATTRIBUTES = 1,
    REPORT_ATTR_REQUEST,
    REPORT_ATTR_RESPONSE,
    REPORT_MULTI_UPDATE,
    REPORT_RANGE_UPDATE,
    REPORT_CONTROL,
};

#define MULTI_MAX 8
#define LOGICAL_MAX_U16 0x27, 0xff, 0xff, 0x00, 0x00
#define LOGICAL_MAX_I32 0x27, 0xff, 0xff, 0xff, 0x7f
#define RGBI 0x09, 0x51, 0x09, 0x52, 0x09, 0x53, 0x09, 0x54

static const uint8_t report_desc[] = {
    0x05, 0x59, // Usage Page (Lighting and Illumination)
    0x09, 0x01, // Usage (LampArray)
    0xa1, 0x01, // Collection (Application)

    0x85, REPORT_ATTRIBUTES, 0x09, 0x02, 0xa1, 0x02, // LampArrayAttributesReport
    0x09, 0x03, 0x15, 0x00, LOGICAL_MAX_U16, 0x75, 0x10, 0x95, 0x01, 0xb1, 0x03, // LampCount
    0x09, 0x04, 0x09, 0x05, 0x09, 0x06, 0x09, 0x07, 0x09, 0x08, // bounding box, kind, interval
    0x15, 0x00, LOGICAL_MAX_I32, 0x75, 0x20, 0x95, 0x05, 0xb1, 0x03, 0xc0,

    0x85, REPORT_ATTR_REQUEST, 0x09, 0x20, 0xa1, 0x02, // LampAttributesRequestReport
    0x09, 0x21, 0x15, 0x00, LOGICAL_MAX_U16, 0x75, 0x10, 0x95, 0x01, 0xb1, 0x02, 0xc0,

    0x85, REPORT_ATTR_RESPONSE, 0x09, 0x22, 0xa1, 0x02, // LampAttributesResponseReport
    0x09, 0x21, 0x15, 0x00, LOGICAL_MAX_U16, 0x75, 0x10, 0x95, 0x01, 0xb1, 0x02, // LampId
    0x09, 0x23, 0x09, 0x24, 0x09, 0x25, 0x09, 0x27, 0x09, 0x26, // x, y, z, latency, purposes
    0x15, 0x00, LOGICAL_MAX_I32, 0x75, 0x20, 0x95, 0x05, 0xb1, 0x02,
    0x09, 0x28, 0x09, 0x29, 0x09, 0x2a, 0x09, 0x2b, 0x09, 0x2c, 0x09, 0x2d, // level counts, ...
    0x15, 0x00, 0x26, 0xff, 0x00, 0x75, 0x08, 0x95, 0x06, 0xb1, 0x02, 0xc0,

    0x85, REPORT_MULTI_UPDATE, 0x09, 0x50, 0xa1, 0x02, // LampMultiUpdateReport
    0x09, 0x03, 0x09, 0x55, 0x15, 0x00, 0x25, MULTI_MAX, 0x75, 0x08, 0x95, 0x02, 0xb1, 0x02,
    0x09, 0x21, 0x09, 0x21, 0x09, 0x21, 0x09, 0x21, 0x09, 0x21, 0x09, 0x21, 0x09, 0x21, 0x09, 0x21,
    0x15, 0x00, LOGICAL_MAX_U16, 0x75, 0x10, 0x95, MULTI_MAX, 0xb1, 0x02,
    RGBI, RGBI, RGBI, RGBI, RGBI, RGBI, RGBI, RGBI,
    0x15, 0x00, 0x26, 0xff, 0x00, 0x75, 0x08, 0x95, 4 * MULTI_MAX, 0xb1, 0x02, 0xc0,

    0x85, REPORT_RANGE_UPDATE, 0x09, 0x60, 0xa1, 0x02, // LampRangeUpdateReport
    0x09, 0x55, 0x15, 0x00, 0x25, 0x08, 0x75, 0x08, 0x95, 0x01, 0xb1, 0x02,
    0x09, 0x61, 0x09, 0x62, 0x15, 0x00, LOGICAL_MAX_U16, 0x75, 0x10, 0x95, 0x02, 0xb1, 0x02,
    RGBI, 0x15, 0x00, 0x26, 0xff, 0x00, 0x75, 0x08, 0x95, 0x04, 0xb1, 0x02, 0xc0,

    0x85, REPORT_CONTROL, 0x09, 0x70, 0xa1, 0x02, // LampArrayControlReport
    0x09, 0x71, 0x15, 0x00, 0x25, 0x01, 0x75, 0x08, 0x95, 0x01, 0xb1, 0x02, 0xc0,

    0xc0,
};

// Geometry in micrometres. Placeholder: the lamps on a 7 x 3 grid over the left half's
// footprint; real LED positions come with the right half's support.
#define HALF_W 130000
#define HALF_H 95000
#define GRID_COLS 7
#define LAMP_PURPOSE_ACCENT 0x02
#define LAMP_ARRAY_KIND_KEYBOARD 1
#define UPDATE_INTERVAL_US 33000

static const struct device *const strip = DEVICE_DT_GET(STRIP);
static const struct device *const ext_power = DEVICE_DT_GET(DT_INST(0, zmk_ext_power_generic));
static const struct device *hid_dev;

static struct led_rgb pixels[LAMPS];
static uint16_t requested_lamp;
static bool host_control;
static uint8_t get_buf[1 + 28];

/* Strip writes run on ZMK's low-priority queue, after ZMK's own underglow off work. */

static atomic_t want_host;

static void strip_cb(struct k_work *work) {
    bool host = atomic_get(&want_host);
    if (host != host_control) {
        host_control = host;
        rgb_idle_host_control(host);
        if (host) {
            zmk_rgb_underglow_off(); // stops ZMK's animation; the LEDs need the power back
            if (device_is_ready(ext_power)) {
                ext_power_enable(ext_power);
            }
            return; // the next update draws; ZMK's off work zeroes the strip first
        }
    }
    if (host_control) {
        led_strip_update_rgb(strip, pixels, LAMPS);
    }
}
static K_WORK_DEFINE(strip_work, strip_cb);

static void redraw(void) { k_work_submit_to_queue(zmk_workqueue_lowprio_work_q(), &strip_work); }

static void set_lamp(uint16_t id, const uint8_t *rgbi) {
    if (id < LAMPS) {
        // Intensity scales the colour: ws2812 has no separate brightness channel.
        pixels[id] = (struct led_rgb){.r = rgbi[0] * rgbi[3] / 255,
                                      .g = rgbi[1] * rgbi[3] / 255,
                                      .b = rgbi[2] * rgbi[3] / 255};
    }
}

static int get_report_cb(const struct device *dev, struct usb_setup_packet *setup, int32_t *len,
                         uint8_t **data) {
    uint8_t id = setup->wValue & 0xff;
    memset(get_buf, 0, sizeof(get_buf));
    get_buf[0] = id;
    uint8_t *p = &get_buf[1];
    switch (id) {
    case REPORT_ATTRIBUTES:
        sys_put_le16(LAMPS, &p[0]);
        sys_put_le32(HALF_W, &p[2]);
        sys_put_le32(HALF_H, &p[6]);
        sys_put_le32(10000, &p[10]); // depth
        sys_put_le32(LAMP_ARRAY_KIND_KEYBOARD, &p[14]);
        sys_put_le32(UPDATE_INTERVAL_US, &p[18]);
        *len = 1 + 22;
        break;
    case REPORT_ATTR_RESPONSE: {
        uint16_t lamp = requested_lamp;
        sys_put_le16(lamp, &p[0]);
        sys_put_le32((lamp % GRID_COLS) * (HALF_W / GRID_COLS), &p[2]);
        sys_put_le32((lamp / GRID_COLS) * (HALF_H / 3), &p[6]);
        sys_put_le32(0, &p[10]);
        sys_put_le32(UPDATE_INTERVAL_US, &p[14]);
        sys_put_le32(LAMP_PURPOSE_ACCENT, &p[18]);
        p[22] = p[23] = p[24] = p[25] = 255; // red, green, blue, intensity levels
        p[26] = 1;                           // programmable
        p[27] = 0;                           // no key binding
        *len = 1 + 28;
        requested_lamp = (lamp + 1) % LAMPS; // hosts walk the lamps by reading again
        break;
    }
    default:
        return -ENOTSUP;
    }
    *data = get_buf;
    return 0;
}

static int set_report_cb(const struct device *dev, struct usb_setup_packet *setup, int32_t *len,
                         uint8_t **data) {
    const uint8_t *d = *data;
    int n = *len;
    if (n < 1) {
        return -EINVAL;
    }
    switch (d[0]) {
    case REPORT_ATTR_REQUEST:
        if (n >= 3) {
            requested_lamp = MIN(sys_get_le16(&d[1]), LAMPS - 1);
        }
        break;
    case REPORT_MULTI_UPDATE: {
        if (n < 3 + 2 * MULTI_MAX + 4 * MULTI_MAX) {
            return -EINVAL;
        }
        int count = MIN(d[1], MULTI_MAX);
        for (int i = 0; i < count; i++) {
            set_lamp(sys_get_le16(&d[3 + 2 * i]), &d[3 + 2 * MULTI_MAX + 4 * i]);
        }
        if (d[2] & 0x01) { // LampUpdateComplete
            redraw();
        }
        break;
    }
    case REPORT_RANGE_UPDATE: {
        if (n < 10) {
            return -EINVAL;
        }
        uint16_t start = sys_get_le16(&d[2]), end = MIN(sys_get_le16(&d[4]), LAMPS - 1);
        for (uint16_t id = start; id <= end; id++) {
            set_lamp(id, &d[6]);
        }
        if (d[1] & 0x01) {
            redraw();
        }
        break;
    }
    case REPORT_CONTROL:
        if (n >= 2) {
            atomic_set(&want_host, !d[1]); // autonomous mode off = the host draws
            redraw();
        }
        break;
    default:
        return -ENOTSUP;
    }
    return 0;
}

static const struct hid_ops ops = {
    .get_report = get_report_cb,
    .set_report = set_report_cb,
};

// Registers the interface before ZMK enables USB, like the Raw HID module does; nothing else.
static int lamparray_init(void) {
    hid_dev = device_get_binding(CONFIG_FOXGLOVE_LAMPARRAY_DEVICE);
    if (hid_dev == NULL) {
        LOG_ERR("LampArray: no %s", CONFIG_FOXGLOVE_LAMPARRAY_DEVICE);
        return -ENODEV;
    }
    usb_hid_register_device(hid_dev, report_desc, sizeof(report_desc), &ops);
    return usb_hid_init(hid_dev);
}

SYS_INIT(lamparray_init, APPLICATION, CONFIG_APPLICATION_INIT_PRIORITY);
