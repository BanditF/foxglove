/* Simulator copy of the Raw HID module's events (same names and fields). SPDX-License-Identifier: MIT */
#pragma once

#include <stdint.h>

#include <zmk/event_manager.h>

struct raw_hid_received_event {
    uint8_t *data;
    uint8_t length;
};
ZMK_EVENT_DECLARE(raw_hid_received_event);

struct raw_hid_sent_event {
    uint8_t *data;
    uint8_t length;
};
ZMK_EVENT_DECLARE(raw_hid_sent_event);
