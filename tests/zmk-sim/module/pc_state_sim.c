/*
 * Simulator case for the PC state (tests/zmk-sim/pc-state, #609, #604): the real pc_state.c,
 * host_link.c (D7 11 / D7 91), rgb_idle.c and activity_relay.c on ZMK's real activity, at the
 * production idle (30 s), USB idle (600 s, from the shield's Kconfig.defconfig) and sleep (30 min,
 * config/eyelash_corne.conf) times. Faked here: ZMK's underglow (each on/off prints "LEDs"), the
 * cable (pc_state_read_usb), and the relay to the right (prints what the right would be sent).
 * The driver plays the app; simulated time runs unslowed, and the run ends at ZMK's deep sleep.
 * SPDX-License-Identifier: MIT
 */

#include <string.h>

#include <zephyr/input/input.h>
#include <zephyr/kernel.h>
#include <zephyr/sys/byteorder.h>
#include <zephyr/sys/printk.h>

#include <raw_hid/events.h>
#include <zmk/activity.h>
#include <zmk/event_manager.h>
#include <zmk/rgb_underglow.h>

#include "activity_relay.h"
#include "art_relay.h"
#include "pc_state.h"
#include "rgb_idle.h"

// ZMK_LISTENER(activity, ...) in ZMK's activity.c, as usb_wake.c uses it.
extern const struct zmk_listener zmk_listener_activity;

static bool watching; // prints start with the driver, after boot

static void note(const char *what) { printk("pcstate: -- %s\n", what); }

/* The cable */

static bool usb;

bool pc_state_read_usb(void) { return usb; }

// usb_wake.c's note when a cable goes in: on the system work queue, as there.
static void usb_wake_cb(struct k_work *work) { zmk_listener_activity.callback(NULL); }
static K_WORK_DEFINE(usb_wake_work, usb_wake_cb);

static void cable(bool in, bool wake) {
    printk("pcstate: -- the cable %s\n", in ? "goes in" : "comes out");
    usb = in;
    pc_state_usb_changed();
    if (in && wake) {
        k_work_submit(&usb_wake_work);
    }
    k_msleep(200);
}

/* ZMK's underglow (rgb_underglow.c, which the simulator doesn't build) */

static bool zmk_on;
static struct zmk_led_hsb hsb = {.h = 30, .s = 80, .b = 60};

static void leds(bool on) {
    if (zmk_on != on && watching) {
        printk("pcstate: LEDs %s\n", on ? "on" : "off");
    }
    zmk_on = on;
}

int zmk_rgb_underglow_get_state(bool *on) {
    *on = zmk_on;
    return 0;
}
int zmk_rgb_underglow_on(void) {
    leds(true);
    return 0;
}
int zmk_rgb_underglow_off(void) {
    leds(false);
    return 0;
}
struct zmk_led_hsb zmk_rgb_underglow_calc_hue(int direction) { return hsb; }
int zmk_rgb_underglow_calc_effect(int direction) { return 0; }
int zmk_rgb_underglow_set_hsb(struct zmk_led_hsb color) {
    hsb = color;
    return 0;
}
int zmk_rgb_underglow_select_effect(int e) { return 0; }

/* The relay to the right (art_relay_central.c): what each op would carry now */

static int8_t rgb_on = -1, rgb_own = -1, activity = -1;

void art_relay_rgb_kick(void) {
    bool on, own;
    rgb_idle_relay_on(&on, &own);
    if (rgb_on != on || rgb_own != own) {
        rgb_on = on;
        rgb_own = own;
        if (watching) {
            printk("pcstate: to the right: RGB on %d (the left's own %d)\n", on, own);
        }
    }
}

void art_relay_activity_kick(void) {
    uint8_t op[2];
    art_relay_activity_op(op);
    if (activity != op[1]) {
        activity = op[1];
        if (watching) {
            printk("pcstate: to the right: %02X %02X\n", op[0], op[1]);
        }
    }
}

void art_relay_pc_state_kick(void) {
    uint8_t op[PC_STATE_RELAY_SIZE];
    uint32_t changes;
    pc_state_relay_op(op, &changes);
    printk("pcstate: to the right: %02X %02X lease %u\n", op[0], op[1], sys_get_le16(&op[2]));
}

/* The keyboard's reports to the PC: HELLO's bit and every D7 91 */

