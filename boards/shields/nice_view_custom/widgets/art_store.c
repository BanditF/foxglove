/*
 * The art store's core (#834; art_store.h): receive, check, play and keep pushed art sets.
 * SPDX-License-Identifier: MIT
 *
 * Two slots: one holds the pushed set on screen (if any), the other takes the next push. The
 * screen only changes once a blob has fully arrived and passed its CRC and header checks, so a
 * push that fails, stops halfway or never commits leaves whatever is playing alone.
 */

#include <string.h>

#include <zephyr/kernel.h>
#include <zephyr/sys/byteorder.h>
#include <zephyr/sys/crc.h>

#include <zephyr/logging/log.h>
LOG_MODULE_DECLARE(zmk, CONFIG_ZMK_LOG_LEVEL);

#include <zmk/display.h>

#include "art_pace.h"
#include "art_store.h"
#include "art_slots.h"
#include "custom_art.h"
#include "health.h"

static uint8_t slots[2][ART_STORE_MAX_SIZE];
static int shown = -1; // slot playing, or -1 for a built-in set
static int rx;         // slot being received into; never the one shown
static uint16_t rx_len;
static uint16_t slot_len[2]; // blob length in each RAM slot, once committed
static int flash_shown = -1;  // flash slot playing, or -1
static uint32_t rx_crc;

// The front ends add the version and the underglow (art_store_status).
static struct art_relay_status status = {
    .capacity = ART_STORE_MAX_SIZE,
    .set = -1,
};

static const uint8_t *ram_deltas[2][256];
static struct custom_art_set ram_sets[2] = {
    {.name = "pushed", .deltas = ram_deltas[0]},
    {.name = "pushed", .deltas = ram_deltas[1]},
};
static const uint8_t *flash_deltas[256];
static struct custom_art_set flash_set = {.name = "saved", .deltas = flash_deltas};

// Fills set (and its deltas) from blob b; false, with set untouched, if it doesn't hold together.
// The set must be made for this target: NVA1's w and h are its native size.
static bool parse(const uint8_t *b, uint16_t slot_len, const uint8_t **deltas,
                  struct custom_art_set *set) {
    if (slot_len < ART_RELAY_HEADER_SIZE || memcmp(b, ART_RELAY_MAGIC, 4) != 0 ||
        b[4] != ART_STORE_W || b[5] != ART_STORE_H) {
        return false;
    }
    int unique = sys_get_le16(&b[8]);
    int frames = sys_get_le16(&b[10]);
    int deltas_at = ART_RELAY_HEADER_SIZE + frames + 2 * unique;
    if (unique < 1 || unique > ARRAY_SIZE(flash_deltas) || frames < 1 || deltas_at >= slot_len) {
        return false;
    }
    const uint8_t *order = &b[ART_RELAY_HEADER_SIZE];
    for (int i = 0; i < frames; i++) {
        if (order[i] >= unique) {
            return false;
        }
    }
    for (int i = 0; i < unique; i++) {
        int at = sys_get_le16(&b[ART_RELAY_HEADER_SIZE + frames + 2 * i]);
        if (at < deltas_at || at >= slot_len) {
            return false;
        }
    }
    for (int i = 0; i < unique; i++) { // checked above; the set may be on screen, so fill last
        deltas[i] = &b[sys_get_le16(&b[ART_RELAY_HEADER_SIZE + frames + 2 * i])];
    }
    set->order = order;
    set->frame_count = frames;
    set->fps = b[6];
    return true;
}

// Fills ram_sets[rx] from the blob in slots[rx].
static bool parse_blob(void) { return parse(slots[rx], rx_len, ram_deltas[rx], &ram_sets[rx]); }

// The built-in set to play when neither a kept nor a pushed set is shown, already resolved
// (art_builtin_index), and status.set with it before the op returns (#851).
static int8_t select_set;

