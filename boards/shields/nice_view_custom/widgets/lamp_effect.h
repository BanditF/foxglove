/*
 * SET_EFFECT (D7 0F, #490): the effect maths, the same as the desktop's crates/scene (effects.rs,
 * space.rs, colour.rs), so each half draws its own LEDs from their positions and the shared clock.
 * The maths is pure C: the simulator checks it against crates/scene's vectors
 * (tests/zmk-sim/module/effect_vectors.inc, written by crates/scene/tests/keyboard_effect_vectors.rs).
 * Its tables hold as many LEDs as the board's halves have (lamp_parts.h, #561).
 * SPDX-License-Identifier: MIT
 */

#pragma once

#include <stdbool.h>
#include <stdint.h>

#include "lamp_parts.h"

enum {
    LAMP_EFFECT_NONE = 0,
    LAMP_EFFECT_SCANLINE = 1, // colours: colour, background
    LAMP_EFFECT_SWEEP = 2,    // colours: the stops (1-4), scrolling, wrapping
    LAMP_EFFECT_RIPPLE = 3,   // colours: colour, background; grows from origin
    LAMP_EFFECT_BREATHE = 4,  // colours: from, to
    LAMP_EFFECT_KINDS,
};

#define LAMP_EFFECT_COLOURS 4

// Lamps per half the tables hold: the most either half has (lamp_parts.h). A half draws its own.
#define LAMP_EFFECT_LAMPS LAMP_PARTS_MAX
// A lamp position the PC hasn't seen (EFFECT op LAMPS, #550): the half's centre.
#define LAMP_EFFECT_UNSEEN 0xFFFF

// One effect for one half, as the left keeps it and as it goes to the right over the relay
// (art_relay.h ART_RELAY_EFFECT). All little endian; mm are room millimetres (crates/scene's
// space.rs: x to the right, y down).
struct lamp_effect_wire {
    uint8_t kind;          // LAMP_EFFECT_*
    uint8_t n;             // colours used, 1-4
    uint8_t period_ms[3];  // u24; 0 stands still at phase 0
    uint8_t angle_cdeg[2]; // u16, hundredths of a degree, 0-35999: 0 left to right, 90 down
    uint8_t width_dmm[2];  // u16, tenths of a mm: the band's or the ring's width
    uint8_t origin[4];     // i16 x, y (mm): the ripple's centre
    uint8_t rgb[LAMP_EFFECT_COLOURS][3];
    uint8_t room[8];       // i16 x, y, w, h (mm): the room's bounds
    uint8_t half[8];       // i16 x, y, w, h (mm): this half in the room
    // Appended for EFFECT op LAMPS (#550); zero from a left without it. lamps_set: lamp holds
    // this half's own points (u16 x, y in thousandths of the half's width and height from its
    // top-left, LAMP_EFFECT_UNSEEN for its centre; never mirrored). Else the built-in table.
    uint8_t lamps_set;
    uint8_t lamp[LAMP_EFFECT_LAMPS][4];
} __attribute__((packed));

// The prepared effect: floats worked out once per SET_EFFECT, so a frame is a few sums a lamp.
struct lamp_effect {
    uint8_t kind, n;
    uint32_t period_ms;
    float c, s;         // cos, sin of the direction
    float lo, hi, span; // the room's corners projected on the direction; its extent along it
    float width;        // mm, >= 0
    float ox, oy;       // ripple origin
    float reach;        // ripple: the farthest room corner from the origin
    float lab[LAMP_EFFECT_COLOURS][3];
    uint8_t rgb[LAMP_EFFECT_COLOURS][3];
    float lamp[LAMP_EFFECT_LAMPS][2]; // each lamp's room position
};

// The built-in (provisional) table's lamp i, in the wire's thousandths; mirror: the right half.
// The table is the Eyelash Corne's 21; a lamp past it sits at the half's centre until the PC
// places it (EFFECT op LAMPS).
void lamp_effect_table(int i, bool mirror, uint16_t out[2]);

// Fills fx from w. mirror: the right half, whose keys (and LEDs) are the left's mirror image
// (the built-in table only: the PC's own points, lamps_set, are the half's as they are).
// Returns false (fx untouched) for a kind or colour count it doesn't know. w must not be in fx.
bool lamp_effect_prepare(struct lamp_effect *fx, const struct lamp_effect_wire *w, bool mirror);

// crates/scene's SceneTime::phase: (t mod period) / period, 0 for period 0.
float lamp_effect_phase(uint64_t t_ms, uint32_t period_ms);

// The colour at a room point at a phase (crates/scene Effect::colour).
void lamp_effect_at(const struct lamp_effect *fx, float x, float y, float phase, uint8_t out[3]);

// Every lamp of the half.
void lamp_effect_render(const struct lamp_effect *fx, float phase,
                        uint8_t out[LAMP_EFFECT_LAMPS][3]);

// Whether the effect looks the same at every phase (period 0): drawn once, no frame timer.
bool lamp_effect_still(const struct lamp_effect *fx);
