/*
 * Simulator case for the colour Swirl (tests/zmk-sim/colour-swirl, #626): the real dim_strip.c,
 * battery_dim.c, colour_effects.c, rgb_idle.c and host_link.c, with ZMK's underglow faked here
 * and a fake strip at the end that keeps the frames it gets (what the LEDs show).
 *
 * The fake underglow draws its frames the way ZMK v0.3.0's rgb_underglow.c does (its HSB to RGB
 * conversion and its Swirl, the full rainbow: 360 / 21 degrees between LEDs, speed * 2 a
 * frame), through the chosen zmk,underglow, which is dim_strip.c. The case checks every LED of
 * every frame against the chosen colour: within CONFIG_FOXGLOVE_COLOUR_SWIRL_BAND degrees
 * (plus 1 for rounding), reaching both edges of the band, moving every frame, while ZMK's own
 * hues span the whole wheel. Then the right: a half with an older look takes the left's from
 * the relay's RGB op (rgb_idle.c, #624) and draws the same frames. Spectrum, Solid and Breathe
 * pass through untouched; D7 83 byte 29 and HELLO bit 29 say what the left draws in colour;
 * the lighting health counters (colour_effects.h) count the frames.
 * SPDX-License-Identifier: MIT
 */
#define DT_DRV_COMPAT foxglove_sim_strip

#include <stdlib.h>
#include <string.h>

#include <zephyr/device.h>
#include <zephyr/drivers/led_strip.h>
#include <zephyr/kernel.h>
#include <zephyr/sys/byteorder.h>
#include <zephyr/sys/printk.h>

#include <raw_hid/events.h>
#include <zmk/event_manager.h>
#include <zmk/rgb_underglow.h>

#include "battery_dim.h"
#include "colour_effects.h"
#include "rgb_idle.h"

#define LEDS 21
#define FRAMES 60 // one turn of ZMK's swirl at speed 3 (6 degrees a frame)
#define SLACK 2   // degrees of rounding between the hue drawn and the 8-bit LEDs read back

static void note(const char *what) { printk("swirl: -- %s\n", what); }

/* The strip at the end: the last frame it got */

static struct led_rgb shown[LEDS];
static int shown_n;

static int sim_update_rgb(const struct device *dev, struct led_rgb *pixels, size_t n) {
    memcpy(shown, pixels, MIN(n, LEDS) * sizeof(*pixels));
    shown_n = n;
    return 0;
}

static int sim_update_channels(const struct device *dev, uint8_t *channels, size_t n) {
    return -ENOTSUP;
}

static const struct led_strip_driver_api sim_api = {
    .update_rgb = sim_update_rgb,
    .update_channels = sim_update_channels,
};

DEVICE_DT_INST_DEFINE(0, NULL, NULL, NULL, NULL, POST_KERNEL, CONFIG_KERNEL_INIT_PRIORITY_DEFAULT,
                      &sim_api);

static const struct device *const underglow = DEVICE_DT_GET(DT_CHOSEN(zmk_underglow));

// What battery_dim.c reads before its first event.
bool battery_dim_read_usb(void) { return false; }
uint8_t battery_dim_read_soc(void) { return 0; }

/* ZMK's underglow (rgb_underglow.c v0.3.0, MIT, The ZMK Contributors): its state and frames */

static struct {
    struct zmk_led_hsb color;
    uint8_t speed;
    uint8_t effect;
    uint16_t step;
    bool on;
} state = {.color = {.h = 39, .s = 70, .b = 42}, .speed = 3, .effect = 3, .on = true};
static int hsb_writes, effect_writes;

// ZMK's hsb_to_rgb, as it is.
static struct led_rgb hsb_to_rgb(struct zmk_led_hsb hsb) {
    float r = 0, g = 0, b = 0;
    uint8_t i = hsb.h / 60;
    float v = hsb.b / 100.0f;
    float s = hsb.s / 100.0f;
    float f = hsb.h / 360.0f * 6 - i;
    float p = v * (1 - s);
    float q = v * (1 - f * s);
    float t = v * (1 - (1 - f) * s);
    switch (i % 6) {
    case 0:
        r = v, g = t, b = p;
        break;
    case 1:
        r = q, g = v, b = p;
        break;
    case 2:
        r = p, g = v, b = t;
        break;
    case 3:
        r = p, g = q, b = v;
        break;
    case 4:
        r = t, g = p, b = v;
        break;
    case 5:
        r = v, g = p, b = q;
        break;
    }
    return (struct led_rgb){.r = r * 255, .g = g * 255, .b = b * 255};
}

static struct led_rgb zmk_pixels[LEDS];

