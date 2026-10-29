/*
 * The PC's state and this half's effective activity (pc_state.h, #609, #604).
 * SPDX-License-Identifier: MIT
 *
 * Start/stop table (no timer repeats; every one-shot is rescheduled by an event):
 *   eval      starts on any input below; raises foxglove_awake_changed when the effective state
 *             or the keys' state differs from the last one raised, then plans the two timers.
 *   end_work  one-shot at the hold's next boundary (the lease's end, the USB idle time's end);
 *             stops at it. A new input reschedules it; away cancels it.
 *   ask_work  the left only, on USB power while a lease runs: one-shot a minute before the
 *             lease's end, then 20 s before it (D7 91 event 1 to the PC, after host traffic).
 *             Stops after the second, or on a renewal (which starts over), away, the lease
 *             lapsing or USB power going.
 *   wake_work one-shot per PC_STATE awake with the wake flag (#604): notes activity once.
 */

#include <zephyr/kernel.h>
#include <zephyr/spinlock.h>
#include <zephyr/sys/byteorder.h>
#include <zephyr/sys/util.h>

#include <zmk/activity.h>
#include <zmk/event_manager.h>
#include <zmk/events/activity_state_changed.h>
#include <zmk/events/position_state_changed.h>
#include <zmk/events/sensor_event.h>
#if IS_ENABLED(CONFIG_ZMK_POINTING)
#include <zephyr/input/input.h>
#endif
#if IS_ENABLED(CONFIG_USB_DEVICE_STACK)
#include <zmk/usb.h>
#include <zmk/events/usb_conn_state_changed.h>
#endif

#include "pc_state.h"

#define CENTRAL (!IS_ENABLED(CONFIG_ZMK_SPLIT) || IS_ENABLED(CONFIG_ZMK_SPLIT_ROLE_CENTRAL))
#define HOST_CENTRAL (CENTRAL && IS_ENABLED(CONFIG_NICE_VIEW_HOST_LINK))
#define RELAY_CENTRAL (CENTRAL && IS_ENABLED(CONFIG_NICE_VIEW_ART_RELAY))

#if HOST_CENTRAL
#include "host_link.h"
#endif
#if RELAY_CENTRAL
#include "art_relay.h"
#endif

ZMK_EVENT_IMPL(foxglove_awake_changed);

// ZMK_LISTENER(activity, ...) in ZMK's activity.c, as usb_wake.c uses it.
extern const struct zmk_listener zmk_listener_activity;

// Without the app: lit this long after the last key on USB power (decision 2 on #609). ZMK idles
// CONFIG_ZMK_IDLE_TIMEOUT after the last key, so the hold covers the rest from there.
#define USB_IDLE_MS ((int64_t)CONFIG_FOXGLOVE_PC_STATE_USB_IDLE_S * 1000)
#define GRACE_AFTER_IDLE_MS (USB_IDLE_MS - CONFIG_ZMK_IDLE_TIMEOUT)
#define ASK_FIRST_MS 60000
#define ASK_LAST_MS 20000

static struct k_spinlock lock;
static enum pc_state_kind kind;
static int64_t lease_end;  // uptime ms; the lease runs while kind is AWAKE and now < lease_end
static int64_t grace_end;  // uptime ms: the USB idle time's end, without a lease
static bool forced_idle;   // away: idle now, until the next key or awake
static uint32_t changes;   // PC_STATE messages taken (the left relays each, renewals too)
static uint8_t asks;       // asks sent for the current lease

// The system work queue only:
static bool raised_valid, raised_active;
static uint8_t raised_keys;

__weak bool pc_state_read_usb(void) {
#if IS_ENABLED(CONFIG_USB_DEVICE_STACK)
    if (!zmk_usb_is_powered()) {
        return false;
    }
    // On the left a suspended bus is a sleeping PC. The right's USB carries nothing from the PC
    // (and Windows may suspend a device it has no driver for): its power is what counts.
    return !CENTRAL || zmk_usb_get_status() != USB_DC_SUSPEND;
#else
    return false;
#endif
}

static bool lease_runs(int64_t now) { return kind == PC_STATE_AWAKE && now < lease_end; }

// Under the lock.
static bool held_at(int64_t now, bool usb) {
    return usb && !forced_idle && (lease_runs(now) || (kind != PC_STATE_AWAY && now < grace_end));
}

static uint8_t keys_locked(void) {
    uint8_t z = (uint8_t)zmk_activity_get_state();
    return forced_idle && z == ZMK_ACTIVITY_ACTIVE ? ZMK_ACTIVITY_IDLE : z;
}

bool pc_state_held(void) {
    bool usb = pc_state_read_usb();
    k_spinlock_key_t key = k_spin_lock(&lock);
    bool held = held_at(k_uptime_get(), usb);
    k_spin_unlock(&lock, key);
    return held;
}

uint8_t pc_state_keys(void) {
    k_spinlock_key_t key = k_spin_lock(&lock);
    uint8_t keys = keys_locked();
    k_spin_unlock(&lock, key);
    return keys;
}

