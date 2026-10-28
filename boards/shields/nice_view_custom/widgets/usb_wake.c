/*
 * A USB cable plugged in wakes the keyboard (#432).
 * SPDX-License-Identifier: MIT
 *
 * ZMK counts only key presses, encoder steps and pointer input as activity, so a half that had
 * gone idle (underglow, backlight and screen animation off) stayed dark after its cable went back
 * in, until a key. Here ZMK's own USB event counts too, on the transitions that mean a host or a
 * charger arrived: the cable going in (no power before), and the host taking the keyboard (HID:
 * enumerated, or the PC waking from sleep). Losing power or the host suspending (the PC going to
 * sleep) wakes nothing. Activity then runs ZMK's normal idle timeout, so nothing is kept awake.
 *
 * It calls ZMK's own activity listener (activity.c) for those events, which notes the activity
 * exactly as a key press does; that listener ignores the event it is given.
 *
 * Start/stop: one note per qualifying USB event, nothing between them. No timer, no polling.
 */

#include <zmk/event_manager.h>
#include <zmk/events/usb_conn_state_changed.h>
#include <zmk/usb.h>

// ZMK_LISTENER(activity, ...) in ZMK's activity.c: a global, as every ZMK listener is.
extern const struct zmk_listener zmk_listener_activity;

static enum zmk_usb_conn_state last = ZMK_USB_CONN_NONE;

// Whether going from `was` to `now` means a cable or a host just arrived.
static bool usb_wakes(enum zmk_usb_conn_state was, enum zmk_usb_conn_state now) {
    return (was == ZMK_USB_CONN_NONE && now != ZMK_USB_CONN_NONE) ||
           (was != ZMK_USB_CONN_HID && now == ZMK_USB_CONN_HID);
}

static int usb_wake_listener(const zmk_event_t *eh) {
    const struct zmk_usb_conn_state_changed *ev = as_zmk_usb_conn_state_changed(eh);
    if (ev == NULL) {
        return ZMK_EV_EVENT_BUBBLE;
    }
    enum zmk_usb_conn_state was = last;
    last = ev->conn_state;
    if (usb_wakes(was, ev->conn_state)) {
        zmk_listener_activity.callback(eh);
    }
    return ZMK_EV_EVENT_BUBBLE;
}

ZMK_LISTENER(usb_wake, usb_wake_listener);
ZMK_SUBSCRIPTION(usb_wake, zmk_usb_conn_state_changed);
