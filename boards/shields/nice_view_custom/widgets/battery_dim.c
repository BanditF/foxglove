/*
 * The underglow's brightness by power state (battery_dim.h), per half.
 * SPDX-License-Identifier: MIT
 *
 * The level follows ZMK's own events: zmk_usb_conn_state_changed when the cable goes in or out,
 * zmk_battery_state_changed when the charge reading changes. There's no timer. Before the first
 * event the level comes from ZMK's current readings, on the first frame drawn.
 *
 * Start/stop: a level is worked out on each of those events and on a new config, and nothing runs
 * between them; a new config is saved once, SAVE_DELAY after the last change.
 */

#include <errno.h>
#include <string.h>

#include <zephyr/kernel.h>
#include <zephyr/settings/settings.h>
#include <zephyr/spinlock.h>
#include <zephyr/sys/atomic.h>

#include <zmk/event_manager.h>
#if IS_ENABLED(CONFIG_USB_DEVICE_STACK)
#include <zmk/usb.h>
#include <zmk/events/usb_conn_state_changed.h>
#endif
#if IS_ENABLED(CONFIG_ZMK_BATTERY_REPORTING)
#include <zmk/battery.h>
#include <zmk/events/battery_state_changed.h>
#endif

#include "battery_dim.h"

#define CENTRAL (!IS_ENABLED(CONFIG_ZMK_SPLIT) || IS_ENABLED(CONFIG_ZMK_SPLIT_ROLE_CENTRAL))
#define HOST_CENTRAL (CENTRAL && IS_ENABLED(CONFIG_NICE_VIEW_HOST_LINK))
#define RELAY_CENTRAL (CENTRAL && IS_ENABLED(CONFIG_NICE_VIEW_ART_RELAY))
#if HOST_CENTRAL
#include "host_link.h"
#endif
#if RELAY_CENTRAL
#include "art_relay.h"
#endif
#if IS_ENABLED(CONFIG_FOXGLOVE_HOST_FRAMES)
#include "lamp_strip.h"
#endif

// A few quick changes from the app's sliders make one flash write.
#define SAVE_DELAY K_SECONDS(10)

static struct k_spinlock lock;
static struct battery_dim_config config = {
    .enabled = 1,
    .power = CONFIG_NICE_VIEW_RGB_DIM_POWER,
    .battery = CONFIG_NICE_VIEW_RGB_DIM_BATTERY,
    .low = CONFIG_NICE_VIEW_RGB_DIM_LOW,
    .low_below = CONFIG_NICE_VIEW_RGB_DIM_LOW_BELOW,
};
static bool usb, low, known; // known: an event (or the first frame) has set usb and soc
static uint8_t soc;
static atomic_t level = ATOMIC_INIT(CONFIG_NICE_VIEW_RGB_DIM_BATTERY);

// ZMK's readings now. Weak so the simulator can fake them (tests/zmk-sim/battery-dim).
__weak bool battery_dim_read_usb(void) {
#if IS_ENABLED(CONFIG_USB_DEVICE_STACK)
    return zmk_usb_is_powered();
#else
    return false;
#endif
}

__weak uint8_t battery_dim_read_soc(void) {
#if IS_ENABLED(CONFIG_ZMK_BATTERY_REPORTING)
    return zmk_battery_state_of_charge();
#else
    return 0;
#endif
}

// Under the lock.
static void recompute(void) { atomic_set(&level, battery_dim_level_for(&config, usb, soc, &low)); }

static void changed(void) {
#if HOST_CENTRAL
    host_link_battery_dim_changed();
#endif
#if IS_ENABLED(CONFIG_FOXGLOVE_HOST_FRAMES)
    lamp_strip_show(); // a frame from the PC shows at the new level too (ZMK redraws on its own)
#endif
}

uint8_t battery_dim_level(void) {
    if (!known) {
        // No event yet: ZMK's readings once, on the first frame. Not boot work: the first
        // frame comes after ZMK has started the underglow.
        battery_dim_update(battery_dim_read_usb(), battery_dim_read_soc());
    }
    return atomic_get(&level);
}

