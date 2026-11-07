/*
 * The on-board art step's toast (#852): after &art_next / &art_prev steps the right's art without
 * the app (art_relay_step), the left screen shows "ART", the piece's place in the step list
 * (n/m), a triangle the way it stepped and whether it is a built-in or a kept piece, over the
 * panel area for ART_TOAST_MS. The app draws its own card when it holds the art keys (#60).
 * SPDX-License-Identifier: MIT
 *
 * Start/stop of its one timer (status.c): starts at a step, a newer step restarts it, and it ends
 * at ART_TOAST_MS with a redraw. Nothing else wakes it.
 */

#pragma once

#include <stdbool.h>
#include <stdint.h>

#define ART_TOAST_MS 1200 // as the app's card (screens.rs ART_CARD_FOR)

struct art_toast {
    uint8_t index; // the piece's place in the step list, from 0
    uint8_t total; // the step list's length
    int8_t dir;    // 1 next, -1 previous
    bool kept;     // a saved slot, not a built-in
};

// A step went out: show the toast (any thread; the drawing is on the display queue).
void art_toast_show(const struct art_toast *t);
