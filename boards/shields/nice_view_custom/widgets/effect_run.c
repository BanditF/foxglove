/*
 * SET_EFFECT (#490): this half's effect, drawn from its lamp positions (effect_run.h).
 * SPDX-License-Identifier: MIT
 *
 * The effect's clock is the scene's, modulo the period: a start gives its value now (at_ms), and
 * the half counts on from its own uptime, so both halves and the PC agree on the phase with
 * nothing streamed (the left re-anchors the right on every relay send). Frames are drawn on ZMK's
 * low-priority queue, where ZMK's own underglow ticks, through lamp_strip.c and so through the
 * battery dim (#465).
 *
 * The scene owns the lights (#542): the effect draws whatever this half's underglow on/off and
 * idle say (rgb_idle.c keeps that for hand-back), as SET_FRAME's frames do.
 *
 * Start/stop (no polling):
 *   frame timer  starts on a start; each frame schedules the next while the effect moves. Stops
 *                on stop, or after one frame for a still effect (period 0).
 */

#include <string.h>

#include <zephyr/kernel.h>
#include <zephyr/spinlock.h>
#include <zephyr/sys/util.h>

#include <zmk/workqueue.h>

#include "effect_run.h"
#include "lamp_strip.h"

// The right half's keys (and LEDs) are the left's mirror image.
#define MIRROR (IS_ENABLED(CONFIG_ZMK_SPLIT) && !IS_ENABLED(CONFIG_ZMK_SPLIT_ROLE_CENTRAL))
#define LAMPS MIN(LAMP_STRIP_LEN, LAMP_EFFECT_LAMPS)
#define FRAME K_MSEC(CONFIG_FOXGLOVE_HOST_EFFECTS_FRAME_MS)

// One lock for what a start hands over and for the strip writes: a stop that returns has no
// frame of the old effect still to land (SET_FRAME's pixels come next). Spinlock: the Raw HID and
// Bluetooth callbacks start effects and must not block. Only copies happen under it.
static struct k_spinlock lock;
static struct lamp_effect_wire pending; // under lock
static uint32_t generation;             // under lock: one more per start
static bool running;                    // under lock
static int64_t epoch;                   // under lock: uptime when the effect's clock read 0

// The low-priority queue's own (static: that queue's stack is ZMK's 768 bytes):
static struct lamp_effect fx;
static uint32_t fx_generation;
static bool fx_ok;
static struct lamp_effect_wire fx_wire;
static uint8_t out[LAMP_EFFECT_LAMPS][3];

static void frame_cb(struct k_work *work);
static K_WORK_DELAYABLE_DEFINE(frame_work, frame_cb);

static void frame_cb(struct k_work *work) {
    k_spinlock_key_t key = k_spin_lock(&lock);
    bool run = running;
    uint32_t g = generation;
    int64_t e = epoch;
    if (run && g != fx_generation) {
        fx_wire = pending;
    }
    k_spin_unlock(&lock, key);
    if (!run) {
        return;
    }
    if (g != fx_generation) {
        fx_ok = lamp_effect_prepare(&fx, &fx_wire, MIRROR);
        fx_generation = g;
    }
    memset(out, 0, sizeof(out));
    if (fx_ok) {
        int64_t t = k_uptime_get() - e;
        lamp_effect_render(&fx, lamp_effect_phase(t > 0 ? (uint64_t)t : 0, fx.period_ms), out);
    }
    key = k_spin_lock(&lock);
    if (running && generation == g) { // else stopped or replaced meanwhile: draw nothing
        for (int i = 0; i < LAMPS; i++) {
            lamp_strip_set(i, out[i][0], out[i][1], out[i][2]);
        }
        lamp_strip_show();
        if (fx_ok && !lamp_effect_still(&fx)) {
            k_work_schedule_for_queue(zmk_workqueue_lowprio_work_q(), &frame_work, FRAME);
        }
    }
    k_spin_unlock(&lock, key);
}

void effect_run_start(const struct lamp_effect_wire *w, uint32_t at_ms) {
    k_spinlock_key_t key = k_spin_lock(&lock);
    pending = *w;
    epoch = k_uptime_get() - at_ms;
    generation++;
    running = true;
    k_spin_unlock(&lock, key);
    lamp_strip_host(true); // first: its take-over is queued ahead of the first frame
    k_work_reschedule_for_queue(zmk_workqueue_lowprio_work_q(), &frame_work, K_NO_WAIT);
}

void effect_run_stop(bool hand_back) {
    k_spinlock_key_t key = k_spin_lock(&lock);
    bool was = running;
    running = false;
    k_spin_unlock(&lock, key);
    k_work_cancel_delayable(&frame_work);
    if (was && hand_back) {
        lamp_strip_host(false);
    }
}

bool effect_run_active(void) {
    k_spinlock_key_t key = k_spin_lock(&lock);
    bool run = running;
    k_spin_unlock(&lock, key);
    return run;
}

uint32_t effect_run_clock(void) {
    k_spinlock_key_t key = k_spin_lock(&lock);
    uint32_t period = pending.period_ms[0] | pending.period_ms[1] << 8 |
                      (uint32_t)pending.period_ms[2] << 16;
    int64_t t = k_uptime_get() - epoch;
    bool run = running;
    k_spin_unlock(&lock, key);
    return run && period > 0 && t > 0 ? (uint32_t)(t % period) : 0;
}
