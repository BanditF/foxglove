/*
 *
 * Copyright (c) 2023 The ZMK Contributors
 * SPDX-License-Identifier: MIT
 *
 */

#pragma once

#include <lvgl.h>
#include <zephyr/kernel.h>
#include "util.h"

struct zmk_widget_status {
    sys_snode_t node;
    lv_obj_t *obj;
    lv_color_t cbuf[CANVAS_SIZE * CANVAS_SIZE];
    struct status_state state;
};

struct custom_art_set;

// Art control, for art_relay_peripheral.c. art_play runs on the display work queue; art_hold
// (freeze the current frame until the next art_play) from any thread.
void art_play(const struct custom_art_set *set);
void art_hold(void);
// Rewrite every pixel of the panel soon (#613): after anything that may have switched its power
// back on, and after an art change. Any thread.
void art_redraw_all(void);
// The built-in set index to use for index, picking one at random when it's out of range.
int art_builtin_index(int index);

int zmk_widget_status_init(struct zmk_widget_status *widget, lv_obj_t *parent);
lv_obj_t *zmk_widget_status_obj(struct zmk_widget_status *widget);
