/*
 * ZMK's underglow effects in the chosen colour (#626): Swirl moves a band of hues around the
 * chosen colour, with a brightness wave, instead of the whole rainbow.
 * SPDX-License-Identifier: MIT
 *
 * ZMK v0.3.0's Swirl gives each LED its own hue (360 / LEDs apart) and turns them all a step per
 * frame; its saturation and brightness are the chosen ones. dim_strip.c, the LED strip between
 * ZMK and the WS2812s, hands each of ZMK's frames here first: while ZMK's effect is Swirl, every
 * LED's hue is read back as its place in the swirl (its phase) and drawn again as the chosen hue
 * plus up to CONFIG_FOXGLOVE_COLOUR_SWIRL_BAND degrees either way, a little dimmer away from the
 * crest. The motion, speed and path are ZMK's own, so &rgb_ug keys, combos, RGB_EFF and the PC's
 * invokes drive it as before. The right half runs the same code on the left's look (rgb_idle.h,
 * #624), so it shows the same band. Solid, Breathe and Spectrum pass through untouched.
 */

#pragma once

#include <stddef.h>
#include <stdint.h>

#include <zephyr/toolchain.h>

struct led_rgb;

// ZMK's effect numbers (rgb_underglow.c, v0.3.0).
#define COLOUR_EFFECT_SOLID 0
#define COLOUR_EFFECT_BREATHE 1
#define COLOUR_EFFECT_SPECTRUM 2
#define COLOUR_EFFECT_SWIRL 3

// The ZMK effects this build draws in the chosen colour, one bit per effect number: D7 83 byte 29
// (HELLO bit 28, host_link.h). Solid and Breathe always; Swirl with CONFIG_FOXGLOVE_COLOUR_SWIRL.
uint8_t colour_effects_mask(void);

// dim_strip.c, ZMK's frames only (the lowest-priority queue): the frame to send. `in` itself,
// or `out` (n LEDs) holding it in the chosen colour. `in` is never written.
struct led_rgb *colour_effects_frame(struct led_rgb *in, struct led_rgb *out, size_t n);

/* Health (the keyboard self-report, #618): a small fixed struct any reader can copy out. */

#define LIGHTING_HEALTH_VERSION 1

#define LIGHTING_HEALTH_COLOUR (1 << 0)     // the effect in the last frame shows the chosen colour
#define LIGHTING_HEALTH_RECOLOURED (1 << 1) // that frame was redrawn in it (colour Swirl)
#define LIGHTING_HEALTH_BUILT (1 << 2)      // CONFIG_FOXGLOVE_COLOUR_SWIRL is built in

struct lighting_health {
    uint8_t version;    // LIGHTING_HEALTH_VERSION
    uint8_t effect;     // ZMK's effect when it last drew (0-3), 0xff before its first frame
    uint8_t flags;      // LIGHTING_HEALTH_*
    uint8_t band;       // degrees of hue either side of the chosen colour (the Kconfig)
    uint16_t hue;       // the chosen hue (0-359) of the last frame
    uint8_t max_offset; // the widest hue offset drawn since boot, degrees: never above band
    uint8_t reserved;
    uint32_t frames;    // ZMK's frames redrawn in the chosen colour since boot
    uint32_t last_ms;   // uptime (ms, wraps) of the last one; 0 when none yet
} __packed;

BUILD_ASSERT(sizeof(struct lighting_health) == 16, "the self-report reads 16 bytes");

// A copy of the counters. Any thread.
void lighting_health_get(struct lighting_health *out);
