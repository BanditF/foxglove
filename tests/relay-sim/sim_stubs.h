/* relay-sim: the left half's state around the relay (sim_stubs.c). SPDX-License-Identifier: MIT */
#pragma once

#include "zephyr_sim.h"

struct sim_left {
    bool on;                          // the user's underglow on/off
    enum zmk_activity_state activity; // ZMK's activity
    uint8_t look[5];
    int art_status_events, rgb_events, rgb_followed, dim_events, sends;
    bool snap_done;
    uint8_t snap_status;
    uint16_t snap_frame;
};
extern struct sim_left sim_left;
