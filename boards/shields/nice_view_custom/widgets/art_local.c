/*
 * The art store's local front end (#834, art_store.h): a board whose art target is on the
 * host-link half (a single-screen board, CONFIG_FOXGLOVE_ART_LOCAL). host_link.c hands the PC's D6
 * ops here, on the USB thread, and they go straight to the store: no relay, no BLE.
 * SPDX-License-Identifier: MIT
 *
 * The replies keep the relay's layout (host_link.h, art_relay_central.c's art_relay_report), so an
 * app that only knows the relay reads them the same:
 * - link is always 2 (ready): the store is on this half;
 * - the "left" state is final when the reply goes out: DONE or FAILED, never PUSHING, because the
 *   store has already taken COMMIT, STORE, SELECT, SAVE or ERASE. SAVE, ERASE and STORE then run
 *   on the flash queue: STATUS shows the store's SAVING, then SAVED / SAVE_FAILED, as on the relay;
 * - the errors are the relay's: TOO_BIG, RANGE, BAD_CRC (the set didn't arrive whole, or doesn't
 *   match its CRC), RIGHT (the store refused the set: bad blob, the wrong size for this target),
 *   TIMEOUT (the PC stopped mid-set), and BLE (6) for an op the store refused (busy saving, no such
 *   slot, nothing to save), where the relay's write to the right would have failed;
 * - the status version is ART_LOCAL_STATUS_VERSION: slots, STORE.
 * D6 87 SLOT answers at once, with the slot's CRC (#863).
 *
 * Start/stop of the one timer: a receive that sees no BEGIN or DATA for
 * ART_RELAY_RECEIVE_TIMEOUT drops its partial set (as the relay's left does). Starts on BEGIN and
 * each DATA; stops on COMMIT, STORE, a range error, or a new BEGIN.
 */

#include <string.h>

#include <zephyr/kernel.h>
#include <zephyr/sys/byteorder.h>

#include <zephyr/logging/log.h>
LOG_MODULE_DECLARE(zmk, CONFIG_ZMK_LOG_LEVEL);

#include "art_slots.h"
#include "art_store.h"
#include "host_link.h"
#if IS_ENABLED(CONFIG_NICE_VIEW_RGB_IDLE)
#include "rgb_idle.h"
#endif

static uint8_t state = ART_LEFT_IDLE; // enum art_relay_left_state
static uint8_t error;                 // enum art_relay_error
static uint16_t blob_len, pushed;

// The PC went quiet mid-set (app killed, cable pulled): drop what came so far.
static void receive_timeout_cb(struct k_work *work) {
    if (state == ART_LEFT_RECEIVING) {
        LOG_WRN("art store: no data from the PC for a while, dropping the set");
        blob_len = 0;
        error = ART_ERR_TIMEOUT;
        state = ART_LEFT_IDLE;
    }
}
static K_WORK_DELAYABLE_DEFINE(receive_timeout, receive_timeout_cb);

static int fail(uint8_t err) {
    state = ART_LEFT_FAILED;
    error = err;
    return -EIO;
}

static uint8_t store_state(void) {
    struct art_relay_status st;
    art_store_status(&st);
    return st.state;
}

// A set the store took (COMMIT, STORE): done when it ended in one of ok_a / ok_b / ok_c.
static int took(uint8_t ok_a, uint8_t ok_b, uint8_t ok_c) {
    uint8_t s = store_state();
    if (s == ok_a || s == ok_b || s == ok_c) {
        pushed = blob_len;
        state = ART_LEFT_DONE;
        return 0;
    }
    return fail(s == ART_RIGHT_BAD_CRC ? ART_ERR_BAD_CRC : ART_ERR_RIGHT);
}

// An op the store either takes at once or refuses (SELECT, SAVE, ERASE).
static int op(int refused) {
    k_work_cancel_delayable(&receive_timeout);
    pushed = blob_len = 0;
    error = ART_ERR_NONE;
    if (refused) {
        return fail(ART_ERR_BLE);
    }
    state = ART_LEFT_DONE;
    return 0;
}

int art_local_begin(uint16_t len, uint32_t crc) {
    k_work_cancel_delayable(&receive_timeout); // a BEGIN also replaces a stuck receive
    pushed = 0;
    blob_len = len;
    error = ART_ERR_NONE;
    if (art_store_begin(len, crc)) {
        return fail(ART_ERR_BUSY); // a flash write runs
    }
    if (store_state() == ART_RIGHT_TOO_BIG) {
        return fail(ART_ERR_TOO_BIG);
    }
    state = ART_LEFT_RECEIVING;
    k_work_reschedule(&receive_timeout, ART_RELAY_RECEIVE_TIMEOUT);
    return 0;
}

