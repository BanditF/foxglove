/*
 * Art sets for the right-hand screen, generated into custom_art.c by scripts/flipper_art.py.
 * SPDX-License-Identifier: MIT
 */

#pragma once

#include <stdint.h>

#define CUSTOM_ART_W 140
#define CUSTOM_ART_H 68
#define CUSTOM_ART_STRIDE ((CUSTOM_ART_W + 7) / 8)

// Frames are LVGL INDEXED_1BIT rows (MSB first, no palette). Each set keeps its unique frames as
// RLE-packed XOR deltas: applying deltas[k] to a buffer holding unique frame k - 1 gives frame k
// (deltas[0] is against an all-zero buffer), and applying it again goes back.
struct custom_art_set {
    const char *name;
    const uint8_t *const *deltas;
    const uint8_t *order; // play order, as indices into deltas
    int frame_count;
    int fps; // 0 for still images
};

extern const struct custom_art_set custom_art_sets[];
extern const int custom_art_set_count;