static void flags_text(uint8_t f, char out[32]) {
    out[0] = 0;
    if (f & PC_STATE_HELD) {
        strcat(out, " held");
    }
    if (f & PC_STATE_USB) {
        strcat(out, " usb");
    }
    if (f & PC_STATE_KEYS) {
        strcat(out, " keys");
    }
    if (f & PC_STATE_ACTIVE) {
        strcat(out, " active");
    }
    if (out[0] == 0) {
        strcat(out, " -");
    }
}

static bool asked_83;     // the driver asked for RGB_STATE: print the reply, not the events
static bool lease_hidden; // a read whose lease seconds depend on the driver's pace

static int sent_listener(const zmk_event_t *eh) {
    const struct raw_hid_sent_event *ev = as_raw_hid_sent_event(eh);
    if (ev == NULL || ev->length < 8) {
        return ZMK_EV_EVENT_BUBBLE;
    }
    const uint8_t *d = ev->data;
    if (d[0] == 0xD5 && d[1] == 0x81) {
        printk("pcstate: HELLO caps bit 26 PC_STATE %d\n", (sys_get_le32(&d[21]) >> 26) & 1);
    }
    if (d[0] == 0xD7 && d[1] == 0x91) {
        char f[32];
        flags_text(d[5], f);
        if (lease_hidden) {
            printk("pcstate: keyboard D7 91: status %u, event %u, state %u, flags%s, lease -, "
                   "reason %u\n",
                   d[2], d[3], d[4], f, d[8]);
        } else {
            printk("pcstate: keyboard D7 91: status %u, event %u, state %u, flags%s, lease %u, "
                   "reason %u\n",
                   d[2], d[3], d[4], f, sys_get_le16(&d[6]), d[8]);
        }
    }
    if (asked_83 && ev->length >= 29 && d[0] == 0xD7 && d[1] == 0x83) {
        asked_83 = false;
        const uint8_t s = d[17]; // the left's RGB_SHOWN flags, as foxglove-lab status prints them
        printk("pcstate: keyboard D7 83: left on %u; shown lit %d, wanted %d, idle %d\n", d[2],
               !!(s & RGB_SHOWN_LIT), !!(s & RGB_SHOWN_WANTED), !!(s & RGB_SHOWN_IDLE));
    }
    return ZMK_EV_EVENT_BUBBLE;
}

ZMK_LISTENER(pc_state_sim, sent_listener);
ZMK_SUBSCRIPTION(pc_state_sim, raw_hid_sent_event);

/* The app */

static void put(const uint8_t *d, int len) {
    static uint8_t report[32];
    memset(report, 0, sizeof(report));
    memcpy(report, d, len);
    raise_raw_hid_received_event((struct raw_hid_received_event){.data = report, .length = 32});
    k_msleep(200); // replies go out from the system work queue
}

// D7 11 as crates/foxglove-link builds it.
static void pc(uint8_t state, uint8_t flags, uint16_t lease) {
    uint8_t d[6] = {0xD7, 0x11, state, flags};
    sys_put_le16(lease, &d[4]);
    printk("pcstate: PC sends D7 11 %02X %02X lease %u\n", state, flags, lease);
    put(d, sizeof(d));
}

#define AWAY() pc(PC_STATE_REQ_AWAY, 0, 0)
#define AWAKE(lease) pc(PC_STATE_REQ_AWAKE, 0, lease)
#define BACK(lease) pc(PC_STATE_REQ_AWAKE, PC_STATE_WAKE, lease)
#define LEASE 600 // the app's (app/src/foxglove/pc_state.rs)

// A key (pointer input reaches ZMK's activity the same way, and pc_state.c's).
static void key(void) {
    note("a key");
    input_report_rel(NULL, INPUT_REL_X, 1, true, K_NO_WAIT);
    k_msleep(100);
}

// What this half shows and its state, read directly (no report, so nothing else prints).
static void state(const char *when) {
    struct pc_state_wire w;
    pc_state_report(&w);
    char f[32];
    flags_text(w.flags, f);
    printk("pcstate: %s: LEDs %s, state %u, flags%s\n", when, zmk_on ? "on" : "off", w.state, f);
}

// The user's &rgb_ug on/off key, as rgb_idle.c picks it up after a binding.
static void user_rgb(bool on) {
    printk("pcstate: -- the user turns the underglow %s\n", on ? "on" : "off");
    leds(on);
    rgb_idle_check();
    k_msleep(200);
}

