/*
 * Simulator case for the host-held modes' lease (tests/zmk-sim/hold-lease, #847): the real
 * host_link.c (D7 15 HOLD_LEASE / D7 95), art_keys.c (&art_next, D7 0D) and knob.c (&knob, D7 14)
 * with the production keymap's bindings. This plays the PC (a live app, an app before #847, a
 * killed app), the art key (through the behaviour, as the keymap invokes it) and the encoder (the
 * behaviour's sensor API). Printed: the PC's requests and the probes here, the keyboard's reports
 * by hostlink_sim.c, the keys by ZMK's HID listener. Simulated time runs unslowed.
 * SPDX-License-Identifier: MIT
 */

#include <string.h>

#include <zephyr/device.h>
#include <zephyr/drivers/sensor.h>
#include <zephyr/kernel.h>
#include <zephyr/sys/byteorder.h>
#include <zephyr/sys/printk.h>

#include <dt-bindings/zmk/keys.h>
#include <drivers/behavior.h>
#include <raw_hid/events.h>
#include <zmk/behavior.h>
#include <zmk/event_manager.h>
#include <zmk/events/endpoint_changed.h>
#include <zmk/sensors.h>
#include <zmk/virtual_key_position.h>

#include "art_relay.h"
#include "host_link.h"
#include "posix_board_if.h"

// The art key's board-side step needs the right half; here it prints.
void art_relay_step(int dir) { printk("lease: board steps %d\n", dir); }

static void note(const char *what) { printk("lease: -- %s\n", what); }

static int caps_listener(const zmk_event_t *eh) {
    const struct raw_hid_sent_event *ev = as_raw_hid_sent_event(eh);
    if (ev != NULL && ev->length >= 7 && ev->data[0] == 0xD5 && ev->data[1] == 0x88) {
        printk("lease: CAPS word 2 HOLD_LEASE %u\n",
               (sys_get_le32(&ev->data[3]) & HOST_LINK_CAPS2_HOLD_LEASE) ? 1 : 0);
    }
    return ZMK_EV_EVENT_BUBBLE;
}

ZMK_LISTENER(hold_lease_sim, caps_listener);
ZMK_SUBSCRIPTION(hold_lease_sim, raw_hid_sent_event);

static void pc(const uint8_t *d, int len) {
    static uint8_t report[32];
    memset(report, 0, sizeof(report));
    memcpy(report, d, len);
    printk("lease: pc sends");
    for (int i = 0; i < len; i++) {
        printk(" %02X", d[i]);
    }
    printk("\n");
    raise_raw_hid_received_event((struct raw_hid_received_event){.data = report, .length = 32});
    k_msleep(200); // replies go out from the system work queue
}

#define PC(...)                                                                                    \
    do {                                                                                           \
        const uint8_t d_[] = {__VA_ARGS__};                                                        \
        pc(d_, sizeof(d_));                                                                        \
    } while (0)

// Quiet for `ms`: the PC sends nothing.
static void quiet(int ms) {
    printk("lease: pc quiet %d s\n", ms / 1000);
    k_msleep(ms);
}

// &art_next pressed and released, as the keymap invokes it.
static void art_key(void) {
    printk("lease: art key\n");
    struct zmk_behavior_binding b = {.behavior_dev = DEVICE_DT_NAME(DT_NODELABEL(art_next))};
    struct zmk_behavior_binding_event ev = {.position = 0, .timestamp = k_uptime_get()};
    zmk_behavior_invoke_binding(&b, ev, true);
    ev.timestamp = k_uptime_get();
    zmk_behavior_invoke_binding(&b, ev, false);
    k_msleep(200);
}

