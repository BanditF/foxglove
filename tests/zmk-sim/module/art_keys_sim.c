/*
 * Simulator stand-in for the art relay's on-board stepping (tests/zmk-sim, ART_KEYS cases): the
 * real art_keys.c and host_link.c decide whether a press goes to the PC (D7 8D 01 dir) or to the
 * board; the board side, which needs the right half, only prints here.
 * SPDX-License-Identifier: MIT
 */
#include <zephyr/sys/printk.h>

#include "art_relay.h"

void art_relay_step(int dir) { printk("artkeys: board steps %d\n", dir); }

/* The PC for the art-keys case: subscribes at 1.5 s, unsubscribes at 4 s (the case's keys go to
 * the board before, to the PC between, and to the board after), then subscribes again at 6 s and
 * the endpoint changes at 7 s, which must end the opt-in (#302). Replaces the protocol walk. */

#include <string.h>

#include <zephyr/kernel.h>

#include <raw_hid/events.h>
#include <zmk/events/endpoint_changed.h>

static void put(const uint8_t *d, int len) {
    static uint8_t report[32];
    memset(report, 0, sizeof(report));
    memcpy(report, d, len);
    printk("artkeys: pc sends D7 0D %02X\n", d[2]);
    raise_raw_hid_received_event((struct raw_hid_received_event){.data = report, .length = 32});
}

void art_keys_sim_driver(void) {
    const uint8_t on[] = {0xD7, 0x0D, 0x01}, off[] = {0xD7, 0x0D, 0x00};
    k_msleep(1000); // the thread starts at 0.5 s
    put(on, sizeof(on));
    k_msleep(2500);
    put(off, sizeof(off));
    k_msleep(2000);
    put(on, sizeof(on));
    k_msleep(1000);
    printk("artkeys: the endpoint changes
");
    raise_zmk_endpoint_changed((struct zmk_endpoint_changed){0});
}
