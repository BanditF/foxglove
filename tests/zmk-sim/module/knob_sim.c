/*
 * Simulator case for the knob (tests/zmk-sim/knob, #719): the real knob.c (&knob, &knob_press)
 * and host_link.c (D7 14 KNOB / D7 94, D7 16 KNOB_VOLUME / D7 96 for #855's master volume), with
 * the production keymap's base-layer bindings. This
 * plays the PC and the encoder: the turns go through the behaviour's own sensor API (the
 * simulator has no encoder), the click through ZMK's keymap at position 34. Printed: the PC's
 * requests here, the keyboard's reports by hostlink_sim.c, the keys by ZMK's HID listener.
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
#include <zmk/events/position_state_changed.h>
#include <zmk/sensors.h>
#include <zmk/virtual_key_position.h>

#include "posix_board_if.h"

#define KNOB_CLICK_POSITION 34 // the encoder's switch on the Eyelash Corne

static void note(const char *what) { printk("knob: -- %s\n", what); }

static int hello_listener(const zmk_event_t *eh) {
    const struct raw_hid_sent_event *ev = as_raw_hid_sent_event(eh);
    if (ev != NULL && ev->length >= 25 && ev->data[0] == 0xD5 && ev->data[1] == 0x81) {
        printk("knob: HELLO caps bit 31 KNOB %u\n", (sys_get_le32(&ev->data[21]) >> 31) & 1);
    }
    return ZMK_EV_EVENT_BUBBLE;
}

ZMK_LISTENER(knob_sim, hello_listener);
ZMK_SUBSCRIPTION(knob_sim, raw_hid_sent_event);

static void pc(const uint8_t *d, int len) {
    static uint8_t report[32];
    memset(report, 0, sizeof(report));
    memcpy(report, d, len);
    printk("knob: pc sends");
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

// `detents` of the encoder (18 degrees each at 20 per rotation), as ZMK's keymap hands them over.
static void turn(int detents) {
    printk("knob: turn %d\n", detents);
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
        .value = {.val1 = detents * 18, .val2 = 0},
        .channel = SENSOR_CHAN_ROTATION,
    };
    behavior_sensor_keymap_binding_accept_data(&b, ev, &cfg, 1, &data);
    behavior_sensor_keymap_binding_process(&b, ev, BEHAVIOR_SENSOR_BINDING_PROCESS_MODE_TRIGGER);
    k_msleep(200);
}

static void knob_switch(bool down) {
    raise_zmk_position_state_changed((struct zmk_position_state_changed){
        .source = ZMK_POSITION_STATE_CHANGE_SOURCE_LOCAL,
        .state = down,
        .position = KNOB_CLICK_POSITION,
        .timestamp = k_uptime_get()});
}

static void click(void) {
    printk("knob: click\n");
    knob_switch(true);
    k_msleep(50);
    knob_switch(false);
    k_msleep(500); // a held click goes once the 300 ms double-click time has passed
}

static void double_click(void) {
    printk("knob: double click\n");
    knob_switch(true);
    k_msleep(50);
    knob_switch(false);
    k_msleep(100);
    knob_switch(true);
    k_msleep(50);
    knob_switch(false);
    k_msleep(400);
}

static void hold(void) {
    printk("knob: hold 700 ms\n");
    knob_switch(true);
    k_msleep(700); // the hold goes at 500 ms, before the release
    knob_switch(false);
    k_msleep(200);
}

void knob_sim_driver(void) {
    k_msleep(500);
    PC(0xD5, 0x01, 0x01);
    PC(0xD5, 0x08); // word 2: HOLD_LEASE (bit 2) and KNOB_VOLUME (bit 3, #855)

    note("not held: the turns and the click are the keys");
    turn(1);
    turn(-2);
    click();
    PC(0xD7, 0x14, 0xFF);

    note("held, master: the turns stay the keys; click and hold are events; a double mutes here");
    PC(0xD7, 0x14, 0x01, 0x00);
    turn(1);
    click();
    double_click();
    hold();

    note("held, an app: the turns are events (a burst is one), and so is everything else");
    PC(0xD7, 0x14, 0x01, 0x01);
    turn(3);
    turn(-1);
    click();
    double_click();
    hold();

    note("master volume (#855): held on master, the turns are events and a double taps no key");
    PC(0xD7, 0x16, 0xFF);
    PC(0xD7, 0x14, 0x01, 0x00);
    PC(0xD7, 0x16, 0x01);
    turn(2);
    click();
    double_click();

    note("the knob handed back, master volume still held: turns to the PC, the click is mute");
    PC(0xD7, 0x14, 0x00);
    turn(-1);
    click();

    note("master volume handed back: master's turns are the volume keys again; bad is 5");
    PC(0xD7, 0x14, 0x01, 0x00);
    PC(0xD7, 0x16, 0x00);
    turn(1);
    PC(0xD7, 0x16, 0x02);
    PC(0xD7, 0x16, 0x01);

    note("the endpoint changes: the board drops both holds and says so; the keys again");
    printk("knob: the endpoint changes\n");
    raise_zmk_endpoint_changed((struct zmk_endpoint_changed){0});
    k_msleep(200);
    turn(1);
    click();

    note("a bad request changes nothing; handing back is the keys again");
    PC(0xD7, 0x14, 0x02, 0x00);
    PC(0xD7, 0x14, 0x01, 0x01);
    PC(0xD7, 0x14, 0x00);
    turn(-1);
    note("done");
    posix_exit(0); // the end of the run: nothing may print after it
}
