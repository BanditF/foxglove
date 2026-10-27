/*
 * Simulator case for SET_FRAME (tests/zmk-sim/host-frames, #272): the real host_link.c and
 * host_frames.c, with this half's strip and the art relay faked here. The fakes print what each
 * half would draw: the left strip on every draw, and for the right what the relay would send
 * over the split link (art_relay_lamp_span from the real art_relay.h: only the lamps that changed
 * since the right's last acknowledged frame, one colour as a fill, nothing for a static frame).
 * SPDX-License-Identifier: MIT
 */

#include <string.h>

#include <zephyr/kernel.h>
#include <zephyr/sys/byteorder.h>
#include <zephyr/sys/printk.h>

#include <raw_hid/events.h>
#include <zmk/event_manager.h>
#include <zmk/events/endpoint_changed.h>

#include "art_relay.h"
#include "lamp_strip.h"

/* Runs of one colour, black left out: "3=FF0000 21-41=0000FF". */
static void print_lamps(const char *what, const uint8_t rgb[][3], int n, int base) {
    printk("frames: %s", what);
    bool any = false;
    for (int i = 0; i < n;) {
        int j = i;
        while (j + 1 < n && memcmp(rgb[j + 1], rgb[i], 3) == 0) {
            j++;
        }
        if (rgb[i][0] | rgb[i][1] | rgb[i][2]) {
            any = true;
            if (j > i) {
                printk(" %d-%d=", base + i, base + j);
            } else {
                printk(" %d=", base + i);
            }
            printk("%02X%02X%02X", rgb[i][0], rgb[i][1], rgb[i][2]);
        }
        i = j + 1;
    }
    printk(any ? "\n" : " black\n");
}

/* The left strip (lamp_strip.c) */

static uint8_t left[LAMP_STRIP_LEN][3];
static bool left_host;

void lamp_strip_host(bool on) {
    if (on != left_host) {
        left_host = on;
        printk("frames: left %s\n", on ? "taken from its effect" : "back to its effect");
    }
}

void lamp_strip_set(int index, uint8_t r, uint8_t g, uint8_t b) {
    if (index >= 0 && index < LAMP_STRIP_LEN) {
        left[index][0] = r;
        left[index][1] = g;
        left[index][2] = b;
    }
}

void lamp_strip_show(void) {
    if (left_host) {
        print_lamps("left draws", left, LAMP_STRIP_LEN, 0);
    }
}

/* The art relay to the right (art_relay_central.c): one write per frame, always acknowledged */

static uint8_t acked[ART_RELAY_LAMPS_MAX][3];
static bool acked_valid, right_host;

void art_relay_lamps(bool host, const uint8_t rgb[ART_RELAY_LAMPS_MAX][3]) {
    if (host != right_host) {
        right_host = host;
        acked_valid = false;
        printk("frames: right %s\n", host ? "taken from its effect" : "back to its effect");
    }
    if (!host) {
        return;
    }
    int start = 0, n = ART_RELAY_LAMPS_MAX;
    bool uniform = true;
    if (acked_valid) {
        n = art_relay_lamp_span(acked, rgb, ART_RELAY_LAMPS_MAX, &start, &uniform);
    } else {
        for (int i = 1; i < n && uniform; i++) {
            uniform = memcmp(rgb[i], rgb[0], 3) == 0;
        }
    }
    if (n == 0) {
        printk("frames: right link sends nothing (no change)\n");
        return;
    }
    if (uniform && n > 1) {
        printk("frames: right link sends a fill of %d lamps from %d: %02X%02X%02X\n", n,
               ART_RELAY_LAMPS_MAX + start, rgb[start][0], rgb[start][1], rgb[start][2]);
    } else {
        printk("frames: right link sends %d lamps from %d\n", n, ART_RELAY_LAMPS_MAX + start);
    }
    memcpy(acked, rgb, sizeof(acked));
    acked_valid = true;
    print_lamps("right draws", acked, ART_RELAY_LAMPS_MAX, ART_RELAY_LAMPS_MAX);
}

uint8_t art_relay_lamps_right(void) { return ART_LAMPS_OK; }

/* The PC */

static uint32_t caps;
static atomic_t replies;
static uint8_t parts[8]; // the last D7 90 (LAMP_PARTS) reply

static int frames_sent_listener(const zmk_event_t *eh) {
    const struct raw_hid_sent_event *ev = as_raw_hid_sent_event(eh);
    if (ev != NULL && ev->length >= 25 && ev->data[0] == 0xD5 && ev->data[1] == 0x81) {
        caps = sys_get_le32(&ev->data[21]);
    }
    if (ev != NULL && ev->length >= 2 && ev->data[0] == 0xD7 && ev->data[1] == 0x8C) {
        atomic_inc(&replies);
    }
    if (ev != NULL && ev->length >= 4 && ev->data[0] == 0xD7 && ev->data[1] == 0x90) {
        memcpy(parts, ev->data, MIN(ev->length, (int)sizeof(parts)));
    }
    return ZMK_EV_EVENT_BUBBLE;
}

ZMK_LISTENER(frames_sim, frames_sent_listener);
ZMK_SUBSCRIPTION(frames_sim, raw_hid_sent_event);

