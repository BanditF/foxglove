/*
 * Simulator case for the keyboard's self-report (tests/zmk-sim/health, #705): the real health.c
 * and host_link.c (D7 13 / D7 93), as the left of a one-piece keyboard. Faked here: the Raw HID
 * module's counters (raw_hid/stats.h: every report the module would send counts as sent). The
 * reply-queue drop is real: host_link.c's queue holds 4, and a burst the system queue can't drain
 * loses the rest.
 * SPDX-License-Identifier: MIT
 */

#include <string.h>

#include <zephyr/kernel.h>
#include <zephyr/sys/byteorder.h>
#include <zephyr/sys/printk.h>

#include <raw_hid/events.h>
#include <raw_hid/stats.h>
#include <zmk/event_manager.h>

#include "health.h"
#include "posix_board_if.h"

static void note(const char *what) { printk("health: -- %s\n", what); }

/* The Raw HID module's counters: every report it is handed goes out over USB; Bluetooth has no
 * PC listening (the PC uses USB), so each report is skipped there, as the module counts it. */

static atomic_t sent;
void raw_hid_stats_get(struct raw_hid_stats *out) {
    memset(out, 0, sizeof(*out));
    out->sent = (uint32_t)atomic_get(&sent);
    out->usb_sent = out->sent;
    out->ble_skipped = out->sent;
    out->ble_last_err = -12; // an earlier refusal's (-ENOMEM), kept as the module keeps it
}

/* The keyboard's reports */

static uint8_t block[HEALTH_SIZE];
static int got_mask, replies;

static int sent_listener(const zmk_event_t *eh) {
    const struct raw_hid_sent_event *ev = as_raw_hid_sent_event(eh);
    if (ev == NULL || ev->length < 8) {
        return ZMK_EV_EVENT_BUBBLE;
    }
    atomic_inc(&sent);
    const uint8_t *d = ev->data;
    if (d[0] == 0xD5 && d[1] == 0x81) {
        printk("health: HELLO caps bit 28 HEALTH %d\n", (sys_get_le32(&d[21]) >> 28) & 1);
    }
    if (d[0] == 0xD7 && d[1] == 0x93) {
        replies++;
        printk("health: keyboard D7 93: status %u, half %u, chunk %u of %u, length %u\n", d[2], d[3],
               d[5], d[6], d[7]);
        if (d[2] == 0 && d[5] < HEALTH_CHUNKS) {
            int at = d[5] * HEALTH_CHUNK;
            memcpy(&block[at], &d[8], MIN(HEALTH_CHUNK, HEALTH_SIZE - at));
            got_mask |= 1 << d[5];
        }
    }
    return ZMK_EV_EVENT_BUBBLE;
}

ZMK_LISTENER(health_sim, sent_listener);
ZMK_SUBSCRIPTION(health_sim, raw_hid_sent_event);

/* The app */

static void raise_report(const uint8_t *d, int len, int report_len) {
    static uint8_t report[32];
    memset(report, 0, sizeof(report));
    memcpy(report, d, len);
    raise_raw_hid_received_event(
        (struct raw_hid_received_event){.data = report, .length = report_len});
}

static void put(const uint8_t *d, int len, int report_len) {
    raise_report(d, len, report_len);
    k_msleep(200); // replies go out from the system work queue
}

static void ask(uint8_t half, uint8_t chunk) {
    const uint8_t d[] = {0xD7, 0x13, half, chunk};
    printk("health: PC sends D7 13 %02X %02X\n", half, chunk);
    put(d, sizeof(d), 32);
}

static const char *state(uint8_t r) {
    static const char *names[] = {"pending", "pass", "fail", "n/a"};
    return names[r & 3];
}

// One read as the app does it: chunk 0, then the rest of that copy; parsed field by field.
static void read_left(void) {
    got_mask = 0;
    for (uint8_t c = 0; c < HEALTH_CHUNKS; c++) {
        ask(0, c);
    }
    if (got_mask != (1 << HEALTH_CHUNKS) - 1) {
        printk("health: incomplete block (chunks 0x%x)\n", got_mask);
        return;
    }
    struct health_wire h;
    memcpy(&h, block, sizeof(h));
    printk("health: block version %u, half %u, length %u, flags 0x%02X, build %u.%u.%u %.8s\n",
           h.version, h.half, h.length, h.flags, h.fw_major, h.fw_minor, h.fw_patch, h.sha);
    printk("health: self-test display %s, settings %s, relay %s, LEDs %s\n",
           state(h.selftest[HEALTH_CHECK_DISPLAY]), state(h.selftest[HEALTH_CHECK_SETTINGS]),
           state(h.selftest[HEALTH_CHECK_RELAY]), state(h.selftest[HEALTH_CHECK_LEDS]));
    printk("health: raw HID sent %s, reply drops %u; split connected %u, drops %u; pc state %02X, "
           "polarity %02X, LEDs %u, uptime %s\n",
           h.hid_sent > 0 ? "some" : "none", h.hid_reply_drops, h.split_connected, h.split_drops,
           h.pc_state, h.polarity, h.leds, h.uptime_s >= 1 ? "1 s or more" : "0 s");
    printk("health: raw HID per transport: USB failed %u; BLE sent %u, failed %u, skipped %s; "
           "last error USB %d, BLE %d\n",
           h.hid_usb_failed, h.hid_ble_sent, h.hid_ble_failed,
           h.hid_ble_skipped > 0 && h.hid_ble_skipped <= h.hid_sent ? "some, at most the sent"
                                                                   : "wrong",
           h.hid_usb_last_err, h.hid_ble_last_err);
}

void health_sim_driver(void) {
    k_msleep(1000); // the first block then says an uptime of 1 s
    {
        const uint8_t d[] = {0xD5, 0x01, 0x01};
        put(d, sizeof(d), 32);
    }
    note("one read: five chunks of one copy");
    read_left();

    note("six requests while the system queue can't run: four replies, two dropped");
    int before = replies;
    k_sched_lock();
    for (int i = 0; i < 6; i++) {
        const uint8_t d[] = {0xD7, 0x13, 0, 0};
        raise_report(d, sizeof(d), 32);
    }
    k_sched_unlock();
    k_msleep(300);
    printk("health: %d of 6 replies came\n", replies - before);
    note("the next read counts the two dropped");
    read_left();

    note("bad half, bad chunk, the right's chunk before its copy, the right without a relay");
    ask(2, 0);
    ask(0, HEALTH_CHUNKS);
    ask(1, 1);
    ask(1, 0);
    note("a report cut short reads the left's chunk 0");
    {
        const uint8_t d[] = {0xD7, 0x13};
        put(d, sizeof(d), 2);
    }
    note("done");
    posix_exit(0); // the end of the run: nothing may print after it
}