// One tick of ZMK's underglow (BRT_MIN 0, BRT_MAX 100: the defaults): Solid, Breathe,
// Spectrum or Swirl into its pixels, then to the chosen strip.
static void zmk_tick(void) {
    for (int i = 0; i < LEDS; i++) {
        struct zmk_led_hsb hsb = state.color;
        switch (state.effect) {
        case 1:
            hsb.b = abs(state.step - 1200) / 12;
            break;
        case 2:
            hsb.h = state.step;
            break;
        case 3:
            hsb.h = (360 / LEDS * i + state.step) % 360;
            break;
        }
        zmk_pixels[i] = hsb_to_rgb(hsb);
    }
    switch (state.effect) {
    case 1:
        state.step += state.speed * 10;
        state.step = state.step > 2400 ? 0 : state.step;
        break;
    case 2:
        state.step = (state.step + state.speed) % 360;
        break;
    case 3:
        state.step = (state.step + state.speed * 2) % 360;
        break;
    }
    led_strip_update_rgb(underglow, zmk_pixels, LEDS);
}

int zmk_rgb_underglow_get_state(bool *on) {
    *on = state.on;
    return 0;
}
int zmk_rgb_underglow_on(void) {
    state.on = true;
    return 0;
}
int zmk_rgb_underglow_off(void) {
    state.on = false;
    return 0;
}
struct zmk_led_hsb zmk_rgb_underglow_calc_hue(int direction) {
    struct zmk_led_hsb c = state.color;
    c.h = (c.h + 360 + direction * 10) % 360; // ZMK's HUE_STEP default
    return c;
}
int zmk_rgb_underglow_calc_effect(int direction) { return (state.effect + 4 + direction) % 4; }
int zmk_rgb_underglow_set_hsb(struct zmk_led_hsb color) {
    state.color = color;
    hsb_writes++;
    return 0;
}
int zmk_rgb_underglow_select_effect(int e) {
    state.effect = e;
    state.step = 0;
    effect_writes++;
    return 0;
}

/* The relay: the RGB op the right would be sent (art_relay_central.c's rgb op) */

static uint8_t op_look[RGB_IDLE_LOOK_SIZE];

void art_relay_rgb_kick(void) { rgb_idle_look(op_look); }

/* The PC: HELLO and D7 83 */

static uint8_t hello[32], rgb83[32];
static bool want_83, want_hello;

static int sent_listener(const zmk_event_t *eh) {
    const struct raw_hid_sent_event *ev = as_raw_hid_sent_event(eh);
    if (ev == NULL || ev->length < 2) {
        return ZMK_EV_EVENT_BUBBLE;
    }
    if (want_hello && ev->data[0] == 0xD5 && ev->data[1] == 0x81) {
        memcpy(hello, ev->data, MIN(ev->length, sizeof(hello)));
        want_hello = false;
    }
    if (want_83 && ev->data[0] == 0xD7 && ev->data[1] == 0x83) {
        memcpy(rgb83, ev->data, MIN(ev->length, sizeof(rgb83)));
        want_83 = false;
    }
    return ZMK_EV_EVENT_BUBBLE;
}

ZMK_LISTENER(colour_swirl_sim, sent_listener);
ZMK_SUBSCRIPTION(colour_swirl_sim, raw_hid_sent_event);

static void put(const uint8_t *d, int len) {
    static uint8_t report[32];
    memset(report, 0, sizeof(report));
    memcpy(report, d, len);
    raise_raw_hid_received_event((struct raw_hid_received_event){.data = report, .length = 32});
    k_msleep(200); // replies go out from the system work queue
}

/* The checks */

static int hue_of(const struct led_rgb *p, int *sat) {
    int mx = MAX(p->r, MAX(p->g, p->b)), mn = MIN(p->r, MIN(p->g, p->b)), d = mx - mn;
    *sat = mx == 0 ? 0 : (d * 100 + mx / 2) / mx;
    if (d == 0) {
        return 0;
    }
    int h = mx == p->r ? 60 * (p->g - p->b) : mx == p->g ? 120 * d + 60 * (p->b - p->r)
                                                         : 240 * d + 60 * (p->r - p->g);
    h = (h + (h < 0 ? -d / 2 : d / 2)) / d;
    return (h + 360) % 360;
}

static int apart(int a, int b) { // signed, -180..179
    return ((a - b) % 360 + 540) % 360 - 180;
}

static const char *yes(bool ok) { return ok ? "yes" : "NO"; }

static struct led_rgb frames[FRAMES][LEDS]; // the left's, to hold the right's against