// One report of length bytes, as hidapi sends it, with no wait (a PC streaming).
static void put_len(const uint8_t *d, int len, int length) {
    static uint8_t report[32];
    memset(report, 0, sizeof(report));
    memcpy(report, d, len);
    raise_raw_hid_received_event((struct raw_hid_received_event){.data = report, .length = length});
}

static void put(const uint8_t *d, int len) { put_len(d, len, 32); }

static void note(const char *what) { printk("frames: -- %s\n", what); }

#define SHOW 0x01
#define RELEASE 0x02

// Lamps start..start+count-1 all one colour, in one report.
static void lamps(uint8_t flags, uint8_t start, uint8_t count, uint8_t r, uint8_t g, uint8_t b) {
    uint8_t d[32] = {0xD7, 0x0C, flags, start, count};
    for (int i = 0; i < count && 5 + 3 * i + 2 < 32; i++) {
        d[5 + 3 * i] = r;
        d[6 + 3 * i] = g;
        d[7 + 3 * i] = b;
    }
    put(d, sizeof(d));
}

// A whole frame in 5 reports: black but for one lit lamp each given.
static void frame(int lit_a, int lit_b) {
    for (int start = 0; start < 42; start += 9) {
        uint8_t d[32] = {0xD7, 0x0C, start + 9 >= 42 ? SHOW : 0, start, MIN(9, 42 - start)};
        for (int i = 0; i < d[4]; i++) {
            if (start + i == lit_a) {
                d[5 + 3 * i] = 0xFF;
            } else if (start + i == lit_b) {
                d[6 + 3 * i] = 0xFF;
            }
        }
        put(d, sizeof(d));
    }
}

static void settle(void) { k_msleep(300); } // replies and the right's send run on the system queue

void frames_sim_driver(void) {
    k_msleep(500);
    note("hello: capability bit 14 FRAMES");
    {
        const uint8_t d[] = {0xD5, 0x01, 0x01};
        put(d, sizeof(d));
    }
    settle();
    printk("frames: caps bit 14 %d\n", (caps >> 14) & 1);
    printk("frames: caps bit 22 %d\n", (caps >> 22) & 1);

    note("lamp parts (D7 10, #561): the left's chain, then the right's");
    {
        const uint8_t d[] = {0xD7, 0x10};
        put(d, sizeof(d));
    }
    settle();
    printk("frames: parts status %d, %d parts: %d %d\n", parts[2], parts[3], parts[4], parts[5]);

    note("a chunk without SHOW: no reply, nothing drawn, the lights stay with the keyboard");
    lamps(0, 0, 9, 0x10, 0x20, 0x30);
    settle();
    printk("frames: replies so far %d\n", (int)atomic_get(&replies));

    note("a whole frame (5 reports): lamp 3 red on the left, lamp 30 green on the right");
    frame(3, 30);
    settle();

    note("one lamp changes on the right: only it goes over the link");
    lamps(SHOW, 30, 1, 0, 0, 0xFF);
    settle();

    note("the same frame again: nothing goes over the link");
    lamps(SHOW, 30, 1, 0, 0, 0xFF);
    settle();

    note("one colour per half (the scene fallback): left orange, right blue, as a fill");
    lamps(0, 0, 9, 0xFF, 0x80, 0x00);
    lamps(0, 9, 9, 0xFF, 0x80, 0x00);
    lamps(0, 18, 3, 0xFF, 0x80, 0x00);
    lamps(0, 21, 9, 0x00, 0x00, 0xFF);
    lamps(0, 30, 9, 0x00, 0x00, 0xFF);
    lamps(SHOW, 39, 3, 0x00, 0x00, 0xFF);
    settle();

    note("a burst of 5 frames: the left draws each, the right the first and the newest only");
    k_msleep(200);
    for (int i = 1; i <= 5; i++) {
        lamps(SHOW, 21, 1, 0, i * 0x10, 0);
    }
    settle();

    note("bad requests: 10 lamps, lamps past 41, a report cut short");
    {
        uint8_t d[32] = {0xD7, 0x0C, SHOW, 0, 10};
        put(d, sizeof(d));
        uint8_t past[32] = {0xD7, 0x0C, SHOW, 40, 3};
        put(past, sizeof(past));
        uint8_t cut[] = {0xD7, 0x0C, SHOW, 0, 2, 0xFF, 0, 0};
        put_len(cut, sizeof(cut), sizeof(cut));
    }
    settle();

    note("RELEASE: both halves back to their effect");
    {
        const uint8_t d[] = {0xD7, 0x0C, RELEASE};
        put(d, sizeof(d));
    }
    settle();

    note("a frame, then silence: the timeout hands back and says so (event 1)");
    lamps(SHOW, 0, 1, 0xFF, 0xFF, 0xFF);
    k_msleep(CONFIG_FOXGLOVE_HOST_FRAMES_TIMEOUT_S * 1000 + 500);

    note("a frame, then the endpoint changes: hands back and says so");
    lamps(SHOW, 41, 1, 0xFF, 0xFF, 0xFF);
    settle();
    raise_zmk_endpoint_changed((struct zmk_endpoint_changed){0});
    settle();

    note("RELEASE with nothing taken: ok, owner none, no change");
    {
        const uint8_t d[] = {0xD7, 0x0C, RELEASE};
        put(d, sizeof(d));
    }
    settle();
    note("done");
}
