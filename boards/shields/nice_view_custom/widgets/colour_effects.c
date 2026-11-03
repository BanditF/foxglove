/*
 * ZMK's underglow effects in the chosen colour (#626); see colour_effects.h.
 * SPDX-License-Identifier: MIT
 */

#include <stdlib.h>

#include <zephyr/drivers/led_strip.h>
#include <zephyr/kernel.h>
#include <zephyr/spinlock.h>
#include <zephyr/sys/util.h>

#include <zmk/rgb_underglow.h>

#include "colour_effects.h"

#if IS_ENABLED(CONFIG_FOXGLOVE_COLOUR_SWIRL)
#define BAND CONFIG_FOXGLOVE_COLOUR_SWIRL_BAND
#define DEPTH CONFIG_FOXGLOVE_COLOUR_SWIRL_DEPTH
#else
#define BAND 0
#define DEPTH 0
#endif

// Below this saturation (percent) the LEDs' hues are too close to white to read the swirl's
// phase back from them; the phase then runs on at the speed last read.
#define PHASE_SAT_MIN 10
// ZMK turns its swirl by speed * 2 degrees a frame (speed 1-5); a jump beyond is a restart.
#define STEP_MAX 10
#ifdef CONFIG_ZMK_RGB_UNDERGLOW_SPD_START
#define STEP_START (2 * CONFIG_ZMK_RGB_UNDERGLOW_SPD_START)
#else
#define STEP_START 6
#endif

uint8_t colour_effects_mask(void) {
    return BIT(COLOUR_EFFECT_SOLID) | BIT(COLOUR_EFFECT_BREATHE) |
           (IS_ENABLED(CONFIG_FOXGLOVE_COLOUR_SWIRL) ? BIT(COLOUR_EFFECT_SWIRL) : 0);
}

static struct lighting_health health = {
    .version = LIGHTING_HEALTH_VERSION,
    .effect = 0xff,
    .flags = IS_ENABLED(CONFIG_FOXGLOVE_COLOUR_SWIRL) ? LIGHTING_HEALTH_BUILT : 0,
    .band = BAND,
};
static struct k_spinlock health_lock;

void lighting_health_get(struct lighting_health *out) {
    k_spinlock_key_t key = k_spin_lock(&health_lock);
    *out = health;
    k_spin_unlock(&health_lock, key);
}

#if IS_ENABLED(CONFIG_FOXGLOVE_COLOUR_SWIRL)

// The hue (0-359, rounded) of an LED; 0 for a grey one.
static uint16_t hue_of(const struct led_rgb *p) {
    int mx = MAX(p->r, MAX(p->g, p->b)), mn = MIN(p->r, MIN(p->g, p->b)), d = mx - mn;
    if (d == 0) {
        return 0;
    }
    int h = mx == p->r ? 60 * (p->g - p->b)
            : mx == p->g ? 120 * d + 60 * (p->b - p->r)
                         : 240 * d + 60 * (p->r - p->g);
    h = (h + (h < 0 ? -d / 2 : d / 2)) / d;
    return (h + 360) % 360;
}

// hue 0-359, saturation 0-100, value 0-255.
static struct led_rgb from_hsv(uint16_t h, uint8_t s, uint8_t v) {
    uint32_t f = (h % 60) * 255 / 60; // 0-255 across the sixth
    uint8_t p = v * (100 - s) / 100;
    uint8_t q = v * (100 * 255 - s * f) / (100 * 255);
    uint8_t t = v * (100 * 255 - s * (255 - f)) / (100 * 255);
    switch (h / 60) {
    case 0:
        return (struct led_rgb){.r = v, .g = t, .b = p};
    case 1:
        return (struct led_rgb){.r = q, .g = v, .b = p};
    case 2:
        return (struct led_rgb){.r = p, .g = v, .b = t};
    case 3:
        return (struct led_rgb){.r = p, .g = q, .b = v};
    case 4:
        return (struct led_rgb){.r = t, .g = p, .b = v};
    default:
        return (struct led_rgb){.r = v, .g = p, .b = q};
    }
}

// The swirl's phase at LED 0 and how far it turns a frame, for frames too pale to read.
static uint16_t phase0;
static uint8_t step = STEP_START;
static bool phase_known;

static void swirl(const struct led_rgb *in, struct led_rgb *out, size_t n,
                  struct zmk_led_hsb chosen, uint8_t *widest) {
    bool readable = chosen.s >= PHASE_SAT_MIN;
    if (readable) {
        uint16_t p0 = hue_of(&in[0]);
        uint16_t turned = (p0 + 360 - phase0) % 360;
        if (phase_known && turned >= 1 && turned <= STEP_MAX) {
            step = turned;
        }
        phase0 = p0;
        phase_known = true;
    } else {
        phase0 = (phase0 + step) % 360;
    }
    for (size_t i = 0; i < n; i++) {
        uint8_t v = MAX(in[i].r, MAX(in[i].g, in[i].b));
        if (v == 0) {
            out[i] = (struct led_rgb){0};
            continue;
        }
        uint16_t phase = readable ? hue_of(&in[i]) : (phase0 + 360 * i / n) % 360;
        // 0 at the swirl's red, 180 opposite it: a hue band and a brightness wave that run
        // back and forth along ZMK's path instead of round the colour wheel.
        int tri = phase <= 180 ? phase : 360 - phase;
        int offset = (BAND * (2 * tri - 180) + (tri >= 90 ? 90 : -90)) / 180;
        uint16_t hue = (chosen.h % 360 + 360 + offset) % 360;
        uint8_t level = MAX(v * (100 * 180 - DEPTH * (180 - tri)) / (100 * 180), 1);
        out[i] = from_hsv(hue, MIN(chosen.s, 100), level);
        *widest = MAX(*widest, (uint8_t)abs(offset));
    }
}

#endif // CONFIG_FOXGLOVE_COLOUR_SWIRL

struct led_rgb *colour_effects_frame(struct led_rgb *in, struct led_rgb *out, size_t n) {
    int effect = zmk_rgb_underglow_calc_effect(0);
    struct zmk_led_hsb chosen = zmk_rgb_underglow_calc_hue(0);
    struct led_rgb *frame = in;
    uint8_t widest = 0;
#if IS_ENABLED(CONFIG_FOXGLOVE_COLOUR_SWIRL)
    if (effect == COLOUR_EFFECT_SWIRL && n > 0) {
        swirl(in, out, n, chosen, &widest);
        frame = out;
    }
#endif
    k_spinlock_key_t key = k_spin_lock(&health_lock);
    health.effect = effect;
    health.hue = chosen.h;
    health.flags &= LIGHTING_HEALTH_BUILT;
    if (colour_effects_mask() & BIT(effect & 7)) {
        health.flags |= LIGHTING_HEALTH_COLOUR;
    }
    if (frame == out) {
        health.flags |= LIGHTING_HEALTH_RECOLOURED;
        health.frames++;
        health.last_ms = MAX(k_uptime_get_32(), 1);
        health.max_offset = MAX(health.max_offset, widest);
    }
    k_spin_unlock(&health_lock, key);
    return frame;
}
