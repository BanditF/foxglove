/*
 * SET_EFFECT (D7 0F, #490): an effect from the PC that each half draws on its own. Protocol in
 * host_link.h. Left half only.
 * SPDX-License-Identifier: MIT
 */

#pragma once

#include <stdbool.h>
#include <stdint.h>

#define HOST_EFFECTS_REPLY_SIZE 7

// A D7 0F report: d is padded to the report size, len is its real length. Fills r with the D7 8F
// reply (every op replies).
void host_effects_command(const uint8_t *d, int len, uint8_t r[HOST_EFFECTS_REPLY_SIZE]);

// SET_FRAME takes the lights (host_frames.c, under its lock): the effect stops on both halves
// without handing back; the right hears it from the frames' own take-over. Any thread.
void host_effects_yield(void);
