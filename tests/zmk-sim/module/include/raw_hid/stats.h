/* Simulator copy of the Raw HID module's counters (BanditF/zmk-raw-hid, raw_hid/stats.h); the
 * health case (module/health_sim.c) supplies raw_hid_stats_get. SPDX-License-Identifier: MIT */
#pragma once

#include <stdint.h>

#define RAW_HID_STATS_VERSION 3

struct raw_hid_stats {
    uint32_t sent;
    uint32_t queued;
    uint32_t dropped;
    uint32_t failed;
    uint32_t stuck;
    // Version 2: per transport.
    uint32_t usb_sent;
    uint32_t usb_failed;
    uint32_t ble_sent;
    uint32_t ble_failed;
    uint32_t ble_skipped;
    // Version 3: each transport's last refusal's error code.
    int32_t usb_last_err;
    int32_t ble_last_err;
};

void raw_hid_stats_get(struct raw_hid_stats *out);
