/*
 * The keyboard's self-report (#705): counters, states and the boot self-test of this half, and
 * its block for D7 13 HEALTH (health.h, docs/health.md).
 * SPDX-License-Identifier: MIT
 *
 * Start/stop: nothing runs on its own. Counters move when their subsystem's event happens; the
 * self-test and every live value are read when the block is filled (a PC read, or the left's
 * relay read of the right). No timer, no boot work, no settings read. Every target is little
 * endian, so the packed struct is the wire as it stands (as struct art_relay_status is).
 */

#include <string.h>

#include <zephyr/devicetree.h>
#include <zephyr/kernel.h>
#include <zephyr/sys/atomic.h>

#include "health.h"
#include "half_marker.h"
#include "lamp_parts.h"
#include "panel_polarity.h"
#include "knob.h"

#define HEALTH_LEFT (!IS_ENABLED(CONFIG_ZMK_SPLIT) || IS_ENABLED(CONFIG_ZMK_SPLIT_ROLE_CENTRAL))

#if IS_ENABLED(CONFIG_NICE_VIEW_ART_RELAY)
#include "art_relay.h"
#endif
#if IS_ENABLED(CONFIG_FOXGLOVE_PC_STATE)
#include "pc_state.h"
#endif
#if IS_ENABLED(CONFIG_NICE_VIEW_RGB_IDLE)
#include "rgb_idle.h"
#endif
#if IS_ENABLED(CONFIG_ZMK_DISPLAY)
#include <zephyr/device.h>
#include <zephyr/devicetree.h>
#include <zmk/display.h>
#endif
#if IS_ENABLED(CONFIG_HWINFO)
#include <zephyr/drivers/hwinfo.h>
#endif
#if IS_ENABLED(CONFIG_SETTINGS)
#include <zephyr/settings/settings.h>
#endif
#if IS_ENABLED(CONFIG_LED_STRIP) && DT_HAS_CHOSEN(zmk_underglow)
#include <zephyr/device.h>
#define HEALTH_HAS_STRIP 1
#endif
// The Raw HID module's counters (BanditF/zmk-raw-hid, #680), when the module has them.
#if HEALTH_LEFT && defined(__has_include)
#if __has_include(<raw_hid/stats.h>)
#include <raw_hid/stats.h>
#define HEALTH_HAS_HID_STATS 1
#endif
#endif
#if HEALTH_LEFT && IS_ENABLED(CONFIG_NICE_VIEW_ART_RELAY)
#define HEALTH_RELAY_CENTRAL 1
#elif IS_ENABLED(CONFIG_NICE_VIEW_ART_RELAY)
#define HEALTH_RELAY_PERIPHERAL 1
uint32_t art_redraw_count(void); // peripheral_status.c (#618)
#endif

#ifndef CONFIG_FOXGLOVE_HEALTH_RELAY_S
#define CONFIG_FOXGLOVE_HEALTH_RELAY_S 60
#endif

static atomic_t counters[HEALTH_COUNTERS];
static atomic_t values[HEALTH_VALUES] = {
    [HEALTH_ART_SET] = ATOMIC_INIT(0x80),
};
static atomic_t link_up_at_ms;   // uptime of the last link-up (+1 so 0 means never)
static atomic_t link_connected;
static atomic_t relay_found_ms; // uptime the relay was first found (+1; 0 = not yet)
static atomic_t ever_linked;
static atomic_t settings_done;

// The fields that are u16 on the wire saturate there; the rest (redraws) are u32.
static atomic_val_t limit(enum health_counter c) {
    return c == HEALTH_REDRAWS ? (atomic_val_t)0x7fffffff : (atomic_val_t)UINT16_MAX;
}

void health_inc(enum health_counter c) {
    if (c >= HEALTH_COUNTERS) {
        return;
    }
    atomic_val_t v;
    do {
        v = atomic_get(&counters[c]);
        if (v >= limit(c)) {
            return;
        }
    } while (!atomic_cas(&counters[c], v, v + 1));
}

