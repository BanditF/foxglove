/*
 * How fast the art may play on this half's art target (#842): the firmware's own playback (built-in
 * sets, sets kept in flash slots, pushed sets) follows the same rule the app uses for what it
 * sends (#833, art_encode.rs Pace).
 * SPDX-License-Identifier: MIT
 *
 * The target's refresh class and shortest frame interval come from this half's screens table, its
 * SCREENS art target's v2 descriptor (#832, host_link.h): art_refresh and art_min_frame_100ms.
 * - A fast screen (refresh 0) plays a set at its own rate, held to the shortest frame interval
 *   when it has one. The Corne's right (fast, no limit) plays exactly as before: 1000 / fps.
 * - On e-paper (refresh 1 or 2, or a value this firmware doesn't know) a set shows its first frame
 *   only, unless the PC lets art animate there (D6 0A ANIMATE, the app's "animate on e-paper"
 *   setting; HELLO v2 capability 33 ART_PACE). Then it plays held to the interval as above.
 * The PC's choice lives in RAM only and starts off: an e-paper screen without a PC stays still.
 *
 * The drawer asks art_pace_frame_ms() whenever it starts a set (art_play) or the pace changes
 * (art_repace, art_store.h).
 */

#pragma once

#include <stdbool.h>
#include <stdint.h>

#include <zephyr/sys/util.h>

#include "host_link.h"

struct custom_art_set;

struct art_pace {
    uint8_t refresh;       // SCREEN_REFRESH_*
    uint16_t min_frame_ms; // 0: no limit
    bool animate;          // the PC lets art animate on e-paper (D6 0A)
};

// The interval between frames for a set of `frames` frames at `fps`, on a target paced by p: 0
// shows the first frame only (a still set, or e-paper without the PC's go).
static inline uint32_t art_pace_interval_ms(struct art_pace p, int fps, int frames) {
    if (frames <= 1 || fps <= 0) {
        return 0;
    }
    if (p.refresh != SCREEN_REFRESH_FAST && !p.animate) {
        return 0;
    }
    return MAX(1000U / (uint32_t)fps, (uint32_t)p.min_frame_ms);
}

// The pace of the art target on half `half` in this half's screens table (foxglove_screens[]);
// a fast screen with no limit when the table has none there.
struct art_pace art_pace_for_half(uint8_t half);
// This half's own art target's pace (half 0 on a one-piece board or a split board's central,
// 1 on its peripheral).
struct art_pace art_pace_now(void);
// The drawer's frame interval for set on this half's target (art_pace_interval_ms); 0: still.
uint32_t art_pace_frame_ms(const struct custom_art_set *set);
// D6 0A: the PC's "animate on e-paper". Returns whether it changed. Any thread.
bool art_pace_set_animate(bool on);
