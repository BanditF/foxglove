/*
 * Simulator case for RGB_STATE's truth (tests/zmk-sim/rgb-state, #605, #624): the real
 * host_link.c and rgb_idle.c, with ZMK's underglow, the LED strip and the relay faked here.
 *
 * The fake underglow draws each change into a fake strip the way dim_strip.c hands frames to
 * rgb_idle.c, and the case keeps that strip as the truth: what the LEDs show. Each read of
 * D7 83 (the PC's view, parsed as the app does) is checked against it through idle, wake, the
 * PC's frames (lit while ZMK's flag is off, dark while it would be on), a breathe passing through
 * black, and sync off and on. Then #624: a look change on the left goes to the right in the
 * relay's RGB op, and a right that rebooted with an older saved look takes the left's from it.
 * SPDX-License-Identifier: MIT
 */

#include <stdlib.h>
#include <string.h>

#include <zephyr/drivers/led_strip.h>
#include <zephyr/kernel.h>
#include <zephyr/sys/byteorder.h>
#include <zephyr/sys/printk.h>

#include <zephyr/input/input.h>

#include <raw_hid/events.h>
#include <zmk/event_manager.h>
#include <zmk/rgb_underglow.h>

#include "art_relay.h"
#include "art_relay_doorbell.h"
#include "rgb_idle.h"

#define LEDS 21

static void note(const char *what) { printk("rgbstate: -- %s\n", what); }

/* The strip: the last frame each owner drew, and who owns it (as dim_strip.c's hold) */

static struct led_rgb zmk_frame[LEDS], host_frame[LEDS];
static bool host_owns;

static void show(bool host, struct led_rgb *frame) {
    rgb_idle_frame(host, frame, LEDS, 100); // as dim_strip.c does, no battery dim here
}

/* ZMK's underglow (rgb_underglow.c, which the simulator doesn't build) */

static bool zmk_on;
static struct zmk_led_hsb hsb = {.h = 30, .s = 80, .b = 60};
static int effect;
static int hsb_writes, effect_writes;
static bool breathe_black; // the case draws a breathe at its darkest point

static void hsb_rgb(struct zmk_led_hsb c, struct led_rgb *out) {
    int v = c.b * 255 / 100, s = c.s * 255 / 100;
    int region = c.h / 60, rem = (c.h % 60) * 255 / 60;
    int p = v * (255 - s) / 255, q = v * (255 - s * rem / 255) / 255,
        t = v * (255 - s * (255 - rem) / 255) / 255;
    int r[6] = {v, q, p, p, t, v}, g[6] = {t, v, v, q, p, p}, b[6] = {p, p, t, v, v, q};
    *out = (struct led_rgb){.r = r[region % 6], .g = g[region % 6], .b = b[region % 6]};
}

static void zmk_draw(void) {
    struct led_rgb px = {0};
    if (zmk_on && !breathe_black) {
        hsb_rgb(hsb, &px);
    }
    for (int i = 0; i < LEDS; i++) {
        zmk_frame[i] = px;
    }
    if (!host_owns) {
        show(false, zmk_frame); // dim_strip.c drops ZMK's writes while the PC draws
    }
}

int zmk_rgb_underglow_get_state(bool *on) {
    *on = zmk_on;
    return 0;
}
int zmk_rgb_underglow_on(void) {
    zmk_on = true;
    zmk_draw();
    return 0;
}
int zmk_rgb_underglow_off(void) {
    zmk_on = false;
    zmk_draw();
    return 0;
}
struct zmk_led_hsb zmk_rgb_underglow_calc_hue(int direction) { return hsb; }
int zmk_rgb_underglow_calc_effect(int direction) { return effect; }
int zmk_rgb_underglow_set_hsb(struct zmk_led_hsb color) {
    hsb = color;
    hsb_writes++;
    zmk_draw();
    return 0;
}
int zmk_rgb_underglow_select_effect(int e) {
    effect = e;
    effect_writes++;
    return 0;
}

/* The relay (art_relay_central.c rgb_op): the RGB op the right would be sent now */

static uint8_t op_on, op_follow, op_look[RGB_IDLE_LOOK_SIZE];
static int kicks;

void art_relay_rgb_kick(void) {
    bool on = false;
    rgb_idle_get_on(&on);
    op_on = on;
    op_follow = rgb_idle_sync();
    rgb_idle_look(op_look);
    kicks++;
}

/* The PC: D7 83 replies and events, parsed as crates/foxglove-link does */

static uint8_t reply_83[32], event_83[32];
static bool asked;
static int events;

