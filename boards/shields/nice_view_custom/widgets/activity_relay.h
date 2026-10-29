/*
 * The left's activity, relayed to the right half (#603, #605).
 * SPDX-License-Identifier: MIT
 *
 * ZMK counts activity on each half by its own keys, so the right's art went still 30 s after its
 * last own key even while the left was typing, and stayed still after the left woke (a key there,
 * or a cable going in: usb_wake.c) until a right key. The left now tells the right each change
 * of its activity state over the art relay (art_relay.h op ACTIVITY); the right's art plays
 * while either half is active (peripheral_status.c). Nothing is faked into ZMK on the right: its
 * own idle, sleep and battery rules stay its own.
 *
 * Wire: op 16, state u8, ZMK's enum zmk_activity_state (0 active, 1 idle, 2 going to sleep).
 * A right before this firmware has no case for the op and ignores it (data_write answers
 * success), so the left sends it to any right without a version check.
 */

#pragma once

#include <stdint.h>

#include <zephyr/toolchain.h>

#include <zmk/activity.h>

#define ART_RELAY_ACTIVITY_OP 16

BUILD_ASSERT(ZMK_ACTIVITY_ACTIVE == 0 && ZMK_ACTIVITY_IDLE == 1 && ZMK_ACTIVITY_SLEEP == 2,
             "the ACTIVITY op carries ZMK's activity states as they are");

// The op as it goes to the right: this half's activity state now.
static inline void art_relay_activity_op(uint8_t out[2]) {
    out[0] = ART_RELAY_ACTIVITY_OP;
    out[1] = (uint8_t)zmk_activity_get_state();
}

// Left (art_relay_central.c): this half's activity state changed (activity_relay.c's listener);
// send the right the state as it is now. Newest wins; any thread.
void art_relay_activity_kick(void);
