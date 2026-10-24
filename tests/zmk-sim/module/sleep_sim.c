/*
 * Simulator driver for deep sleep (tests/zmk-sim/sleep): prints the activity state changes, and
 * the state 29 min 50 s after the last key, so the case's snapshot pins when ZMK powers off with
 * the production timeout. sys_poweroff itself is stubbed in stubs.c (SIM_SLEEP, end of run).
 * SPDX-License-Identifier: MIT
 */

#include <zephyr/kernel.h>
#include <zephyr/sys/printk.h>

#include <zmk/activity.h>
#include <zmk/event_manager.h>
#include <zmk/events/activity_state_changed.h>
#include <zmk/events/position_state_changed.h>

static K_SEM_DEFINE(key_released, 0, 1);

static const char *name(enum zmk_activity_state s) {
    switch (s) {
    case ZMK_ACTIVITY_ACTIVE:
        return "active";
    case ZMK_ACTIVITY_IDLE:
        return "idle";
    case ZMK_ACTIVITY_SLEEP:
        return "sleep";
    default:
        return "?";
    }
}

static int sleep_sim_listener(const zmk_event_t *eh) {
    const struct zmk_activity_state_changed *a = as_zmk_activity_state_changed(eh);
    if (a != NULL) {
        printk("sleep: state %s\n", name(a->state));
        return ZMK_EV_EVENT_BUBBLE;
    }
    const struct zmk_position_state_changed *p = as_zmk_position_state_changed(eh);
    if (p != NULL && !p->state) {
        k_sem_give(&key_released);
    }
    return ZMK_EV_EVENT_BUBBLE;
}

ZMK_LISTENER(sleep_sim, sleep_sim_listener);
ZMK_SUBSCRIPTION(sleep_sim, zmk_activity_state_changed);
ZMK_SUBSCRIPTION(sleep_sim, zmk_position_state_changed);

static void driver(void) {
    k_sem_take(&key_released, K_FOREVER);
    k_sleep(K_SECONDS(29 * 60 + 50));
    printk("sleep: 29 min 50 s after the last key: %s\n", name(zmk_activity_get_state()));
}

K_THREAD_DEFINE(sleep_sim_driver, 1024, driver, NULL, NULL, NULL, 7, 0, 0);