bool pc_state_active(void) { return pc_state_keys() == ZMK_ACTIVITY_ACTIVE || pc_state_held(); }

void pc_state_report(struct pc_state_wire *out) {
    bool usb = pc_state_read_usb();
    k_spinlock_key_t key = k_spin_lock(&lock);
    int64_t now = k_uptime_get();
    bool lease = lease_runs(now);
    bool held = held_at(now, usb);
    bool keys = keys_locked() == ZMK_ACTIVITY_ACTIVE;
    *out = (struct pc_state_wire){
        .state = lease ? PC_STATE_AWAKE : kind == PC_STATE_AWAY ? PC_STATE_AWAY : PC_STATE_NONE,
        .flags = (held ? PC_STATE_HELD : 0) | (usb ? PC_STATE_USB : 0) |
                 (keys ? PC_STATE_KEYS : 0) | (keys || held ? PC_STATE_ACTIVE : 0),
        .lease_s = lease ? (uint16_t)DIV_ROUND_UP(lease_end - now, 1000) : 0,
    };
    k_spin_unlock(&lock, key);
}

/* The timers */

static void eval(void);

static void end_cb(struct k_work *work) {
    k_spinlock_key_t key = k_spin_lock(&lock);
    if (kind == PC_STATE_AWAKE && k_uptime_get() >= lease_end) {
        kind = PC_STATE_NONE; // the app didn't renew: closed, crashed or asleep
    }
    k_spin_unlock(&lock, key);
    eval();
}
static K_WORK_DELAYABLE_DEFINE(end_work, end_cb);

static void ask_cb(struct k_work *work);
static K_WORK_DELAYABLE_DEFINE(ask_work, ask_cb);

// Plans both timers from the state now. The system work queue only.
static void plan(bool usb) {
    k_spinlock_key_t key = k_spin_lock(&lock);
    int64_t now = k_uptime_get();
    int64_t next = INT64_MAX;
    if (kind == PC_STATE_AWAKE && lease_end > now) {
        next = lease_end;
    }
    if (kind != PC_STATE_AWAY && grace_end > now) {
        next = MIN(next, grace_end);
    }
    int64_t ask = 0;
    if (CENTRAL && usb && lease_runs(now) && asks < 2) {
        ask = lease_end - (asks == 0 ? ASK_FIRST_MS : ASK_LAST_MS);
    }
    k_spin_unlock(&lock, key);
    if (!usb) {
        next = INT64_MAX; // no hold without USB power: its next USB event plans again
    }
    if (next == INT64_MAX) {
        k_work_cancel_delayable(&end_work);
    } else {
        k_work_reschedule(&end_work, K_MSEC(next - now));
    }
    if (ask == 0) {
        k_work_cancel_delayable(&ask_work);
    } else {
        k_work_reschedule(&ask_work, K_MSEC(MAX(ask - now, 0)));
    }
}

static void ask_cb(struct k_work *work) {
    bool usb = pc_state_read_usb();
    k_spinlock_key_t key = k_spin_lock(&lock);
    bool due = CENTRAL && usb && lease_runs(k_uptime_get()) && asks < 2;
    if (due) {
        asks++;
    }
    k_spin_unlock(&lock, key);
    if (due) {
#if HOST_CENTRAL
        host_link_pc_state_event(); // the PC renews the lease if it is awake
#endif
        plan(usb);
    }
}

static void eval(void) {
    bool usb = pc_state_read_usb();
    k_spinlock_key_t key = k_spin_lock(&lock);
    uint8_t keys = keys_locked();
    bool active = keys == ZMK_ACTIVITY_ACTIVE || held_at(k_uptime_get(), usb);
    k_spin_unlock(&lock, key);
    if (!raised_valid || raised_active != active || raised_keys != keys) {
        raised_valid = true;
        raised_active = active;
        raised_keys = keys;
        raise_foxglove_awake_changed((struct foxglove_awake_changed){.active = active, .keys = keys});
    }
    plan(usb);
}

static void eval_cb(struct k_work *work) { eval(); }
static K_WORK_DEFINE(eval_work, eval_cb);

static void wake_cb(struct k_work *work) {
    zmk_listener_activity.callback(NULL); // ZMK's activity listener ignores the event it's given
}
static K_WORK_DEFINE(wake_work, wake_cb);

/* The PC (the left: D7 11) and the left (the right: the relay op) */

// Under the lock.
static void take(uint8_t state, uint16_t lease_s) {
    int64_t now = k_uptime_get();
    switch (state) {
    case PC_STATE_AWAKE:
        kind = PC_STATE_AWAKE;
        lease_end = now + (int64_t)lease_s * 1000;
        forced_idle = false;
        asks = 0;
        break;
    case PC_STATE_AWAY:
        kind = PC_STATE_AWAY;
        lease_end = 0;
        grace_end = 0;
        forced_idle = true;
        break;
    default:
        kind = PC_STATE_NONE;
        lease_end = 0;
        break;
    }
    changes++;
}

