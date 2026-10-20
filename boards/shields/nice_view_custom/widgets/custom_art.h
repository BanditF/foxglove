/*
 * Art sets for the right-hand screen, generated into custom_art.c by scripts/flipper_art.py.
 * SPDX-License-Identifier: MIT
 */

#pragma once

#include <lvgl.h>

struct custom_art_set {
    const char *name;
    const lv_img_dsc_t *const *frames;
    int frame_count;
    int fps; // 0 for still images
};

extern const struct custom_art_set custom_art_sets[];
extern const int custom_art_set_count;
