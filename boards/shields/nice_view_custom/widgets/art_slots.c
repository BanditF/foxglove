/*
 * Right half: art sets saved in flash. Layout and wire in art_relay.h; design in #121.
 * SPDX-License-Identifier: MIT
 *
 * Each slot is SLOT_SIZE bytes (whole erase pages) in art_partition: a 32-byte header, then the
 * blob. The header goes in last, so a write that stops halfway leaves an empty slot, never a
 * broken one. Flash only changes on an explicit SAVE or ERASE from the PC, on a work queue of
 * its own so keyscan and the split link keep going while pages erase.
 */

#include <string.h>

#include <zephyr/kernel.h>
#include <zephyr/storage/flash_map.h>
#include <zephyr/sys/byteorder.h>
#include <zephyr/sys/crc.h>

#include <zephyr/logging/log.h>
LOG_MODULE_DECLARE(zmk, CONFIG_ZMK_LOG_LEVEL);

#include "art_slots.h"

#define ART_NODE DT_NODELABEL(art_partition)

#if DT_NODE_EXISTS(ART_NODE)

#define SLOT_SIZE (20 * 1024)
#define SLOT_HEADER 32
#define SLOT_MAGIC "NVS1"
#define SLOTS MIN(ART_RELAY_FLASH_SLOTS, DT_REG_SIZE(ART_NODE) / SLOT_SIZE)
#define FLASH_AREA FIXED_PARTITION_ID(art_partition)

BUILD_ASSERT(SLOT_HEADER + CONFIG_NICE_VIEW_ART_RELAY_MAX_SIZE <= SLOT_SIZE, "slot too small");

// Header: magic[4], version u8, 0, len u16, crc32 u32, writes u32, name[16]
struct slot_header {
    char magic[4];
    uint8_t version;
    uint8_t reserved;
    uint16_t len;
    uint32_t crc;
    uint32_t writes;
    char name[16];
} __packed;
BUILD_ASSERT(sizeof(struct slot_header) == SLOT_HEADER, "header layout");

static const struct slot_header *header(int slot) {
    return (const struct slot_header *)(CONFIG_FLASH_BASE_ADDRESS +
                                        FIXED_PARTITION_OFFSET(art_partition) + slot * SLOT_SIZE);
}

static bool valid(int slot) {
    const struct slot_header *h = header(slot);
    if (memcmp(h->magic, SLOT_MAGIC, 4) != 0 || h->version != 1 || h->len == 0 ||
        h->len > CONFIG_NICE_VIEW_ART_RELAY_MAX_SIZE) {
        return false;
    }
    return crc32_ieee((const uint8_t *)h + SLOT_HEADER, h->len) == h->crc;
}

int art_slots_count(void) { return SLOTS; }

uint8_t art_slots_used(void) {
    uint8_t used = 0;
    for (int i = 0; i < SLOTS; i++) {
        if (memcmp(header(i)->magic, SLOT_MAGIC, 4) == 0) { // cheap; SELECT checks the CRC
            used |= BIT(i);
        }
    }
    return used;
}

void art_slots_info(int slot, struct art_relay_slot_info *out) {
    memset(out, 0, sizeof(*out));
    if (slot < 0 || slot >= SLOTS) {
        return;
    }
    const struct slot_header *h = header(slot);
    bool used = memcmp(h->magic, SLOT_MAGIC, 4) == 0;
    out->used = used;
    out->writes = h->writes == UINT32_MAX ? 0 : h->writes; // erased flash reads all ones
    if (used) {
        out->len = h->len;
        memcpy(out->name, h->name, sizeof(out->name));
    }
}

const uint8_t *art_slots_blob(int slot, uint16_t *len) {
    if (slot < 0 || slot >= SLOTS || !valid(slot)) {
        return NULL;
    }
    *len = header(slot)->len;
    return (const uint8_t *)header(slot) + SLOT_HEADER;
}

/* Writes, on their own queue */

