/*
 * The status strip's icons, shared by both halves (#610): battery, charging bolt, connection.
 * SPDX-License-Identifier: MIT
 *
 * Everything is drawn in portrait coordinates (68 wide, the strip at the top), shifted up by oy
 * for canvases further down the screen, in LVGL_FOREGROUND on whatever the caller filled with
 * LVGL_BACKGROUND, so both halves follow the one screen theme.
 *
 *   x 1-24   battery: rounded 22x10 outline, a 17 px fill for the charge, 2x4 nub
 *   x 26-31  charging bolt, beside the battery (rows 2-10)
 *   x 34-66  connection: the left's output as text in unscii-8 (USB, BT1-BT5; struck through
 *            while that profile isn't connected), or the right's split link as a glyph: two
 *            halves joined by a bar, filled and joined when linked, outlined and broken when not.
 *            Never a Wi-Fi symbol.
 */

#pragma once

#include <stdbool.h>
#include <stdint.h>

#include <lvgl.h>

#include "util.h"

void icons_rect(lv_obj_t *c, int oy, int x, int y, int w, int h, int radius, bool filled,
                bool outline, lv_color_t color);
void icons_text(lv_obj_t *c, int oy, int x, int y, int w, const char *t, lv_color_t color,
                lv_text_align_t align);

// Battery level (0-100) with the bolt beside it while charging.
void icons_battery(lv_obj_t *c, int oy, uint8_t level, bool charging);
// The left's output: text, struck through when not connected.
void icons_output(lv_obj_t *c, int oy, const char *label, bool connected);
// The right's split link to the left.
void icons_split_link(lv_obj_t *c, int oy, bool connected);
