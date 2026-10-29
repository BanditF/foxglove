/*
 * Left half: what an RGB ring of the right's doorbell (art_relay.h, #478, #605) tells it. No
 * Bluetooth in here, so the ZMK simulator builds it (tests/zmk-sim/rgb-state); the doorbell's
 * notify callback in art_relay_central.c calls it.
 * SPDX-License-Identifier: MIT
 */

#pragma once

#include <stdbool.h>
#include <stdint.h>
#include <string.h>

#include <zephyr/sys/atomic.h>

#include "rgb_idle.h"

#define ART_RELAY_RING_CARRIED (1 << 0) // the ring's rgb went into *rgb: no status read needed
#define ART_RELAY_RING_TELL (1 << 1)    // the right's on/off or lit changed: D7 83 is due

/* An RGB ring: byte 0 the ART_RELAY_RANG_* bits, then struct rgb_idle_wire and struct
 * rgb_shown_wire (a right before #605 sends byte 0 only: nothing is taken, 0 is returned).
 * The shown block needs nothing else, so it is taken whenever it comes (#618): the ring that
 * answers the left's subscribe on a new link comes before the left's first status read there,
 * and the right sends no other ring until its LEDs go lit or dark, so a shown block dropped then
 * left RGB_STATE without the right's shown state for the rest of that lit or dark spell. The
 * rgb is taken only when the left already knows the right's status (rgb_known), else a status
 * read brings it with the rest. */
static inline uint8_t art_relay_ring_rgb(const uint8_t *d, uint16_t len, bool rgb_known,
                                         struct rgb_idle_wire *rgb, struct rgb_shown_wire *shown,
                                         atomic_t *shown_valid) {
    if (len < 1 + sizeof(struct rgb_idle_wire) + sizeof(struct rgb_shown_wire)) {
        return 0;
    }
    uint8_t result = 0;
    bool was_lit = atomic_get(shown_valid) && (shown->flags & RGB_SHOWN_LIT);
    memcpy(shown, &d[1 + sizeof(*rgb)], sizeof(*shown));
    atomic_set(shown_valid, 1);
    if (was_lit != !!(shown->flags & RGB_SHOWN_LIT)) {
        result |= ART_RELAY_RING_TELL;
    }
    if (rgb_known) {
        uint8_t was_on = rgb->on;
        memcpy(rgb, &d[1], sizeof(*rgb));
        result |= ART_RELAY_RING_CARRIED;
        if (rgb->on != was_on) {
            result |= ART_RELAY_RING_TELL;
        }
    }
    return result;
}