void battery_dim_update(bool now_usb, uint8_t now_soc) {
    k_spinlock_key_t key = k_spin_lock(&lock);
    uint8_t before = atomic_get(&level);
    bool was = usb, had = known;
    usb = now_usb;
    soc = now_soc;
    known = true;
    recompute();
    bool differs = !had || before != atomic_get(&level) || was != usb;
    k_spin_unlock(&lock, key);
    if (differs) {
        changed();
    }
}

void battery_dim_state(struct battery_dim_wire *out) {
    k_spinlock_key_t key = k_spin_lock(&lock);
    *out = (struct battery_dim_wire){
        .flags = (usb ? BATTERY_DIM_USB : 0) | (low ? BATTERY_DIM_LOW : 0),
        .level = atomic_get(&level),
        .soc = soc,
        .config = config,
    };
    k_spin_unlock(&lock, key);
}

void battery_dim_get_config(struct battery_dim_config *out) {
    k_spinlock_key_t key = k_spin_lock(&lock);
    *out = config;
    k_spin_unlock(&lock, key);
}

#if IS_ENABLED(CONFIG_SETTINGS)
static void save_cb(struct k_work *work) {
    struct battery_dim_config c;
    battery_dim_get_config(&c);
    settings_save_one("nvc_dim/cfg", &c, sizeof(c));
}
static K_WORK_DELAYABLE_DEFINE(save_work, save_cb);
#endif

int battery_dim_set_config(const struct battery_dim_config *c) {
    if (!battery_dim_valid(c)) {
        return -EINVAL;
    }
    k_spinlock_key_t key = k_spin_lock(&lock);
    bool same = memcmp(&config, c, sizeof(config)) == 0;
    config = *c;
    recompute();
    k_spin_unlock(&lock, key);
    if (same) {
        return 0;
    }
#if IS_ENABLED(CONFIG_SETTINGS)
    k_work_reschedule(&save_work, SAVE_DELAY);
#endif
#if RELAY_CENTRAL
    art_relay_rgb_kick(); // the right takes the left's levels
#endif
    changed();
    return 0;
}

#if IS_ENABLED(CONFIG_USB_DEVICE_STACK) || IS_ENABLED(CONFIG_ZMK_BATTERY_REPORTING)
static int power_listener(const zmk_event_t *eh) {
    uint8_t now_soc = battery_dim_read_soc();
#if IS_ENABLED(CONFIG_ZMK_BATTERY_REPORTING)
    const struct zmk_battery_state_changed *b = as_zmk_battery_state_changed(eh);
    if (b != NULL) {
        now_soc = b->state_of_charge;
    }
#endif
    battery_dim_update(battery_dim_read_usb(), now_soc);
    return ZMK_EV_EVENT_BUBBLE;
}

ZMK_LISTENER(battery_dim, power_listener);
#if IS_ENABLED(CONFIG_USB_DEVICE_STACK)
ZMK_SUBSCRIPTION(battery_dim, zmk_usb_conn_state_changed);
#endif
#if IS_ENABLED(CONFIG_ZMK_BATTERY_REPORTING)
ZMK_SUBSCRIPTION(battery_dim, zmk_battery_state_changed);
#endif
#endif

#if IS_ENABLED(CONFIG_SETTINGS)
// Called by ZMK's own settings_load; nothing is read here on our own.
static int dim_settings_set(const char *name, size_t len, settings_read_cb read_cb, void *cb_arg) {
    struct battery_dim_config c;
    if (settings_name_steq(name, "cfg", NULL) && len == sizeof(c) &&
        read_cb(cb_arg, &c, sizeof(c)) == sizeof(c) && battery_dim_valid(&c)) {
        k_spinlock_key_t key = k_spin_lock(&lock);
        config = c;
        recompute();
        k_spin_unlock(&lock, key);
    }
    return 0;
}

SETTINGS_STATIC_HANDLER_DEFINE(nvc_dim, "nvc_dim", NULL, dim_settings_set, NULL, NULL);
#endif