static int sent_listener(const zmk_event_t *eh) {
    const struct raw_hid_sent_event *ev = as_raw_hid_sent_event(eh);
    if (ev != NULL && ev->length >= 29 && ev->data[0] == 0xD7 && ev->data[1] == 0x83) {
        if (asked) {
            memcpy(reply_83, ev->data, sizeof(reply_83));
            asked = false;
        } else {
            memcpy(event_83, ev->data, sizeof(event_83));
            events++;
        }
    }
    if (ev != NULL && ev->length >= 25 && ev->data[0] == 0xD5 && ev->data[1] == 0x81) {
        printk("rgbstate: HELLO caps bit 24 RGB_SHOWN %d\n",
               (sys_get_le32(&ev->data[21]) >> 24) & 1);
    }
    return ZMK_EV_EVENT_BUBBLE;
}

ZMK_LISTENER(rgb_state_sim, sent_listener);
ZMK_SUBSCRIPTION(rgb_state_sim, raw_hid_sent_event);

static void put(const uint8_t *d, int len) {
    static uint8_t report[32];
    memset(report, 0, sizeof(report));
    memcpy(report, d, len);
    raise_raw_hid_received_event((struct raw_hid_received_event){.data = report, .length = 32});
}

// The LEDs as shown: the owner's last frame. Lit, and the brightest LED's level 0-100.
static void truth(bool *lit, int *value) {
    const struct led_rgb *f = host_owns ? host_frame : zmk_frame;
    int best = 0;
    for (int i = 0; i < LEDS; i++) {
        best = MAX(best, MAX(f[i].r, MAX(f[i].g, f[i].b)));
    }
    *lit = best > 0;
    *value = (best * 100 + 127) / 255;
}

// Reads D7 83 and checks the left's on (as the app parses it: the shown block's lit when it is
// there) and the shown value against the LEDs. Breathe at its darkest is lit by design.
static void check(const char *when) {
    const uint8_t d[] = {0xD7, 0x03};
    asked = true;
    put(d, sizeof(d));
    k_msleep(100);
    const uint8_t *r = reply_83, *s = &reply_83[17];
    bool valid = s[0] & RGB_SHOWN_VALID, lit = s[0] & RGB_SHOWN_LIT;
    bool on = valid ? lit : r[2];
    bool led_lit;
    int led_value;
    truth(&led_lit, &led_value);
    bool breathing = effect == 1 && zmk_on && !host_owns;
    bool ok = on == (led_lit || breathing) &&
              (!(s[0] & RGB_SHOWN_DRAWN) || abs(s[4] - led_value) <= 1);
    printk("rgbstate: %s: D7 83 left on %d (ZMK's flag %d); shown: wanted %d, pc %d, idle %d, "
           "drawn %d, value %d, dim %d; the LEDs: lit %d, value %d: %s\n",
           when, on, r[2], !!(s[0] & RGB_SHOWN_WANTED), !!(s[0] & RGB_SHOWN_HOST),
           !!(s[0] & RGB_SHOWN_IDLE), !!(s[0] & RGB_SHOWN_DRAWN), s[4], s[5], led_lit, led_value,
           ok ? "match" : "MISMATCH");
}

// The unsolicited D7 83 since the last call: whether one came and its lit.
static void heard(void) {
    if (events == 0) {
        printk("rgbstate: no D7 83 event\n");
    } else {
        printk("rgbstate: D7 83 event: left lit %d\n", !!(event_83[17] & RGB_SHOWN_LIT));
    }
    events = 0;
}

static void wake(void) {
    input_report_rel(NULL, INPUT_REL_X, 1, true, K_NO_WAIT);
    k_msleep(800); // the D7 83 event comes 600 ms after a change
}

static void wait_idle(void) { k_msleep(4500); } // idle 3 s after activity (case.conf)

// The PC takes or hands back the LEDs, as lamp_strip.c does on ZMK's low-priority queue.
static void pc_takes(void) {
    rgb_idle_host_control(true);
    zmk_rgb_underglow_off();
    host_owns = true;
}
static void pc_draws(uint8_t r, uint8_t g, uint8_t b) {
    for (int i = 0; i < LEDS; i++) {
        host_frame[i] = (struct led_rgb){.r = r, .g = g, .b = b};
    }
    show(true, host_frame);
}
static void pc_hands_back(void) {
    host_owns = false;
    zmk_rgb_underglow_off(); // clears the PC's last frame
    rgb_idle_host_control(false);
    k_msleep(100);
}

static void print_op(void) {
    printk("rgbstate: the right is sent: on %d, follow %d, look hue %d sat %d bri %d effect %d\n",
           op_on, op_follow, sys_get_le16(op_look), op_look[2], op_look[3], op_look[4]);
}

/* #618: the left's side of the right's doorbell (art_relay_doorbell.h) */

