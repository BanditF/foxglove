/*
 * Panel polarity (panel_polarity.h, #666), both halves.
 * SPDX-License-Identifier: MIT
 *
 * Start/stop table (nothing repeats):
 *   apply_work  one shot on the display queue per change (D7 12, the relay op, a loaded value
 *               that differs from the one drawn); stops after the redraw.
 *   save_work   one shot SAVE_DELAY after the last change (a burst of changes is one flash write);
 *               writes only when the value differs from the one saved; stops after it.
 *   the relay   the left kicks art_relay_central.c once per change; it sends the value as an
 *               acknowledged write (newest wins) and again on link-up (its own start/stop).
 */

#include <errno.h>

#include <zephyr/kernel.h>
#include <zephyr/settings/settings.h>
#include <zephyr/sys/atomic.h>

#if IS_ENABLED(CONFIG_ZMK_DISPLAY)
#include <zmk/display.h>
#endif

#include "panel_polarity.h"

#define CENTRAL (!IS_ENABLED(CONFIG_ZMK_SPLIT) || IS_ENABLED(CONFIG_ZMK_SPLIT_ROLE_CENTRAL))
#define RELAY_CENTRAL (CENTRAL && IS_ENABLED(CONFIG_NICE_VIEW_ART_RELAY))
#if RELAY_CENTRAL
#include "art_relay.h"
#endif

// A few quick flips from the app make one flash write.
#define SAVE_DELAY K_SECONDS(2)
#define SETTINGS_KEY "nvc_pol/v"

static atomic_t polarity = ATOMIC_INIT(PANEL_POLARITY_DEFAULT);
// The value in the settings: loaded or last saved; UNKNOWN when none is stored. Settings load
// and the system work queue only.
static uint8_t saved = PANEL_POLARITY_UNKNOWN;

static bool valid(uint8_t p) { return p == PANEL_POLARITY_DARK || p == PANEL_POLARITY_LIGHT; }

uint8_t panel_polarity_get(void) { return (uint8_t)atomic_get(&polarity); }

__weak void panel_polarity_apply(void) {}

__weak int panel_polarity_store(uint8_t p) {
#if IS_ENABLED(CONFIG_SETTINGS)
    return settings_save_one(SETTINGS_KEY, &p, sizeof(p));
#else
    ARG_UNUSED(p);
    return 0;
#endif
}

#if IS_ENABLED(CONFIG_ZMK_DISPLAY)
static void apply_cb(struct k_work *work) { panel_polarity_apply(); }
static K_WORK_DEFINE(apply_work, apply_cb);
#endif

static void redraw(void) {
#if IS_ENABLED(CONFIG_ZMK_DISPLAY)
    if (zmk_display_is_initialized()) {
        k_work_submit_to_queue(zmk_display_work_q(), &apply_work);
    } // before that the first draw reads the value anyway
#else
    panel_polarity_apply(); // no display (the simulator): the drawer is told directly
#endif
}

static void save_cb(struct k_work *work) {
    uint8_t p = panel_polarity_get();
    if (p != saved && panel_polarity_store(p) == 0) {
        saved = p;
    }
}
static K_WORK_DELAYABLE_DEFINE(save_work, save_cb);

// The value changed to p (any thread): redraw and save once.
static bool take(uint8_t p) {
    if ((uint8_t)atomic_set(&polarity, p) == p) {
        return false;
    }
    redraw();
    k_work_reschedule(&save_work, SAVE_DELAY);
    return true;
}

int panel_polarity_set(uint8_t p) {
    if (!valid(p)) {
        return -EINVAL;
    }
    if (take(p)) {
#if RELAY_CENTRAL
        art_relay_polarity_kick();
#endif
    }
    return 0;
}

void panel_polarity_remote(uint8_t p) {
    if (valid(p)) {
        take(p);
    }
}

void panel_polarity_relay_op(uint8_t out[PANEL_POLARITY_RELAY_SIZE]) {
    out[0] = PANEL_POLARITY_RELAY_OP;
    out[1] = panel_polarity_get();
}

#if IS_ENABLED(CONFIG_SETTINGS)
// Called by ZMK's own settings_load (and settings_runtime_set); nothing is read here on our own.
static int polarity_settings_set(const char *name, size_t len, settings_read_cb read_cb,
                                 void *cb_arg) {
    uint8_t p;
    if (settings_name_steq(name, "v", NULL) && len == sizeof(p) &&
        read_cb(cb_arg, &p, sizeof(p)) == sizeof(p) && valid(p)) {
        saved = p;
        if ((uint8_t)atomic_set(&polarity, p) != p) {
            redraw(); // a display already up drew the default
        }
    }
    return 0;
}

SETTINGS_STATIC_HANDLER_DEFINE(nvc_pol, "nvc_pol", NULL, polarity_settings_set, NULL, NULL);
#endif
