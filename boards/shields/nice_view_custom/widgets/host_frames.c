/*
 * SET_FRAME (D7 0C, #272): per-LED frames from the PC for both halves (host_frames.h; the wire
 * format is in host_link.h). Left half only.
 * SPDX-License-Identifier: MIT
 *
 * Reports fill a staging frame; SHOW draws it. The left's lamps (its chain, lamp_parts.h) go to
 * its strip at once (lamp_strip.c, on ZMK's low-priority queue). The right's go to the art
 * relay at most every
 * RIGHT_GAP_MS, newest first, and the relay sends only the lamps that changed since the right's
 * last acknowledged frame (art_relay_central.c), so a static frame costs the split link nothing.
 * Both halves draw through their dim_strip, so the battery dim (#465) applies on each half on top
 * of the PC's colours.
 *
 * The first SHOW takes the lights from the keyboard's own effect. They go back on RELEASE, after
 * CONFIG_FOXGLOVE_HOST_FRAMES_TIMEOUT_S without a frame report, on a USB unplug while USB is the
 * endpoint, or on an endpoint change; the last three send D7 8C unsolicited (event byte 1).
 *
 * Start/stop (no polling):
 *   left draw   starts on SHOW; runs once.
 *   right send  starts on SHOW (at most every RIGHT_GAP_MS, coalesced); stops on the right's ack,
 *               link loss or the relay's retry cap.
 *   timeout     starts (restarts) on each frame report; stops on RELEASE or a hand-back.
 */

#include <string.h>

#include <zephyr/kernel.h>
#include <zephyr/spinlock.h>
#include <zephyr/sys/util.h>

#include <zmk/endpoints.h>
#include <zmk/event_manager.h>
#include <zmk/events/endpoint_changed.h>
#include <zmk/events/usb_conn_state_changed.h>
#include <zmk/usb.h>

#include "art_relay.h"
#if IS_ENABLED(CONFIG_FOXGLOVE_HOST_EFFECTS)
#include "host_effects.h"
#endif
#include "host_frames.h"
#include "host_link.h"
#include "lamp_strip.h"

// The right half's lamps go through the art relay (which the simulator fakes).
#define RIGHT (IS_ENABLED(CONFIG_NICE_VIEW_ART_RELAY) || IS_ENABLED(CONFIG_FOXGLOVE_SIM_HOSTLINK))
#define LEFT_LAMPS LAMP_STRIP_LEN
#define LAMPS (LEFT_LAMPS + (RIGHT ? ART_RELAY_LAMPS_MAX : 0))
BUILD_ASSERT(LAMPS <= UINT8_MAX, "lamp ids are one byte");

#define FRAME_SHOW BIT(0)
#define FRAME_RELEASE BIT(1)
#define FRAME_MAX_LAMPS 9 // 5 + 27 bytes: one 32-byte report
#define RIGHT_GAP_MS 100  // the right draws at most 10 frames a second
#define TIMEOUT K_SECONDS(CONFIG_FOXGLOVE_HOST_FRAMES_TIMEOUT_S)

enum { STATUS_OK = 0, STATUS_BAD = 5 };

// One lock for the frame and the owner: commands (the USB or Bluetooth thread), hand-backs (the
// system queue, event listeners) and the right's send take turns, so a hand-back never lands
// between taking the lights and drawing them. Spinlock, not mutex: the Raw HID callback must not
// block. Everything under it only copies bytes and queues work.
static struct k_spinlock lock;
static uint8_t frame[LAMPS][3]; // staging
static bool owned;              // the host link has the lights

static uint8_t right_state(void) {
#if RIGHT
    return art_relay_lamps_right();
#else
    return ART_LAMPS_NO_LINK;
#endif
}

static void fill_reply(uint8_t r[HOST_FRAMES_REPLY_SIZE], uint8_t status, bool event) {
    r[0] = HOST_LINK_SETTINGS;
    r[1] = 0x8C;
    r[2] = status;
    r[3] = owned ? HOST_FRAMES_OWNER_HOST_LINK : HOST_FRAMES_OWNER_NONE;
    r[4] = MIN(CONFIG_FOXGLOVE_HOST_FRAMES_TIMEOUT_S, UINT8_MAX);
    r[5] = right_state();
    r[6] = event;
}

#if RIGHT
static int64_t right_sent_at = -RIGHT_GAP_MS; // under lock

static void right_cb(struct k_work *work) {
    k_spinlock_key_t key = k_spin_lock(&lock);
    if (owned) { // else handed back meanwhile, and release() told the relay
        right_sent_at = k_uptime_get();
        art_relay_lamps(true, (const uint8_t(*)[3])&frame[LEFT_LAMPS]); // copies
    }
    k_spin_unlock(&lock, key);
}
static K_WORK_DELAYABLE_DEFINE(right_work, right_cb);
#endif

