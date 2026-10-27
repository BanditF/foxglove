/*
 * SET_EFFECT (D7 0F, #490): an effect from the PC that each half draws on its own from its lamp
 * positions and the shared clock (host_effects.h; the wire format is in host_link.h). Left only.
 * SPDX-License-Identifier: MIT
 *
 * The PC sends the effect once; the left draws its half (effect_run.c) and sends the right its
 * half's copy over the art relay, which re-anchors the right's clock on every send. Nothing is
 * streamed: the effect runs on with the app closed or the cable out, until RELEASE, SET_FRAME
 * taking the lights, or a reboot. The scene owns the lights (#542): each half draws it whatever
 * its underglow on/off and idle say, and RELEASE puts that on/off back. The battery dim applies
 * on each half.
 *
 * Start/stop (no polling): the left's frames are effect_run.c's; the right's send starts on an
 * effect, a PLACE or RELEASE, or the relay coming up while one runs, and stops on the right's ack,
 * link loss or the relay's retry cap (art_relay_central.c). Each wake from idle re-anchors the
 * right's clock to the left's, so the halves' crystals can't drift apart over a long run.
 */

#include <string.h>

#include <zephyr/kernel.h>
#include <zephyr/spinlock.h>
#include <zephyr/sys/byteorder.h>

#include <zmk/activity.h>
#include <zmk/event_manager.h>
#include <zmk/events/activity_state_changed.h>

#include "art_relay.h"
#include "effect_run.h"
#include "host_effects.h"
#include "host_frames.h"
#include "host_link.h"

// The right half goes through the art relay (which the simulator fakes).
#define RIGHT (IS_ENABLED(CONFIG_NICE_VIEW_ART_RELAY) || IS_ENABLED(CONFIG_FOXGLOVE_SIM_HOSTLINK))

enum { OP_RELEASE = 0, OP_PLACE = 1, OP_EFFECT = 2, OP_LAMPS = 3 };
enum { STATUS_OK = 0, STATUS_BAD = 5 };
enum { ROOM, LEFT, RIGHT_HALF };

#define EFFECT_COLOURS_AT 19
#define PLACE_LEN (3 + 3 * 4 * 2)

static struct k_spinlock lock;
static struct lamp_effect_wire effect; // the PC's effect; room and half filled per half
// Before any PLACE: the halves side by side, 80 mm apart, as the scene's room puts them, and the
// room just them.
static int16_t place[3][4] = {{0, 0, 328, 86}, {0, 0, 124, 86}, {204, 0, 124, 86}};
static bool running;
// EFFECT op LAMPS (#550): each half's own LED points from the PC, once any came (else the
// built-in table). RAM only, like PLACE: the PC sends them before PLACE, and a reboot ends the
// effect anyway (a right that reboots gets them again from here on link-up).
static uint8_t lamp_pts[2][LAMP_EFFECT_LAMPS][4];
static bool lamps_on[2];

#define LAMPS_AT 6
#define LAMPS_MAX ((CONFIG_RAW_HID_REPORT_SIZE - LAMPS_AT) / 4)

static uint8_t right_state(void) {
#if RIGHT
    return art_relay_effect_right();
#else
    return ART_LAMPS_NO_LINK;
#endif
}

static void fill_reply(uint8_t r[HOST_EFFECTS_REPLY_SIZE], uint8_t status) {
    r[0] = HOST_LINK_SETTINGS;
    r[1] = 0x8F;
    r[2] = status;
    r[3] = running ? HOST_FRAMES_OWNER_HOST_LINK : HOST_FRAMES_OWNER_NONE;
    r[4] = running ? effect.kind : LAMP_EFFECT_NONE;
    r[5] = right_state();
    r[6] = 0; // no unsolicited D7 8F yet: nothing hands an effect back on its own
}

static void for_half(struct lamp_effect_wire *w, int half) {
    *w = effect;
    for (int i = 0; i < 4; i++) {
        sys_put_le16(place[ROOM][i], &w->room[2 * i]);
        sys_put_le16(place[half][i], &w->half[2 * i]);
    }
    w->lamps_set = lamps_on[half - LEFT];
    if (w->lamps_set) {
        memcpy(w->lamp, lamp_pts[half - LEFT], sizeof(w->lamp));
    }
}

// Draws the effect on both halves, the clock at at_ms now. Under lock.
static void run(uint32_t at_ms) {
    static struct lamp_effect_wire w; // under lock: one at a time, off the caller's stack
    running = true;
#if RIGHT
    for_half(&w, RIGHT_HALF);
    art_relay_effect(true, true, &w); // the relay takes the left's clock when it sends
#endif
    for_half(&w, LEFT);
    effect_run_start(&w, at_ms);
}

// Under lock.
static void stop(bool hand_back) {
    if (!running) {
        return;
    }
    running = false;
    effect_run_stop(hand_back);
#if RIGHT
    art_relay_effect(false, hand_back, NULL);
#endif
}