void pc_state_host(uint8_t req, uint8_t flags, uint16_t lease_s, struct pc_state_wire *out) {
    bool ok = req == PC_STATE_REQ_READ || req == PC_STATE_REQ_AWAY ||
              (req == PC_STATE_REQ_AWAKE && lease_s >= 1 && lease_s <= PC_STATE_LEASE_MAX_S);
    if (ok && req != PC_STATE_REQ_READ) {
        k_spinlock_key_t key = k_spin_lock(&lock);
        take(req == PC_STATE_REQ_AWAKE ? PC_STATE_AWAKE : PC_STATE_AWAY, lease_s);
        k_spin_unlock(&lock, key);
        if (req == PC_STATE_REQ_AWAKE && (flags & PC_STATE_WAKE)) {
            k_work_submit(&wake_work); // the PC came back: wake as a key does (#604)
        }
        k_work_submit(&eval_work);
#if RELAY_CENTRAL
        art_relay_pc_state_kick();
#endif
    }
    pc_state_report(out);
    out->status = ok ? 0 : 5;
}

void pc_state_relay_op(uint8_t out[PC_STATE_RELAY_SIZE], uint32_t *changed) {
    struct pc_state_wire w;
    pc_state_report(&w);
    k_spinlock_key_t key = k_spin_lock(&lock);
    *changed = changes;
    k_spin_unlock(&lock, key);
    out[0] = PC_STATE_RELAY_OP;
    out[1] = w.state;
    sys_put_le16(w.lease_s, &out[2]);
}

void pc_state_remote(uint8_t state, uint16_t lease_s) {
    if (state > PC_STATE_AWAY || (state == PC_STATE_AWAKE && lease_s == 0)) {
        return;
    }
    k_spinlock_key_t key = k_spin_lock(&lock);
    take(state, MIN(lease_s, PC_STATE_LEASE_MAX_S));
    k_spin_unlock(&lock, key);
    k_work_submit(&eval_work);
}

void pc_state_remote_keys(bool active) {
    if (active || GRACE_AFTER_IDLE_MS <= 0) {
        return;
    }
    // The left idles CONFIG_ZMK_IDLE_TIMEOUT after its last key, as this half does.
    k_spinlock_key_t key = k_spin_lock(&lock);
    grace_end = MAX(grace_end, k_uptime_get() + GRACE_AFTER_IDLE_MS);
    k_spin_unlock(&lock, key);
    k_work_submit(&eval_work);
}

void pc_state_usb_changed(void) { k_work_submit(&eval_work); }

/* ZMK's activity, keys and USB */

// A key, an encoder step or pointer input while ZMK is already active: away's "idle now" ends.
// Any thread.
static void end_forced_idle(void) {
    k_spinlock_key_t key = k_spin_lock(&lock);
    bool was = forced_idle;
    forced_idle = false;
    if (was && kind == PC_STATE_AWAY) {
        kind = PC_STATE_NONE;
    }
    k_spin_unlock(&lock, key);
    if (was) {
        k_work_submit(&eval_work);
    }
}

static int activity_listener(const zmk_event_t *eh) {
    const struct zmk_activity_state_changed *ev = as_zmk_activity_state_changed(eh);
    if (ev != NULL) {
        k_spinlock_key_t key = k_spin_lock(&lock);
        if (ev->state == ZMK_ACTIVITY_ACTIVE) {
            forced_idle = false; // a key, a cable or a wake: away no longer overrules it
            if (kind == PC_STATE_AWAY) {
                kind = PC_STATE_NONE;
            }
            grace_end = 0;
        } else if (ev->state == ZMK_ACTIVITY_IDLE && GRACE_AFTER_IDLE_MS > 0) {
            grace_end = MAX(grace_end, k_uptime_get() + GRACE_AFTER_IDLE_MS);
        }
        k_spin_unlock(&lock, key);
        eval(); // here, so our users hear it within ZMK's own event (sleep powers off after it)
        return ZMK_EV_EVENT_BUBBLE;
    }
    end_forced_idle();
    return ZMK_EV_EVENT_BUBBLE;
}

ZMK_LISTENER(pc_state, activity_listener);
ZMK_SUBSCRIPTION(pc_state, zmk_activity_state_changed);
ZMK_SUBSCRIPTION(pc_state, zmk_position_state_changed);
ZMK_SUBSCRIPTION(pc_state, zmk_sensor_event);

#if IS_ENABLED(CONFIG_ZMK_POINTING)
// As ZMK's activity.c counts pointer input. Only a flag check per input event.
static void input_listener(struct input_event *ev) { end_forced_idle(); }
INPUT_CALLBACK_DEFINE(NULL, input_listener);
#endif

#if IS_ENABLED(CONFIG_USB_DEVICE_STACK)
static int usb_listener(const zmk_event_t *eh) {
    pc_state_usb_changed();
    return ZMK_EV_EVENT_BUBBLE;
}

ZMK_LISTENER(pc_state_usb, usb_listener);
ZMK_SUBSCRIPTION(pc_state_usb, zmk_usb_conn_state_changed);
#endif