uint32_t health_get(enum health_counter c) {
    return c < HEALTH_COUNTERS ? (uint32_t)atomic_get(&counters[c]) : 0;
}

void health_set(enum health_value v, uint8_t value) {
    if (v < HEALTH_VALUES) {
        atomic_set(&values[v], value);
    }
}

static uint32_t now_ms(void) { return (uint32_t)k_uptime_get(); }

void health_link_up(void) {
    atomic_set(&link_up_at_ms, (atomic_val_t)(now_ms() + 1));
    atomic_set(&link_connected, 1);
    atomic_set(&ever_linked, 1);
}

void health_link_down(void) {
    if (atomic_cas(&link_connected, 1, 0)) {
        health_inc(HEALTH_SPLIT_DROPS);
    }
}

void health_relay_found(void) {
    atomic_cas(&relay_found_ms, 0, (atomic_val_t)(now_ms() + 1));
    health_inc(HEALTH_RELAY_READY);
}

#if IS_ENABLED(CONFIG_SETTINGS)
// The settings were loaded (ZMK's main calls settings_load; every handler's commit follows). Only
// a flag: nothing is read here.
static int health_settings_commit(void) {
    atomic_set(&settings_done, 1);
    return 0;
}
SETTINGS_STATIC_HANDLER_DEFINE(foxglove_health, "fxh", NULL, NULL, health_settings_commit, NULL);
#endif

uint8_t health_selftest(enum health_check c) {
    uint32_t up_s = now_ms() / 1000;
    switch (c) {
    case HEALTH_CHECK_DISPLAY:
#if IS_ENABLED(CONFIG_ZMK_DISPLAY) && DT_HAS_CHOSEN(zephyr_display)
        if (!device_is_ready(DEVICE_DT_GET(DT_CHOSEN(zephyr_display)))) {
            return HEALTH_RESULT(HEALTH_FAIL, 1);
        }
        if (!zmk_display_is_initialized()) {
            return up_s < HEALTH_DISPLAY_AFTER_S ? HEALTH_RESULT(HEALTH_PENDING, 0)
                                                 : HEALTH_RESULT(HEALTH_FAIL, 2);
        }
        return HEALTH_RESULT(HEALTH_PASS, 0);
#else
        return HEALTH_RESULT(HEALTH_NA, 0);
#endif
    case HEALTH_CHECK_SETTINGS:
#if IS_ENABLED(CONFIG_SETTINGS)
        if (atomic_get(&settings_done)) {
            return HEALTH_RESULT(HEALTH_PASS, 0);
        }
        return up_s < HEALTH_SETTINGS_AFTER_S ? HEALTH_RESULT(HEALTH_PENDING, 0)
                                              : HEALTH_RESULT(HEALTH_FAIL, 1);
#else
        return HEALTH_RESULT(HEALTH_NA, 0);
#endif
    case HEALTH_CHECK_RELAY: {
#if IS_ENABLED(CONFIG_NICE_VIEW_ART_RELAY) && IS_ENABLED(CONFIG_ZMK_SPLIT)
        uint32_t found = (uint32_t)atomic_get(&relay_found_ms);
        if (found != 0) {
            return (found - 1) / 1000 <= CONFIG_FOXGLOVE_HEALTH_RELAY_S
                       ? HEALTH_RESULT(HEALTH_PASS, 0)
                       : HEALTH_RESULT(HEALTH_FAIL, 1);
        }
        if (up_s <= CONFIG_FOXGLOVE_HEALTH_RELAY_S) {
            return HEALTH_RESULT(HEALTH_PENDING, 0);
        }
        return HEALTH_RESULT(HEALTH_FAIL, atomic_get(&ever_linked) ? 3 : 2);
#else
        return HEALTH_RESULT(HEALTH_NA, 0);
#endif
    }
    case HEALTH_CHECK_LEDS:
#if defined(HEALTH_HAS_STRIP)
        if (!device_is_ready(DEVICE_DT_GET(DT_CHOSEN(zmk_underglow)))) {
            return HEALTH_RESULT(HEALTH_FAIL, 1);
        }
        return LAMP_PARTS_OWN > 0 ? HEALTH_RESULT(HEALTH_PASS, 0) : HEALTH_RESULT(HEALTH_FAIL, 2);
#else
        return HEALTH_RESULT(HEALTH_NA, 0);
#endif
    default:
        return HEALTH_RESULT(HEALTH_NA, 0);
    }
}

