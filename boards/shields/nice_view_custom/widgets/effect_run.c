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
 * Key rings (LAMP_EFFECT_KEYS, CONFIG_FOXGLOVE_HOST_EFFECTS_KEYS): a key pressed on this half
 * (ZMK's position event with source LOCAL: the right's keys reach the left with the right's
 * source, so each half rings only its own) starts a ring at that key's place in ZMK's physical
 * layout, with no PC and nothing over the split link. The listener only notes the key and the time;
 * the frame work turns notes into rings.
 *
 * Start/stop (no polling):
 *   frame timer  starts on a start, and on a key press under key rings; each frame schedules the
 *                next while the effect moves (key rings: while a ring still grows). Stops on stop,
 *                after one frame for a still effect (period 0), or after the frame that draws the
 *                last ring gone.
 */

#include <stddef.h>
#include <string.h>

#include <zephyr/kernel.h>
#include <zephyr/spinlock.h>
#include <zephyr/sys/util.h>

#include <zmk/workqueue.h>
#if IS_ENABLED(CONFIG_FOXGLOVE_HOST_EFFECTS_KEYS)
#include <zmk/event_manager.h>
#include <zmk/events/position_state_changed.h>
#include <zmk/physical_layouts.h>
#endif

#include "effect_run.h"
#include "lamp_strip.h"

// The right half's keys (and LEDs) are the left's mirror image.
#define MIRROR (IS_ENABLED(CONFIG_ZMK_SPLIT) && !IS_ENABLED(CONFIG_ZMK_SPLIT_ROLE_CENTRAL))
#define LAMPS MIN(LAMP_STRIP_LEN, LAMP_EFFECT_LAMPS)
#define FRAME K_MSEC(CONFIG_FOXGLOVE_HOST_EFFECTS_FRAME_MS)
#define KEYS IS_ENABLED(CONFIG_FOXGLOVE_HOST_EFFECTS_KEYS)
// Which keys of the physical layout are this half's (lamp_effect_key_point): the left's, the
// right's, or all of a one-piece board's.
#define SIDE (!IS_ENABLED(CONFIG_ZMK_SPLIT) ? -1 : MIRROR ? 1 : 0)

// One lock for what a start hands over and for the strip writes: a stop that returns has no
// frame of the old effect still to land (SET_FRAME's pixels come next). Spinlock: the Raw HID and
// Bluetooth callbacks start effects and must not block. Only copies happen under it.
static struct k_spinlock lock;
static struct lamp_effect_wire pending; // under lock
static uint32_t generation;             // under lock: one more per start
static bool running;                    // under lock
static int64_t epoch;                   // under lock: uptime when the effect's clock read 0
// Key presses under key rings, newest at press_next - 1 (under lock; a start clears them).
static struct press {
    uint32_t position;
    int64_t at; // uptime ms
} presses[LAMP_EFFECT_RINGS];
static uint8_t press_next, press_count;

// The low-priority queue's own (static: that queue's stack is ZMK's 768 bytes):
static struct lamp_effect fx;
static uint32_t fx_generation;
static bool fx_ok;
static struct lamp_effect_wire fx_wire;
static uint8_t out[LAMP_EFFECT_LAMPS][3];
static struct press fx_presses[LAMP_EFFECT_RINGS];
static uint8_t fx_press_count;

#if KEYS
#define SAME_FIELD(f)                                                                              \
    (offsetof(struct lamp_effect_key, f) == offsetof(struct zmk_key_physical_attrs, f))
BUILD_ASSERT(sizeof(struct lamp_effect_key) == sizeof(struct zmk_key_physical_attrs) &&
                 SAME_FIELD(width) && SAME_FIELD(height) && SAME_FIELD(x) && SAME_FIELD(y) &&
                 SAME_FIELD(rx) && SAME_FIELD(ry) && SAME_FIELD(r),
             "lamp_effect_key is ZMK's zmk_key_physical_attrs");

// Where key position sits in this half (thousandths): from ZMK's selected physical layout, else
// (a board without key places) the half's centre.
static void key_point(uint32_t position, uint16_t pt[2]) {
    struct zmk_physical_layout const *const *layouts;
    size_t n = zmk_physical_layouts_get_list(&layouts);
    int sel = zmk_physical_layouts_get_selected();
    if (sel >= 0 && (size_t)sel < n && layouts[sel]->keys != NULL &&
        lamp_effect_key_point((const struct lamp_effect_key *)layouts[sel]->keys,
                              (int)layouts[sel]->keys_len, (int)position, SIDE, pt)) {
        return;
    }
    pt[0] = pt[1] = 500;
}

