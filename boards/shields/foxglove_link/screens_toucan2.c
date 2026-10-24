/*
 * Beekeeb Toucan2's screens for D5 06 SCREENS (#160, #75): one 144x168 memory LCD on the left,
 * drawn by the board's own widgets (nice_view_gem), and the PC's panel in its bottom third
 * (panel_lvgl.c; D5 02-05, one 18-byte row per ROWS report). No readback yet.
 * SPDX-License-Identifier: MIT
 */

#include <zephyr/kernel.h>
#include <zephyr/sys/util.h>

#include "host_link.h"

BUILD_ASSERT(sizeof(struct screen_descriptor) == 24, "descriptor layout");

const struct screen_descriptor foxglove_screens[] = {
    {.id = 0, .kind = SCREEN_DISPLAY, .half = 0, .parent = SCREEN_UNSET, .width = 144,
     .height = 168, .rotation = 0, .ops = 0, .stride = 18, .readback_half = SCREEN_UNSET},
    {.id = 1, .kind = SCREEN_PANEL, .half = 0, .parent = 0, .width = HOST_PANEL_W,
     .height = HOST_PANEL_H, .x = HOST_PANEL_X, .y = HOST_PANEL_Y, .ops = SCREEN_OP_ROWS | SCREEN_OP_ROWS_AT,
     .stride = HOST_PANEL_STRIDE, .rows_per_report = 1, .readback_half = SCREEN_UNSET},
};
const uint8_t foxglove_screen_count = ARRAY_SIZE(foxglove_screens);