// Under lock.
static void show(void) {
    for (int i = 0; i < LEFT_LAMPS; i++) {
        lamp_strip_set(i, frame[i][0], frame[i][1], frame[i][2]);
    }
    lamp_strip_host(true);
    lamp_strip_show();
#if RIGHT
    // Coalesces: a pending send keeps its time and takes the newest frame when it runs.
    int64_t wait = right_sent_at + RIGHT_GAP_MS - k_uptime_get();
    k_work_schedule(&right_work, K_MSEC(CLAMP(wait, 0, RIGHT_GAP_MS)));
#endif
}

static void timeout_cb(struct k_work *work);
static K_WORK_DELAYABLE_DEFINE(timeout_work, timeout_cb);

// Hands both halves back to their own effect. Returns whether the host link had the lights.
// Under lock.
static bool release(void) {
    if (!owned) {
        return false;
    }
    owned = false;
    k_work_cancel_delayable(&timeout_work);
    lamp_strip_host(false);
    memset(frame, 0, sizeof(frame)); // a new take-over starts from black, as the strips do
#if RIGHT
    k_work_cancel_delayable(&right_work);
    right_sent_at = -RIGHT_GAP_MS;
    art_relay_lamps(false, (const uint8_t(*)[3])&frame[LEFT_LAMPS]);
#endif
    return true;
}

// Hand-backs the PC didn't ask for: it hears at once (it has sent frames, so it's listening).
static void release_event(void) {
    k_spinlock_key_t key = k_spin_lock(&lock);
    if (release()) {
        uint8_t r[HOST_FRAMES_REPLY_SIZE];
        fill_reply(r, STATUS_OK, true);
        host_link_send(r, sizeof(r)); // queued: goes out from the system queue
    }
    k_spin_unlock(&lock, key);
}

static void timeout_cb(struct k_work *work) { release_event(); }

int host_frames_parts(uint8_t *out, int max) {
    int n = 0;
    if (n < max) {
        out[n++] = LEFT_LAMPS;
    }
#if RIGHT
    if (n < max) {
        out[n++] = ART_RELAY_LAMPS_MAX;
    }
#endif
    return n;
}

void host_frames_yield(void) {
    k_spinlock_key_t key = k_spin_lock(&lock);
    release();
    k_spin_unlock(&lock, key);
}

// Under lock.
static bool command(const uint8_t *d, int len, uint8_t r[HOST_FRAMES_REPLY_SIZE]) {
    uint8_t flags = len >= 3 ? d[2] : 0, start = len >= 4 ? d[3] : 0, count = len >= 5 ? d[4] : 0;
    if (flags & FRAME_RELEASE) {
        release();
        fill_reply(r, STATUS_OK, false);
        return true;
    }
    if (count > FRAME_MAX_LAMPS || len < 5 + 3 * count || start + count > LAMPS) {
        fill_reply(r, STATUS_BAD, false);
        return true;
    }
    memcpy(frame[start], &d[5], 3 * count);
    if (CONFIG_FOXGLOVE_HOST_FRAMES_TIMEOUT_S > 0) {
        k_work_reschedule(&timeout_work, TIMEOUT);
    }
    if (!(flags & FRAME_SHOW)) {
        return false; // chunks don't reply, like the panel's ROWS
    }
#if IS_ENABLED(CONFIG_FOXGLOVE_HOST_EFFECTS)
    if (!owned) {
        host_effects_yield(); // a SET_EFFECT gives way: frames draw over it on both halves
    }
#endif
    owned = true;
    show();
    fill_reply(r, STATUS_OK, false);
    return true;
}

bool host_frames_command(const uint8_t *d, int len, uint8_t r[HOST_FRAMES_REPLY_SIZE]) {
    k_spinlock_key_t key = k_spin_lock(&lock);
    bool answer = command(d, len, r);
    k_spin_unlock(&lock, key);
    return answer;
}

// The frames came over a link that's gone: USB unplugged while it's the endpoint, or the
// endpoint changed (USB <-> BLE).
static int frames_link_listener(const zmk_event_t *eh) {
    if (as_zmk_endpoint_changed(eh) != NULL) {
        release_event();
    }
#if IS_ENABLED(CONFIG_ZMK_USB)
    else if (!zmk_usb_is_hid_ready() && zmk_endpoints_selected().transport == ZMK_TRANSPORT_USB) {
        release_event();
    }
#endif
    return ZMK_EV_EVENT_BUBBLE;
}

ZMK_LISTENER(host_frames, frames_link_listener);
ZMK_SUBSCRIPTION(host_frames, zmk_endpoint_changed);
#if IS_ENABLED(CONFIG_ZMK_USB)
ZMK_SUBSCRIPTION(host_frames, zmk_usb_conn_state_changed);
#endif
