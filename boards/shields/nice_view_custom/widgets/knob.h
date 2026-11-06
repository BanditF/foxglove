/*
 * The knob (#719): `&knob` (an encoder's turns) and `&knob_press` (its click), for any ZMK
 * encoder board. Without the PC each is its fallback key (by default the volume keys and mute),
 * so the system volume works with or without Foxglove. The PC may hold the knob (D7 14 KNOB,
 * HELLO bit 31; host_link.h): then the click, a double click and a hold go to the PC as events,
 * and so do the turns while the PC's target is an app. With master as the target the turns stay
 * the fallback keys, and a double click taps the fallback mute here, unless the PC also holds
 * master volume (D7 16, #855): then master's turns are events too and no mute key goes. The board
 * drops both on a USB disconnect, an endpoint change or a lapsed hold lease (#847).
 * SPDX-License-Identifier: MIT
 */

#pragma once

#include <stdbool.h>
#include <stdint.h>

#include <zephyr/sys/util.h>

// D7 94 01's kinds (host_link.h, 14 KNOB).
enum knob_event {
    KNOB_TURN = 1,      // steps: + clockwise, - counter-clockwise (an app is the target, or
                        // master with master volume held, #855)
    KNOB_CLICK = 2,     // a click, once double-tap-ms passed without a second (the next app)
    KNOB_DOUBLE = 3,    // a second click within double-tap-ms (mute the target)
    KNOB_HOLD = 4,      // pressed for hold-ms (back to master), sent as the time is reached
    KNOB_HANDED_BACK = 5, // the board dropped the hold (an endpoint change); the PC holds again
                          // on its next connect
};

// D7 14's targets.
enum knob_target {
    KNOB_MASTER = 0,
    KNOB_APP = 1,
};

// Which knob behaviours the keymap has (D7 94 reply byte 6, the health block's knob flags).
#define KNOB_HAS_TURN BIT(0)
#define KNOB_HAS_PRESS BIT(1)
