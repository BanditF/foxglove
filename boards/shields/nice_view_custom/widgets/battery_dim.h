/*
 * The underglow's brightness by power state, per half: USB, battery, low battery.
 * SPDX-License-Identifier: MIT
 *
 * Each half caps its own underglow (dim_strip.c) from its own cable and charge: on USB it draws
 * `power` percent of what ZMK draws, on battery `battery` percent, and below `low_below` percent
 * charge `low` percent. ZMK's brightness setting is untouched; this goes on top of it. The levels
 * are saved on each half (nvc_dim/cfg); the left sends its levels to the right with the
 * underglow state (ART_RELAY_RGB), and the PC sets them on the left (D7 0E, host_link.h).
 */

#pragma once

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#include <zephyr/sys/util.h>
#include <zephyr/toolchain.h>

// The levels, as saved and on the wire (D7 0E, ART_RELAY_RGB bytes 3-7, the right's status).
struct battery_dim_config {
    uint8_t enabled;   // 0: `power` everywhere, whatever the cable or charge
    uint8_t power;     // percent on USB power, 1-100
    uint8_t battery;   // percent on battery, 1-100
    uint8_t low;       // percent on battery below low_below, 1-100
    uint8_t low_below; // charge percent, 0-100 (0: no low level)
} __packed;

// One half's state now (D7 8E, the right's status tail).
struct battery_dim_wire {
    uint8_t flags; // BATTERY_DIM_*
    uint8_t level; // percent the underglow is drawn at now
    uint8_t soc;   // charge percent the level went by, 0 = not known yet
    struct battery_dim_config config;
} __packed;

#define BATTERY_DIM_USB BIT(0) // on USB power
#define BATTERY_DIM_LOW BIT(1) // at the low level

// A charge must climb this far over low_below to leave the low level, so a reading that wobbles
// around the threshold doesn't make the lights flicker between two levels.
#define BATTERY_DIM_HYSTERESIS 2

// Whether a config is one the halves take: levels 1-100, threshold 0-100, enabled 0/1.
static inline bool battery_dim_valid(const struct battery_dim_config *c) {
    return c->enabled <= 1 && c->power >= 1 && c->power <= 100 && c->battery >= 1 &&
           c->battery <= 100 && c->low >= 1 && c->low <= 100 && c->low_below <= 100;
}

// The level for a half: soc 0 means "not known yet" (no reading so far), which counts as
// battery, not low. *low says whether it's at the low level, and goes in as the previous one.
static inline uint8_t battery_dim_level_for(const struct battery_dim_config *c, bool usb,
                                            uint8_t soc, bool *low) {
    if (!c->enabled || usb) {
        *low = false;
        return c->power;
    }
    unsigned int leave = c->low_below + (*low ? BATTERY_DIM_HYSTERESIS : 0);
    *low = soc > 0 && soc < leave;
    return *low ? c->low : c->battery;
}

// This half's level now, in percent (dim_strip.c, every frame). Any thread.
uint8_t battery_dim_level(void);

// The cable or charge changed (the event listener; the simulator calls it directly). soc 0 = not
// known. Any thread.
void battery_dim_update(bool usb, uint8_t soc);

// This half's state and levels (for the PC, and the right's status read). Any thread.
void battery_dim_state(struct battery_dim_wire *out);

// New levels: from the PC on the left, from the left on the right. Saved a little later. Returns
// -EINVAL for a config battery_dim_valid() refuses. Any thread.
int battery_dim_set_config(const struct battery_dim_config *config);

// The levels in force (a copy). Any thread.
void battery_dim_get_config(struct battery_dim_config *out);

// The PC draws this half (lamp_strip.c, SET_FRAME and SET_EFFECT): while held, ZMK's own writes
// to the strip are dropped, so its animation (an &rgb_ug key that turns ZMK's underglow back on)
// can't draw over the PC's frame or effect. Any thread.
void dim_strip_hold(bool held);
struct led_rgb;
// lamp_strip.c's writes, which a hold lets through (dimmed as ever).
int dim_strip_update_host(struct led_rgb *pixels, size_t n);
