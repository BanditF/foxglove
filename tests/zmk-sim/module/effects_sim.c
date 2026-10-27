/*
 * Simulator case for SET_EFFECT (tests/zmk-sim/host-effects, #490): the real host_link.c,
 * host_effects.c, effect_run.c, lamp_effect.c and host_frames.c, with this half's strip, the art
 * relay and the underglow's on/off faked here.
 *
 * First the maths: every vector crates/scene wrote (effect_vectors.inc) through lamp_effect.c,
 * within 2 per channel. Then the PC's side: PLACE, EFFECT and RELEASE over the host link, a still
 * effect drawn once, a moving one drawn every frame, idle going dark and coming back, SET_FRAME
 * taking over and an effect taking back, the bad requests, and the effect living through an
 * endpoint change.
 * SPDX-License-Identifier: MIT
 */

#include <stdlib.h>
#include <string.h>

#include <zephyr/kernel.h>
#include <zephyr/sys/byteorder.h>
#include <zephyr/sys/printk.h>

#include <raw_hid/events.h>
#include <zmk/event_manager.h>
#include <zmk/events/endpoint_changed.h>

#include "art_relay.h"
#include "effect_run.h"
#include "lamp_effect.h"
#include "lamp_strip.h"
#include "rgb_idle.h"

/* Runs of one colour, black left out: "0-20=204060". */
static void print_lamps(const char *what, const uint8_t rgb[][3], int n) {
    printk("effects: %s", what);
    bool any = false;
    for (int i = 0; i < n;) {
        int j = i;
        while (j + 1 < n && memcmp(rgb[j + 1], rgb[i], 3) == 0) {
            j++;
        }
        if (rgb[i][0] | rgb[i][1] | rgb[i][2]) {
            any = true;
            if (j > i) {
                printk(" %d-%d=", i, j);
            } else {
                printk(" %d=", i);
            }
            printk("%02X%02X%02X", rgb[i][0], rgb[i][1], rgb[i][2]);
        }
        i = j + 1;
    }
    printk(any ? "\n" : " black\n");
}

/* The left strip (lamp_strip.c): draws are counted; a draw that differs from the last printed
 * one is printed unless the case is counting a moving effect. */

static uint8_t pixels[LAMP_STRIP_LEN][3], printed[LAMP_STRIP_LEN][3];
static bool host, quiet;
static atomic_t draws;

void lamp_strip_host(bool on) {
    if (on != host) {
        host = on;
        memset(pixels, 0, sizeof(pixels));
        memset(printed, 0xff, sizeof(printed));
        printk("effects: left %s\n", on ? "taken from its effect" : "back to its effect");
    }
}

void lamp_strip_set(int index, uint8_t r, uint8_t g, uint8_t b) {
    if (index >= 0 && index < LAMP_STRIP_LEN) {
        pixels[index][0] = r;
        pixels[index][1] = g;
        pixels[index][2] = b;
    }
}

void lamp_strip_show(void) {
    if (!host) {
        return;
    }
    atomic_inc(&draws);
    if (!quiet && memcmp(pixels, printed, sizeof(pixels)) != 0) {
        memcpy(printed, pixels, sizeof(pixels));
        print_lamps("left draws", pixels, LAMP_STRIP_LEN);
    }
}

/* The underglow's on/off as the user set it (rgb_idle.c) */

static bool lights = true;

int rgb_idle_get_on(bool *on) {
    *on = lights;
    return 0;
}

/* The art relay (art_relay_central.c): prints what the right would be sent */

void art_relay_lamps(bool on, const uint8_t rgb[ART_RELAY_LAMPS_MAX][3]) {
    static bool right_host;
    if (on != right_host) {
        right_host = on;
        printk("effects: right frames %s\n", on ? "on" : "off");
    }
}

uint8_t art_relay_lamps_right(void) { return ART_LAMPS_OK; }

