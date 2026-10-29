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
#if IS_ENABLED(CONFIG_FOXGLOVE_PC_STATE)
    const struct foxglove_awake_changed *ev = as_foxglove_awake_changed(eh);
    static int16_t last = -1; // the keys' state last kicked; the system work queue only
    if (ev == NULL || ev->keys == last) {
        return ZMK_EV_EVENT_BUBBLE; // only the hold changed: nothing new for the right
    }
    last = ev->keys;
#endif
    art_relay_activity_kick();
    return ZMK_EV_EVENT_BUBBLE;
}

ZMK_LISTENER(activity_relay, activity_relay_listener);
#if IS_ENABLED(CONFIG_FOXGLOVE_PC_STATE)
// pc_state.c raises it for each change of ZMK's state too, and when the PC's away overrules the
// keys (#609).
ZMK_SUBSCRIPTION(activity_relay, foxglove_awake_changed);
#else
ZMK_SUBSCRIPTION(activity_relay, zmk_activity_state_changed);
#endif