// Draws the key rings at now into out; whether one still grows.
static bool draw_rings(int64_t now) {
    struct lamp_effect_ring rings[LAMP_EFFECT_RINGS];
    int n = 0;
    for (int i = 0; i < fx_press_count; i++) {
        int64_t age = now - fx_presses[i].at;
        if (age < 0 || age >= (int64_t)fx.period_ms) {
            continue; // grown out (or not yet)
        }
        uint16_t pt[2];
        float o[2];
        key_point(fx_presses[i].position, pt);
        lamp_effect_half_point(&fx, pt, o);
        rings[n++] = (struct lamp_effect_ring){
            .ox = o[0], .oy = o[1], .phase = (float)((double)age / fx.period_ms)};
    }
    lamp_effect_rings(&fx, rings, n, out);
    return n > 0;
}
#else
static bool draw_rings(int64_t now) { return false; }
#endif

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
    fx_press_count = press_count;
    memcpy(fx_presses, presses, sizeof(presses));
    k_spin_unlock(&lock, key);
    if (!run) {
        return;
    }
    if (g != fx_generation) {
        fx_ok = lamp_effect_prepare(&fx, &fx_wire, MIRROR);
        fx_generation = g;
    }
    memset(out, 0, sizeof(out));
    bool moves = false;
    if (fx_ok && KEYS && fx.kind == LAMP_EFFECT_KEYS) {
        moves = draw_rings(k_uptime_get());
    } else if (fx_ok) {
        int64_t t = k_uptime_get() - e;
        lamp_effect_render(&fx, lamp_effect_phase(t > 0 ? (uint64_t)t : 0, fx.period_ms), out);
        moves = !lamp_effect_still(&fx);
    }
    key = k_spin_lock(&lock);
    if (running && generation == g) { // else stopped or replaced meanwhile: draw nothing
        for (int i = 0; i < LAMPS; i++) {
            lamp_strip_set(i, out[i][0], out[i][1], out[i][2]);
        }
        lamp_strip_show();
        if (moves) {
            k_work_schedule_for_queue(zmk_workqueue_lowprio_work_q(), &frame_work, FRAME);
        }
    }
    k_spin_unlock(&lock, key);
}

void effect_run_start(const struct lamp_effect_wire *w, uint32_t at_ms) {
    k_spinlock_key_t key = k_spin_lock(&lock);
    // The same effect again (a re-anchor: a wake, a GO sent again) only moves the clock:
    // key rings in flight stay. Another effect starts without rings.
    bool same = running && memcmp(&pending, w, sizeof(*w)) == 0;
    if (!same) {
        pending = *w;
        press_count = press_next = 0;
        generation++;
    }
    epoch = k_uptime_get() - at_ms;
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

uint32_t effect_run_clock(void) {
    k_spinlock_key_t key = k_spin_lock(&lock);
    uint32_t period = pending.period_ms[0] | pending.period_ms[1] << 8 |
                      (uint32_t)pending.period_ms[2] << 16;
    int64_t t = k_uptime_get() - epoch;
    bool run = running;
    k_spin_unlock(&lock, key);
    return run && period > 0 && t > 0 ? (uint32_t)(t % period) : 0;
}

#if KEYS
// A key pressed on this half while key rings run: noted, and a frame now (the ring starts at the
// press, whenever the frame comes). Any other effect, a release or the other half's key: nothing.
static int effect_key_listener(const zmk_event_t *eh) {
    const struct zmk_position_state_changed *ev = as_zmk_position_state_changed(eh);
    if (ev == NULL || !ev->state || ev->source != ZMK_POSITION_STATE_CHANGE_SOURCE_LOCAL) {
        return ZMK_EV_EVENT_BUBBLE;
    }
    k_spinlock_key_t key = k_spin_lock(&lock);
    bool rings = running && pending.kind == LAMP_EFFECT_KEYS;
    if (rings) {
        presses[press_next] = (struct press){.position = ev->position, .at = k_uptime_get()};
        press_next = (press_next + 1) % LAMP_EFFECT_RINGS;
        press_count = MIN(press_count + 1, LAMP_EFFECT_RINGS);
    }
    k_spin_unlock(&lock, key);
    if (rings) {
        k_work_reschedule_for_queue(zmk_workqueue_lowprio_work_q(), &frame_work, K_NO_WAIT);
    }
    return ZMK_EV_EVENT_BUBBLE;
}

ZMK_LISTENER(effect_keys, effect_key_listener);
ZMK_SUBSCRIPTION(effect_keys, zmk_position_state_changed);
#endif