// One detent clockwise of the encoder, as ZMK's keymap hands it over.
static void turn(void) {
    printk("lease: turn 1\n");
    struct zmk_behavior_binding b = {
        .behavior_dev = DEVICE_DT_NAME(DT_NODELABEL(knob)),
        .param1 = C_VOLUME_UP,
        .param2 = C_VOLUME_DOWN,
    };
    struct zmk_behavior_binding_event ev = {
        .layer = 0,
        .position = ZMK_VIRTUAL_KEY_POSITION_SENSOR(0),
        .timestamp = k_uptime_get(),
    };
    const struct zmk_sensor_config cfg = {.triggers_per_rotation = 20};
    const struct zmk_sensor_channel_data data = {
        .value = {.val1 = 18, .val2 = 0},
        .channel = SENSOR_CHAN_ROTATION,
    };
    behavior_sensor_keymap_binding_accept_data(&b, ev, &cfg, 1, &data);
    behavior_sensor_keymap_binding_process(&b, ev, BEHAVIOR_SENSOR_BINDING_PROCESS_MODE_TRIGGER);
    k_msleep(200);
}

// Both probes: held, the art key goes to the PC (D7 8D 01 01) and the turn is a knob event (the
// target is an app); let go, the board steps the art and the turn is the volume key.
static void probe(void) {
    art_key();
    turn();
}

void hold_lease_sim_driver(void) {
    k_msleep(500);
    note("hello: word 2 has HOLD_LEASE");
    PC(0xD5, 0x01, 0x01);
    PC(0xD5, 0x08);

    note("an app before #847: no lease, the holds stay through a minute of silence");
    PC(0xD7, 0x0D, 0x01);
    PC(0xD7, 0x14, 0x01, 0x01);
    quiet(60000);
    probe();

    note("a lease of 30 s; a lease out of range answers 5 and changes nothing; a read");
    PC(0xD7, 0x15, 30);
    PC(0xD7, 0x15, 2);
    PC(0xD7, 0x15, 121);
    PC(0xD7, 0x15, 0xFF);

    note("a live app answers each ask (a third of the lease after its last report): held");
    for (int i = 0; i < 3; i++) {
        quiet(10500); // the ask comes at 10 s
        PC(0xD7, 0x15, 30);
    }
    probe();

    note("existing traffic renews: a report every 8 s, no ask");
    for (int i = 0; i < 5; i++) {
        quiet(8000);
        PC(0xD5, 0x01, 0x01);
    }
    probe();

    note("the app is killed: asks at 10 and 20 s, every hold let go at 30 s");
    quiet(35000);
    probe();

    note("the PC's next report hears of the lapse; it takes the holds back");
    PC(0xD5, 0x01, 0x01);
    PC(0xD7, 0x0D, 0x01);
    PC(0xD7, 0x14, 0x01, 0x01);
    probe();

    note("nothing held: nothing asked, nothing lapses");
    PC(0xD7, 0x0D, 0x00);
    PC(0xD7, 0x14, 0x00);
    quiet(40000);
    PC(0xD7, 0x15, 0xFF);

    note("the lease off (D7 15 00): a hold stays through silence");
    PC(0xD7, 0x15, 0x00);
    PC(0xD7, 0x0D, 0x01);
    quiet(40000);
    art_key();

    note("master volume (#855, D7 16) is a held mode too: asked for, and it lapses (mask 04)");
    PC(0xD7, 0x0D, 0x00);
    PC(0xD7, 0x15, 30);
    PC(0xD7, 0x16, 0x01);
    turn(); // master's turn goes to the PC
    quiet(35000);
    turn(); // the volume key: Windows' own popup again
    PC(0xD7, 0x16, 0xFF);
    PC(0xD7, 0x16, 0x01);
    turn();
    PC(0xD7, 0x16, 0x00);
    PC(0xD7, 0x0D, 0x01);

    note("the link drops (an endpoint change): the holds and the lease go");
    PC(0xD7, 0x15, 30);
    printk("lease: the endpoint changes\n");
    raise_zmk_endpoint_changed((struct zmk_endpoint_changed){0});
    k_msleep(200);
    PC(0xD7, 0x15, 0xFF);
    art_key();
    note("done");
    posix_exit(0); // the end of the run: nothing may print after it
}
