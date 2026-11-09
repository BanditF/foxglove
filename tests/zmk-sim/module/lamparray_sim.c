/*
 * Simulator case for the HID LampArray (tests/zmk-sim/lamparray, #88): the real lamparray.c core
 * (its six feature reports, without a transport) on the real host_frames.c and host_link.c, with
 * frames_sim.c's fake strip and art relay printing what each half draws. This driver plays
 * Windows Dynamic Lighting (feature reports straight into lamparray_get_report/set_report, over
 * "USB" and "BLE") and Foxglove (Raw HID reports), so the snapshot pins the attributes, the lamp
 * walk, updates landing on both halves, the one-owner rule and its D7 8C events.
 * SPDX-License-Identifier: MIT
 */

#include <errno.h>
#include <string.h>

#include <zephyr/kernel.h>
#include <zephyr/sys/byteorder.h>
#include <zephyr/sys/printk.h>

#include <raw_hid/events.h>
#include <zmk/event_manager.h>
#include <zmk/events/endpoint_changed.h>

#include "host_frames.h"
#include "lamparray.h"

static uint8_t state[LAMPARRAY_STATE_SIZE]; // the last D7 90's bytes 12-23

static int lamparray_sent_listener(const zmk_event_t *eh) {
    const struct raw_hid_sent_event *ev = as_raw_hid_sent_event(eh);
    if (ev != NULL && ev->length >= LAMPARRAY_STATE_AT + LAMPARRAY_STATE_SIZE &&
        ev->data[0] == 0xD7 && ev->data[1] == 0x90) {
        memcpy(state, &ev->data[LAMPARRAY_STATE_AT], sizeof(state));
    }
    return ZMK_EV_EVENT_BUBBLE;
}

ZMK_LISTENER(lamparray_sim, lamparray_sent_listener);
ZMK_SUBSCRIPTION(lamparray_sim, raw_hid_sent_event);

static void note(const char *what) { printk("lamparray: -- %s\n", what); }

static void settle(void) { k_msleep(300); } // replies and the right's send run on the system queue

/* Foxglove: Raw HID reports */

static void put(const uint8_t *d, int len) {
    static uint8_t report[32];
    memset(report, 0, sizeof(report));
    memcpy(report, d, len);
    raise_raw_hid_received_event((struct raw_hid_received_event){.data = report, .length = 32});
}

#define PUT(...)                                                                                   \
    do {                                                                                           \
        const uint8_t d_[] = {__VA_ARGS__};                                                        \
        put(d_, sizeof(d_));                                                                       \
    } while (0)

static void read_state(void) {
    PUT(0xD7, 0x10);
    settle();
    printk("lamparray: state flags 0x%02X owner %d takes %d hand-backs %d drawn %d refused %d "
           "bad %d\n",
           state[0], state[1], sys_get_le16(&state[2]), sys_get_le16(&state[4]),
           sys_get_le16(&state[6]), sys_get_le16(&state[8]), sys_get_le16(&state[10]));
}

/* Windows: feature reports */

static const char *name(enum lamparray_transport t) { return t == LAMPARRAY_BLE ? "BLE" : "USB"; }

static const char *err_name(int err) {
    switch (err) {
    case 0:
        return "ok";
    case -EINVAL:
        return "EINVAL";
    case -ENOTSUP:
        return "ENOTSUP";
    default:
        return "other";
    }
}

static void owner(void) { printk("lamparray: owner %d\n", host_frames_owner()); }

static void autonomous(enum lamparray_transport t, bool on) {
    printk("lamparray: -- %s: autonomous mode %s\n", name(t), on ? "on" : "off");
    uint8_t d[1] = {on};
    int err = lamparray_set_report(t, LAMPARRAY_REPORT_CONTROL, d, sizeof(d));
    settle();
    if (err) {
        printk("lamparray: control %s\n", err_name(err));
    }
    owner();
}

