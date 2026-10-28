/*
 * A USB cable plugged in wakes the keyboard (#432).
 * SPDX-License-Identifier: MIT
 *
 * ZMK counts only key presses, encoder steps and pointer input as activity, so a half that had
 * gone idle (underglow, backlight and screen animation off) stayed dark after its cable went back
 * in, until a key. Here ZMK's own USB event counts too, when a host or a charger arrived: the
 * cable going in (no power before), the host taking the keyboard (HID: enumerated), and the PC
 * waking from sleep (USB resume; ZMK reports a suspended bus as HID too, so that is the status,
 * not a state change). Losing power or the host suspending (the PC going to sleep) wakes nothing.
 * Activity then runs ZMK's normal idle timeout, so nothing is kept awake. At boot with the cable
 * in, the first event notes activity once, which only refreshes ZMK's timestamp.
 *
 * It calls ZMK's own activity listener (activity.c in ZMK v0.3.0, a global as every
 * ZMK_LISTENER is) for those events, which notes the activity exactly as a key press does; that
 * listener ignores the event it is given. Should ZMK rename it, the link fails, not the wake:
 * then use whatever public "note activity" ZMK offers by then.
 *
 * The left only for now: the right half has no USB stack (CONFIG_ZMK_USB=n) and goes by its own
 * activity, as it does for the left's key presses.
 *
 * Start/stop: one note per qualifying USB event (on the system work queue, where ZMK raises it
 * and runs its idle check), nothing between them. No timer, no polling.
 */

#include <zephyr/usb/usb_device.h>

#include <zmk/event_manager.h>
#include <zmk/events/usb_conn_state_changed.h>
#include <zmk/usb.h>

// ZMK_LISTENER(activity, ...) in ZMK's activity.c: a global, as every ZMK listener is.
extern const struct zmk_listener zmk_listener_activity;

static enum zmk_usb_conn_state last = ZMK_USB_CONN_NONE;

// Whether going from `was` to `now` (with the bus status now) means a cable or a host arrived.
static bool usb_wakes(enum zmk_usb_conn_state was, enum zmk_usb_conn_state now,
                      enum usb_dc_status_code status) {
    return (was == ZMK_USB_CONN_NONE && now != ZMK_USB_CONN_NONE) ||
           (was != ZMK_USB_CONN_HID && now == ZMK_USB_CONN_HID) || status == USB_DC_RESUME;
}

static int usb_wake_listener(const zmk_event_t *eh) {
    const struct zmk_usb_conn_state_changed *ev = as_zmk_usb_conn_state_changed(eh);
    if (ev == NULL) {
        return ZMK_EV_EVENT_BUBBLE;
    }
    enum zmk_usb_conn_state was = last;
    last = ev->conn_state;
    if (usb_wakes(was, ev->conn_state, zmk_usb_get_status())) {
        zmk_listener_activity.callback(eh);
    }
    return ZMK_EV_EVENT_BUBBLE;
}

ZMK_LISTENER(usb_wake, usb_wake_listener);
ZMK_SUBSCRIPTION(usb_wake, zmk_usb_conn_state_changed);
