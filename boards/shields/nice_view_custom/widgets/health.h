/*
 * The keyboard's self-report (#705): each half keeps small counters and states per subsystem in
 * RAM, and the PC reads them as one versioned block per half (D7 13 HEALTH, HELLO bit 28). The
 * left's block comes from here; the right's from the right's own copy of this file, read by the
 * left over the art relay (art_relay.h, ART_RELAY_HEALTH_AT). Wire format: docs/health.md.
 * SPDX-License-Identifier: MIT
 *
 * Cheap on purpose: a counter is one saturating atomic increment, a state one atomic store. No
 * timers and no boot work: the boot self-test (health_selftest) is worked out when the block is
 * read, from what the subsystems noted as it happened (a link, the relay found, settings loaded).
 * Without CONFIG_FOXGLOVE_HEALTH every call below is an empty inline, so callers need no #if.
 */

#pragma once

#include <stdbool.h>
#include <stdint.h>

#include <zephyr/sys/util.h>
#include <zephyr/toolchain.h>

// Version 2 (#719) appends the knob (bytes 96-103) to version 1's 96 bytes; version 3 appends the
// Raw HID counts per transport (bytes 104-111).
#define HEALTH_VERSION 3
#define HEALTH_SIZE 112
#define HEALTH_V1_SIZE 96
// D7 93 carries this many bytes of the block per report (bytes 8-31).
#define HEALTH_CHUNK 24
#define HEALTH_CHUNKS ((HEALTH_SIZE + HEALTH_CHUNK - 1) / HEALTH_CHUNK)

// Counters (saturating: u16 fields stop at 0xffff, u32 ones at 0x7fffffff).
enum health_counter {
    HEALTH_RELAY_TIMEOUTS,    // relay requests let go without an answer, since boot
    HEALTH_RELAY_UNSTICKS,    // split links dropped to end a stuck request
    HEALTH_RELAY_READY,       // left: the relay found on a link; right: the left subscribed
    HEALTH_RELAY_REFUSED,     // left: relay requests that failed; right: ops refused (ATT error)
    HEALTH_HID_REPLY_DROPS,   // host link replies dropped: its reply queue was full
    HEALTH_REDRAWS,           // left: host panel redraws (the right counts its own, #618)
    HEALTH_ART_OK,            // left: pushes and SELECTs done; right: blobs taken and played
    HEALTH_ART_FAILED,        // left: pushes and SELECTs failed; right: blobs refused
    HEALTH_SNAP_ERRORS,       // screen snapshots that failed (both: capture; left: transfer)
    HEALTH_LIGHT_APPLIES,     // the PC's colour, frames or effect applied on this half
    HEALTH_LIGHT_SYNCS,       // left: RGB ops the right acknowledged; right: RGB ops taken
    HEALTH_LIGHT_SYNC_FAILS,  // left: RGB ops that failed (each retry counts)
    HEALTH_RGB_MISMATCHES,    // left: a status read showed the right with another look (#624)
    HEALTH_SPLIT_DROPS,       // split links that went down
    HEALTH_KNOB_EVENTS,       // left: knob events sent to the PC (D7 94 01, #719)
    HEALTH_KNOB_KEYS,         // left: fallback keys the knob tapped (detents, presses)
    HEALTH_KNOB_DROPS,        // left: holds the board dropped (USB disconnect, endpoint change)
    HEALTH_COUNTERS,
};

// Single values (last seen).
enum health_value {
    HEALTH_ART_LAST_ERROR, // left: enum art_relay_error of the last failed job; right: its state
    HEALTH_ART_SET,        // the art set shown (int8; 0x80 unknown)
    HEALTH_LAST_EFFECT,    // the PC effect kind running (0 none)
    HEALTH_KNOB_STATE,     // left: the knob's hold (host_link.c: bit 0 held, bit 1 an app)
    HEALTH_KNOB_LAST,      // left: the last knob event's kind (knob.h; 0 none)
    HEALTH_VALUES,
};

// The boot self-test, one result per check (D7 93 bytes 24-27 of the block).
enum health_check {
    HEALTH_CHECK_DISPLAY, // the display device is ready and ZMK's display is up
    HEALTH_CHECK_SETTINGS, // the settings were loaded (commit) within the time
    HEALTH_CHECK_RELAY,    // the relay was found within CONFIG_FOXGLOVE_HEALTH_RELAY_S of boot
    HEALTH_CHECK_LEDS,     // the LED strip is ready and has LEDs
    HEALTH_CHECKS,
};
// A check's byte: state in bits 0-1, code in bits 2-7.
enum health_state { HEALTH_PENDING, HEALTH_PASS, HEALTH_FAIL, HEALTH_NA };
#define HEALTH_RESULT(state, code) ((uint8_t)((state) | ((code) << 2)))
// Codes (0 = none). Display: 1 device not ready, 2 ZMK's display not up after
// HEALTH_DISPLAY_AFTER_S. Settings: 1 not loaded after HEALTH_SETTINGS_AFTER_S. Relay: 1 found,
// but after the time; 2 no split link by then; 3 a link, but the relay not found. LEDs: 1 strip
// device not ready, 2 no LEDs in the chain.
#define HEALTH_DISPLAY_AFTER_S 10
#define HEALTH_SETTINGS_AFTER_S 30