static void attributes(enum lamparray_transport t) {
    uint8_t p[LAMPARRAY_REPORT_MAX];
    int n = lamparray_get_report(t, LAMPARRAY_REPORT_ATTRIBUTES, p);
    printk("lamparray: attributes (%s): %d bytes, %d lamps, box %u x %u x %u um, kind %u, "
           "interval %u us\n",
           name(t), n, sys_get_le16(&p[0]), sys_get_le32(&p[2]), sys_get_le32(&p[6]),
           sys_get_le32(&p[10]), sys_get_le32(&p[14]), sys_get_le32(&p[18]));
}

static void request_lamp(uint16_t id) {
    uint8_t d[2];
    sys_put_le16(id, d);
    lamparray_set_report(LAMPARRAY_USB, LAMPARRAY_REPORT_ATTR_REQUEST, d, sizeof(d));
}

static void read_lamp(void) {
    uint8_t p[LAMPARRAY_REPORT_MAX];
    int n = lamparray_get_report(LAMPARRAY_USB, LAMPARRAY_REPORT_ATTR_RESPONSE, p);
    printk("lamparray: lamp %d at %d, %d um (%d bytes), purposes %u, levels %d %d %d %d, "
           "programmable %d, key %d\n",
           sys_get_le16(&p[0]), (int32_t)sys_get_le32(&p[2]), (int32_t)sys_get_le32(&p[6]), n,
           sys_get_le32(&p[18]), p[22], p[23], p[24], p[25], p[26], p[27]);
}

#define COMPLETE 0x01

struct lamp {
    uint16_t id;
    uint8_t r, g, b, i;
};

static int multi(enum lamparray_transport t, uint8_t flags, const struct lamp *l, int count) {
    uint8_t d[LAMPARRAY_REPORT_MAX] = {count, flags};
    for (int k = 0; k < count; k++) {
        sys_put_le16(l[k].id, &d[2 + 2 * k]);
        uint8_t *c = &d[2 + 2 * LAMPARRAY_MULTI_MAX + 4 * k];
        c[0] = l[k].r;
        c[1] = l[k].g;
        c[2] = l[k].b;
        c[3] = l[k].i;
    }
    return lamparray_set_report(t, LAMPARRAY_REPORT_MULTI_UPDATE, d, sizeof(d));
}

static int range(enum lamparray_transport t, uint8_t flags, uint16_t from, uint16_t to, uint8_t r,
                 uint8_t g, uint8_t b, uint8_t i) {
    uint8_t d[9] = {flags};
    sys_put_le16(from, &d[1]);
    sys_put_le16(to, &d[3]);
    d[5] = r;
    d[6] = g;
    d[7] = b;
    d[8] = i;
    return lamparray_set_report(t, LAMPARRAY_REPORT_RANGE_UPDATE, d, sizeof(d));
}

