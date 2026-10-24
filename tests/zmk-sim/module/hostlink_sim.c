/*
 * Simulator driver for the host-link protocol (tests/zmk-sim/hostlink): plays the PC. It sends
 * Raw HID reports into the real host_link.c and prints every request and reply, so the case's
 * snapshot pins the wire format and the refusal, gating and nonce rules.
 * SPDX-License-Identifier: MIT
 */

#include <string.h>

#include <zephyr/kernel.h>
#include <zephyr/sys/byteorder.h>
#include <zephyr/sys/printk.h>

#include <raw_hid/events.h>
#include <zmk/behavior.h>
#include <zmk/display.h>
#include <zmk/event_manager.h>

/* The Raw HID module's events, and what the board's other widgets would provide */

ZMK_EVENT_IMPL(raw_hid_received_event);
ZMK_EVENT_IMPL(raw_hid_sent_event);

bool zmk_display_is_initialized(void) { return false; }
struct k_work_q *zmk_display_work_q(void) { return &k_sys_work_q; }
void host_panel_redraw(void) {}
// The screen readback needs the nice!view; without it D7 06/07 get no reply here.
void screen_snap_command(const uint8_t *d, int len) {}

/* Replies: printed, and the last one kept for the driver */

static uint8_t last[32];

// Prints a report up to its last non-zero byte. ARM nonces are random: shown as "nonce".
static void print_report(const char *dir, const uint8_t *d, int len) {
    int n = len;
    while (n > 2 && d[n - 1] == 0) {
        n--;
    }
    bool nonce = d[0] == 0xD7 && d[1] == 0x88 && n >= 7 && dir[0] == 'r' && d[2] == 0;
    printk("hostlink: %s", dir);
    for (int i = 0; i < n; i++) {
        if (nonce && i >= 3 && i <= 6) {
            if (i == 3) {
                printk(" nonce");
            }
            continue;
        }
        printk(" %02X", d[i]);
    }
    printk("\n");
}

static int sent_listener(const zmk_event_t *eh) {
    const struct raw_hid_sent_event *ev = as_raw_hid_sent_event(eh);
    if (ev != NULL) {
        memset(last, 0, sizeof(last));
        memcpy(last, ev->data, MIN(ev->length, sizeof(last)));
        print_report("rx", ev->data, ev->length);
    }
    return ZMK_EV_EVENT_BUBBLE;
}

ZMK_LISTENER(hostlink_sim, sent_listener);
ZMK_SUBSCRIPTION(hostlink_sim, raw_hid_sent_event);

/* The PC */

static void send(const uint8_t *d, int len) {
    static uint8_t report[32];
    memset(report, 0, sizeof(report));
    memcpy(report, d, len);
    print_report("tx", report, sizeof(report));
    memset(last, 0, sizeof(last));
    raise_raw_hid_received_event((struct raw_hid_received_event){.data = report, .length = 32});
    k_msleep(200); // replies go out from the system work queue
}

#define SEND(...)                                                                                  \
    do {                                                                                           \
        const uint8_t d_[] = {__VA_ARGS__};                                                        \
        send(d_, sizeof(d_));                                                                      \
    } while (0)

static void note(const char *what) { printk("hostlink: -- %s\n", what); }

static void invoke(const char *behavior, uint32_t p1, uint32_t p2, uint16_t hold) {
    uint8_t d[14] = {0xD7, 0x02};
#if IS_ENABLED(CONFIG_ZMK_BEHAVIOR_LOCAL_IDS)
    sys_put_le16(zmk_behavior_get_local_id(behavior), &d[2]);
#endif
    sys_put_le32(p1, &d[4]);
    sys_put_le32(p2, &d[8]);
    sys_put_le16(hold, &d[12]);
    printk("hostlink: -- invoke %s %u %u\n", behavior, p1, p2);
    send(d, sizeof(d));
}

static void trigger(uint8_t half, uint32_t nonce) {
    uint8_t d[8] = {0xD7, 0x08, 0x01, half};
    sys_put_le32(nonce, &d[4]);
    send(d, sizeof(d));
}

static uint32_t arm(void) {
    SEND(0xD7, 0x08, 0x00);
    return sys_get_le32(&last[3]);
}

static void driver(void) {
    note("hello: version, caps, panel timeout, build");
    SEND(0xD5, 0x01, 0x01);

    note("build info: left, right (no relay here), bad half");
    SEND(0xD7, 0x09, 0x00);
    SEND(0xD7, 0x09, 0x01);
    SEND(0xD7, 0x09, 0x07);

    note("conn state");
    SEND(0xD7, 0x04);

    note("invoke: a key, a refused behavior, an unknown id");
    invoke("key_press", 0x07001D /* Z */, 0, 0);
    invoke("bluetooth", 0 /* BT_SEL */, 0, 0);
    invoke("sysreset", 0, 0, 0);
    {
        const uint8_t d[] = {0xD7, 0x02, 0xFE, 0xFF};
        send(d, sizeof(d));
    }

    note("position: press and release Q (pos 1)");
    SEND(0xD7, 0x05, 1, 1);
    SEND(0xD7, 0x05, 1, 0);
    note("position: refused on SYSTEM (BT_SEL 0 under Q), then back to QWERTY");
    invoke("toggle_layer", 6, 0, 0);
    SEND(0xD7, 0x05, 1, 1);
    invoke("toggle_layer", 6, 0, 0);
    note("position: bad position, bad state, short report");
    SEND(0xD7, 0x05, 99, 1);
    SEND(0xD7, 0x05, 1, 2);
    SEND(0xD7, 0x05, 1);
    note("position: W (pos 2) held, released by the firmware after 10 s");
    SEND(0xD7, 0x05, 2, 1);
    for (int i = 0; i < 6; i++) {
        SEND(0xD7, 0x04); // host traffic, so only the 10 s limit applies
        k_msleep(1800);
    }
    note("position: W released");

    note("bootloader: trigger without arm, wrong nonce, used nonce, expired nonce");
    trigger(1, 0x12345678);
    uint32_t nonce = arm();
    trigger(1, nonce ^ 1);
    trigger(1, nonce); // used up by the wrong one
    nonce = arm();
    trigger(1, nonce); // right half: accepted, but there's no relay here
    trigger(1, nonce); // single use
    nonce = arm();
    k_msleep(5200);
    trigger(1, nonce); // expired
    note("bootloader: bad sub-command");
    SEND(0xD7, 0x08, 0x05);

    note("done");
}

K_THREAD_DEFINE(hostlink_driver, 2048, driver, NULL, NULL, NULL, 7, 0, 500);
