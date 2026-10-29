/*
 * Left: each change of this half's activity state goes to the right half (activity_relay.h).
 * SPDX-License-Identifier: MIT
 *
 * One ZMK listener, so every way the left wakes or idles counts the same: a key, an encoder step
 * or pointer input (ZMK's activity.c), a cable or a host arriving (usb_wake.c calls ZMK's own
 * activity listener, which raises this event), ZMK's idle and sleep checks.
 *
 * Start/stop: one kick per activity state change; art_relay_central.c sends what the state is
 * then, with its own stop rules (ack, link down, retry cap). No timer here, nothing between
 * changes.
 */

#include <zmk/event_manager.h>
#include <zmk/events/activity_state_changed.h>

#include "activity_relay.h"

static int activity_relay_listener(const zmk_event_t *eh) {
    if (as_zmk_activity_state_changed(eh) != NULL) {
        art_relay_activity_kick();
    }
    return ZMK_EV_EVENT_BUBBLE;
}

ZMK_LISTENER(activity_relay, activity_relay_listener);
ZMK_SUBSCRIPTION(activity_relay, zmk_activity_state_changed);
