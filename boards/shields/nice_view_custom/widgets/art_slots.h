/*
 * The art store's flash slots (art_store.h; art_partition, 0xAD000 on the Corne's right), so pushed
 * art survives a reboot. A slot is the board's largest blob plus a header, in whole erase pages.
 * SPDX-License-Identifier: MIT
 */

#pragma once

#include <stdbool.h>
#include <stdint.h>

#include "art_relay.h"

// Slots there are (0 if the build has no art_partition).
int art_slots_count(void);
// Bit n set: slot n holds a valid set.
uint8_t art_slots_used(void);
void art_slots_info(int slot, struct art_relay_slot_info *out);
// The slot's blob in (memory-mapped) flash, or NULL if the slot is empty or doesn't check out.
const uint8_t *art_slots_blob(int slot, uint16_t *len);

// Erase and write happen on a work queue of their own; done(err) runs there when finished.
// Both return -EBUSY while another one is running. blob must stay put until done.
int art_slots_save(int slot, const uint8_t *blob, uint16_t len, const char name[16],
                   void (*done)(int err));
int art_slots_erase(int slot, void (*done)(int err));