int art_local_data(uint16_t offset, const uint8_t *data, uint8_t len) {
    if (state != ART_LEFT_RECEIVING) {
        return -EINVAL;
    }
    if (offset + len > blob_len) {
        k_work_cancel_delayable(&receive_timeout);
        return fail(ART_ERR_RANGE);
    }
    art_store_data(offset, data, len);
    k_work_reschedule(&receive_timeout, ART_RELAY_RECEIVE_TIMEOUT);
    return 0;
}

int art_local_commit(void) {
    if (state != ART_LEFT_RECEIVING) {
        return -EINVAL;
    }
    k_work_cancel_delayable(&receive_timeout);
    art_store_commit();
    return took(ART_RIGHT_PLAYING, ART_RIGHT_PLAYING, ART_RIGHT_PLAYING);
}

int art_local_store(uint8_t slot, const uint8_t name[16]) {
    if (state != ART_LEFT_RECEIVING) {
        return -EINVAL;
    }
    k_work_cancel_delayable(&receive_timeout);
    if (art_store_store(slot, (const char *)name)) {
        return fail(ART_ERR_BLE);
    }
    // Writing (SAVING), or already back to what the screen shows.
    return took(ART_RIGHT_SAVING, ART_RIGHT_IDLE, ART_RIGHT_PLAYING);
}

int art_local_select(int8_t set) { return op(art_store_select(set)); }

int art_local_save(uint8_t slot, const uint8_t name[16]) {
    return op(art_store_save(slot, (const char *)name));
}

int art_local_erase(uint8_t slot) { return op(art_store_erase(slot)); }

// A setting, not an op on a set: a receive in progress and the last op's state stay as they are.
int art_local_animate(uint8_t flags) {
    art_store_animate(flags & HOST_LINK_ART_ANIMATE_EPAPER);
    return 0;
}

BUILD_ASSERT(4 + sizeof(struct art_relay_slot_info) == HOST_LINK_SLOT_CRC_AT, "host_link.h");

int art_local_slot(uint8_t slot) {
    struct art_relay_slot_info info;
    uint8_t r[HOST_LINK_SLOT_CRC_AT + 5] = {HOST_LINK_ART, 0x87, 4, slot}; // 4: no such slot
    if (slot < art_slots_count()) {
        art_slots_info(slot, &info);
        r[2] = 0;
        memcpy(&r[4], &info, sizeof(info));
        r[HOST_LINK_SLOT_CRC_AT] = HOST_LINK_SLOT_CRC_V1; // the store is here: its CRC at once
        sys_put_le32(art_slots_crc(slot), &r[HOST_LINK_SLOT_CRC_AT + 1]);
    }
    host_link_send(r, sizeof(r));
    return 0;
}

// D6 0B SLOT_READ (#878): straight from the slot in flash. The slot's CRC is checked once
// (art_slots_blob) and the set kept at hand while the slot's header CRC stays the same, so a
// read of a whole set doesn't check it once per reply.
int art_local_slot_read(uint8_t slot, uint16_t offset) {
    static const uint8_t *blob;
    static uint16_t blob_n;
    static uint8_t blob_slot = 0xff;
    static uint32_t blob_crc;
    uint8_t r[7 + HOST_LINK_SLOT_READ_MAX] = {HOST_LINK_ART, 0x8B, 0, slot};
    sys_put_le16(offset, &r[4]);
    if (art_slots_count() == 0) {
        r[2] = 4; // no slots on this board
    } else if (store_state() == ART_RIGHT_SAVING) {
        r[2] = 1; // a slot is being written
        blob_slot = 0xff;
    } else {
        uint32_t crc = art_slots_crc(slot);
        if (slot != blob_slot || crc != blob_crc || blob == NULL) {
            blob = slot < art_slots_count() ? art_slots_blob(slot, &blob_n) : NULL;
            blob_slot = blob != NULL ? slot : 0xff;
            blob_crc = crc;
        }
        if (blob == NULL || offset > blob_n) {
            r[2] = HOST_LINK_SLOT_READ_BAD; // no such slot, an empty one, or past its end
        } else {
            r[6] = MIN(HOST_LINK_SLOT_READ_MAX, blob_n - offset);
            memcpy(&r[7], &blob[offset], r[6]);
        }
    }
    host_link_send(r, sizeof(r));
    return 0;
}

void art_local_report(uint8_t out[ART_RELAY_REPORT_SIZE]) {
    struct art_relay_status st;
    art_store_status(&st);
    st.version = ART_LOCAL_STATUS_VERSION;
#if IS_ENABLED(CONFIG_NICE_VIEW_RGB_IDLE)
    rgb_idle_state(&st.rgb);
#endif
    out[0] = state;
    out[1] = error;
    sys_put_le16(blob_len, &out[2]);
    sys_put_le16(pushed, &out[4]);
    out[6] = 2; // the store is on this half: always ready
    memcpy(&out[7], &st, sizeof(st));
}
