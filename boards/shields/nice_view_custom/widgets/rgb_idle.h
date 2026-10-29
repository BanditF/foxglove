/*
 * Underglow idle auto-off that survives a reboot, and the right half following the left's.
 * SPDX-License-Identifier: MIT
 */

#pragma once

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#include <zephyr/toolchain.h>

// One half's underglow, as the PC sees it (0xD7 03) and as the right reports it (art_relay.h).
struct rgb_idle_wire {
    uint8_t on;
    uint16_t hue;       // 0-359
    uint8_t saturation; // 0-100
    uint8_t brightness; // 0-100
    uint8_t effect;     // 0 solid, 1 breathe, 2 spectrum, 3 swirl
    uint8_t speed;      // 1-5, as last saved (ZMK has no getter; lags a minute after a change)
} __packed;

// What one half's LEDs show (#605): D7 83 bytes 17-28 (HELLO bit 24 RGB_SHOWN) and the right's
// doorbell (art_relay.h). The stored colour and effect stay in struct rgb_idle_wire.
struct rgb_shown_wire {
    uint8_t flags;      // RGB_SHOWN_*; 0 = no data (older firmware, or the right not heard yet)
    uint16_t hue;       // 0-359: the brightest LED in the last frame drawn; 0xffff when none
    uint8_t saturation; // 0-100, the same LED's
    uint8_t value;      // 0-100: that LED's level as emitted, after the battery dim
    uint8_t dim;        // the battery dim level in force, 1-100 (100 without dimming)
} __packed;

#define RGB_SHOWN_VALID (1 << 0)
#define RGB_SHOWN_LIT (1 << 1)       // the LEDs are lit: after idle, sync, the PC's frames, ext power
#define RGB_SHOWN_WANTED (1 << 2)    // the user's on/off as saved on this half
#define RGB_SHOWN_HOST (1 << 3)      // the PC draws this half's LEDs (SET_FRAME / SET_EFFECT)
#define RGB_SHOWN_IDLE (1 << 4)      // this half is idle or asleep: its keys, and no PC state hold
#define RGB_SHOWN_UNPOWERED (1 << 5) // ext power is off: the LEDs have no power
#define RGB_SHOWN_DRAWN (1 << 6)     // hue, saturation and value come from a frame drawn
#define RGB_SHOWN_FOLLOWING (1 << 7) // the right: the left's on/off is in charge (sync)

// This half's underglow. Any thread.
void rgb_idle_state(struct rgb_idle_wire *out);
// What this half's LEDs show now. Any thread.
void rgb_idle_shown(struct rgb_shown_wire *out);
// Whether this half's LEDs are lit (RGB_SHOWN_LIT). Any thread.
bool rgb_idle_lit(void);

// dim_strip.c: a frame went to the LEDs, the PC's (host) or ZMK's own, scaled to cap percent.
// The lowest-priority queue, every frame: keeps a summary only.
struct led_rgb;
void rgb_idle_frame(bool host, const struct led_rgb *pixels, size_t n, unsigned int cap);

// The left's look for the right (art relay op RGB, bytes 8-12, #624): hue u16 LE, saturation,
// brightness (ZMK's stored values) and effect. Any thread.
#define RGB_IDLE_LOOK_SIZE 5
void rgb_idle_look(uint8_t out[RGB_IDLE_LOOK_SIZE]);
// Right: take the left's look while following it (only what differs is set). Any thread.
void rgb_idle_remote_look(const uint8_t look[RGB_IDLE_LOOK_SIZE]);
// Whether the underglow is on as the user sees it: ZMK's state, or under the PC's frames
// (SET_FRAME) or effect (SET_EFFECT) the on/off that comes back on hand-back. The scene itself
// draws whatever this says (#542). 0 or ZMK's error. Any thread.
int rgb_idle_get_on(bool *on);

// An rgb_ug binding may just have run (a key, an encoder, a PC invoke): look at the underglow's
// on/off soon. Any thread.
void rgb_idle_check(void);

// Left: whether the right half's underglow follows the left's (persisted, default on).
bool rgb_idle_sync(void);
void rgb_idle_set_sync(bool sync);

// Right: the left's underglow state, from the art relay link. follow = false hands the right its
// own idle auto-off back.
void rgb_idle_remote(bool on, bool follow);
// Right: the same, with the left's own on/off (the RGB op's byte 13, #609), which the right shows
// while the PC state holds it on its own USB power. Any thread.
void rgb_idle_remote_wanted(bool on, bool follow, bool left_wanted);
// Left: the on/off the right should follow (the RGB op's byte 1): the user's with the left's
// keys, not the PC state's hold, so a right on battery keeps its own rules (#609); own is the
// user's on/off as saved. 0 or ZMK's error. Any thread.
int rgb_idle_relay_on(bool *on, bool *own);
// The PC draws this half's LEDs (SET_FRAME, lamp_strip.c): idle, sync and key toggles leave
// ZMK's underglow alone until it hands back, which re-applies the user's on/off. Any thread.
void rgb_idle_host_control(bool on);
// Right: the split link dropped; go by the right's own activity until the left is back.
void rgb_idle_remote_lost(void);
