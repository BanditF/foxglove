/*
 * Simulator case for SET_EFFECT (tests/zmk-sim/host-effects, #490): the real host_link.c,
 * host_effects.c, effect_run.c, lamp_effect.c, host_frames.c and rgb_idle.c, with this half's
 * strip, the art relay and ZMK's underglow faked here.
 *
 * First the maths: every vector crates/scene wrote (effect_vectors.inc) through lamp_effect.c,
 * within 2 per channel. Then the PC's side: PLACE, EFFECT and RELEASE over the host link, a still
 * effect drawn once, a moving one drawn every frame, SET_FRAME taking over and an effect taking
 * back, the bad requests, and the effect living through an endpoint change. Then the scene owning
 * the lights (#542): it draws through idle and the user's off, RGB_SYNC and the right hear the
 * left's real on/off meanwhile, RELEASE puts that on/off back, and (as the right) it draws on
 * through the left's link dropping and coming back with "off".
 * SPDX-License-Identifier: MIT
 */

#include <stdlib.h>
#include <string.h>

#include <zephyr/kernel.h>
#include <zephyr/sys/byteorder.h>
#include <zephyr/sys/printk.h>

#include <zephyr/input/input.h>

#include <raw_hid/events.h>
#include <zmk/event_manager.h>
#include <zmk/events/endpoint_changed.h>
#include <zmk/rgb_underglow.h>
#include <zmk/workqueue.h>

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

/* ZMK's underglow (rgb_underglow.c, which the simulator doesn't build): on/off only */

static atomic_t zmk_on;

int zmk_rgb_underglow_get_state(bool *on) {
    *on = atomic_get(&zmk_on);
    return 0;
}
int zmk_rgb_underglow_on(void) {
    atomic_set(&zmk_on, 1);
    return 0;
}
int zmk_rgb_underglow_off(void) {
    atomic_set(&zmk_on, 0);
    return 0;
}
struct zmk_led_hsb zmk_rgb_underglow_calc_hue(int direction) {
    return (struct zmk_led_hsb){.h = 30, .s = 80, .b = 60};
}
int zmk_rgb_underglow_calc_effect(int direction) { return 0; }

/* The left strip (lamp_strip.c): draws are counted; a draw that differs from the last printed
 * one is printed unless the case is counting a moving effect. Taking it and handing it back go
 * through rgb_idle.c on ZMK's low-priority queue, as the real one's do. */

static uint8_t pixels[LAMP_STRIP_LEN][3], printed[LAMP_STRIP_LEN][3];
static bool host, quiet;
static atomic_t draws;
static atomic_t want_owned;
static bool owned; // the low-priority queue's

static void owner_cb(struct k_work *work) {
    bool want = atomic_get(&want_owned);
    if (want == owned) {
        return;
    }
    owned = want;
    if (want) {
        rgb_idle_host_control(true);
        zmk_rgb_underglow_off();
    } else {
        zmk_rgb_underglow_off();
        rgb_idle_host_control(false);
    }
}
static K_WORK_DEFINE(owner_work, owner_cb);