void lamparray_sim_driver(void) {
    k_msleep(500);

    note("before any host traffic: a take-over and a hand-back draw, but send no D7 8C");
    autonomous(LAMPARRAY_USB, false);
    autonomous(LAMPARRAY_USB, true);

    note("hello (host traffic from here on)");
    PUT(0xD5, 0x01, 0x01);
    settle();
    read_state();

    note("attributes: the lamps SET_FRAME has, in SET_EFFECT's room");
    attributes(LAMPARRAY_USB);
    attributes(LAMPARRAY_BLE);

    note("the lamp walk: request one, then each read moves on; ids past the end clamp");
    request_lamp(0);
    read_lamp();
    read_lamp();
    request_lamp(21);
    read_lamp();
    request_lamp(500);
    read_lamp();
    read_lamp();

    note("an update while the keyboard has the lights: taken, not drawn");
    {
        const struct lamp l[] = {{0, 0xFF, 0, 0, 0xFF}};
        printk("lamparray: multi %s\n", err_name(multi(LAMPARRAY_USB, COMPLETE, l, 1)));
    }
    settle();
    owner();

    autonomous(LAMPARRAY_USB, false);

    note("a multi update on both halves: lamp 0 red, 5 green at half intensity, 30 blue");
    {
        const struct lamp l[] = {
            {0, 0xFF, 0, 0, 0xFF}, {5, 0, 0xFF, 0, 0x80}, {30, 0, 0, 0xFF, 0xFF}};
        printk("lamparray: multi %s\n", err_name(multi(LAMPARRAY_USB, COMPLETE, l, 3)));
    }
    settle();

    note("a range update: the right half all blue");
    printk("lamparray: range %s\n",
           err_name(range(LAMPARRAY_USB, COMPLETE, 21, 41, 0, 0, 0xFF, 0xFF)));
    settle();

    note("lamp 1 white without LampUpdateComplete: nothing drawn until a range of 2-3 completes");
    {
        const struct lamp l[] = {{1, 0xFF, 0xFF, 0xFF, 0xFF}};
        printk("lamparray: multi %s\n", err_name(multi(LAMPARRAY_USB, 0, l, 1)));
    }
    settle();
    printk("lamparray: range %s\n",
           err_name(range(LAMPARRAY_USB, COMPLETE, 2, 3, 0xFF, 0xFF, 0xFF, 0xFF)));
    settle();

    note("Foxglove while Windows has the lights: SHOW busy, a chunk dropped, RELEASE leaves them");
    PUT(0xD7, 0x0C, 0x01, 0, 1, 0xFF, 0xFF, 0xFF);
    settle();
    PUT(0xD7, 0x0C, 0x00, 0, 1, 0x10, 0x10, 0x10);
    settle();
    PUT(0xD7, 0x0C, 0x02);
    settle();
    owner();
    read_state();

    autonomous(LAMPARRAY_USB, true);
    note("autonomous mode on again: nothing to hand back");
    autonomous(LAMPARRAY_USB, true);

    note("over BLE: owner 2, the last host to turn autonomous mode off draws");
    autonomous(LAMPARRAY_BLE, false);
    autonomous(LAMPARRAY_USB, false);
    note("BLE's autonomous mode on and an endpoint change leave USB's lights alone");
    autonomous(LAMPARRAY_BLE, true);
    raise_zmk_endpoint_changed((struct zmk_endpoint_changed){0});
    settle();
    owner();
    note("BLE's autonomous mode off while USB's is off too: USB keeps them (the cable wins)");
    autonomous(LAMPARRAY_BLE, false);
    note("USB's autonomous mode on passes them to BLE, still off: one event, owner 2");
    autonomous(LAMPARRAY_USB, true);
    note("an endpoint change hands BLE's back");
    raise_zmk_endpoint_changed((struct zmk_endpoint_changed){0});
    settle();
    owner();

    note("Foxglove's frames, then Windows takes them (owner 3, then 1)");
    PUT(0xD7, 0x0C, 0x01, 0, 1, 0xFF, 0xFF, 0xFF);
    settle();
    autonomous(LAMPARRAY_USB, false);
    autonomous(LAMPARRAY_USB, true);

    note("reports refused: a short multi update, an unknown id, an empty control, a set-only read");
    {
        uint8_t d[10] = {1, COMPLETE};
        printk("lamparray: short multi %s\n",
               err_name(lamparray_set_report(LAMPARRAY_USB, LAMPARRAY_REPORT_MULTI_UPDATE, d,
                                             sizeof(d))));
        printk("lamparray: id 9 %s\n", err_name(lamparray_set_report(LAMPARRAY_USB, 9, d, 1)));
        printk("lamparray: empty control %s\n",
               err_name(lamparray_set_report(LAMPARRAY_USB, LAMPARRAY_REPORT_CONTROL, d, 0)));
        uint8_t p[LAMPARRAY_REPORT_MAX];
        printk("lamparray: read multi %s\n",
               err_name(lamparray_get_report(LAMPARRAY_USB, LAMPARRAY_REPORT_MULTI_UPDATE, p)));
    }
    read_state();
    note("done");
}