static void play_cb(struct k_work *work) {
    if (flash_shown >= 0) {
        art_play(&flash_set);
    } else if (shown >= 0) {
        art_play(&ram_sets[shown]);
    } else {
        art_play(&custom_art_sets[art_builtin_index(select_set)]);
    }
    health_set(HEALTH_ART_SET, (uint8_t)status.set); // -1 (0xff): the pushed set (#705)
    // The art alone rewrites its rows; this also covers a panel that lost power since (#613).
    art_redraw_all();
}
static K_WORK_DEFINE(play_work, play_cb);

static void play(void) {
    if (zmk_display_is_initialized()) {
        k_work_submit_to_queue(zmk_display_work_q(), &play_work);
    }
}

// From art_slots.c's queue when a SAVE or ERASE has finished.
static void save_done(int err) { status.state = err ? ART_RIGHT_SAVE_FAILED : ART_RIGHT_SAVED; }

// STORE (#522): once written, the state goes back to what the screen shows (it never changed).
static uint8_t store_resume;
static void store_done(int err) { status.state = err ? ART_RIGHT_SAVE_FAILED : store_resume; }

static void failed(void) {
    health_inc(HEALTH_ART_FAILED);
    health_set(HEALTH_ART_LAST_ERROR, status.state);
}

// Whether the set received since BEGIN arrived whole and matches its CRC.
static bool received_ok(void) {
    return status.received == rx_len && crc32_ieee(slots[rx], rx_len) == rx_crc;
}

int art_store_begin(uint16_t len, uint32_t crc) {
    if (status.state == ART_RIGHT_SAVING) {
        return ART_STORE_BUSY; // the shown slot is being copied to flash
    }
    // The screen keeps playing; this only fills the slot that isn't shown.
    rx = shown == 0 ? 1 : 0;
    rx_len = len;
    rx_crc = crc;
    status.received = 0;
    status.state = rx_len <= sizeof(slots[0]) ? ART_RIGHT_RECEIVING : ART_RIGHT_TOO_BIG;
    if (status.state == ART_RIGHT_TOO_BIG) {
        failed();
    }
    return 0;
}

void art_store_data(uint16_t at, const uint8_t *d, uint16_t n) {
    if (status.state != ART_RIGHT_RECEIVING) {
        return;
    }
    if (at + n <= rx_len) {
        memcpy(&slots[rx][at], d, n);
        status.received += n;
    }
}

void art_store_commit(void) {
    if (status.state != ART_RIGHT_RECEIVING) {
        return;
    }
    if (!received_ok()) {
        status.state = ART_RIGHT_BAD_CRC;
        failed();
    } else if (!parse_blob()) {
        status.state = ART_RIGHT_BAD_BLOB;
        failed();
    } else {
        health_inc(HEALTH_ART_OK);
        // Freeze the old set until play_cb switches over, so the next push can take its slot.
        art_hold();
        shown = rx;
        slot_len[rx] = rx_len;
        flash_shown = -1;
        status.state = ART_RIGHT_PLAYING;
        status.set = -1;
        LOG_INF("art store: playing pushed set, %d frames", ram_sets[shown].frame_count);
        play();
    }
}

void art_store_booted(int set) {
    if (status.set == -1 && shown < 0 && flash_shown < 0 && set >= 0 &&
        set < custom_art_set_count) {
        select_set = (int8_t)set;
        status.set = (int8_t)set;
        health_set(HEALTH_ART_SET, (uint8_t)status.set);
    }
}