// The knob byte (96): which knob behaviours the keymap has (knob.h's KNOB_HAS_*), and its hold.
#define HEALTH_KNOB_TURN BIT(0)
#define HEALTH_KNOB_PRESS BIT(1)
#define HEALTH_KNOB_HELD BIT(2)
#define HEALTH_KNOB_ON_APP BIT(3)

// Block flags (byte 3): what this half has built in.
#define HEALTH_F_RELAY_CENTRAL BIT(0)
#define HEALTH_F_HID_STATS BIT(1) // the Raw HID module reports its counters (raw_hid/stats.h)
#define HEALTH_F_HOST_LINK BIT(2)
#define HEALTH_F_RELAY_PERIPHERAL BIT(3)
#define HEALTH_F_LIGHTING BIT(4)
#define HEALTH_F_PC_STATE BIT(5)
#define HEALTH_F_POLARITY BIT(6)
#define HEALTH_F_DISPLAY BIT(7)

// The block, all little endian (docs/health.md). Fields a half doesn't have are 0 (or 0xff
// where 0 means something); the flags say which parts it has.
struct health_wire {
    uint8_t version; // HEALTH_VERSION
    uint8_t half;    // 0 left, 1 right
    uint8_t length;  // sizeof(struct health_wire)
    uint8_t flags;   // HEALTH_F_*
    uint8_t fw_major, fw_minor, fw_patch, fw_profile; // struct foxglove_build_info
    char sha[8];
    uint32_t uptime_s;
    uint32_t reset_cause; // Zephyr's RESET_* bits (hwinfo); 0 unknown
    uint8_t selftest[HEALTH_CHECKS];
    // Relay
    uint8_t relay_debug[10]; // the left: HELLO's relay debug bytes (art_relay_debug), this link
    uint16_t relay_timeouts;
    uint16_t relay_unsticks;
    uint16_t relay_ready;
    uint8_t relay_in_flight;
    int8_t relay_discovery; // enum art_relay_discovery (negative: a bt_gatt_discover error)
    uint16_t relay_refused;
    // Raw HID (the left; the module's counters with HEALTH_F_HID_STATS)
    uint32_t hid_sent;
    uint16_t hid_queued;
    uint16_t hid_dropped;
    uint16_t hid_failed;
    uint16_t hid_reply_drops;
    // Screens
    uint32_t redraws;
    uint16_t art_ok;
    uint16_t art_failed;
    uint8_t art_last_error;
    int8_t art_set;
    uint16_t snap_errors;
    // Lighting
    uint16_t light_applies;
    uint16_t light_syncs;
    uint16_t light_sync_fails;
    uint16_t rgb_mismatches;
    uint8_t last_effect;
    uint8_t underglow_effect; // ZMK's effect index, 0xff unknown
    // Split link
    uint16_t split_drops;
    uint32_t split_up_at_s; // uptime of the last link-up, 0 never
    uint8_t split_connected;
    uint8_t guard_drops; // #122 guard (the left)
    // PC state and polarity
    uint8_t pc_state;  // pc_state.h's state, 0xff not built
    uint8_t pc_reason; // pc_state.h's reason, 0xff not built
    uint8_t polarity;       // 0 dark, 1 light, 0xff not built
    uint8_t polarity_right; // the left: what the right acknowledged (0xff none)
    uint8_t leds;          // this half's LEDs (its chain)
    uint8_t leds_right;    // the left: how many the left frames for the right (0 on the right)
    // The knob (version 2, #719; the left)
    uint8_t knob;          // HEALTH_KNOB_*
    uint8_t knob_last;     // the last event's kind (knob.h; 0 none)
    uint16_t knob_events;  // events sent to the PC
    uint16_t knob_keys;    // fallback keys tapped
    uint16_t knob_drops;   // holds dropped by the board
    // Raw HID per transport (version 3; the left, with HEALTH_F_HID_STATS and the module's
    // raw_hid/stats.h version 2, else 0). hid_sent and hid_failed above are the sums.
    uint16_t hid_usb_failed;  // reports the USB driver refused
    uint16_t hid_ble_sent;    // BLE notifications queued
    uint16_t hid_ble_failed;  // BLE notifies refused on a link that listens
    uint16_t hid_ble_skipped; // BLE reports nobody listened for (no link, notifications off,
                              // not encrypted yet): normal while the PC uses USB
} __packed;

#if IS_ENABLED(CONFIG_FOXGLOVE_HEALTH)
void health_inc(enum health_counter c);
uint32_t health_get(enum health_counter c);
void health_set(enum health_value v, uint8_t value);
// The split link came up / went down (this half's role on it).
void health_link_up(void);
void health_link_down(void);
// Left: the relay was found on this link. Right: the left subscribed to the doorbell.
void health_relay_found(void);
// Fills out with this half's block now.
void health_fill(struct health_wire *out);
// The boot self-test's result for check c now (HEALTH_RESULT).
uint8_t health_selftest(enum health_check c);
#else
static inline void health_inc(int c) { ARG_UNUSED(c); }
static inline uint32_t health_get(int c) {
    ARG_UNUSED(c);
    return 0;
}
static inline void health_set(int v, uint8_t value) {
    ARG_UNUSED(v);
    ARG_UNUSED(value);
}
static inline void health_link_up(void) {}
static inline void health_link_down(void) {}
static inline void health_relay_found(void) {}
#endif
