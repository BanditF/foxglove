/*
 * Beekeeb Toucan2's screens for D5 06 SCREENS (#160, #75): one 144x168 memory LCD on the left,
 * drawn by the board's own widgets (nice_view_gem). Nothing the PC can draw into or read back
 * yet, so no ops; a host panel and readback come with their own work.
 * SPDX-License-Identifier: MIT
 */

#include <zephyr/kernel.h>
#include <zephyr/sys/util.h>

#include "host_link.h"

BUILD_ASSERT(sizeof(struct screen_descriptor) == 24, "descriptor layout");

const struct screen_descriptor foxglove_screens[] = {
    {.id = 0, .kind = SCREEN_DISPLAY, .half = 0, .parent = SCREEN_UNSET, .width = 144,
     .height = 168, .rotation = 0, .ops = 0, .stride = 18, .readback_half = SCREEN_UNSET},
};
const uint8_t foxglove_screen_count = ARRAY_SIZE(foxglove_screens);
