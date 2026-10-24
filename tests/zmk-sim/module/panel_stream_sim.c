/*
 * Simulator driver for the left panel stream (tests/zmk-sim/panel-stream): plays a PC running the
 * plugin screens (#238). The focus timer redraws its 64x52 frame once a second (twice while a
 * paused frame blinks) and mic-mute on each change, so this streams whole frames (FILL, 18 ROWS,
 * PRESENT) at 2 Hz with HELLO and CONN_STATE requests in between, then a burst of frames back to
 * back, and checks every request got its reply, the last frame is the one shown, and the keys
 * pressed meanwhile (the case's kscan events) still type. Simulated time doesn't advance while
 * code runs, so this checks ordering and loss, not CPU time.
 * SPDX-License-Identifier: MIT
 */

#include <string.h>

#include <zephyr/kernel.h>
#include <zephyr/sys/byteorder.h>
#include <zephyr/sys/printk.h>

#include <raw_hid/events.h>
#include <zmk/event_manager.h>

#include "host_link.h"

#define ROWS_PER_REPORT 3
#define PACED_FRAMES 10 // 5 s at 2 Hz
#define BURST_FRAMES 30

static atomic_t hello_replies, conn_replies, other_replies;

static int stream_sent_listener(const zmk_event_t *eh) {
    const struct raw_hid_sent_event *ev = as_raw_hid_sent_event(eh);
    if (ev == NULL || ev->length < 2) {
        return ZMK_EV_EVENT_BUBBLE;
    }
    if (ev->data[0] == 0xD5 && ev->data[1] == 0x81) {
        atomic_inc(&hello_replies);
    } else if (ev->data[0] == 0xD7 && ev->data[1] == 0x84) {
        atomic_inc(&conn_replies);
    } else {
        atomic_inc(&other_replies);
    }
    return ZMK_EV_EVENT_BUBBLE;
}

ZMK_LISTENER(panel_stream_sim, stream_sent_listener);
ZMK_SUBSCRIPTION(panel_stream_sim, raw_hid_sent_event);

// One report, as hidapi sends it: no printing and no wait, like a PC streaming.
static void put(const uint8_t *d, int len) {
    static uint8_t report[32];
    memset(report, 0, sizeof(report));
    memcpy(report, d, len);
    raise_raw_hid_received_event((struct raw_hid_received_event){.data = report, .length = 32});
}

static const uint8_t fill_black[] = {0xD5, 0x02, 0x01};
static const uint8_t present[] = {0xD5, 0x04};

// Frame f: every byte of row y is (f + y) & 0xff, so the shown frame says which one it is.
static uint8_t frame_byte(int f, int y) { return (uint8_t)(f + y); }

static int send_frame(int f) {
    int reports = 0;
    const uint8_t fill[] = {0xD5, 0x02, 0x00};
    put(fill, sizeof(fill));
    reports++;
    for (int y = 0; y < HOST_PANEL_H; y += ROWS_PER_REPORT) {
        int n = MIN(ROWS_PER_REPORT, HOST_PANEL_H - y);
        uint8_t d[4 + ROWS_PER_REPORT * HOST_PANEL_STRIDE] = {0xD5, 0x03, y, n};
        for (int i = 0; i < n; i++) {
            memset(&d[4 + i * HOST_PANEL_STRIDE], frame_byte(f, y + i), HOST_PANEL_STRIDE);
        }
        put(d, 4 + n * HOST_PANEL_STRIDE);
        reports++;
    }
    put(present, sizeof(present));
    return reports + 1;
}

static bool shows_frame(int f) {
    static uint8_t buf[HOST_PANEL_BYTES];
    if (!host_panel_get(buf)) {
        return false;
    }
    for (int y = 0; y < HOST_PANEL_H; y++) {
        for (int x = 0; x < HOST_PANEL_STRIDE; x++) {
            if (buf[y * HOST_PANEL_STRIDE + x] != frame_byte(f, y)) {
                return false;
            }
        }
    }
    return true;
}

void panel_stream_driver(void) {
    const uint8_t hello[] = {0xD5, 0x01, 0x01};
    const uint8_t conn[] = {0xD7, 0x04};
    int reports = 0, requests = 0;

    // Paced, as the plugins draw: a frame, then HELLO and CONN_STATE, every 500 ms.
    for (int f = 0; f < PACED_FRAMES; f++) {
        reports += send_frame(f);
        put(hello, sizeof(hello));
        put(conn, sizeof(conn));
        requests++;
        k_msleep(500);
    }
    bool paced_shown = shows_frame(PACED_FRAMES - 1);

    // A burst, as an app catching up would send: frames back to back, then one HELLO.
    for (int f = 0; f < BURST_FRAMES; f++) {
        reports += send_frame(100 + f);
    }
    put(hello, sizeof(hello));
    requests++;
    k_msleep(500);
    bool burst_shown = shows_frame(100 + BURST_FRAMES - 1);

    // ROWS_AT (#289): the same kind of frame, byte-addressed in 25-byte pieces that cross rows,
    // then a piece past the last row and one past the row's end, which must change nothing.
    put(fill_black, sizeof(fill_black));
    for (int at = 0; at < HOST_PANEL_BYTES; at += 25) {
        int n = MIN(25, HOST_PANEL_BYTES - at);
        uint8_t d[7 + 25] = {0xD5, 0x07};
        sys_put_le16(at / HOST_PANEL_STRIDE, &d[2]);
        sys_put_le16(at % HOST_PANEL_STRIDE, &d[4]);
        d[6] = n;
        for (int i = 0; i < n; i++) {
            d[7 + i] = frame_byte(200, (at + i) / HOST_PANEL_STRIDE);
        }
        put(d, 7 + n);
        reports++;
    }
    put(present, sizeof(present));
    k_msleep(100);
    bool rows_at_shown = shows_frame(200);
    const uint8_t past_rows[] = {0xD5, 0x07, HOST_PANEL_H, 0, 0, 0, 1, 0xAA};
    const uint8_t past_row_end[] = {0xD5, 0x07, 0, 0, HOST_PANEL_STRIDE, 0, 1, 0xAA};
    put(past_rows, sizeof(past_rows));
    put(past_row_end, sizeof(past_row_end));
    put(present, sizeof(present));
    k_msleep(100);
    bool bad_ignored = shows_frame(200);

    k_msleep(3000); // after the case's key taps, so the summary always comes last
    printk("stream: %d paced + %d burst frames, %d panel reports\n", PACED_FRAMES, BURST_FRAMES,
           reports);
    printk("stream: HELLO replies %d of %d, CONN_STATE replies %d of %d, other %d\n",
           (int)atomic_get(&hello_replies), requests, (int)atomic_get(&conn_replies), PACED_FRAMES,
           (int)atomic_get(&other_replies));
    printk("stream: last paced frame shown: %s; last burst frame shown: %s\n",
           paced_shown ? "yes" : "no", burst_shown ? "yes" : "no");
    printk("stream: ROWS_AT frame shown: %s; out-of-range pieces ignored: %s\n",
           rows_at_shown ? "yes" : "no", bad_ignored ? "yes" : "no");
}
