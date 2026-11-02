/* relay-sim: the left half's state around the relay (sim_stubs.c). SPDX-License-Identifier: MIT */
#pragma once

#include "zephyr_sim.h"

struct sim_left {
    bool on;                          // the user's underglow on/off
    enum zmk_activity_state activity; // ZMK's activity
    uint8_t look[5];
    int art_status_events, rgb_events, rgb_followed, dim_events, sends;
    uint8_t last_send[64]; // the last reply host_link_send got (D6 87 SLOT, ...)
    int last_send_len;
    bool snap_done;
    uint8_t snap_status;
    uint16_t snap_frame;
    uint8_t polarity; // the panel polarity the PC set (#666, panel_polarity.c)
    int health_status; // the last D7 13 half 1 read's status (host_link_health_right), -1 none
    int health_replies;
};
extern struct sim_left sim_left;
