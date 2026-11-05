/*
 * Simulator case for a per-key scene on a zone board (tests/zmk-sim/host-zones, #611): the
 * SET_FRAME reports the app sends for a few looks on the Eyelash Corne as its profile ships (6
 * column zones per half, 15 lamps per half that light nothing, #608), played through the real
 * host_link.c and host_frames.c. frames_sim.c's fakes print what each half draws; this driver
 * holds both halves to the colours the app worked out from the profile's map: each zone's lamp
 * its zone's colour, every dead lamp black. The vectors come from
 * app/src/foxglove/keyboard_zone_vectors.rs.
 * SPDX-License-Identifier: MIT
 */

#include <string.h>

#include <zephyr/kernel.h>
#include <zephyr/sys/printk.h>
#include <zephyr/sys/util.h>

#include <raw_hid/events.h>

#include "art_relay.h"
#include "lamp_strip.h"

#define LAMPS (LAMP_STRIP_LEN + ART_RELAY_LAMPS_MAX)

void frames_sim_shown(uint8_t out[LAMPS][3]);

struct zone_vector {
    const char *name;
    uint8_t reports[5][32];
    uint8_t want[42][3];
};

static const struct zone_vector vectors[] = {
#include "zone_vectors.inc"
};

BUILD_ASSERT(LAMPS == 42, "the Corne's frame: 21 lamps a half");

static void put(const uint8_t *d, int len) {
    static uint8_t report[32];
    memset(report, 0, sizeof(report));
    memcpy(report, d, len);
    raise_raw_hid_received_event((struct raw_hid_received_event){.data = report, .length = 32});
}

static void note(const char *what) { printk("frames: -- %s\n", what); }

static void settle(void) { k_msleep(300); } // replies and the right's send run on the system queue

void zones_sim_driver(void) {
    k_msleep(500);
    {
        const uint8_t d[] = {0xD5, 0x01, 0x01};
        put(d, sizeof(d));
    }
    settle();
    int good = 0, lit_dead = 0;
    for (int v = 0; v < (int)ARRAY_SIZE(vectors); v++) {
        const struct zone_vector *c = &vectors[v];
        note(c->name);
        for (int r = 0; r < (int)ARRAY_SIZE(c->reports); r++) {
            put(c->reports[r], sizeof(c->reports[r]));
        }
        settle();
        uint8_t shown[LAMPS][3];
        frames_sim_shown(shown);
        bool ok = true;
        for (int i = 0; i < LAMPS; i++) {
            const uint8_t *want = c->want[i];
            bool dead = !(want[0] | want[1] | want[2]);
            if (memcmp(shown[i], want, 3) != 0) {
                ok = false;
                lit_dead += dead;
                printk("frames: lamp %d shows %02X%02X%02X, the app's zone colour %02X%02X%02X\n",
                       i, shown[i][0], shown[i][1], shown[i][2], want[0], want[1], want[2]);
            }
        }
        good += ok;
    }
    printk("frames: %d of %d looks show each zone's colour on both halves; %d dead lamps lit\n",
           good, (int)ARRAY_SIZE(vectors), lit_dead);
    note("RELEASE: both halves back to their effect");
    {
        const uint8_t d[] = {0xD7, 0x0C, 0x02};
        put(d, sizeof(d));
    }
    settle();
    note("done");
}
