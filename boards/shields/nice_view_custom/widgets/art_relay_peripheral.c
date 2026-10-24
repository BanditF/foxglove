/*
 * Right half: takes an art set pushed over the split link (art_relay.h) and plays it from RAM.
 * SPDX-License-Identifier: MIT
 *
 * Two slots: one holds the pushed set on screen (if any), the other takes the next push. The
 * screen only changes once a blob has fully arrived and passed its CRC and header checks, so a
 * push that fails, stops halfway or never commits leaves whatever is playing alone.
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
#if IS_ENABLED(CONFIG_NICE_VIEW_RGB_IDLE)
#include <zephyr/bluetooth/conn.h>

#include "rgb_idle.h"
#endif

#include "art_relay.h"
#include "screen_snap.h"
#if IS_ENABLED(CONFIG_FOXGLOVE_TEST_HOOKS)
#include "test_hooks.h"
#endif
#include "custom_art.h"
#include "peripheral_status.h"

static uint8_t slots[2][CONFIG_NICE_VIEW_ART_RELAY_MAX_SIZE];
static int shown = -1; // slot playing, or -1 for a built-in set
static int rx;         // slot being received into; never the one shown
static uint16_t rx_len;
static uint32_t rx_crc;

static struct art_relay_status status = {
    .version = 2,
    .capacity = CONFIG_NICE_VIEW_ART_RELAY_MAX_SIZE,
    .set = -1,
};

static const uint8_t *ram_deltas[2][256];
static struct custom_art_set ram_sets[2] = {
    {.name = "pushed", .deltas = ram_deltas[0]},
    {.name = "pushed", .deltas = ram_deltas[1]},
};

// Fills ram_sets[rx] from the blob in slots[rx]; false if it doesn't hold together.
static bool parse_blob(void) {
    const uint8_t *b = slots[rx];
    uint16_t slot_len = rx_len;
    struct custom_art_set *ram_set = &ram_sets[rx];
    if (slot_len < ART_RELAY_HEADER_SIZE || memcmp(b, ART_RELAY_MAGIC, 4) != 0 ||
        b[4] != CUSTOM_ART_W || b[5] != CUSTOM_ART_H) {
        return false;
    }
    int unique = sys_get_le16(&b[8]);
    int frames = sys_get_le16(&b[10]);
    int deltas_at = ART_RELAY_HEADER_SIZE + frames + 2 * unique;
    if (unique < 1 || unique > ARRAY_SIZE(ram_deltas[0]) || frames < 1 || deltas_at >= slot_len) {
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
        ram_deltas[rx][i] = &b[at];
    }
    ram_set->order = order;
    ram_set->frame_count = frames;
    ram_set->fps = b[6];
    return true;
}

static int8_t select_set;

static void play_cb(struct k_work *work) {
    if (shown >= 0) {
        art_play(&ram_sets[shown]);
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

#if IS_ENABLED(CONFIG_NICE_VIEW_RGB_IDLE)
static void rgb_disconnected(struct bt_conn *conn, uint8_t reason) { rgb_idle_remote_lost(); }

BT_CONN_CB_DEFINE(art_relay_rgb_conn_cb) = {
    .disconnected = rgb_disconnected,
};
#endif

// Runs in the Bluetooth RX thread; keep it to copying and checking.
static ssize_t data_write(struct bt_conn *conn, const struct bt_gatt_attr *attr, const void *buf,
                          uint16_t len, uint16_t offset, uint8_t flags) {
    const uint8_t *d = buf;
    if (offset != 0 || len < 1) {
        return BT_GATT_ERR(BT_ATT_ERR_INVALID_OFFSET);
    }

    switch (d[0]) {
    case ART_RELAY_BOOT:
#if IS_ENABLED(CONFIG_FOXGLOVE_TEST_HOOKS)
        if (len != 5 || memcmp(&d[1], "BOOT", 4) != 0) {
            return BT_GATT_ERR(0x85);
        }
        test_hooks_reboot_soon(); // after this write's response has gone out
        break;
#else
        return BT_GATT_ERR(0x84);
#endif
    case ART_RELAY_PRESS:
#if IS_ENABLED(CONFIG_FOXGLOVE_TEST_HOOKS)
        if (len != 3) {
            return BT_GATT_ERR(0x85);
        }
        uint8_t result = test_hooks_press(d[1], d[2]);
        if (result) {
            return BT_GATT_ERR(0x80 + result);
        }
        break;
#else
        return BT_GATT_ERR(0x84);
#endif
    case ART_RELAY_BEGIN:
        if (len < 7) {
            return BT_GATT_ERR(BT_ATT_ERR_INVALID_ATTRIBUTE_LEN);
        }
        // The screen keeps playing; this only fills the slot that isn't shown.
        rx = shown == 0 ? 1 : 0;
        rx_len = sys_get_le16(&d[1]);
        rx_crc = sys_get_le32(&d[3]);
        status.received = 0;
        status.state = rx_len <= sizeof(slots[0]) ? ART_RIGHT_RECEIVING : ART_RIGHT_TOO_BIG;
        break;

    case ART_RELAY_DATA: {
        if (status.state != ART_RIGHT_RECEIVING || len < 3) {
            break;
        }
        uint16_t at = sys_get_le16(&d[1]);
        uint16_t n = len - 3;
        if (at + n <= rx_len) {
            memcpy(&slots[rx][at], &d[3], n);
            status.received += n;
        }
        break;
    }

    case ART_RELAY_COMMIT:
        if (status.state != ART_RIGHT_RECEIVING) {
            break;
        }
        if (status.received != rx_len || crc32_ieee(slots[rx], rx_len) != rx_crc) {
            status.state = ART_RIGHT_BAD_CRC;
        } else if (!parse_blob()) {
            status.state = ART_RIGHT_BAD_BLOB;
        } else {
            // Freeze the old set until play_cb switches over, so the next push can take its slot.
            art_hold();
            shown = rx;
            status.state = ART_RIGHT_PLAYING;
            status.set = -1;
            LOG_INF("art relay: playing pushed set, %d frames", ram_sets[shown].frame_count);
            play();
        }
        break;

    case ART_RELAY_SELECT:
        if (len < 2) {
            return BT_GATT_ERR(BT_ATT_ERR_INVALID_ATTRIBUTE_LEN);
        }
        select_set = (int8_t)d[1];
        art_hold(); // as for COMMIT: a push may reuse the shown slot before play_cb runs
        shown = -1;
        status.state = ART_RIGHT_IDLE;
        play();
        break;

    case ART_RELAY_SNAP:
        if (len != 1) {
            return BT_GATT_ERR(BT_ATT_ERR_INVALID_ATTRIBUTE_LEN);
        }
        if (screen_snap_local() != SCREEN_OK) {
            return BT_GATT_ERR(BT_ATT_ERR_UNLIKELY);
        }
        break;

    case ART_RELAY_RGB:
        if (len < 2) {
            return BT_GATT_ERR(BT_ATT_ERR_INVALID_ATTRIBUTE_LEN);
        }
#if IS_ENABLED(CONFIG_NICE_VIEW_RGB_IDLE)
        rgb_idle_remote(d[1] != 0, len < 3 || d[2] != 0);
#endif
        break;
    }
    return len;
}

static ssize_t status_read(struct bt_conn *conn, const struct bt_gatt_attr *attr, void *buf,
                           uint16_t len, uint16_t offset) {
    status.set_count = custom_art_set_count;
#if IS_ENABLED(CONFIG_NICE_VIEW_RGB_IDLE)
    rgb_idle_state(&status.rgb);
#endif
    return bt_gatt_attr_read(conn, attr, buf, len, offset, &status, sizeof(status));
}

static ssize_t screen_read(struct bt_conn *conn, const struct bt_gatt_attr *attr, void *buf,
                           uint16_t len, uint16_t offset) {
    if (offset > SCREEN_SNAP_WIRE) {
        return BT_GATT_ERR(BT_ATT_ERR_INVALID_OFFSET);
    }
    int n = screen_snap_wire_read(buf, len, offset);
    return n < 0 ? BT_GATT_ERR(BT_ATT_ERR_UNLIKELY) : n;
}

BT_GATT_SERVICE_DEFINE(
    art_relay_svc, BT_GATT_PRIMARY_SERVICE(BT_UUID_DECLARE_128(ART_RELAY_SERVICE_UUID)),
    BT_GATT_CHARACTERISTIC(BT_UUID_DECLARE_128(ART_RELAY_DATA_UUID),
                           BT_GATT_CHRC_WRITE | BT_GATT_CHRC_WRITE_WITHOUT_RESP,
                           BT_GATT_PERM_WRITE_ENCRYPT, NULL, data_write, NULL),
    BT_GATT_CHARACTERISTIC(BT_UUID_DECLARE_128(ART_RELAY_STATUS_UUID), BT_GATT_CHRC_READ,
                           BT_GATT_PERM_READ_ENCRYPT, status_read, NULL, NULL),
    BT_GATT_CHARACTERISTIC(BT_UUID_DECLARE_128(ART_RELAY_SCREEN_UUID), BT_GATT_CHRC_READ,
                           BT_GATT_PERM_READ_ENCRYPT, screen_read, NULL, NULL), );
