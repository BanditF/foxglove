/*
 * HID LampArray (usage page 0x59, #88): Windows Dynamic Lighting drives both halves' underglow.
 * The reports without their transport, so USB (lamparray_usb.c) and Bluetooth (lamparray_hog.c,
 * FOXGLOVE_LAMPARRAY_BLE) share one LampArray; both draw through SET_FRAME's core
 * (host_frames.c): one lamp numbering, one owner, the right half over the art relay.
 * SPDX-License-Identifier: MIT
 */

#pragma once

#include <stddef.h>
#include <stdint.h>

enum {
    LAMPARRAY_REPORT_ATTRIBUTES = 1,
    LAMPARRAY_REPORT_ATTR_REQUEST,
    LAMPARRAY_REPORT_ATTR_RESPONSE,
    LAMPARRAY_REPORT_MULTI_UPDATE,
    LAMPARRAY_REPORT_RANGE_UPDATE,
    LAMPARRAY_REPORT_CONTROL,
};
#define LAMPARRAY_REPORTS LAMPARRAY_REPORT_CONTROL

#define LAMPARRAY_MULTI_MAX 8
// The largest report payload (without its report ID): LampMultiUpdate.
#define LAMPARRAY_REPORT_MAX (2 + 2 * LAMPARRAY_MULTI_MAX + 4 * LAMPARRAY_MULTI_MAX)

// The transports, as their owner bytes (host_frames.h HOST_FRAMES_OWNER_LAMPARRAY_*).
enum lamparray_transport { LAMPARRAY_USB = 1, LAMPARRAY_BLE = 2 };

extern const uint8_t lamparray_report_desc[];
extern const size_t lamparray_report_desc_len;

// A report's payload size without its ID, 0 for an unknown ID.
int lamparray_report_len(uint8_t id);

// Fills out (LAMPARRAY_REPORT_MAX bytes) with a get-feature report's payload: its length, or
// -ENOTSUP. Reading the attributes response moves on to the next lamp.
int lamparray_get_report(enum lamparray_transport t, uint8_t id, uint8_t *out);

// A set-feature report's payload: 0, -EINVAL (too short) or -ENOTSUP. Autonomous mode off takes
// the lights for t (host_frames_lamparray); on hands them back. Any thread.
int lamparray_set_report(enum lamparray_transport t, uint8_t id, const uint8_t *d, int n);

// A transport says it's up (its interface or service registered), for the state below.
void lamparray_transport_ready(enum lamparray_transport t);

// The LampArray's state for D7 90 (LAMP_PARTS) bytes 12-23 (host_link.h): LAMPARRAY_STATE_SIZE.
#define LAMPARRAY_STATE_AT 12
#define LAMPARRAY_STATE_SIZE 12
void lamparray_state(uint8_t out[LAMPARRAY_STATE_SIZE]);