static uint16_t u16_of(enum health_counter c) { return (uint16_t)MIN(health_get(c), UINT16_MAX); }

void health_fill(struct health_wire *out) {
    memset(out, 0, sizeof(*out));
    BUILD_ASSERT(sizeof(struct health_wire) == HEALTH_SIZE, "docs/health.md");
    out->version = HEALTH_VERSION;
    out->half = HEALTH_LEFT ? 0 : 1;
    out->length = sizeof(*out);
    struct foxglove_build_info build;
    foxglove_build_info(&build);
    out->fw_major = build.major;
    out->fw_minor = build.minor;
    out->fw_patch = build.patch;
    out->fw_profile = build.profile;
    memcpy(out->sha, build.sha, sizeof(out->sha));
    out->uptime_s = now_ms() / 1000;
#if IS_ENABLED(CONFIG_HWINFO)
    uint32_t cause = 0;
    if (hwinfo_get_reset_cause(&cause) == 0) {
        out->reset_cause = cause;
    }
#endif
    for (int i = 0; i < HEALTH_CHECKS; i++) {
        out->selftest[i] = health_selftest((enum health_check)i);
    }

    // Relay
    out->relay_timeouts = u16_of(HEALTH_RELAY_TIMEOUTS);
    out->relay_unsticks = u16_of(HEALTH_RELAY_UNSTICKS);
    out->relay_ready = u16_of(HEALTH_RELAY_READY);
    out->relay_refused = u16_of(HEALTH_RELAY_REFUSED);
#if defined(HEALTH_RELAY_CENTRAL)
    out->flags |= HEALTH_F_RELAY_CENTRAL;
    art_relay_debug(out->relay_debug); // HELLO's bytes, folded in rather than kept twice
    out->relay_in_flight = out->relay_debug[6];
    int8_t discovery;
    uint8_t tries;
    art_relay_discovery(&discovery, &tries);
    out->relay_discovery = discovery;
    out->guard_drops = art_relay_guard_drops();
#elif defined(HEALTH_RELAY_PERIPHERAL)
    out->flags |= HEALTH_F_RELAY_PERIPHERAL;
#endif

    // Raw HID
#if HEALTH_LEFT && IS_ENABLED(CONFIG_NICE_VIEW_HOST_LINK)
    out->flags |= HEALTH_F_HOST_LINK;
#endif
    out->hid_reply_drops = u16_of(HEALTH_HID_REPLY_DROPS);
#if defined(HEALTH_HAS_HID_STATS)
    struct raw_hid_stats hid;
    raw_hid_stats_get(&hid);
    out->flags |= HEALTH_F_HID_STATS;
    out->hid_sent = hid.sent;
    out->hid_queued = (uint16_t)MIN(hid.queued, UINT16_MAX);
    out->hid_dropped = (uint16_t)MIN(hid.dropped, UINT16_MAX);
    out->hid_failed = (uint16_t)MIN(hid.failed, UINT16_MAX);
#endif

    // Screens
#if defined(HEALTH_RELAY_PERIPHERAL) && defined(FOXGLOVE_HAS_WIDGETS)
    out->redraws = art_redraw_count();
#else
    out->redraws = health_get(HEALTH_REDRAWS);
#endif
#if IS_ENABLED(CONFIG_ZMK_DISPLAY)
    out->flags |= HEALTH_F_DISPLAY;
#endif
    out->art_ok = u16_of(HEALTH_ART_OK);
    out->art_failed = u16_of(HEALTH_ART_FAILED);
    out->art_last_error = (uint8_t)atomic_get(&values[HEALTH_ART_LAST_ERROR]);
    out->art_set = (int8_t)(uint8_t)atomic_get(&values[HEALTH_ART_SET]);
    out->snap_errors = u16_of(HEALTH_SNAP_ERRORS);

    // Lighting
    out->light_applies = u16_of(HEALTH_LIGHT_APPLIES);
    out->light_syncs = u16_of(HEALTH_LIGHT_SYNCS);
    out->light_sync_fails = u16_of(HEALTH_LIGHT_SYNC_FAILS);
    out->rgb_mismatches = u16_of(HEALTH_RGB_MISMATCHES);
    out->last_effect = (uint8_t)atomic_get(&values[HEALTH_LAST_EFFECT]);
    out->underglow_effect = 0xff;
#if IS_ENABLED(CONFIG_NICE_VIEW_RGB_IDLE)
    out->flags |= HEALTH_F_LIGHTING;
    struct rgb_idle_wire rgb;
    rgb_idle_state(&rgb);
    out->underglow_effect = rgb.effect;
#endif
    out->leds = (uint8_t)MIN(LAMP_PARTS_OWN, UINT8_MAX);
#if HEALTH_LEFT && IS_ENABLED(CONFIG_ZMK_SPLIT)
    out->leds_right = (uint8_t)MIN(LAMP_PARTS_RIGHT, UINT8_MAX);
#endif

    // Split link
    out->split_drops = u16_of(HEALTH_SPLIT_DROPS);
    uint32_t up_at = (uint32_t)atomic_get(&link_up_at_ms);
    out->split_up_at_s = up_at ? MAX((up_at - 1) / 1000, 1) : 0;
    out->split_connected = (uint8_t)atomic_get(&link_connected);

    // PC state and polarity
    out->pc_state = 0xff;
    out->pc_reason = 0xff;
#if IS_ENABLED(CONFIG_FOXGLOVE_PC_STATE)
    out->flags |= HEALTH_F_PC_STATE;
    struct pc_state_wire pc;
    pc_state_report(&pc);
    out->pc_state = pc.state;
    out->pc_reason = pc.reason;
#endif
    out->polarity = 0xff;
    out->polarity_right = 0xff;
#if IS_ENABLED(CONFIG_FOXGLOVE_PANEL_POLARITY)
    out->flags |= HEALTH_F_POLARITY;
    out->polarity = panel_polarity_get();
#if defined(HEALTH_RELAY_CENTRAL)
    out->polarity_right = art_relay_right_polarity();
#endif
#endif

    // The knob (#719)
#if HEALTH_LEFT && IS_ENABLED(CONFIG_FOXGLOVE_KNOB)
    BUILD_ASSERT(HEALTH_KNOB_TURN == KNOB_HAS_TURN && HEALTH_KNOB_PRESS == KNOB_HAS_PRESS, "knob.h");
    if (DT_HAS_COMPAT_STATUS_OKAY(foxglove_behavior_knob)) {
        out->knob |= HEALTH_KNOB_TURN;
    }
    if (DT_HAS_COMPAT_STATUS_OKAY(foxglove_behavior_knob_press)) {
        out->knob |= HEALTH_KNOB_PRESS;
    }
    uint8_t hold = (uint8_t)atomic_get(&values[HEALTH_KNOB_STATE]);
    if (hold & BIT(0)) {
        out->knob |= HEALTH_KNOB_HELD;
    }
    if (hold & BIT(1)) {
        out->knob |= HEALTH_KNOB_ON_APP;
    }
    out->knob_last = (uint8_t)atomic_get(&values[HEALTH_KNOB_LAST]);
    out->knob_events = u16_of(HEALTH_KNOB_EVENTS);
    out->knob_keys = u16_of(HEALTH_KNOB_KEYS);
    out->knob_drops = u16_of(HEALTH_KNOB_DROPS);
#endif
}
