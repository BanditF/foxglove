/*
 * Key backlight idle auto-off, in place of ZMK's CONFIG_ZMK_BACKLIGHT_AUTO_OFF_IDLE (#609).
 * SPDX-License-Identifier: MIT
 *
 * ZMK's auto-off follows ZMK's own activity, so the PC state's hold (pc_state.h) couldn't keep
 * the backlight on while the PC is awake. This follows the effective activity instead: off while
 * the half is idle, back to ZMK's brightness when it is active. It drives the LEDs directly, as
 * ZMK's auto-off does, so nothing is saved: the user's brightness stays ZMK's (a &bl key or the
 * boot default), and a key that changes it while idle turns the half active anyway.
 *
 * Start/stop: one write per change of the effective activity (foxglove_awake_changed), nothing
 * between them.
 */

#include <zephyr/device.h>
#include <zephyr/devicetree.h>
#include <zephyr/drivers/led.h>

#include <zmk/backlight.h>
#include <zmk/event_manager.h>

#include "pc_state.h"

BUILD_ASSERT(!IS_ENABLED(CONFIG_ZMK_BACKLIGHT_AUTO_OFF_IDLE),
             "FOXGLOVE_BACKLIGHT_IDLE replaces ZMK_BACKLIGHT_AUTO_OFF_IDLE; turn that off");

#define BACKLIGHT_NODE DT_CHOSEN(zmk_backlight)
#define CHILD_COUNT(...) +1
#define BACKLIGHT_LEDS (DT_FOREACH_CHILD(BACKLIGHT_NODE, CHILD_COUNT))

static const struct device *const backlight = DEVICE_DT_GET(BACKLIGHT_NODE);

static int backlight_idle_listener(const zmk_event_t *eh) {
    const struct foxglove_awake_changed *ev = as_foxglove_awake_changed(eh);
    static int8_t shown = -1; // the active state last applied; the system work queue only
    if (ev == NULL || ev->active == shown || !device_is_ready(backlight)) {
        return ZMK_EV_EVENT_BUBBLE;
    }
    shown = ev->active;
    // zmk_backlight_get_brt is ZMK's brightness, 0 when the user turned it off.
    uint8_t brt = ev->active ? zmk_backlight_get_brt() : 0;
    for (int i = 0; i < BACKLIGHT_LEDS; i++) {
        led_set_brightness(backlight, i, brt);
    }
    return ZMK_EV_EVENT_BUBBLE;
}

ZMK_LISTENER(backlight_idle, backlight_idle_listener);
ZMK_SUBSCRIPTION(backlight_idle, foxglove_awake_changed);
