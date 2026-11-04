/*
 * The HID LampArray on the left's USB (#88): one more HID interface (CONFIG_USB_HID_DEVICE_COUNT
 * one up, CONFIG_FOXGLOVE_LAMPARRAY_DEVICE) whose six feature reports go through the control
 * pipe to the transport-neutral core (lamparray.c). The hand-back on an unplug or a suspend is
 * host_frames.c's, with the owner rule.
 * SPDX-License-Identifier: MIT
 */

#include <errno.h>

#include <zephyr/device.h>
#include <zephyr/init.h>
#include <zephyr/kernel.h>
#include <zephyr/usb/usb_device.h>
#include <zephyr/usb/class/usb_hid.h>

#include <zephyr/logging/log.h>
LOG_MODULE_DECLARE(zmk, CONFIG_ZMK_LOG_LEVEL);

#include "lamparray.h"

static uint8_t get_buf[1 + LAMPARRAY_REPORT_MAX];

static int get_report_cb(const struct device *dev, struct usb_setup_packet *setup, int32_t *len,
                         uint8_t **data) {
    uint8_t id = setup->wValue & 0xff;
    int n = lamparray_get_report(LAMPARRAY_USB, id, &get_buf[1]);
    if (n < 0) {
        return n;
    }
    get_buf[0] = id;
    *len = 1 + n;
    *data = get_buf;
    return 0;
}

static int set_report_cb(const struct device *dev, struct usb_setup_packet *setup, int32_t *len,
                         uint8_t **data) {
    if (*len < 1) {
        return -EINVAL;
    }
    return lamparray_set_report(LAMPARRAY_USB, (*data)[0], *data + 1, *len - 1);
}

static const struct hid_ops ops = {
    .get_report = get_report_cb,
    .set_report = set_report_cb,
};

// Registers the interface before ZMK enables USB, as the Raw HID module does; nothing else (no
// settings, no I/O, no work at boot).
static int lamparray_usb_init(void) {
    const struct device *hid_dev = device_get_binding(CONFIG_FOXGLOVE_LAMPARRAY_DEVICE);
    if (hid_dev == NULL) {
        LOG_ERR("LampArray: no %s", CONFIG_FOXGLOVE_LAMPARRAY_DEVICE);
        return -ENODEV;
    }
    usb_hid_register_device(hid_dev, lamparray_report_desc, lamparray_report_desc_len, &ops);
    int err = usb_hid_init(hid_dev);
    if (err == 0) {
        lamparray_transport_ready(LAMPARRAY_USB);
    }
    return err;
}

SYS_INIT(lamparray_usb_init, APPLICATION, CONFIG_APPLICATION_INIT_PRIORITY);
