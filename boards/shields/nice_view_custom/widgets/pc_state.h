/*
 * The PC's state, as the app tells it (#609, #604): awake with a lease, or away.
 * SPDX-License-Identifier: MIT
 *
 * ZMK goes idle 30 s after the last key whatever the power source, so a plugged-in keyboard went
 * dark while the PC was in use (mouse only, reading), and stayed dark after the PC woke: Modern
 * Standby never suspends the keyboard's USB, so the keyboard can't tell a sleeping PC from an
 * awake one. The app can (display on/off, unlock), and tells the left (D7 11 PC_STATE, HELLO bit
 * 26); the left passes it to the right (art relay op PC_STATE).
 *
 * Each half keeps an "effective" activity: ZMK's own (its keys), or a hold that keeps it out of
 * idle. ZMK's activity itself is never touched, so its idle and 30-minute sleep stay exactly as
 * they are on battery, and nothing pokes ZMK on a timer. Our idle users read the effective state
 * and hear its changes (struct foxglove_awake_changed): the underglow (rgb_idle.c), the key
 * backlight (backlight_idle.c, in place of ZMK_BACKLIGHT_AUTO_OFF_IDLE) and the right's art
 * (peripheral_status.c). The ext-power rail, which feeds the right's screen, follows the
 * underglow, so the hold keeps it up only while it holds the half out of idle.
 *
 * The hold needs this half's own USB power (the left: and a bus that isn't suspended), and one of:
 *   - a lease from the PC: awake (D7 11 01, or the left's relay op) holds until the lease runs out.
 *     The app renews it when the keyboard asks (an unsolicited D7 91, a minute and again 20 s
 *     before the end), only while the PC is awake, so a crashed or closed app lets it lapse;
 *   - without a lease (the app closed, or never ran): CONFIG_FOXGLOVE_PC_STATE_USB_IDLE_S (10 min)
 *     after the last key, user decision 2 on #609. On the right a key on the left counts too.
 * Away (display off or sleep) drops both and idles the half at once, even within 30 s of a key;
 * the next key, or awake, ends that. Awake with the wake flag notes activity once, as a key does
 * (#604): ZMK's own wake, which the left also relays to the right (activity_relay.c).
 *
 * The right on battery keeps its own rules (user decision 3 on #609): it gets the lease but holds
 * only on its own USB power, and the left hands it its keys' activity, never the hold
 * (activity_relay.h, the RGB op's on/off).
 *
 * Start/stop: every change starts from an event: a PC_STATE message (D7 11 or the relay op), a
 * ZMK activity change, a key, a USB change. Two one-shot timers: the hold's end (the lease or the
 * USB idle time, rescheduled on each event) and the lease's asks (two per lease, only on USB
 * power). Nothing repeats between events.
 */

#pragma once

#include <stdbool.h>
#include <stdint.h>

#include <zephyr/sys/util.h>
#include <zephyr/toolchain.h>

#include <zmk/activity.h>
#if IS_ENABLED(CONFIG_FOXGLOVE_PC_STATE)
#include <zmk/event_manager.h>
#endif

// D7 11 PC_STATE's state byte, and the art relay op's.
enum pc_state_kind {
    PC_STATE_NONE = 0,  // no lease (nothing heard, or it lapsed): the USB idle time applies
    PC_STATE_AWAKE = 1, // a lease runs
    PC_STATE_AWAY = 2,  // the PC sleeps or its display is off: idle at once
};

// D7 11's request state byte.
#define PC_STATE_REQ_AWAY 0
#define PC_STATE_REQ_AWAKE 1
#define PC_STATE_REQ_READ 0xff
#define PC_STATE_WAKE BIT(0) // request flags: note activity now, as a key does (#604)
#define PC_STATE_LEASE_MAX_S 3600

// D7 91 flags.
#define PC_STATE_HELD BIT(0)     // the hold keeps this half out of idle now
#define PC_STATE_USB BIT(1)      // this half has USB power (the left: on a bus not suspended)
#define PC_STATE_KEYS BIT(2)     // ZMK's own activity is active (and not overruled by away)
#define PC_STATE_ACTIVE BIT(3)   // the effective state: keys or held

// D7 91 PC_STATE reply and event, bytes 2-7.
struct pc_state_wire {
    uint8_t status; // 0 ok, 5 bad request
    uint8_t event;  // 0 the reply to a request, 1 the lease ends soon: renew it if the PC is awake
    uint8_t state;  // enum pc_state_kind
    uint8_t flags;  // PC_STATE_*
    uint16_t lease_s; // seconds left on the lease, 0 without one
} __packed;

// The art relay op PC_STATE (art_relay.h): state u8 (enum pc_state_kind), lease seconds u16 LE.
#define PC_STATE_RELAY_OP 17
#define PC_STATE_RELAY_SIZE 4

#if IS_ENABLED(CONFIG_FOXGLOVE_PC_STATE)

// The effective state changed: active is keys or the hold, keys ZMK's own state (enum
// zmk_activity_state) with away's "idle now" applied. Raised on the system work queue, or inside
// ZMK's own activity event.
struct foxglove_awake_changed {
    bool active;
    uint8_t keys;
};
ZMK_EVENT_DECLARE(foxglove_awake_changed);

// Whether this half is active: its keys, or held. Any thread.
bool pc_state_active(void);
// This half's own activity (enum zmk_activity_state), idle while away overrules it. Any thread.
uint8_t pc_state_keys(void);
// Whether the hold keeps this half out of idle now. Any thread.
bool pc_state_held(void);

// D7 11 from the PC (the left). req is PC_STATE_REQ_*, flags PC_STATE_WAKE, lease_s 1-3600 for
// awake. Fills out with the state after it (status 5 and nothing changed for a bad request).
void pc_state_host(uint8_t req, uint8_t flags, uint16_t lease_s, struct pc_state_wire *out);
// The state now, for D7 91. Any thread.
void pc_state_report(struct pc_state_wire *out);

// The left: the op for the right as it is now; changes counts each PC_STATE the PC sent, so the
// relay sends a renewal too. Any thread.
void pc_state_relay_op(uint8_t out[PC_STATE_RELAY_SIZE], uint32_t *changes);
// The right: the left's op. Any thread.
void pc_state_remote(uint8_t state, uint16_t lease_s);
// The right: the left's keys' activity (activity_relay.h), which counts as a key for the USB idle
// time. Any thread.
void pc_state_remote_keys(bool active);

// USB power or the bus changed. Weak reading, so the simulator can fake it.
bool pc_state_read_usb(void);
void pc_state_usb_changed(void);

#else

static inline bool pc_state_active(void) {
    return zmk_activity_get_state() == ZMK_ACTIVITY_ACTIVE;
}
static inline uint8_t pc_state_keys(void) { return (uint8_t)zmk_activity_get_state(); }
static inline bool pc_state_held(void) { return false; }
static inline void pc_state_remote_keys(bool active) {}

#endif