static void rgb_state(void) {
    const uint8_t d[] = {0xD7, 0x03};
    asked_83 = true;
    put(d, sizeof(d));
}

static void read_pc_state(void) {
    lease_hidden = true;
    pc(PC_STATE_REQ_READ, 0, 0);
    lease_hidden = false;
}

static void until(int64_t at_ms) { k_sleep(K_TIMEOUT_ABS_MS(at_ms)); }

void pc_state_sim_driver(void) {
    k_msleep(1000);
    watching = true;
    {
        const uint8_t d[] = {0xD5, 0x01, 0x01};
        put(d, sizeof(d));
    }
    state("start, on battery");

    note("on battery, as before: idle 30 s after the last key");
    key();
    int64_t t = k_uptime_get();
    until(t + 29000);
    state("29 s after the key");
    until(t + 32000);
    state("32 s after the key");

    note("plugged in, the app not running: lit 10 minutes after the last key (#609 decision 2)");
    cable(true, true); // usb_wake.c notes activity, as for a key
    t = k_uptime_get();
    until(t + 35000);
    state("35 s after the cable, no key");
    until(t + 590000);
    state("9 min 50 s after it");
    until(t + 605000);
    state("10 min 5 s after it");

    note("the app: the PC awake, with the wake (the app starting, or #604's back)");
    note("bad requests first: a lease of 0, an unknown state; nothing changes");
    AWAKE(0);
    pc(3, 0, 0);
    BACK(LEASE);
    t = k_uptime_get();
    until(t + 35000);
    state("PC awake, 35 s after the wake: no idle at 30 s");
    note("a minute before the lease ends the keyboard asks; the app, the PC awake, renews it");
    until(t + 545000);
    AWAKE(LEASE);
    state("renewed");

    note("away (the display goes off): idle at once");
    AWAY();
    state("away");
    note("back (the display on, or an unlock): awake at once (#604)");
    BACK(LEASE);
    state("back");
    note("away 5 s after a key: idle at once all the same");
    key();
    k_msleep(5000);
    AWAY();
    state("away within ZMK's 30 s");
    note("a key ends away: lit again, the USB idle time's rules until the app says more");
    key();
    state("a key after away");

    note("the app closes mid-lease: no renewals; lit 10 minutes after the last key, then idle");
    AWAKE(300);
    t = k_uptime_get();
    until(t + 60000);
    key();
    until(t + 310000);
    state("the lease lapsed, 4 min 10 s after the last key");
    until(t + 650000);
    state("9 min 50 s after the last key");
    until(t + 670000);
    state("10 min 10 s after the last key");

    note("unplugged while the PC holds it: the hold ends at once");
    BACK(LEASE);
    int64_t last = k_uptime_get(); // ZMK's last activity from here on
    until(last + 35000);
    state("held, 35 s after the wake");
    note("#651's hardware reading: with the user's underglow off, RGB_STATE says off while the "
         "hold runs; the shown idle flag and D7 91 tell the hold apart");
    user_rgb(false);
    rgb_state();
    read_pc_state();
    user_rgb(true);
    rgb_state();
    cable(false, false);
    state("unplugged");

    note("as the right (#609 decision 3): the PC state from the left; the left's keys idle, the "
         "left's own on/off on; the right's own keys don't count while it follows the left");
    pc_state_remote(PC_STATE_AWAKE, LEASE);
    rgb_idle_remote_wanted(false, true, true);
    k_msleep(200);
    state("the right on battery, the left held lit");
    cable(true, false);
    state("the right on USB power: held, with the left's own on/off");
    pc_state_remote(PC_STATE_AWAY, 0);
    k_msleep(200);
    state("the left relays away");
    pc_state_remote(PC_STATE_AWAKE, LEASE);
    k_msleep(200);
    state("the left relays awake");
    cable(false, false);
    state("the right unplugged: its battery rules again");
    rgb_idle_remote_lost();
    k_msleep(200);

    note("on battery: ZMK's own deep sleep 30 minutes after the last activity, unchanged");
    until(last + 1790000);
    state("29 min 50 s after the last activity");
    // ZMK sleeps at 30 min: SIM_SLEEP ends the run (module/stubs.c).
    k_sleep(K_FOREVER);
}