// A whole EFFECT report into e; false if it doesn't hold together.
static bool parse_effect(const uint8_t *d, int len, struct lamp_effect_wire *e, uint32_t *at) {
    if (len < EFFECT_COLOURS_AT) {
        return false;
    }
    uint8_t kind = d[3], n = d[4];
    bool colours_ok = kind == LAMP_EFFECT_SWEEP ? n >= 1 && n <= LAMP_EFFECT_COLOURS : n == 2;
    if (kind == LAMP_EFFECT_NONE || kind >= LAMP_EFFECT_KINDS || !colours_ok ||
        len < EFFECT_COLOURS_AT + 3 * n || sys_get_le16(&d[11]) >= 36000) {
        return false;
    }
    memset(e, 0, sizeof(*e));
    e->kind = kind;
    e->n = n;
    memcpy(e->period_ms, &d[8], 3);
    memcpy(e->angle_cdeg, &d[11], 2);
    memcpy(e->width_dmm, &d[13], 2);
    memcpy(e->origin, &d[15], 4);
    memcpy(e->rgb, &d[EFFECT_COLOURS_AT], 3 * n);
    uint32_t period = sys_get_le24(&d[8]);
    *at = period > 0 ? sys_get_le24(&d[5]) % period : 0;
    return true;
}

// LAMPS: half u8, start u8, count u8, then count x (x, y) u16 LE, thousandths of the half from
// its top-left (LAMP_EFFECT_UNSEEN: not seen). count 0 puts the half back on the built-in table.
static bool parse_lamps(const uint8_t *d, int len) {
    if (len < LAMPS_AT) {
        return false;
    }
    uint8_t half = d[3], start = d[4], count = d[5];
    if (half > 1 || count > LAMPS_MAX || start + count > LAMP_EFFECT_LAMPS ||
        len < LAMPS_AT + 4 * count) {
        return false;
    }
    for (int i = 0; i < count; i++) {
        for (int k = 0; k < 2; k++) {
            uint16_t v = sys_get_le16(&d[LAMPS_AT + 4 * i + 2 * k]);
            if (v > 1000 && v != LAMP_EFFECT_UNSEEN) {
                return false;
            }
        }
    }
    return true;
}

// Under lock; d passed parse_lamps.
static void take_lamps(const uint8_t *d) {
    uint8_t half = d[3], start = d[4], count = d[5];
    if (count == 0) {
        lamps_on[half] = false;
        return;
    }
    if (!lamps_on[half]) { // the lamps not sent stay where the table has them
        for (int i = 0; i < LAMP_EFFECT_LAMPS; i++) {
            uint16_t pm[2];
            lamp_effect_table(i, half == 1, pm);
            sys_put_le16(pm[0], &lamp_pts[half][i][0]);
            sys_put_le16(pm[1], &lamp_pts[half][i][2]);
        }
        lamps_on[half] = true;
    }
    memcpy(lamp_pts[half][start], &d[LAMPS_AT], 4 * count);
}

static bool parse_place(const uint8_t *d, int len, int16_t out[3][4]) {
    if (len < PLACE_LEN) {
        return false;
    }
    for (int r = 0; r < 3; r++) {
        for (int i = 0; i < 4; i++) {
            out[r][i] = (int16_t)sys_get_le16(&d[3 + 8 * r + 2 * i]);
        }
        // Sizes: a room may be a line (a scanline along it still works); a half has an area.
        if (out[r][2] < 0 || out[r][3] < 0 || (r != ROOM && (out[r][2] == 0 || out[r][3] == 0))) {
            return false;
        }
    }
    return true;
}

void host_effects_command(const uint8_t *d, int len, uint8_t r[HOST_EFFECTS_REPLY_SIZE]) {
    uint8_t op = len >= 3 ? d[2] : 0xff;
    struct lamp_effect_wire e;
    int16_t p[3][4];
    uint32_t at = 0;
    bool ok = op == OP_RELEASE || (op == OP_PLACE && parse_place(d, len, p)) ||
              (op == OP_EFFECT && parse_effect(d, len, &e, &at)) ||
              (op == OP_LAMPS && parse_lamps(d, len));
    if (ok && op == OP_EFFECT) {
        // The PC's frames give way first (not under our lock: SET_FRAME takes its lock, then ours).
        host_frames_yield();
    }
    k_spinlock_key_t key = k_spin_lock(&lock);
    if (ok && op == OP_RELEASE) {
        stop(true);
    } else if (ok && op == OP_LAMPS) {
        take_lamps(d);
        if (running) {
            run(effect_run_clock()); // the LEDs moved: same effect, same clock
        }
    } else if (ok && op == OP_PLACE) {
        memcpy(place, p, sizeof(place));
        if (running) {
            run(effect_run_clock()); // moved: same effect, same clock
        }
    } else if (ok) {
        effect = e;
        run(at);
    }
    fill_reply(r, ok ? STATUS_OK : STATUS_BAD);
    k_spin_unlock(&lock, key);
}

void host_effects_yield(void) {
    k_spinlock_key_t key = k_spin_lock(&lock);
    stop(false);
    k_spin_unlock(&lock, key);
}

#if RIGHT
// The keyboard woke from idle: the right's clock to the left's again (the effect itself stays).
static int effects_activity_listener(const zmk_event_t *eh) {
    const struct zmk_activity_state_changed *ev = as_zmk_activity_state_changed(eh);
    if (ev != NULL && ev->state == ZMK_ACTIVITY_ACTIVE) {
        k_spinlock_key_t key = k_spin_lock(&lock);
        bool run = running;
        k_spin_unlock(&lock, key);
        if (run) {
            art_relay_effect_sync();
        }
    }
    return ZMK_EV_EVENT_BUBBLE;
}

ZMK_LISTENER(host_effects, effects_activity_listener);
ZMK_SUBSCRIPTION(host_effects, zmk_activity_state_changed);
#endif