// RGB_STATE's right shown block as host_link.c builds it: only with the right's status known.
static void ring_report(const char *when, bool known, const struct rgb_shown_wire *shown,
                        atomic_t *valid) {
    if (!known || !atomic_get(valid)) {
        printk("rgbstate: %s: RGB_STATE's right shown: none\n", when);
        return;
    }
    printk("rgbstate: %s: RGB_STATE's right shown: lit %d, idle %d, following %d\n", when,
           !!(shown->flags & RGB_SHOWN_LIT), !!(shown->flags & RGB_SHOWN_IDLE),
           !!(shown->flags & RGB_SHOWN_FOLLOWING));
}

// An RGB ring from the right as art_relay_peripheral.c builds it: the right's own activity is
// idle and it follows the left, lit or dark as the left's on/off says.
static void ring_in(uint8_t lit, uint8_t on, bool known, struct rgb_idle_wire *rgb,
                    struct rgb_shown_wire *shown, atomic_t *valid, const char *when) {
    uint8_t d[1 + sizeof(struct rgb_idle_wire) + sizeof(struct rgb_shown_wire)] = {
        ART_RELAY_RANG_RGB};
    struct rgb_idle_wire w = {.on = on, .hue = 23, .saturation = 100, .brightness = 42,
                              .effect = 3, .speed = 3};
    struct rgb_shown_wire s = {
        .flags = RGB_SHOWN_VALID | lit | RGB_SHOWN_WANTED | RGB_SHOWN_IDLE | RGB_SHOWN_FOLLOWING,
        .hue = 0xffff,
        .dim = 100};
    memcpy(&d[1], &w, sizeof(w));
    memcpy(&d[1 + sizeof(w)], &s, sizeof(s));
    uint8_t got = art_relay_ring_rgb(d, sizeof(d), known, rgb, shown, valid);
    printk("rgbstate: %s: carried %d, D7 83 due %d\n", when, !!(got & ART_RELAY_RING_CARRIED),
           !!(got & ART_RELAY_RING_TELL));
    ring_report(when, known, shown, valid);
}

// This half as the right: what its LEDs show (the fake strip) and what it reports.
static void right_line(const char *when) {
    bool led_lit, on = false;
    int led_value;
    struct rgb_shown_wire s;
    truth(&led_lit, &led_value);
    rgb_idle_shown(&s);
    zmk_rgb_underglow_get_state(&on);
    printk("rgbstate: the right, %s: the LEDs lit %d; shown lit %d, idle %d, following %d; "
           "ZMK's flag %d: %s\n",
           when, led_lit, !!(s.flags & RGB_SHOWN_LIT), !!(s.flags & RGB_SHOWN_IDLE),
           !!(s.flags & RGB_SHOWN_FOLLOWING), on,
           led_lit == !!(s.flags & RGB_SHOWN_LIT) ? "match" : "MISMATCH");
}