void lamp_strip_host(bool on) {
    if (on != host) {
        host = on;
        memset(pixels, 0, sizeof(pixels));
        memset(printed, 0xff, sizeof(printed));
        printk("effects: left %s\n", on ? "taken from its effect" : "back to its effect");
        atomic_set(&want_owned, on);
        k_work_submit_to_queue(zmk_workqueue_lowprio_work_q(), &owner_work);
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

/* The underglow relay (art_relay_central.c rgb_sync_cb): what the right would be told */

static atomic_t told_on = ATOMIC_INIT(-1), told_follow = ATOMIC_INIT(-1);

void art_relay_rgb_kick(void) {
    bool on = false;
    rgb_idle_get_on(&on);
    atomic_set(&told_on, on);
    atomic_set(&told_follow, rgb_idle_sync());
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
    if (w->lamps_set) { // the PC's LED points for the right (EFFECT op LAMPS, #550)
        printk("effects: right's LEDs from the PC: 0 at %d,%d; 1 at %d,%d\n",
               sys_get_le16(&w->lamp[0][0]), sys_get_le16(&w->lamp[0][2]),
               sys_get_le16(&w->lamp[1][0]), sys_get_le16(&w->lamp[1][2]));
    }
}

uint8_t art_relay_effect_right(void) { return ART_LAMPS_OK; }

void art_relay_effect_sync(void) { printk("effects: right clock again\n"); }

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

/* The Scenes page's directions, end to end (app/src/foxglove/keyboard_direction_vectors.rs):
 * the PLACE and EFFECT reports the app sends for each direction, and the colour the app's scene
 * gives each LED. Here both halves render them with the firmware's lamp table (the right
 * mirrored), as host_effects.c hands each half its copy. The wire rounds the halves' places to
 * whole mm, so a lamp may sit up to about a mm off the app's: within DIR_SLACK per channel. */

#define DIR_SLACK 8

struct direction_vector {
    int16_t place[12]; // room, left, right: x, y, w, h
    uint8_t report[32];
    uint8_t want[2 * LAMP_EFFECT_LAMPS][3];
};

static const struct direction_vector directions[] = {
#include "direction_vectors.inc"
};

static void check_directions(void) {
    static struct lamp_effect fx; // static: the effect is large for a driver stack
    int good = 0;
    for (int v = 0; v < (int)ARRAY_SIZE(directions); v++) {
        const struct direction_vector *c = &directions[v];
        const uint8_t *r = c->report;
        struct lamp_effect_wire w = {.kind = r[3], .n = r[4]};
        memcpy(w.period_ms, &r[8], 3);
        memcpy(w.angle_cdeg, &r[11], 2);
        memcpy(w.width_dmm, &r[13], 2);
        memcpy(w.origin, &r[15], 4);
        memcpy(w.rgb, &r[19], 3 * MIN(w.n, LAMP_EFFECT_COLOURS));
        for (int i = 0; i < 4; i++) {
            sys_put_le16(c->place[i], &w.room[2 * i]);
        }
        float phase = lamp_effect_phase(sys_get_le24(&r[5]), sys_get_le24(&r[8]));
        bool ok = true;
        for (int half = 0; half < 2 && ok; half++) {
            for (int i = 0; i < 4; i++) {
                sys_put_le16(c->place[4 + 4 * half + i], &w.half[2 * i]);
            }
            uint8_t out[LAMP_EFFECT_LAMPS][3];
            ok = lamp_effect_prepare(&fx, &w, half == 1);
            if (ok) {
                lamp_effect_render(&fx, phase, out);
            }
            for (int i = 0; ok && i < LAMP_EFFECT_LAMPS; i++) {
                const uint8_t *want = c->want[half * LAMP_EFFECT_LAMPS + i];
                for (int ch = 0; ch < 3; ch++) {
                    ok = ok && abs(out[i][ch] - want[ch]) <= DIR_SLACK;
                }
                if (!ok) {
                    printk("effects: direction vector %d (kind %d, angle %d) %s lamp %d got "
                           "%02X%02X%02X, the app %02X%02X%02X\n",
                           v, w.kind, sys_get_le16(&r[11]) / 100, half ? "right" : "left", i,
                           out[i][0], out[i][1], out[i][2], want[0], want[1], want[2]);
                }
            }
        }
        good += ok;
    }
    printk("effects: %d of %d direction vectors match the app on both halves\n", good,
           (int)ARRAY_SIZE(directions));
}

/* The PC */

static uint32_t caps;
// The left's on/off in the last unsolicited RGB_STATE (D7 83): it comes 600 ms after a change,
// so the walk reads it here when it reports rather than at a time-dependent place in the log.
static atomic_t pc_heard = ATOMIC_INIT(-1);

static int effects_sent_listener(const zmk_event_t *eh) {
    const struct raw_hid_sent_event *ev = as_raw_hid_sent_event(eh);
    if (ev != NULL && ev->length >= 25 && ev->data[0] == 0xD5 && ev->data[1] == 0x81) {
        caps = sys_get_le32(&ev->data[21]);
    }
    if (ev != NULL && ev->length >= 3 && ev->data[0] == 0xD7 && ev->data[1] == 0x83) {
        atomic_set(&pc_heard, ev->data[2]);
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

// RGB_SYNC read (0xff: just read): the reply's left on is the user's on/off with idle.
static void rgb_sync_read(void) {
    const uint8_t d[] = {0xD7, 0x01, 0xff};
    put(d, sizeof(d));
    settle();
}

// What the PC last heard (D7 83) and the underglow relay last told the right about the left's
// on/off, and ZMK's underglow here. At least 600 ms after a change (RGB_STATE_SETTLE).
static void lights_report(void) {
    atomic_val_t pc = atomic_get(&pc_heard);
    printk("effects: the PC hears the left %s, the right is told %s%s; ZMK's underglow %s\n",
           pc < 0 ? "nothing" : pc ? "on" : "off", atomic_get(&told_on) ? "on" : "off",
           atomic_get(&told_follow) ? ", follow" : "", atomic_get(&zmk_on) ? "on" : "off");
}

static void underglow_report(void) {
    printk("effects: ZMK's underglow %s\n", atomic_get(&zmk_on) ? "on" : "off");
}

// Whether the left strip still shows its last printed frame, lit.
static void still_lit(void) {
    bool lit = false;
    for (int i = 0; i < LAMP_STRIP_LEN; i++) {
        lit |= (pixels[i][0] | pixels[i][1] | pixels[i][2]) != 0;
    }
    printk("effects: the scene still draws: %s\n",
           host && lit && memcmp(pixels, printed, sizeof(pixels)) == 0 ? "yes" : "no");
}

// A pointer move: ZMK's activity wakes (activity.c listens to every input device). Waits out
// the PC's D7 83 (600 ms after the change).
static void wake(void) {
    input_report_rel(NULL, INPUT_REL_X, 1, true, K_NO_WAIT);
    k_msleep(800);
}

// ZMK's idle comes 3 s after the last activity, checked each second (case.conf).
static void wait_idle(void) { k_msleep(4500); }

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
    note("the Scenes page's directions, as the app sends them, on both halves");
    check_directions();

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

    note("idle: the scene still draws; RGB_SYNC and the right hear the left off, then on");
    wait_idle();
    still_lit();
    rgb_sync_read();
    lights_report();
    wake();
    rgb_sync_read();
    lights_report();

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

    note("LAMPS: the PC's LED points (HELLO bit 21); bad ones answer 5");
    printk("effects: caps bit 21 %d\n", (caps >> 21) & 1);
    quiet = true; // the sweep redraws on each LAMPS: checked below, not printed
    {
        // Half 2, past lamp 20, a value past 1000, a report cut short.
        const uint8_t bad_half[] = {0xD7, 0x0F, 3, 2, 0, 1, 0, 0, 0, 0};
        put(bad_half, sizeof(bad_half));
        const uint8_t past[] = {0xD7, 0x0F, 3, 0, 20, 2, 0, 0, 0, 0, 0, 0, 0, 0};
        put(past, sizeof(past));
        const uint8_t big[] = {0xD7, 0x0F, 3, 0, 0, 1, 0xE9, 0x03, 0, 0};
        put(big, sizeof(big));
        const uint8_t cut[] = {0xD7, 0x0F, 3, 0, 0, 1, 0, 0};
        put_len(cut, sizeof(cut), sizeof(cut));
        // The left's 21 LEDs all on its left edge, half way down: four reports.
        for (int start = 0; start < LAMP_EFFECT_LAMPS; start += 6) {
            uint8_t d[32] = {0xD7, 0x0F, 3, 0, start, MIN(6, LAMP_EFFECT_LAMPS - start)};
            for (int i = 0; i < d[5]; i++) {
                sys_put_le16(0, &d[6 + 4 * i]);
                sys_put_le16(500, &d[8 + 4 * i]);
            }
            put(d, sizeof(d));
        }
    }
    settle();
    {
        const int16_t room[4] = {0, 0, 500, 120}, half[4] = {0, 10, 120, 90};
        struct lamp_effect_wire w;
        wire(&w, 2, 3, stops, room, half);
        w.lamps_set = 1;
        for (int i = 0; i < LAMP_EFFECT_LAMPS; i++) {
            sys_put_le16(0, &w.lamp[i][0]);
            sys_put_le16(500, &w.lamp[i][2]);
        }
        printk("effects: the left draws its LEDs where LAMPS put them: %s\n",
               left_shows(&w, 0.0f) ? "yes" : "no");
    }
    {
        // The right's LED 0 at its own top-left (not mirrored); the others keep the table's.
        const uint8_t d[] = {0xD7, 0x0F, 3, 1, 0, 1, 0, 0, 0, 0};
        put(d, sizeof(d));
        settle();
        // count 0: the left back on the built-in table.
        const uint8_t back[] = {0xD7, 0x0F, 3, 0, 0, 0};
        put(back, sizeof(back));
        settle();
        const int16_t room[4] = {0, 0, 500, 120}, half[4] = {0, 10, 120, 90};
        struct lamp_effect_wire w;
        wire(&w, 2, 3, stops, room, half);
        printk("effects: count 0 puts the left back on its table: %s\n",
               left_shows(&w, 0.0f) ? "yes" : "no");
        // And the right, so the walk after this goes on with the built-in table.
        const uint8_t back_right[] = {0xD7, 0x0F, 3, 1, 0, 0};
        put(back_right, sizeof(back_right));
        settle();
    }
    quiet = false;
    memcpy(printed, pixels, sizeof(pixels));

    note("RELEASE: both halves back to their own effect");
    release();
    settle();
    printk("effects: draws in 300 ms after it: %d\n", count_draws(300));

    note("hand-back puts the on/off back: off while idle, on with activity");
    effect(4, 2, 0, 0, 0, 0, 0, 0, still);
    wait_idle();
    release();
    settle();
    lights_report();
    wake();
    lights_report();

    // Each part ends well inside the 3 s idle timeout: a wake (no event while active) restarts it.
    note("the user's off: a scene still draws; RELEASE leaves the underglow off");
    wake();
    atomic_set(&zmk_on, 0); // an &rgb_ug key turned it off
    rgb_idle_check();
    k_msleep(800);
    effect(4, 2, 0, 0, 0, 0, 0, 0, still);
    settle();
    rgb_sync_read();
    lights_report();
    release();
    settle();
    lights_report();
    wake();
    atomic_set(&zmk_on, 1); // and on again
    rgb_idle_check();
    k_msleep(800);
    lights_report();

    note("as the right: the left's link drops mid-scene and comes back saying off");
    rgb_idle_remote(true, true);
    effect(4, 2, 0, 0, 0, 0, 0, 0, still);
    settle();
    rgb_idle_remote_lost();
    settle();
    still_lit();
    rgb_idle_remote(false, true);
    settle();
    still_lit();
    release();
    settle();
    underglow_report();
    rgb_idle_remote(true, true);
    settle();
    underglow_report();
    note("done");
}
