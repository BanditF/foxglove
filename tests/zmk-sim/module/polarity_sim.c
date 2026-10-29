/*
 * Simulator case for the panel polarity (tests/zmk-sim/panel-polarity, #666): the real
 * panel_polarity.c and host_link.c (D7 12 / D7 92), as the left of a keyboard with the relay. Faked
 * here: the drawers (panel_polarity_apply prints what would be redrawn), the settings backend
 * (panel_polarity_store prints the write; a reboot's load goes through settings_runtime_set, which
 * calls the real handler as settings_load does) and the relay to the right (prints the op).
 * SPDX-License-Identifier: MIT
 */

#include <string.h>

#include <zephyr/kernel.h>
#include <zephyr/settings/settings.h>
#include <zephyr/sys/byteorder.h>
#include <zephyr/sys/printk.h>

#include <raw_hid/events.h>
#include <zmk/event_manager.h>

#include "panel_polarity.h"

static bool watching;

static void note(const char *what) { printk("polarity: -- %s\n", what); }

static const char *name(uint8_t p) {
    return p == PANEL_POLARITY_DARK ? "dark" : p == PANEL_POLARITY_LIGHT ? "light" : "?";
}

/* The drawers, the settings and the relay */

void panel_polarity_apply(void) {
    if (watching) {
        printk("polarity: redraw, %s\n", name(panel_polarity_get()));
    }
}

int panel_polarity_store(uint8_t p) {
    printk("polarity: settings nvc_pol/v = %02X\n", p);
    return 0;
}

void art_relay_polarity_kick(void) {
    uint8_t op[PANEL_POLARITY_RELAY_SIZE];
    panel_polarity_relay_op(op);
    printk("polarity: to the right: %02X %02X\n", op[0], op[1]);
}

/* The keyboard's reports: HELLO's bit and every D7 92 */

static int sent_listener(const zmk_event_t *eh) {
    const struct raw_hid_sent_event *ev = as_raw_hid_sent_event(eh);
    if (ev == NULL || ev->length < 6) {
        return ZMK_EV_EVENT_BUBBLE;
    }
    const uint8_t *d = ev->data;
    if (d[0] == 0xD5 && d[1] == 0x81) {
        printk("polarity: HELLO caps bit 27 PANEL_POLARITY %d\n",
               (sys_get_le32(&d[21]) >> 27) & 1);
    }
    if (d[0] == 0xD7 && d[1] == 0x92) {
        printk("polarity: keyboard D7 92: status %u, polarity %u, default %u, right %02X\n", d[2],
               d[3], d[4], d[5]);
    }
    return ZMK_EV_EVENT_BUBBLE;
}

ZMK_LISTENER(polarity_sim, sent_listener);
ZMK_SUBSCRIPTION(polarity_sim, raw_hid_sent_event);

/* The app */

static void put(const uint8_t *d, int len, int report_len) {
    static uint8_t report[32];
    memset(report, 0, sizeof(report));
    memcpy(report, d, len);
    raise_raw_hid_received_event(
        (struct raw_hid_received_event){.data = report, .length = report_len});
    k_msleep(200); // replies go out from the system work queue
}

// D7 12 as crates/foxglove-link builds it.
static void polarity(uint8_t p) {
    const uint8_t d[] = {0xD7, 0x12, p};
    printk("polarity: PC sends D7 12 %02X\n", p);
    put(d, sizeof(d), 32);
}

// A reboot's settings load, as ZMK's settings_load hands the stored bytes to the handler.
static void load(const void *v, size_t len) {
    int err = settings_runtime_set("nvc_pol/v", v, len);
    printk("polarity: -- the settings load %u byte(s) (err %d)\n", (unsigned)len, err);
    k_msleep(100);
}

void polarity_sim_driver(void) {
    k_msleep(500);
    watching = true;
    {
        const uint8_t d[] = {0xD5, 0x01, 0x01};
        put(d, sizeof(d), 32);
    }
    note("the default (Kconfig): dark, nothing stored");
    polarity(PANEL_POLARITY_READ);

    note("the app sets light: this half redraws, the right is sent it, one save 2 s later");
    polarity(PANEL_POLARITY_LIGHT);
    k_msleep(2500);

    note("light again: nothing changes, nothing is sent or saved");
    polarity(PANEL_POLARITY_LIGHT);
    k_msleep(2500);

    note("a bad value: status 5, nothing changes");
    polarity(7);
    note("a report cut short is a read");
    {
        const uint8_t d[] = {0xD7, 0x12};
        put(d, sizeof(d), 2);
    }

    note("dark, then light again within the save delay: two redraws, no save (light is stored)");
    polarity(PANEL_POLARITY_DARK);
    polarity(PANEL_POLARITY_LIGHT);
    k_msleep(2500);

    note("dark: one save");
    polarity(PANEL_POLARITY_DARK);
    k_msleep(2500);

    note("as the right: the left's light over the relay is drawn and saved; a bad value ignored");
    panel_polarity_remote(PANEL_POLARITY_LIGHT);
    panel_polarity_remote(9);
    k_msleep(2500);

    note("a reboot: the stored dark comes back from the settings, drawn once");
    {
        const uint8_t v = PANEL_POLARITY_DARK;
        load(&v, sizeof(v));
    }
    polarity(PANEL_POLARITY_READ);
    note("a stored value of the wrong size or out of range is ignored");
    {
        const uint8_t bad[2] = {1, 1};
        load(bad, sizeof(bad));
        const uint8_t v = 5;
        load(&v, sizeof(v));
    }
    polarity(PANEL_POLARITY_READ);
    note("light after the load: saved, as dark was the stored value");
    polarity(PANEL_POLARITY_LIGHT);
    k_msleep(2500);
    note("done");
}