void rgb_state_sim_driver(void) {
    k_msleep(500);
    note("hello: capability bit 24");
    {
        const uint8_t d[] = {0xD5, 0x01, 0x01};
        put(d, sizeof(d));
    }
    k_msleep(100);
    wake();
    check("lit at start");

    note("idle: ZMK turns the underglow off; the LEDs go dark and D7 83 says so");
    events = 0;
    wait_idle();
    check("idle");
    heard();

    note("wake: lit again");
    wake();
    check("woken");
    heard();

    note("the PC's frames light the LEDs through idle, while ZMK's flag says off (#605 case 1)");
    pc_takes();
    pc_draws(0x20, 0x40, 0x60);
    k_msleep(800);
    check("a scene draws");
    events = 0;
    wait_idle();
    check("a scene draws while idle");
    heard();

    note("a black frame: the LEDs are dark while the user's on/off is on (#605 case 2)");
    wake();
    events = 0;
    pc_draws(0, 0, 0);
    k_msleep(800);
    check("a black frame");
    heard();

    note("hand-back: ZMK's own effect again");
    wake(); // keeps the left active through the next parts (idle comes 3 s after activity)
    pc_hands_back();
    k_msleep(800);
    check("handed back");

    note("a breathe at its darkest is still on");
    effect = 1;
    breathe_black = true;
    zmk_draw();
    k_msleep(200);
    check("breathe through black");
    breathe_black = false;
    effect = 0;
    zmk_draw();

    note("sync off and on: the left's own lights don't change; the right is told");
    wake();
    {
        const uint8_t off[] = {0xD7, 0x01, 0x00};
        put(off, sizeof(off));
        k_msleep(200);
        check("sync off");
        print_op();
        const uint8_t on[] = {0xD7, 0x01, 0x01};
        put(on, sizeof(on));
        k_msleep(200);
        check("sync on");
        print_op();
    }

    note("#624: a look change on the left (an invoke) goes to the right in the RGB op");
    wake();
    kicks = 0;
    hsb = (struct zmk_led_hsb){.h = 39, .s = 70, .b = 42};
    effect = 3;
    zmk_draw();
    rgb_idle_check(); // as INVOKE does after a binding
    k_msleep(200);
    printk("rgbstate: kicked %s\n", kicks ? "yes" : "no");
    print_op();
    kicks = 0;
    rgb_idle_check(); // a key that changed nothing
    k_msleep(200);
    printk("rgbstate: a key that changed no look kicks: %s\n", kicks ? "yes" : "no");
    check("the left's new look");

    note("#624, as the right: rebooted with an older saved look, then the left's RGB op");
    {
        uint8_t look[RGB_IDLE_LOOK_SIZE];
        memcpy(look, op_look, sizeof(look));
        hsb = (struct zmk_led_hsb){.h = 4, .s = 100, .b = 9}; // the right's saved state (#625)
        effect = 2;
        hsb_writes = effect_writes = 0;
        rgb_idle_remote(true, true);
        rgb_idle_remote_look(look);
        k_msleep(100);
        printk("rgbstate: the right shows hue %d sat %d bri %d effect %d (%d hsb, %d effect "
               "writes)\n",
               hsb.h, hsb.s, hsb.b, effect, hsb_writes, effect_writes);
        hsb_writes = effect_writes = 0;
        rgb_idle_remote_look(look); // the same op again
        printk("rgbstate: the same op again writes %d hsb, %d effect\n", hsb_writes,
               effect_writes);
        check("the right on the left's look");
        rgb_idle_remote_lost();
        k_msleep(100);
    }

    note("#618: the left keeps the right's shown state from the ring before its first status read");
    {
        // The left's side of the doorbell (art_relay_doorbell.h, as art_relay_central.c calls
        // it); RGB_STATE reports the right's shown block only once a status read is in (age).
        struct rgb_idle_wire rgb = {0};
        struct rgb_shown_wire shown = {0};
        atomic_t valid = ATOMIC_INIT(0);
        bool known = false; // no status read on this link yet: a new link, as after a re-link
        ring_in(RGB_SHOWN_LIT, 1, known, &rgb, &shown, &valid, "the subscribe's ring, lit");
        known = true;
        ring_report("the first status read lands", known, &shown, &valid);
        ring_in(0, 0, known, &rgb, &shown, &valid, "the right goes dark");
        ring_in(0, 0, known, &rgb, &shown, &valid, "the same ring again");
        {
            const uint8_t old_ring[] = {ART_RELAY_RANG_RGB}; // a right before #605: byte 0 only
            uint8_t got = art_relay_ring_rgb(old_ring, sizeof(old_ring), known, &rgb, &shown,
                                             &valid);
            printk("rgbstate: a one-byte ring: carried %d, D7 83 due %d\n",
                   !!(got & ART_RELAY_RING_CARRIED), !!(got & ART_RELAY_RING_TELL));
            ring_report("after it", known, &shown, &valid);
        }
        atomic_clear(&valid); // drop_peer: the link went
        ring_report("the link went", known, &shown, &valid);
    }

    note("#618, as the right: the left idles while the PC reads the right and sends its colour");
    wait_idle(); // this half's own activity idles: nobody presses the right's keys
    rgb_idle_remote(true, true); // the left is active: the right follows it on
    k_msleep(100);
    right_line("follows the left on");
    rgb_idle_remote(false, true); // the left idles: its RGB op says off
    k_msleep(100);
    right_line("the left idles");
    {
        // What the PC's traffic does to the right meanwhile: status reads (the status value and
        // the doorbell build these), the PC's colour (ART_RELAY_RGB_HSB sets ZMK's colour), the
        // left's look (#624) and the same RGB op again (a retry).
        struct rgb_idle_wire w;
        struct rgb_shown_wire s;
        for (int i = 0; i < 5; i++) {
            rgb_idle_state(&w);
            rgb_idle_shown(&s);
        }
        zmk_rgb_underglow_set_hsb((struct zmk_led_hsb){.h = 200, .s = 90, .b = 20});
        const uint8_t look[RGB_IDLE_LOOK_SIZE] = {23, 0, 100, 42, 3};
        rgb_idle_remote_look(look);
        rgb_idle_remote(false, true);
        k_msleep(100);
    }
    right_line("the left idle, with the PC's traffic");
    rgb_idle_remote(true, true); // a left key
    k_msleep(100);
    right_line("a key on the left");
    rgb_idle_remote_lost();
    k_msleep(100);
    note("done");
}
