/*
 * Right half: takes an art set pushed over the split link (art_relay.h) and plays it from RAM.
 * SPDX-License-Identifier: MIT
 */

#include <string.h>

#include <zephyr/kernel.h>
#include <zephyr/bluetooth/gatt.h>
#include <zephyr/bluetooth/uuid.h>
#include <zephyr/sys/byteorder.h>
#include <zephyr/sys/crc.h>

#include <zephyr/logging/log.h>
LOG_MODULE_DECLARE(zmk, CONFIG_ZMK_LOG_LEVEL);

#include <zmk/display.h>

#include "art_relay.h"
#include "custom_art.h"
#include "peripheral_status.h"

static uint8_t slot[CONFIG_NICE_VIEW_ART_RELAY_MAX_SIZE];
static uint16_t slot_len;
static uint32_t slot_crc;

static struct art_relay_status status = {
    .version = 1,
    .capacity = CONFIG_NICE_VIEW_ART_RELAY_MAX_SIZE,
    .set = -1,
};

static const uint8_t *ram_deltas[256];
static struct custom_art_set ram_set = {.name = "pushed", .deltas = ram_deltas};

// Fills ram_set from the blob in slot; false if it doesn't hold together.
static bool parse_blob(void) {
    const uint8_t *b = slot;
    if (slot_len < ART_RELAY_HEADER_SIZE || memcmp(b, ART_RELAY_MAGIC, 4) != 0 ||
        b[4] != CUSTOM_ART_W || b[5] != CUSTOM_ART_H) {
        return false;
    }
    int unique = sys_get_le16(&b[8]);
    int frames = sys_get_le16(&b[10]);
    int deltas_at = ART_RELAY_HEADER_SIZE + frames + 2 * unique;
    if (unique < 1 || unique > ARRAY_SIZE(ram_deltas) || frames < 1 || deltas_at >= slot_len) {
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
        ram_deltas[i] = &b[at];
    }
    ram_set.order = order;
    ram_set.frame_count = frames;
    ram_set.fps = b[6];
    return true;
}

static int8_t select_set;

static void play_cb(struct k_work *work) {
    if (status.state == ART_RIGHT_PLAYING) {
        art_play(&ram_set);
    } else {
        status.set = art_builtin_index(select_set);
        art_play(&custom_art_sets[status.set]);
    }
}
static K_WORK_DEFINE(play_work, play_cb);

static void play(void) {
    if (zmk_display_is_initialized()) {
        k_work_submit_to_queue(zmk_display_work_q(), &play_work);
    }
}

// Runs in the Bluetooth RX thread; keep it to copying and checking.
static ssize_t data_write(struct bt_conn *conn, const struct bt_gatt_attr *attr, const void *buf,
                          uint16_t len, uint16_t offset, uint8_t flags) {
    const uint8_t *d = buf;
    if (offset != 0 || len < 1) {
        return BT_GATT_ERR(BT_ATT_ERR_INVALID_OFFSET);
    }

    switch (d[0]) {
    case ART_RELAY_BEGIN:
        if (len < 7) {
            return BT_GATT_ERR(BT_ATT_ERR_INVALID_ATTRIBUTE_LEN);
        }
        if (status.state == ART_RIGHT_PLAYING) {
            art_hold(); // the slot is about to change under the playing set
        }
        slot_len = sys_get_le16(&d[1]);
        slot_crc = sys_get_le32(&d[3]);
        status.received = 0;
        status.state = slot_len <= sizeof(slot) ? ART_RIGHT_RECEIVING : ART_RIGHT_TOO_BIG;
        break;

    case ART_RELAY_DATA: {
        if (status.state != ART_RIGHT_RECEIVING || len < 3) {
            break;
        }
        uint16_t at = sys_get_le16(&d[1]);
        uint16_t n = len - 3;
        if (at + n <= slot_len) {
            memcpy(&slot[at], &d[3], n);
            status.received += n;
        }
        break;
    }

    case ART_RELAY_COMMIT:
        if (status.state != ART_RIGHT_RECEIVING) {
            break;
        }
        if (status.received != slot_len || crc32_ieee(slot, slot_len) != slot_crc) {
            status.state = ART_RIGHT_BAD_CRC;
        } else if (!parse_blob()) {
            status.state = ART_RIGHT_BAD_BLOB;
        } else {
            status.state = ART_RIGHT_PLAYING;
            status.set = -1;
            LOG_INF("art relay: playing pushed set, %d frames", ram_set.frame_count);
            play();
        }
        break;

    case ART_RELAY_SELECT:
        if (len < 2) {
            return BT_GATT_ERR(BT_ATT_ERR_INVALID_ATTRIBUTE_LEN);
        }
        select_set = (int8_t)d[1];
        status.state = ART_RIGHT_IDLE;
        play();
        break;
    }
    return len;
}

static ssize_t status_read(struct bt_conn *conn, const struct bt_gatt_attr *attr, void *buf,
                           uint16_t len, uint16_t offset) {
    status.set_count = custom_art_set_count;
    return bt_gatt_attr_read(conn, attr, buf, len, offset, &status, sizeof(status));
}

BT_GATT_SERVICE_DEFINE(
    art_relay_svc, BT_GATT_PRIMARY_SERVICE(BT_UUID_DECLARE_128(ART_RELAY_SERVICE_UUID)),
    BT_GATT_CHARACTERISTIC(BT_UUID_DECLARE_128(ART_RELAY_DATA_UUID),
                           BT_GATT_CHRC_WRITE | BT_GATT_CHRC_WRITE_WITHOUT_RESP,
                           BT_GATT_PERM_WRITE_ENCRYPT, NULL, data_write, NULL),
    BT_GATT_CHARACTERISTIC(BT_UUID_DECLARE_128(ART_RELAY_STATUS_UUID), BT_GATT_CHRC_READ,
                           BT_GATT_PERM_READ_ENCRYPT, status_read, NULL, NULL), );
