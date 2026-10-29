/*
 * Simulator driver for the left's activity relay (tests/zmk-sim/activity-relay, #603): the real
 * activity_relay.c listens to ZMK's real activity state; the relay itself (art_relay_central.c,
 * Bluetooth) is replaced by a print of the op it would send the right, built by the same
 * art_relay_activity_op() the board uses. Each print is one message to the right.
 *
 * A cable going in can't be simulated (no USB here): after the second idle the driver does what
 * usb_wake.c does for one, it calls ZMK's own activity listener, once.
 * SPDX-License-Identifier: MIT
 */

#include <zephyr/kernel.h>
#include <zephyr/sys/printk.h>

#include <zmk/activity.h>
#include <zmk/event_manager.h>

#include "activity_relay.h"

// ZMK_LISTENER(activity, ...) in ZMK's activity.c, as usb_wake.c uses it.
extern const struct zmk_listener zmk_listener_activity;

static const char *name(uint8_t s) {
    switch (s) {
    case ZMK_ACTIVITY_ACTIVE:
        return "active";
    case ZMK_ACTIVITY_IDLE:
        return "idle";
    case ZMK_ACTIVITY_SLEEP:
        return "going to sleep";
    default:
        return "?";
    }
}

static void usb_wake_cb(struct k_work *work) {
    printk("activity: -- a cable goes in: usb_wake.c's call into ZMK's activity listener\n");
    zmk_listener_activity.callback(NULL); // usb_wake.c passes its USB event, which it ignores
}
static K_WORK_DELAYABLE_DEFINE(usb_wake_work, usb_wake_cb);

static int idles;

void art_relay_activity_kick(void) {
    uint8_t op[2];
    art_relay_activity_op(op);
    printk("activity: to the right %02X %02X, the left %s\n", op[0], op[1], name(op[1]));
    if (op[1] == ZMK_ACTIVITY_IDLE && ++idles == 2) {
        k_work_schedule(&usb_wake_work, K_SECONDS(1));
    }
}
