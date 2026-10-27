/*
 * How many LEDs each part of the board has (#561): this half's own chain (its devicetree's
 * underglow), and on the left the right half's as the left frames them. Nothing here assumes a
 * count: SET_FRAME's lamps, SET_EFFECT's tables and the LAMP_PARTS report (D7 10) all come from
 * these.
 * SPDX-License-Identifier: MIT
 */

#pragma once

#include <zephyr/devicetree.h>
#include <zephyr/sys/util.h>

// This half's LEDs: its underglow chain, or what the simulator (no strip) sets.
#if defined(LAMP_STRIP_LEN)
#define LAMP_PARTS_OWN LAMP_STRIP_LEN
#elif DT_HAS_CHOSEN(zmk_underglow)
#define LAMP_PARTS_OWN DT_PROP(DT_CHOSEN(zmk_underglow), chain_length)
#else
#define LAMP_PARTS_OWN 0
#endif

// The right half's LEDs, as the left frames and relays them: CONFIG_FOXGLOVE_RIGHT_LAMPS for
// halves that differ, else as many as this half's own (halves that mirror each other). On the
// right it is its own chain.
#if defined(CONFIG_FOXGLOVE_RIGHT_LAMPS) && CONFIG_FOXGLOVE_RIGHT_LAMPS > 0 &&                     \
    (!IS_ENABLED(CONFIG_ZMK_SPLIT) || IS_ENABLED(CONFIG_ZMK_SPLIT_ROLE_CENTRAL))
#define LAMP_PARTS_RIGHT CONFIG_FOXGLOVE_RIGHT_LAMPS
#else
#define LAMP_PARTS_RIGHT LAMP_PARTS_OWN
#endif

// The most LEDs either half has: what a per-half table holds (at least one, so no array is
// empty on a build without underglow).
#define LAMP_PARTS_MAX MAX(1, MAX(LAMP_PARTS_OWN, LAMP_PARTS_RIGHT))