static const char *const kinds[] = {"none", "scanline", "sweep", "ripple", "breathe"};

void art_relay_effect(bool run, bool tell, const struct lamp_effect_wire *w) {
    if (!run) {
        printk("effects: right effect stops%s\n", tell ? ", back to its own" : " (frames take it)");
        return;
    }
    // The relay reads the left's clock when it sends: it is the left's, so the halves agree.
    printk("effects: right gets %s, %d colours, half at %d,%d %dx%d, room %dx%d\n",
           kinds[w->kind < LAMP_EFFECT_KINDS ? w->kind : 0], w->n,
           (int16_t)sys_get_le16(&w->half[0]), (int16_t)sys_get_le16(&w->half[2]),
           (int16_t)sys_get_le16(&w->half[4]), (int16_t)sys_get_le16(&w->half[6]),
           (int16_t)sys_get_le16(&w->room[4]), (int16_t)sys_get_le16(&w->room[6]));
}

uint8_t art_relay_effect_right(void) { return ART_LAMPS_OK; }

/* crates/scene's vectors */

struct vector {
    uint8_t kind, n;
    uint32_t period;
    uint16_t angle, width;
    int16_t origin[2];
    uint8_t rgb[4][3];
    int16_t room[4];
    float x, y;
    uint64_t t;
    uint8_t want[3];
};

static const struct vector vectors[] = {
#include "effect_vectors.inc"
};

static void check_vectors(void) {
    int good = 0;
    for (int v = 0; v < (int)ARRAY_SIZE(vectors); v++) {
        const struct vector *c = &vectors[v];
        struct lamp_effect_wire w = {.kind = c->kind, .n = c->n};
        sys_put_le24(c->period, w.period_ms);
        sys_put_le16(c->angle, w.angle_cdeg);
        sys_put_le16(c->width, w.width_dmm);
        sys_put_le16(c->origin[0], &w.origin[0]);
        sys_put_le16(c->origin[1], &w.origin[2]);
        memcpy(w.rgb, c->rgb, sizeof(w.rgb));
        for (int i = 0; i < 4; i++) {
            sys_put_le16(c->room[i], &w.room[2 * i]);
            sys_put_le16(c->room[i], &w.half[2 * i]);
        }
        struct lamp_effect fx;
        uint8_t got[3] = {0};
        bool ok = lamp_effect_prepare(&fx, &w, false);
        if (ok) {
            lamp_effect_at(&fx, c->x, c->y, lamp_effect_phase(c->t, c->period), got);
        }
        for (int i = 0; ok && i < 3; i++) {
            ok = abs(got[i] - c->want[i]) <= 2;
        }
        if (ok) {
            good++;
        } else {
            printk("effects: vector %d (kind %d at %d.%02d, %d.%02d, t %d) got %02X%02X%02X, "
                   "crates/scene %02X%02X%02X\n",
                   v, c->kind, (int)c->x, abs((int)(c->x * 100)) % 100, (int)c->y,
                   abs((int)(c->y * 100)) % 100, (int)c->t, got[0], got[1], got[2], c->want[0],
                   c->want[1], c->want[2]);
        }
    }
    printk("effects: %d of %d crates/scene vectors within 2 per channel\n", good,
           (int)ARRAY_SIZE(vectors));
}

/* The PC */

static uint32_t caps;

static int effects_sent_listener(const zmk_event_t *eh) {
    const struct raw_hid_sent_event *ev = as_raw_hid_sent_event(eh);
    if (ev != NULL && ev->length >= 25 && ev->data[0] == 0xD5 && ev->data[1] == 0x81) {
        caps = sys_get_le32(&ev->data[21]);
    }
    return ZMK_EV_EVENT_BUBBLE;
}

ZMK_LISTENER(effects_sim, effects_sent_listener);
ZMK_SUBSCRIPTION(effects_sim, raw_hid_sent_event);

