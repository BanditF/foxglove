/*
 * This half's underglow strip under the PC's control (SET_FRAME, #272), on either half.
 * SPDX-License-Identifier: MIT
 */

#pragma once

#include <stdbool.h>
#include <stdint.h>

#include <zephyr/devicetree.h>

#ifndef LAMP_STRIP_LEN // the simulator, which has no strip, sets it
#define LAMP_STRIP_LEN DT_PROP(DT_CHOSEN(zmk_underglow), chain_length)
#endif

// on: ZMK's animation and rgb_idle step aside and lamp_strip_show() draws; off hands the strip
// back to ZMK's own effect (and the user's on/off). Any thread.
void lamp_strip_host(bool on);
void lamp_strip_set(int index, uint8_t r, uint8_t g, uint8_t b);
// Writes the strip soon (ZMK's low-priority queue), if the PC has it. Any thread. The strip goes
// through dim_strip.c, so a new battery dim level (#465) is a lamp_strip_show() too.
void lamp_strip_show(void);
