/*
 * SET_FRAME (D7 0C, #272): per-LED frames from the PC for both halves. Protocol in host_link.h.
 * SPDX-License-Identifier: MIT
 */

#pragma once

#include <stdbool.h>
#include <stdint.h>

#define HOST_FRAMES_REPLY_SIZE 7

// Who has the lights (the reply's owner byte): the keyboard's own effect, the HID LampArray over
// USB or Bluetooth (Windows Dynamic Lighting, lamparray.c, #88), or the host link (Foxglove).
// One owner at a time; a LampArray wins over the host link (host_link.h, D7 0C).
enum {
    HOST_FRAMES_OWNER_NONE = 0,
    HOST_FRAMES_OWNER_LAMPARRAY_USB = 1,
    HOST_FRAMES_OWNER_LAMPARRAY_BLE = 2,
    HOST_FRAMES_OWNER_HOST_LINK = 3,
};
#define HOST_FRAMES_OWNER_IS_LAMPARRAY(o)                                                          \
    ((o) == HOST_FRAMES_OWNER_LAMPARRAY_USB || (o) == HOST_FRAMES_OWNER_LAMPARRAY_BLE)

// A D7 0C report: d is padded to the report size, len is its real length. Fills r with the D7 8C
// reply and returns whether it goes out (SHOW, RELEASE and errors reply; chunks don't).
bool host_frames_command(const uint8_t *d, int len, uint8_t r[HOST_FRAMES_REPLY_SIZE]);

// LAMP_PARTS (D7 10, #561): each part's LED count in frame order (the left's, then the right's)
// into out (room for max); returns how many parts.
int host_frames_parts(uint8_t *out, int max);

// SET_EFFECT takes the lights (host_effects.c): the frames hand both halves back, quietly (the PC
// asked for the effect, so no D7 8C event). Returns false, changing nothing, while a LampArray
// has them (the effect is refused, busy). Any thread.
bool host_frames_yield(void);

// The owner now (HOST_FRAMES_OWNER_*). Any thread.
uint8_t host_frames_owner(void);

// The lamps in frame order (the left's, then the right's): the LampArray's lamp count.
int host_frames_lamps(void);

// The HID LampArray over `who` (HOST_FRAMES_OWNER_LAMPARRAY_*), autonomous mode off (take) or on.
// Taking stops the host link's frames or effect, starts both halves from black and sends D7 8C
// with owner `who`, event 1; handing back (only the owner's) sends owner 0, event 1. Both events go
// out only after host traffic. Any thread.
void host_frames_lamparray(uint8_t who, bool take);
// One lamp of the LampArray's frame; false (nothing written) unless `who` owns the lights.
bool host_frames_lamparray_set(uint8_t who, int lamp, uint8_t r, uint8_t g, uint8_t b);
// Draws the LampArray's frame on both halves (the right at most 10 a second), if `who` owns it.
void host_frames_lamparray_show(uint8_t who);
// Since boot (saturating): LampArray take-overs, hand-backs (the host's or the board's), and the
// host link's SHOWs and effects refused while a LampArray had the lights.
void host_frames_lamparray_counts(uint16_t *takes, uint16_t *hand_backs, uint16_t *refused);
// SET_EFFECT refused busy (host_effects.c): counts with the refusals above.
void host_frames_lamparray_refused(void);
