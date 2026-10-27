/*
 * SET_FRAME (D7 0C, #272): per-LED frames from the PC for both halves. Protocol in host_link.h.
 * SPDX-License-Identifier: MIT
 */

#pragma once

#include <stdbool.h>
#include <stdint.h>

#define HOST_FRAMES_REPLY_SIZE 7

// Who has the lights (the reply's owner byte). 1 and 2 stay free for the HID LampArray over USB
// and BLE (#115, #239), which would share the one-owner rule.
enum { HOST_FRAMES_OWNER_NONE = 0, HOST_FRAMES_OWNER_HOST_LINK = 3 };

// A D7 0C report: d is padded to the report size, len is its real length. Fills r with the D7 8C
// reply and returns whether it goes out (SHOW, RELEASE and errors reply; chunks don't).
bool host_frames_command(const uint8_t *d, int len, uint8_t r[HOST_FRAMES_REPLY_SIZE]);
