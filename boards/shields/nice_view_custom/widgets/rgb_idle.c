/*
 * Underglow idle auto-off, in place of ZMK's CONFIG_ZMK_RGB_UNDERGLOW_AUTO_OFF_IDLE.
 * SPDX-License-Identifier: MIT
 *
 * ZMK's auto-off turns the underglow off with zmk_rgb_underglow_off(), which saves "off" to flash
 * as if the user had chosen it. The right half runs on battery and deep-sleeps after an hour of no
 * key presses of its own (gaming: right hand on the mouse); a key press then reboots it, it loads
 * "off" and stays dark. Here the user's choice (wanted) is kept apart from the idle state and
 * saved on its own, so the underglow comes back after idle, sleep or a reboot.
 *
 * On a split keyboard the left (central) sees every key press, so its activity is the keyboard's.
 * With sync on (rgb_idle_set_sync, from the PC), the left sends its underglow state to the right
 * over the art relay link and the right follows it; with sync off, or while the link is down, the
 * right goes by its own activity.
 */

#include <zephyr/kernel.h>
#include <zephyr/settings/settings.h>

#include <zmk/activity.h>
#include <zmk/event_manager.h>
#include <zmk/events/activity_state_changed.h>
#include <zmk/rgb_underglow.h>

#include "rgb_idle.h"
#include "art_relay.h"

BUILD_ASSERT(!IS_ENABLED(CONFIG_ZMK_RGB_UNDERGLOW_AUTO_OFF_IDLE),
             "NICE_VIEW_RGB_IDLE replaces ZMK_RGB_UNDERGLOW_AUTO_OFF_IDLE; turn that off");

#define CENTRAL (!IS_ENABLED(CONFIG_ZMK_SPLIT) || IS_ENABLED(CONFIG_ZMK_SPLIT_ROLE_CENTRAL))
#define RELAY_CENTRAL (CENTRAL && IS_ENABLED(CONFIG_NICE_VIEW_ART_RELAY))

// Picks up on/off changes made with the rgb_ug behaviours while active.
#define POLL K_SECONDS(2)
// Saving waits a little, so a few quick toggles make one flash write.
#define SAVE_DELAY K_SECONDS(10)

static bool wanted;      // the user's on/off, saved
static bool loaded;      // wanted came from the settings
static bool sync = true; // left: the right follows the left, saved
static bool following;   // right: the left's state is in charge
static bool remote_on;   // right: the left's state

static bool active(void) { return zmk_activity_get_state() == ZMK_ACTIVITY_ACTIVE; }

static void save_cb(struct k_work *work) {
    settings_save_one("nvc_rgb/wanted", &wanted, sizeof(wanted));
#if CENTRAL
    settings_save_one("nvc_rgb/sync", &sync, sizeof(sync));
#endif
}
static K_WORK_DELAYABLE_DEFINE(save_work, save_cb);

static void save(void) { k_work_reschedule(&save_work, SAVE_DELAY); }

// The work items below all run on the system work queue, one at a time.

static void apply_cb(struct k_work *work) {
    bool on;
    bool target = following ? remote_on : wanted && active();
    if (zmk_rgb_underglow_get_state(&on) == 0 && on != target) {
        if (target) {
            zmk_rgb_underglow_on();
        } else {
            zmk_rgb_underglow_off();
        }
    }
#if RELAY_CENTRAL
    art_relay_rgb_kick();
#endif
}
static K_WORK_DEFINE(apply_work, apply_cb);

static void poll_cb(struct k_work *work);
static K_WORK_DELAYABLE_DEFINE(poll_work, poll_cb);

static void poll_cb(struct k_work *work) {
    if (!active()) {
        return; // the activity listener starts polling again
    }
    bool on;
    if (!following && zmk_rgb_underglow_get_state(&on) == 0 && on != wanted) {
        wanted = on; // toggled with a key
        save();
#if RELAY_CENTRAL
        art_relay_rgb_kick();
#endif
    }
    k_work_schedule(&poll_work, POLL);
}

static int activity_listener(const zmk_event_t *eh) {
    k_work_submit(&apply_work);
    if (active()) {
        k_work_reschedule(&poll_work, POLL);
    }
    return ZMK_EV_EVENT_BUBBLE;
}

ZMK_LISTENER(rgb_idle, activity_listener);
ZMK_SUBSCRIPTION(rgb_idle, zmk_activity_state_changed);

bool rgb_idle_sync(void) { return sync; }

void rgb_idle_set_sync(bool on) {
    if (sync != on) {
        sync = on;
        save();
    }
#if RELAY_CENTRAL
    art_relay_rgb_kick();
#endif
}

void rgb_idle_remote(bool on, bool follow) {
    remote_on = on;
    following = follow;
    k_work_submit(&apply_work);
}

void rgb_idle_remote_lost(void) {
    following = false;
    k_work_submit(&apply_work);
}

static int settings_set(const char *name, size_t len, settings_read_cb read_cb, void *cb_arg) {
    if (settings_name_steq(name, "wanted", NULL) && len == sizeof(wanted)) {
        loaded = read_cb(cb_arg, &wanted, sizeof(wanted)) == sizeof(wanted);
    } else if (settings_name_steq(name, "sync", NULL) && len == sizeof(sync)) {
        read_cb(cb_arg, &sync, sizeof(sync));
    }
    return 0;
}

// After all settings are loaded (ZMK's own underglow state too).
static int settings_commit(void) {
    if (!loaded) {
        zmk_rgb_underglow_get_state(&wanted); // first boot with this firmware: keep what ZMK had
    }
    k_work_submit(&apply_work);
    k_work_reschedule(&poll_work, POLL);
    return 0;
}

SETTINGS_STATIC_HANDLER_DEFINE(nvc_rgb, "nvc_rgb", NULL, settings_set, settings_commit, NULL);
