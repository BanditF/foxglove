/*
 * The Eyelash Corne's screens for D5 06 SCREENS (#160): what the board already does.
 * SPDX-License-Identifier: MIT
 */

#include <zephyr/kernel.h>
#include <zephyr/sys/util.h>

#include "host_link.h"

BUILD_ASSERT(sizeof(struct screen_descriptor) == 24, "descriptor layout");

const struct screen_descriptor foxglove_screens[] = {
    {.id = 0, .kind = SCREEN_DISPLAY, .half = 0, .parent = SCREEN_UNSET, .width = 160, .height = 68,
     .rotation = 1, .ops = SCREEN_OP_READBACK, .stride = 20, .readback_half = 0},
    {.id = 1, .kind = SCREEN_PANEL, .half = 0, .parent = 0, .width = HOST_PANEL_W,
     .height = HOST_PANEL_H, .x = HOST_PANEL_X, .y = HOST_PANEL_Y, .ops = SCREEN_OP_ROWS,
     .stride = HOST_PANEL_STRIDE, .rows_per_report = 3, .readback_half = SCREEN_UNSET},
#if IS_ENABLED(CONFIG_NICE_VIEW_ART_RELAY)
    {.id = 2, .kind = SCREEN_DISPLAY, .half = 1, .parent = SCREEN_UNSET, .width = 160, .height = 68,
     .rotation = 1, .ops = SCREEN_OP_READBACK, .stride = 20, .readback_half = 1},
    // The right's art, upright as the art tools draw it (stored turned in the NVA1 blobs).
    {.id = 3, .kind = SCREEN_ART, .half = 1, .parent = 2, .width = 68, .height = 140, .x = 0,
     .y = 20, .ops = SCREEN_OP_ART | SCREEN_OP_SLOTS, .readback_half = SCREEN_UNSET},
#endif
};
const uint8_t foxglove_screen_count = ARRAY_SIZE(foxglove_screens);
