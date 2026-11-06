/*
 * The art store (#834): takes pushed art sets, checks them, plays them from RAM and keeps them in
 * flash slots, whichever way the ops reach it.
 * SPDX-License-Identifier: MIT
 *
 * One core, two front ends:
 *   relayed  art_relay_peripheral.c: the far half of a split board (the Corne's right). The left
 *            relays the PC's D6 ops over the split link as the art relay's GATT writes (art_relay.h);
 *            that wire is unchanged by the split.
 *   local    art_local.c: a board whose art target is on the host-link half (a single-screen
 *            board, CONFIG_FOXGLOVE_ART_LOCAL). host_link.c hands the D6 ops straight to it: no
 *            BLE, and every reply is final when it goes out (host_link.h, HELLO v2 bit 32).
 *
 * The core keeps two RAM slots of ART_STORE_MAX_SIZE: one holds the pushed set on screen (if any),
 * the other takes the next push. The screen only changes once a blob has fully arrived and passed
 * its CRC and header checks (NVA1, art_relay.h; w and h must be the target's native size,
 * ART_STORE_W x ART_STORE_H), so a push that fails, stops halfway or never commits leaves whatever
 * is playing alone. Flash slots: art_slots.c.
 *
 * The board supplies the drawer (below; the Corne's right: peripheral_status.c) and its built-in
 * sets (custom_art.h: custom_art_sets, custom_art_set_count).
 *
 * Threads: the ops run on the front end's thread (the Bluetooth RX thread, or the USB thread),
 * one at a time; playing runs on the display queue, flash writes on art_slots.c's queue.
 */

#pragma once

#include <stdint.h>

#include "art_relay.h"

#if IS_ENABLED(CONFIG_NICE_VIEW_ART_RELAY)
#include "custom_art.h"
// The relayed store (the Corne's right): the relay's size, and the nice!view art area.
#define ART_STORE_MAX_SIZE CONFIG_NICE_VIEW_ART_RELAY_MAX_SIZE
#define ART_STORE_W CUSTOM_ART_W
#define ART_STORE_H CUSTOM_ART_H
#else
// The local store: the board's own (foxglove_link Kconfig.defconfig).
#define ART_STORE_MAX_SIZE CONFIG_FOXGLOVE_ART_MAX_SIZE
#define ART_STORE_W CONFIG_FOXGLOVE_ART_W
#define ART_STORE_H CONFIG_FOXGLOVE_ART_H
#endif

// NVA1 holds w and h as u8 (art_relay.h); a wider target needs NVA2 (#340 §2).
BUILD_ASSERT(ART_STORE_W <= 255 && ART_STORE_H <= 255, "NVA1 sizes are u8");

// The drawer, as peripheral_status.h has it: art_play runs on the display work queue; art_hold
// (freeze the current frame until the next art_play) and art_redraw_all from any thread;
// art_builtin_index is the built-in set to use for index (a random one when out of range).
struct custom_art_set;
void art_play(const struct custom_art_set *set);
void art_hold(void);
void art_redraw_all(void);
int art_builtin_index(int index);

// What an op returns besides 0: the relay answers them as ATT errors, the local front end as
// relay errors (art_local.c).
#define ART_STORE_BUSY 0x82    // a flash slot is being written or erased
#define ART_STORE_REFUSED 0x85 // nothing to save, no such slot, or the slot on screen

// BEGIN: the next len bytes, with their CRC, go into the RAM slot that isn't shown. A set over
// ART_STORE_MAX_SIZE ends in TOO_BIG.
int art_store_begin(uint16_t len, uint32_t crc);
// DATA: n bytes at offset at of the set BEGIN announced; ignored outside a receive or past its end.
void art_store_data(uint16_t at, const uint8_t *d, uint16_t n);
// COMMIT: checks the set (CRC, header) and plays it, or ends in BAD_CRC / BAD_BLOB.
void art_store_commit(void);
// SELECT: a built-in set (-1 random), or set_count + n for flash slot n.
int art_store_select(int8_t set);
// SAVE: the pushed set on screen into flash slot n. STORE (#522): the set received since BEGIN,
// checked, into slot n without showing it. ERASE: empties slot n. Each runs on art_slots.c's queue:
// SAVING, then SAVED / SAVE_FAILED (STORE: back to the shown set's state).
int art_store_save(uint8_t slot, const char name[16]);
int art_store_store(uint8_t slot, const char name[16]);
int art_store_erase(uint8_t slot);

// The store's state now: struct art_relay_status without its version (the front end's) and the
// underglow (rgb: the relay's). Any thread.
void art_store_status(struct art_relay_status *out);

/* The local front end (art_local.c), for host_link.c's D6 handler: the same calls and report as
 * the relay's left (art_relay.h: art_relay_begin ... art_relay_report), answered by the store on
 * this half. */

// The status version a local store reports: flash slots (3) and STORE (#522) are there.
#define ART_LOCAL_STATUS_VERSION ART_RELAY_STORE_STATUS_VERSION

int art_local_begin(uint16_t len, uint32_t crc);
int art_local_data(uint16_t offset, const uint8_t *data, uint8_t len);
int art_local_commit(void);
int art_local_store(uint8_t slot, const uint8_t name[16]);
int art_local_select(int8_t set);
int art_local_save(uint8_t slot, const uint8_t name[16]);
int art_local_erase(uint8_t slot);
// D6 07: sends D6 87 at once (status 0, or 4 for a slot the board doesn't have).
int art_local_slot(uint8_t slot);
void art_local_report(uint8_t out[ART_RELAY_REPORT_SIZE]);