static void put_len(const uint8_t *d, int len, int length) {
    static uint8_t report[32];
    memset(report, 0, sizeof(report));
    memcpy(report, d, len);
    raise_raw_hid_received_event((struct raw_hid_received_event){.data = report, .length = length});
}

static void put(const uint8_t *d, int len) { put_len(d, len, 32); }

static void note(const char *what) { printk("effects: -- %s\n", what); }

static void settle(void) { k_msleep(300); }

static void place(const int16_t v[12]) {
    uint8_t d[32] = {0xD7, 0x0F, 1};
    for (int i = 0; i < 12; i++) {
        sys_put_le16(v[i], &d[3 + 2 * i]);
    }
    put(d, sizeof(d));
}

// EFFECT: kind, colours, clock, period, angle, width, origin and the colours.
static void effect(uint8_t kind, uint8_t n, uint32_t at, uint32_t period, uint16_t angle,
                   uint16_t width, int16_t ox, int16_t oy, const uint8_t rgb[][3]) {
    uint8_t d[32] = {0xD7, 0x0F, 2, kind, n};
    sys_put_le24(at, &d[5]);
    sys_put_le24(period, &d[8]);
    sys_put_le16(angle, &d[11]);
    sys_put_le16(width, &d[13]);
    sys_put_le16(ox, &d[15]);
    sys_put_le16(oy, &d[17]);
    memcpy(&d[19], rgb, 3 * MIN(n, 4));
    put(d, sizeof(d));
}

static void release(void) {
    const uint8_t d[] = {0xD7, 0x0F, 0};
    put(d, sizeof(d));
}


// Draws over a stretch of time (printed or not as quiet says).
static int count_draws(int ms) {
    atomic_set(&draws, 0);
    k_msleep(ms);
    return atomic_get(&draws);
}

// Whether the left strip shows w (this half's copy) as lamp_effect.c draws it at phase.
static bool left_shows(const struct lamp_effect_wire *w, float phase) {
    struct lamp_effect fx;
    uint8_t out[LAMP_EFFECT_LAMPS][3];
    if (!lamp_effect_prepare(&fx, w, false)) {
        return false;
    }
    lamp_effect_render(&fx, phase, out);
    return memcmp(out, pixels, sizeof(out)) == 0;
}

static void wire(struct lamp_effect_wire *w, uint8_t kind, uint8_t n, const uint8_t rgb[][3],
                 const int16_t room[4], const int16_t half[4]) {
    memset(w, 0, sizeof(*w));
    w->kind = kind;
    w->n = n;
    memcpy(w->rgb, rgb, 3 * n);
    for (int i = 0; i < 4; i++) {
        sys_put_le16(room[i], &w->room[2 * i]);
        sys_put_le16(half[i], &w->half[2 * i]);
    }
}

static const uint8_t still[4][3] = {{0x20, 0x40, 0x60}, {0x20, 0x40, 0x60}};
static const uint8_t band[4][3] = {{0xF0, 0x8A, 0x3C}, {0x06, 0x08, 0x0E}};
static const uint8_t stops[4][3] = {{0xFF, 0x00, 0x00}, {0x00, 0xFF, 0x00}, {0x00, 0x00, 0xFF}};