K_THREAD_STACK_DEFINE(slot_stack, 1024);
static struct k_work_q slot_q;
static atomic_t slot_q_started;
static atomic_t busy;

static int job_slot;
static const uint8_t *job_blob; // NULL = erase only
static uint16_t job_len;
static char job_name[16];
static void (*job_done)(int err);

static int write_slot(void) {
    const struct flash_area *fa;
    int err = flash_area_open(FLASH_AREA, &fa);
    if (err) {
        return err;
    }
    off_t at = job_slot * SLOT_SIZE;
    uint32_t writes = header(job_slot)->writes;
    writes = writes == UINT32_MAX ? 1 : writes + 1; // kept across saves, so the app can show wear
    err = flash_area_erase(fa, at, SLOT_SIZE);
    if (!err && job_blob != NULL) {
        // nRF52 writes whole words; the RAM slot is large enough to round up into.
        err = flash_area_write(fa, at + SLOT_HEADER, job_blob, ROUND_UP(job_len, 4));
    }
    if (!err) {
        struct slot_header h = {.magic = SLOT_MAGIC, .version = 1, .writes = writes};
        if (job_blob != NULL) {
            h.len = job_len;
            h.crc = crc32_ieee(job_blob, job_len);
            memcpy(h.name, job_name, sizeof(h.name));
        } else {
            // An erased slot keeps only its write count: magic stays blank, so it reads as empty.
            memset(h.magic, 0xff, sizeof(h.magic));
            h.version = 0xff;
        }
        err = flash_area_write(fa, at, &h, sizeof(h));
    }
    flash_area_close(fa);
    return err;
}

static void job_cb(struct k_work *work) {
    int err = write_slot();
    if (err) {
        LOG_ERR("art slots: slot %d write failed (%d)", job_slot, err);
    }
    void (*done)(int) = job_done;
    atomic_clear(&busy);
    if (done) {
        done(err);
    }
}
static K_WORK_DEFINE(job_work, job_cb);

static int start(int slot, const uint8_t *blob, uint16_t len, const char *name,
                 void (*done)(int err)) {
    if (slot < 0 || slot >= SLOTS || (blob != NULL && (len == 0 ||
                                                         len > CONFIG_NICE_VIEW_ART_RELAY_MAX_SIZE))) {
        return -EINVAL;
    }
    if (!atomic_cas(&busy, 0, 1)) {
        return -EBUSY;
    }
    if (atomic_cas(&slot_q_started, 0, 1)) { // first write since boot; nothing runs before that
        k_work_queue_start(&slot_q, slot_stack, K_THREAD_STACK_SIZEOF(slot_stack),
                           K_LOWEST_APPLICATION_THREAD_PRIO, NULL);
    }
    job_slot = slot;
    job_blob = blob;
    job_len = len;
    memset(job_name, 0, sizeof(job_name));
    if (name != NULL) {
        memcpy(job_name, name, sizeof(job_name));
    }
    job_done = done;
    k_work_submit_to_queue(&slot_q, &job_work);
    return 0;
}

int art_slots_save(int slot, const uint8_t *blob, uint16_t len, const char name[16],
                   void (*done)(int err)) {
    return blob == NULL ? -EINVAL : start(slot, blob, len, name, done);
}

int art_slots_erase(int slot, void (*done)(int err)) { return start(slot, NULL, 0, NULL, done); }

#else // no art_partition

int art_slots_count(void) { return 0; }
uint8_t art_slots_used(void) { return 0; }
void art_slots_info(int slot, struct art_relay_slot_info *out) { memset(out, 0, sizeof(*out)); }
const uint8_t *art_slots_blob(int slot, uint16_t *len) { return NULL; }
int art_slots_save(int slot, const uint8_t *blob, uint16_t len, const char name[16],
                   void (*done)(int err)) {
    return -ENOTSUP;
}
int art_slots_erase(int slot, void (*done)(int err)) { return -ENOTSUP; }

#endif