int art_store_select(int8_t set) {
    if (status.state == ART_RIGHT_SAVING) {
        return ART_STORE_BUSY;
    }
    if (set >= custom_art_set_count && set < custom_art_set_count + art_slots_count()) {
        uint16_t blob_len;
        const uint8_t *blob = art_slots_blob(set - custom_art_set_count, &blob_len);
        art_hold(); // flash_set may be on screen while parse fills it
        if (blob == NULL || !parse(blob, blob_len, flash_deltas, &flash_set)) {
            status.state = ART_RIGHT_BAD_BLOB;
            play(); // carry on with whatever was showing
            return 0;
        }
        flash_shown = set - custom_art_set_count;
        shown = -1;
        status.set = set;
        status.state = ART_RIGHT_IDLE;
        play();
        return 0;
    }
    art_hold(); // as for COMMIT: a push may reuse the shown slot before play_cb runs
    shown = -1;
    flash_shown = -1;
    // The status names the built-in now, not once the display queue gets to play_cb: a status
    // read right after the SELECT (the left's, art_relay_central.c) must see the new set, or the
    // next on-board step starts from the old one (#851).
    select_set = (int8_t)art_builtin_index(set);
    status.set = select_set;
    status.state = ART_RIGHT_IDLE;
    play();
    return 0;
}

int art_store_save(uint8_t slot, const char name[16]) {
    if (status.state == ART_RIGHT_SAVING) {
        return ART_STORE_BUSY;
    }
    if (shown < 0 || slot >= art_slots_count()) {
        return ART_STORE_REFUSED; // nothing pushed to save, or no such slot
    }
    status.state = ART_RIGHT_SAVING;
    if (art_slots_save(slot, slots[shown], slot_len[shown], name, save_done)) {
        status.state = ART_RIGHT_SAVE_FAILED;
    }
    return 0;
}

int art_store_store(uint8_t slot, const char name[16]) {
    // #522: the set received since BEGIN goes to flash without being shown (the playlist stages
    // its next piece; places a firmware update lost come back).
    if (status.state == ART_RIGHT_SAVING) {
        return ART_STORE_BUSY;
    }
    if (status.state != ART_RIGHT_RECEIVING || slot >= art_slots_count() || slot == flash_shown) {
        return ART_STORE_REFUSED; // nothing received, no such slot, or the slot on screen
    }
    if (!received_ok()) {
        status.state = ART_RIGHT_BAD_CRC;
        return 0;
    }
    if (!parse_blob()) { // rx is never the slot shown: the screen is untouched
        status.state = ART_RIGHT_BAD_BLOB;
        return 0;
    }
    store_resume = shown >= 0 ? ART_RIGHT_PLAYING : ART_RIGHT_IDLE;
    status.state = ART_RIGHT_SAVING; // BEGIN waits: slots[rx] stays put until store_done
    if (art_slots_save(slot, slots[rx], rx_len, name, store_done)) {
        status.state = ART_RIGHT_SAVE_FAILED;
    }
    return 0;
}

int art_store_erase(uint8_t slot) {
    if (status.state == ART_RIGHT_SAVING) {
        return ART_STORE_BUSY;
    }
    if (slot >= art_slots_count()) {
        return ART_STORE_REFUSED;
    }
    if (flash_shown == slot) { // don't play from pages about to be erased
        art_hold();
        flash_shown = -1;
        select_set = (int8_t)art_builtin_index(-1);
        status.set = select_set;
        play();
    }
    status.state = ART_RIGHT_SAVING;
    if (art_slots_erase(slot, save_done)) {
        status.state = ART_RIGHT_SAVE_FAILED;
    }
    return 0;
}

static void repace_cb(struct k_work *work) { art_repace(); }
static K_WORK_DEFINE(repace_work, repace_cb);

void art_store_animate(bool on) {
    // Only a change re-times the set: a repeat (each connect sends it) does nothing.
    if (art_pace_set_animate(on) && zmk_display_is_initialized()) {
        k_work_submit_to_queue(zmk_display_work_q(), &repace_work);
    }
}

void art_store_status(struct art_relay_status *out) {
    *out = status;
    out->version = 0;
    memset(&out->rgb, 0, sizeof(out->rgb));
    out->set_count = custom_art_set_count;
    out->flash_slots = art_slots_count();
    out->flash_used = art_slots_used();
}
