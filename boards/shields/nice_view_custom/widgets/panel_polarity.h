/*
 * Panel polarity (#666): dark (light ink on a dark field) or light (dark ink on a light field),
 * one setting for every mono panel the keyboard draws: both halves, every widget, every piece of
 * art. SPDX-License-Identifier: MIT
 *
 * Art and the PC's panel store ink bits (#610: a set bit is ink, ART_RELAY_FLAG_INK), and every
 * drawer takes its colours from LVGL_FOREGROUND / LVGL_BACKGROUND (util.h), which read this value
 * when they draw. So the polarity alone decides what the panels show; the app normalises each
 * source image at encode time so that its ink is a set bit (art_encode.rs, flipper_art.py).
 *
 * Generic, not a nice!view special case: a value per keyboard, the drawers' colours follow it.
 * CONFIG_NICE_VIEW_WIDGET_INVERTED (Kconfig) is the default, used until the PC sets one.
 *
 * The left (central): D7 12 PANEL_POLARITY from the PC (host_link.h, HELLO bit 27) sets it; it is
 * saved in the settings ("nvc_pol/v") and sent to the right as art relay op PANEL_POLARITY
 * (art_relay.h), an acknowledged write, newest wins, again on every link-up. The right saves
 * and applies the left's value too, so a right that boots without the left shows the same.
 *
 * Start/stop: every change starts from an event: D7 12, the relay op, the settings load. A
 * change redraws once on the display queue and saves once SAVE_DELAY after the last change; no
 * timer repeats.
 */

#pragma once

#include <stdbool.h>
#include <stdint.h>

#include <zephyr/sys/util.h>

enum panel_polarity {
    PANEL_POLARITY_DARK = 0,  // light ink on a dark field (the shipped default)
    PANEL_POLARITY_LIGHT = 1, // dark ink on a light field (CONFIG_NICE_VIEW_WIDGET_INVERTED)
};

// D7 12's request byte: a polarity, or this to read.
#define PANEL_POLARITY_READ 0xff
// The right's value is unknown (no relay link, or not acknowledged yet on this link).
#define PANEL_POLARITY_UNKNOWN 0xff

#define PANEL_POLARITY_DEFAULT                                                                     \
    (IS_ENABLED(CONFIG_NICE_VIEW_WIDGET_INVERTED) ? PANEL_POLARITY_LIGHT : PANEL_POLARITY_DARK)

// The art relay op PANEL_POLARITY (art_relay.h): polarity u8.
#define PANEL_POLARITY_RELAY_OP 18
#define PANEL_POLARITY_RELAY_SIZE 2

#if IS_ENABLED(CONFIG_FOXGLOVE_PANEL_POLARITY)

// The polarity in force. Any thread.
uint8_t panel_polarity_get(void);

// The left: from the PC (D7 12). Saves it, redraws this half and sends it to the right.
// -EINVAL for anything but a polarity (nothing changes). Any thread.
int panel_polarity_set(uint8_t polarity);

// The right: the left's value (relay op). Saves and redraws as set does; ignores a bad value.
void panel_polarity_remote(uint8_t polarity);

// The left: the relay op for the right as it is now. Any thread.
void panel_polarity_relay_op(uint8_t out[PANEL_POLARITY_RELAY_SIZE]);

// The drawers (status.c on the left, peripheral_status.c on the right): redraw everything in the
// new colours. Called on the display queue after a change. Weak no-op otherwise.
void panel_polarity_apply(void);

// Saves the value (settings_save_one, "nvc_pol/v"); 0 on success. Weak, so the simulator can
// watch it.
int panel_polarity_store(uint8_t polarity);

#else

static inline uint8_t panel_polarity_get(void) { return PANEL_POLARITY_DEFAULT; }

#endif

static inline bool panel_polarity_light(void) {
    return panel_polarity_get() == PANEL_POLARITY_LIGHT;
}