// Draws FRAMES of ZMK's Swirl from step 0 and checks each LED against the chosen colour.
static void run_swirl(const char *name, int slack, bool keep) {
    state.effect = 3;
    state.step = 0;
    int lo = 0, hi = 0, outside = 0, still = 0, unlit = 0, grey = 0;
    int zmk_lo = 359, zmk_hi = 0, level_lo = 255, level_hi = 0;
    bool untouched = true;
    struct led_rgb prev[LEDS] = {0};
    for (int f = 0; f < FRAMES; f++) {
        zmk_tick();
        struct led_rgb copy[LEDS];
        memcpy(copy, zmk_pixels, sizeof(copy));
        for (int i = 0; i < LEDS; i++) {
            int s, h = hue_of(&shown[i], &s), zs;
            int zh = hue_of(&zmk_pixels[i], &zs);
            zmk_lo = MIN(zmk_lo, zh);
            zmk_hi = MAX(zmk_hi, zh);
            int v = MAX(shown[i].r, MAX(shown[i].g, shown[i].b));
            level_lo = MIN(level_lo, v);
            level_hi = MAX(level_hi, v);
            unlit += v == 0;
            if (state.color.s == 0) {
                grey += shown[i].r == shown[i].g && shown[i].g == shown[i].b;
                continue;
            }
            int off = apart(h, state.color.h);
            lo = MIN(lo, off);
            hi = MAX(hi, off);
            outside += abs(off) > CONFIG_FOXGLOVE_COLOUR_SWIRL_BAND + slack;
        }
        still += f > 0 && memcmp(prev, shown, sizeof(prev)) == 0;
        memcpy(prev, shown, sizeof(prev));
        untouched &= memcmp(copy, zmk_pixels, sizeof(copy)) == 0;
        if (keep) {
            memcpy(frames[f], shown, sizeof(frames[f]));
        }
    }
    int band = CONFIG_FOXGLOVE_COLOUR_SWIRL_BAND;
    printk("swirl: %s (hue %d, sat %d, bri %d): %d frames of %d LEDs\n", name, state.color.h,
           state.color.s, state.color.b, FRAMES, LEDS);
    if (state.color.s == 0) {
        printk("swirl: %s: ZMK's own frame is all one white: %s\n", name, yes(zmk_hi == 0));
    } else {
        printk("swirl: %s: ZMK's own hues span the wheel (over 300 degrees): %s\n", name,
               yes(zmk_hi - zmk_lo > 300));
    }
    if (state.color.s == 0) {
        printk("swirl: %s: every LED white (no hue): %s\n", name, yes(grey == FRAMES * LEDS));
    } else {
        printk("swirl: %s: every LED within %d+%d degrees of the colour: %s (%d outside)\n", name,
               band, slack, yes(outside == 0), outside);
        printk("swirl: %s: the band reaches both edges (%d degrees or more each way): %s\n", name,
               band - 3, yes(lo <= -(band - 3) && hi >= band - 3));
    }
    printk("swirl: %s: a brightness wave (trough at most %d%% of the crest): %s; no LED dark: "
           "%s\n",
           name, 100 - CONFIG_FOXGLOVE_COLOUR_SWIRL_DEPTH + 5,
           yes(level_lo * 100 <= level_hi * (100 - CONFIG_FOXGLOVE_COLOUR_SWIRL_DEPTH + 5)),
           yes(unlit == 0));
    printk("swirl: %s: it moves every frame: %s; ZMK's frame unchanged: %s\n", name,
           yes(still == 0), yes(untouched));
}

// One frame of `effect` passes through as ZMK drew it.
static void pass_through(int effect, const char *name) {
    state.effect = effect;
    state.step = effect == 1 ? 600 : 90;
    zmk_tick();
    printk("swirl: %s: the LEDs show ZMK's own frame: %s\n", name,
           yes(memcmp(shown, zmk_pixels, sizeof(shown)) == 0));
}

static void health(const char *when) {
    struct lighting_health h;
    lighting_health_get(&h);
    printk("swirl: health, %s: version %d, effect %d, flags %d (colour %d, recoloured %d, built "
           "%d), band %d, hue %d, widest offset within the band %s, frames %u, last apply %s\n",
           when, h.version, h.effect, h.flags, !!(h.flags & LIGHTING_HEALTH_COLOUR),
           !!(h.flags & LIGHTING_HEALTH_RECOLOURED), !!(h.flags & LIGHTING_HEALTH_BUILT), h.band,
           h.hue,
           yes(h.frames == 0 ? h.max_offset == 0
                             : h.max_offset <= h.band && h.max_offset >= h.band - 3),
           h.frames,
           h.frames == 0 ? (h.last_ms == 0 ? "none" : "SET") : (h.last_ms ? "set" : "NONE"));
}