void effects_sim_driver(void) {
    k_msleep(500);
    note("the maths against crates/scene");
    check_vectors();

    note("hello: capability bit 19 EFFECTS");
    {
        const uint8_t d[] = {0xD5, 0x01, 0x01};
        put(d, sizeof(d));
    }
    settle();
    printk("effects: caps bit 19 %d\n", (caps >> 19) & 1);

    note("PLACE: a half without area is refused; a whole place is kept, nothing drawn yet");
    {
        const int16_t bad[12] = {0, 0, 400, 100, 10, 5, 0, 90, 270, 5, 120, 90};
        place(bad);
        const int16_t good[12] = {0, 0, 400, 100, 10, 5, 120, 90, 270, 5, 120, 90};
        place(good);
    }
    settle();

    note("a still effect (period 0): both halves taken, the left drawn once");
    effect(4, 2, 0, 0, 0, 0, 0, 0, still);
    settle();
    printk("effects: draws in 300 ms after it: %d\n", count_draws(300));

    note("idle: the half goes dark, then lit again on activity");
    lights = false;
    effect_run_lights_changed();
    settle();
    lights = true;
    effect_run_lights_changed();
    settle();

    note("a moving scanline: a frame every 33 ms, on the clock it was sent with");
    quiet = true; // its frames move: counted, not printed
    effect(1, 2, 500, 3000, 0, 600, 0, 0, band);
    k_msleep(5);
    // The tick may round 33 ms up (10 ms ticks: 40 ms), so a range.
    int n = count_draws(330);
    printk("effects: draws in 330 ms: %s\n", n >= 7 && n <= 11 ? "one a frame" : "wrong");
    int clock = effect_run_clock();
    printk("effects: clock 335 ms after it read 500: %s\n",
           clock >= 835 - 40 && clock <= 835 + 40 ? "within a frame of 835" : "off");

    note("an endpoint change: the effect stays (nothing is streamed)");
    raise_zmk_endpoint_changed((struct zmk_endpoint_changed){0});
    n = count_draws(330);
    printk("effects: draws in 330 ms after it: %s\n", n >= 7 ? "still drawing" : "stopped");

    note("PLACE while it runs: the right gets its new place, the clock goes on");
    {
        int before = effect_run_clock();
        const int16_t moved[12] = {0, 0, 500, 120, 0, 10, 120, 90, 300, 10, 120, 90};
        place(moved);
        settle();
        int went = (effect_run_clock() - before + 3000) % 3000;
        printk("effects: the clock went on through it: %s\n",
               went >= 300 - 40 && went <= 300 + 40 ? "yes" : "no");
    }

    note("SET_FRAME takes the lights: the effect stops on both halves, the frame draws");
    quiet = false;
    memset(printed, 0xff, sizeof(printed));
    {
        const uint8_t d[] = {0xD7, 0x0C, 0x01, 0, 1, 0xFF, 0x00, 0x00};
        put(d, sizeof(d));
    }
    settle();
    printk("effects: draws in 300 ms after it: %d\n", count_draws(300));

    note("RELEASE of an effect that frames replaced: no change, owner none");
    release();
    settle();

    note("an effect takes the lights from the frames, quietly (no D7 8C event)");
    quiet = true;
    effect(2, 3, 0, 0, 0, 0, 0, 0, stops);
    settle();
    {
        const int16_t room[4] = {0, 0, 500, 120}, half[4] = {0, 10, 120, 90};
        struct lamp_effect_wire w;
        wire(&w, 2, 3, stops, room, half);
        printk("effects: the left shows the sweep at its lamps' places: %s\n",
               left_shows(&w, 0.0f) ? "yes" : "no");
    }
    quiet = false;
    memcpy(printed, pixels, sizeof(pixels));

    note("bad requests: kind 9, a sweep of 5, a breathe of 3, angle 360, cut short, op 7");
    effect(9, 2, 0, 0, 0, 0, 0, 0, band);
    effect(2, 5, 0, 0, 0, 0, 0, 0, stops);
    effect(4, 3, 0, 0, 0, 0, 0, 0, stops);
    effect(1, 2, 0, 1000, 36000, 10, 0, 0, band);
    {
        const uint8_t cut[] = {0xD7, 0x0F, 2, 1, 2, 0, 0, 0, 0xE8, 0x03, 0};
        put_len(cut, sizeof(cut), sizeof(cut));
        const uint8_t op[] = {0xD7, 0x0F, 7};
        put(op, sizeof(op));
    }
    settle();

    note("RELEASE: both halves back to their own effect");
    release();
    settle();
    printk("effects: draws in 300 ms after it: %d\n", count_draws(300));
    note("done");
}
