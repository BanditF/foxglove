/*
 * The art target's pace (#842; art_pace.h): read from this half's screens table, with the PC's
 * "animate on e-paper" choice.
 * SPDX-License-Identifier: MIT
 */

#include <zephyr/kernel.h>

#include "art_pace.h"
#include "custom_art.h"

// The half this firmware runs on, as the screens table numbers them.
#if IS_ENABLED(CONFIG_ZMK_SPLIT) && !IS_ENABLED(CONFIG_ZMK_SPLIT_ROLE_CENTRAL)
#define ART_PACE_HALF 1
#else
#define ART_PACE_HALF 0
#endif

static atomic_t animate; // off until the PC says (D6 0A); never saved

struct art_pace art_pace_for_half(uint8_t half) {
    struct art_pace p = {.refresh = SCREEN_REFRESH_FAST, .animate = atomic_get(&animate) != 0};
    for (int i = 0; i < foxglove_screen_count; i++) {
        const struct screen_descriptor *s = &foxglove_screens[i];
        if (s->kind == SCREEN_ART && s->half == half) {
            p.refresh = s->art_refresh;
            p.min_frame_ms = 100U * s->art_min_frame_100ms;
            break;
        }
    }
    return p;
}

struct art_pace art_pace_now(void) { return art_pace_for_half(ART_PACE_HALF); }

uint32_t art_pace_frame_ms(const struct custom_art_set *set) {
    return art_pace_interval_ms(art_pace_now(), set->fps, set->frame_count);
}

bool art_pace_set_animate(bool on) { return (atomic_set(&animate, on) != 0) != on; }