static void read_83(const char *when) {
    const uint8_t d[] = {0xD7, 0x03};
    want_83 = true;
    put(d, sizeof(d));
    int hue = sys_get_le16(&rgb83[18]);
    printk("swirl: D7 83, %s: effect %d, byte 29 0x%02X; shown hue within %d+%d of %d: %s\n",
           when, rgb83[7], rgb83[29], CONFIG_FOXGLOVE_COLOUR_SWIRL_BAND, SLACK + 1,
           state.color.h,
           yes(hue != 0xffff &&
               abs(apart(hue, state.color.h)) <= CONFIG_FOXGLOVE_COLOUR_SWIRL_BAND + SLACK + 1));
}

void colour_swirl_sim_driver(void) {
    k_msleep(500);
    note("hello: capability bit 29");
    {
        const uint8_t d[] = {0xD5, 0x01, 0x01};
        want_hello = true;
        put(d, sizeof(d));
        printk("swirl: HELLO caps bit 29 COLOUR_EFFECTS %d\n",
               (sys_get_le32(&hello[21]) >> 29) & 1);
    }
    battery_dim_update(true, 100); // on USB: full level, so the hues aren't rounded twice
    printk("swirl: strip ready %d, dim level %d\n", device_is_ready(underglow),
           battery_dim_level());
    health("before any frame");

    note("the user's amber (#d39f3f): a band around it, never the rainbow");
    run_swirl("amber", SLACK, true);
    health("after the amber swirl");
    read_83("amber swirl");

    note("red, across 0: the band wraps round");
    state.color = (struct zmk_led_hsb){.h = 350, .s = 100, .b = 80};
    run_swirl("red", SLACK, false);

    note("white: no hue to band, the brightness wave still moves");
    state.color = (struct zmk_led_hsb){.h = 200, .s = 0, .b = 50};
    run_swirl("white", SLACK, false);

    note("a key: RGB_HUI turns the colour, and the band follows it");
    state.color = (struct zmk_led_hsb){.h = 39, .s = 70, .b = 42};
    state.color = zmk_rgb_underglow_calc_hue(1); // what &rgb_ug RGB_HUI sets
    run_swirl("after RGB_HUI", SLACK, false);

    note("on battery (80%): the dim keeps the band");
    state.color = (struct zmk_led_hsb){.h = 39, .s = 70, .b = 42};
    battery_dim_update(false, 60);
    printk("swirl: dim level %d\n", battery_dim_level());
    run_swirl("amber at 80%", SLACK + 2, false);
    battery_dim_update(true, 100);

    note("Spectrum stays the rainbow; Solid and Breathe are ZMK's own");
    struct lighting_health before;
    lighting_health_get(&before);
    pass_through(2, "spectrum");
    health("spectrum");
    pass_through(0, "solid");
    health("solid");
    pass_through(1, "breathe");
    {
        struct lighting_health h;
        lighting_health_get(&h);
        printk("swirl: no frame counted outside Swirl: %s\n", yes(h.frames == before.frames));
    }

    note("the right: an older look, then the left's from the relay's RGB op (#624)");
    {
        state.color = (struct zmk_led_hsb){.h = 39, .s = 70, .b = 42};
        state.effect = 3;
        rgb_idle_check(); // as after a key or an invoke: the look goes to the relay
        k_msleep(200);
        printk("swirl: the right is sent: hue %d sat %d bri %d effect %d\n",
               sys_get_le16(op_look), op_look[2], op_look[3], op_look[4]);
        uint8_t look[RGB_IDLE_LOOK_SIZE];
        memcpy(look, op_look, sizeof(look));
        state.color = (struct zmk_led_hsb){.h = 4, .s = 100, .b = 9}; // the right's saved look
        state.effect = 2;
        hsb_writes = effect_writes = 0;
        rgb_idle_remote_look(look);
        printk("swirl: the right takes hue %d sat %d bri %d effect %d (%d hsb, %d effect writes)\n",
               state.color.h, state.color.s, state.color.b, state.effect, hsb_writes,
               effect_writes);
        static struct led_rgb left[FRAMES][LEDS];
        memcpy(left, frames, sizeof(left));
        run_swirl("the right", SLACK, true);
        int differ = 0;
        for (int f = 0; f < FRAMES; f++) {
            for (int i = 0; i < LEDS; i++) {
                differ += memcmp(&left[f][i], &frames[f][i], sizeof(struct led_rgb)) != 0;
            }
        }
        printk("swirl: the right's frames match the left's: %s (%d LEDs differ)\n",
               yes(differ == 0), differ);
    }
    health("at the end");
    note("done");
}
