/*
 * Right-hand art pushed from the PC: PC -> Raw HID -> left -> BLE split link -> right.
 * SPDX-License-Identifier: MIT
 *
 * The right half adds its own GATT service to the split connection:
 *   data   (write, write without response)  op byte + payload, below
 *   status (read)                            struct art_relay_status
 * Ops: BEGIN len u16, crc32 u32 | DATA offset u16, bytes | COMMIT | SELECT int8 set
 *
 * Blob, all little endian (scripts/flipper_art.py writes it for OUT.bin):
 *   "NVA1", w u8 (140), h u8 (68), fps u8, 0, unique u16, frames u16
 *   order u8[frames]       play order, as indices into the deltas
 *   offset u16[unique]     where each delta starts, from the start of the blob
 *   deltas                 RLE-packed XOR deltas, as in custom_art.c
 */

#pragma once

#include <stdint.h>

#include <zephyr/bluetooth/uuid.h>
#include <zephyr/toolchain.h>

#define ART_RELAY_SERVICE_UUID BT_UUID_128_ENCODE(0x6e766172, 0x7400, 0x4c6b, 0x9a10, 0x5d1e0bfa7a00)
#define ART_RELAY_DATA_UUID BT_UUID_128_ENCODE(0x6e766172, 0x7401, 0x4c6b, 0x9a10, 0x5d1e0bfa7a00)
#define ART_RELAY_STATUS_UUID BT_UUID_128_ENCODE(0x6e766172, 0x7402, 0x4c6b, 0x9a10, 0x5d1e0bfa7a00)

enum art_relay_op {
    ART_RELAY_BEGIN = 1,
    ART_RELAY_DATA,
    ART_RELAY_COMMIT,
    ART_RELAY_SELECT,
};

#define ART_RELAY_HEADER_SIZE 12
#define ART_RELAY_MAGIC "NVA1"

enum art_relay_right_state {
    ART_RIGHT_IDLE,      // showing a built-in set
    ART_RIGHT_RECEIVING,
    ART_RIGHT_PLAYING,   // showing the pushed set
    ART_RIGHT_BAD_CRC,
    ART_RIGHT_BAD_BLOB,
    ART_RIGHT_TOO_BIG,
};

struct art_relay_status {
    uint8_t version;
    uint8_t state;     // enum art_relay_right_state
    uint16_t received; // bytes of the current blob received so far
    uint16_t capacity; // CONFIG_NICE_VIEW_ART_RELAY_MAX_SIZE
    int8_t set;        // built-in set shown, or -1 for the pushed one
    uint8_t set_count; // built-in sets
} __packed;

// Left half (art_relay_central.c). The blob is collected on the left, then pushed in one go.
enum art_relay_left_state {
    ART_LEFT_IDLE,
    ART_LEFT_RECEIVING, // from the PC
    ART_LEFT_PUSHING,   // to the right half
    ART_LEFT_DONE,
    ART_LEFT_FAILED,
};

enum art_relay_error {
    ART_ERR_NONE,
    ART_ERR_NO_LINK,   // right half not connected, or its firmware has no relay service
    ART_ERR_TOO_BIG,
    ART_ERR_BUSY,      // a push is still running
    ART_ERR_RANGE,     // DATA outside the announced length
    ART_ERR_BAD_CRC,   // what the PC sent doesn't match its CRC
    ART_ERR_BLE,       // a write to the right half failed
    ART_ERR_RIGHT,     // the right half rejected the blob; see its status
};

int art_relay_begin(uint16_t len, uint32_t crc);
int art_relay_data(uint16_t offset, const uint8_t *data, uint8_t len);
int art_relay_commit(void);
int art_relay_select(int8_t set);
// 0 = right half not connected, 1 = connected, 2 = connected and has the relay service.
uint8_t art_relay_link(void);
// Fills out with: state, error, length u16, pushed u16, link, struct art_relay_status.
#define ART_RELAY_REPORT_SIZE (7 + sizeof(struct art_relay_status))
void art_relay_report(uint8_t out[ART_RELAY_REPORT_SIZE]);
