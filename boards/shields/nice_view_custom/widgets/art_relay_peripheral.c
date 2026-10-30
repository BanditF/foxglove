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
#include <zephyr/bluetooth/conn.h>
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
#include "pc_state.h"

// The RGB op's byte with the left's own on/off (art_relay.h, #609).
#define RGB_OP_WANTED_AT (3 + sizeof(struct battery_dim_config) + RGB_IDLE_LOOK_SIZE)
#endif

#include <zephyr/sys/byteorder.h>
#if IS_ENABLED(CONFIG_ZMK_RGB_UNDERGLOW)
#include <zmk/rgb_underglow.h>
#endif

#include "art_relay.h"
#if IS_ENABLED(CONFIG_FOXGLOVE_HOST_FRAMES)
#include "lamp_strip.h"
#endif
#if IS_ENABLED(CONFIG_FOXGLOVE_HOST_EFFECTS)
#include "effect_run.h"
#endif
#include "screen_snap.h"
#if IS_ENABLED(CONFIG_FOXGLOVE_TEST_HOOKS)
#include "test_hooks.h"
#endif
#include "art_slots.h"
#include "half_marker.h"
#include "custom_art.h"
#include "peripheral_status.h"

static uint8_t slots[2][CONFIG_NICE_VIEW_ART_RELAY_MAX_SIZE];
static int shown = -1; // slot playing, or -1 for a built-in set
static int rx;         // slot being received into; never the one shown
static uint16_t rx_len;
static uint16_t slot_len[2]; // blob length in each RAM slot, once committed
static int flash_shown = -1;  // flash slot playing, or -1
static uint32_t rx_crc;

static struct art_relay_status status = {
    // 5: ART_RELAY_RGB_HSB, 6: battery dim (config in RGB, state after build), 7: SET_FRAME's
    // ART_RELAY_LAMPS / LAMP_HOST (#272), 8: SET_EFFECT's ART_RELAY_EFFECT / EFFECT_GO (#490),
    // 9: ART_RELAY_EFFECT takes the whole wire, with the PC's LED points (#550)
    .version = IS_ENABLED(CONFIG_FOXGLOVE_HOST_EFFECTS) ? 9
               : IS_ENABLED(CONFIG_FOXGLOVE_HOST_FRAMES) ? 7
                                                         : 6,
    .capacity = CONFIG_NICE_VIEW_ART_RELAY_MAX_SIZE,
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
static bool parse(const uint8_t *b, uint16_t slot_len, const uint8_t **deltas,
                  struct custom_art_set *set) {
    if (slot_len < ART_RELAY_HEADER_SIZE || memcmp(b, ART_RELAY_MAGIC, 4) != 0 ||
        b[4] != CUSTOM_ART_W || b[5] != CUSTOM_ART_H) {
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

static int8_t select_set;

static void play_cb(struct k_work *work) {
    if (flash_shown >= 0) {
        art_play(&flash_set);
    } else if (shown >= 0) {
        art_play(&ram_sets[shown]);
    } else {
        status.set = art_builtin_index(select_set);
        art_play(&custom_art_sets[status.set]);
    }
    // The art alone rewrites its rows; this also covers a panel that lost power since (#613).
    art_redraw_all();
}
static K_WORK_DEFINE(play_work, play_cb);

static void play(void) {
    if (zmk_display_is_initialized()) {
        k_work_submit_to_queue(zmk_display_work_q(), &play_work);
    }
}

#if IS_ENABLED(CONFIG_NICE_VIEW_RGB_IDLE)
static void rgb_disconnected(struct bt_conn *conn, uint8_t reason) {
    rgb_idle_remote_lost();
    art_redraw_all(); // the right's own idle state may switch the panel's power back on (#613)
#if IS_ENABLED(CONFIG_FOXGLOVE_HOST_EFFECTS)
    effect_run_stop(true); // the left sends it again when the link is back
#endif
#if IS_ENABLED(CONFIG_FOXGLOVE_HOST_FRAMES)
    lamp_strip_host(false); // no PC to draw any more: the right's own effect comes back
#endif
}

BT_CONN_CB_DEFINE(art_relay_rgb_conn_cb) = {
    .disconnected = rgb_disconnected,
};
#endif

// The split link dropped: the art goes by this half's own activity until the left is back
// (link-up sends its state again, #603).
static void activity_disconnected(struct bt_conn *conn, uint8_t reason) {
    art_activity_remote(false);
}

BT_CONN_CB_DEFINE(art_relay_activity_conn_cb) = {
    .disconnected = activity_disconnected,
};

#if IS_ENABLED(CONFIG_FOXGLOVE_HOST_EFFECTS)
static struct lamp_effect_wire effect_rx; // ART_RELAY_EFFECT fills it, EFFECT_GO draws it
#endif

// From art_slots.c's queue when a SAVE or ERASE has finished.
static void save_done(int err) { status.state = err ? ART_RIGHT_SAVE_FAILED : ART_RIGHT_SAVED; }

static void screen_push_ask(struct bt_conn *conn);
static void screen_push_cancel(void);

// Runs in the Bluetooth RX thread; keep it to copying and checking.
static ssize_t data_write(struct bt_conn *conn, const struct bt_gatt_attr *attr, const void *buf,
                          uint16_t len, uint16_t offset, uint8_t flags) {
    const uint8_t *d = buf;
    if (offset != 0 || len < 1) {
        return BT_GATT_ERR(BT_ATT_ERR_INVALID_OFFSET);
    }

    switch (d[0]) {
#if IS_ENABLED(CONFIG_FOXGLOVE_HOST_FRAMES)
    case ART_RELAY_LAMPS: {
        // Commands fill the frame; the request (the frame's last write) draws it.
        if (len < 3) {
            return BT_GATT_ERR(BT_ATT_ERR_INVALID_ATTRIBUTE_LEN);
        }
        bool fill = d[2] & 0x80;
        int n = d[2] & 0x7f;
        if (fill ? len != 6 : len != 3 + 3 * n) {
            return BT_GATT_ERR(BT_ATT_ERR_INVALID_ATTRIBUTE_LEN);
        }
        for (int i = 0; i < n; i++) {
            const uint8_t *c = fill ? &d[3] : &d[3 + 3 * i];
            lamp_strip_set(d[1] + i, c[0], c[1], c[2]);
        }
        if (!(flags & BT_GATT_WRITE_FLAG_CMD)) {
            lamp_strip_show();
        }
        break;
    }
    case ART_RELAY_LAMP_HOST:
        if (len != 2) {
            return BT_GATT_ERR(BT_ATT_ERR_INVALID_ATTRIBUTE_LEN);
        }
#if IS_ENABLED(CONFIG_FOXGLOVE_HOST_EFFECTS)
        effect_run_stop(false); // the frames take over from an effect, or hand back below
#endif
        lamp_strip_host(d[1] != 0);
        art_redraw_all(); // taking or handing back the LEDs switches the panel's power (#613)
        break;
#endif
#if IS_ENABLED(CONFIG_FOXGLOVE_HOST_EFFECTS)
    case ART_RELAY_EFFECT: // offset, bytes: part of the effect, as write commands
        if (len < 2 || d[1] > sizeof(effect_rx)) {
            return BT_GATT_ERR(BT_ATT_ERR_INVALID_OFFSET);
        }
        if (d[1] == 0) {
            // A new copy: a left before #550 sends no LED points, so none are kept from before;
            // LEDs a left sends no point for (one set for fewer, #561) sit at the half's centre.
            effect_rx.lamps_set = 0;
            memset(effect_rx.lamp, 0xFF, sizeof(effect_rx.lamp));
        }
        // LED points past this half's own (a left set for more, #561) are no LEDs here.
        memcpy((uint8_t *)&effect_rx + d[1], &d[2],
               MIN((size_t)(len - 2), sizeof(effect_rx) - d[1]));
        break;
    case ART_RELAY_EFFECT_GO:
        if (len != 6) {
            return BT_GATT_ERR(BT_ATT_ERR_INVALID_ATTRIBUTE_LEN);
        }
        if (d[1]) {
            effect_run_start(&effect_rx, sys_get_le32(&d[2])); // re-anchors the clock
        } else {
            effect_run_stop(true);
            lamp_strip_host(false); // also after frames the effect never replaced
        }
        art_redraw_all(); // as for LAMP_HOST (#613)
        break;
#endif
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
        if (status.state == ART_RIGHT_SAVING) {
            return BT_GATT_ERR(0x82); // the shown slot is being copied to flash
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
            slot_len[rx] = rx_len;
            flash_shown = -1;
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
        if (status.state == ART_RIGHT_SAVING) {
            return BT_GATT_ERR(0x82);
        }
        select_set = (int8_t)d[1];
        if (select_set >= custom_art_set_count &&
            select_set < custom_art_set_count + art_slots_count()) {
            uint16_t blob_len;
            const uint8_t *blob = art_slots_blob(select_set - custom_art_set_count, &blob_len);
            art_hold(); // flash_set may be on screen while parse fills it
            if (blob == NULL || !parse(blob, blob_len, flash_deltas, &flash_set)) {
                status.state = ART_RIGHT_BAD_BLOB;
                play(); // carry on with whatever was showing
                break;
            }
            flash_shown = select_set - custom_art_set_count;
            shown = -1;
            status.set = select_set;
            status.state = ART_RIGHT_IDLE;
            play();
            break;
        }
        art_hold(); // as for COMMIT: a push may reuse the shown slot before play_cb runs
        shown = -1;
        flash_shown = -1;
        status.state = ART_RIGHT_IDLE;
        play();
        break;

    case ART_RELAY_SAVE:
        if (len != 18) {
            return BT_GATT_ERR(BT_ATT_ERR_INVALID_ATTRIBUTE_LEN);
        }
        if (status.state == ART_RIGHT_SAVING) {
            return BT_GATT_ERR(0x82);
        }
        if (shown < 0 || d[1] >= art_slots_count()) {
            return BT_GATT_ERR(0x85); // nothing pushed to save, or no such slot
        }
        status.state = ART_RIGHT_SAVING;
        if (art_slots_save(d[1], slots[shown], slot_len[shown], (const char *)&d[2], save_done)) {
            status.state = ART_RIGHT_SAVE_FAILED;
        }
        break;

    case ART_RELAY_ERASE:
        if (len != 2) {
            return BT_GATT_ERR(BT_ATT_ERR_INVALID_ATTRIBUTE_LEN);
        }
        if (status.state == ART_RIGHT_SAVING) {
            return BT_GATT_ERR(0x82);
        }
        if (d[1] >= art_slots_count()) {
            return BT_GATT_ERR(0x85);
        }
        if (flash_shown == d[1]) { // don't play from pages about to be erased
            art_hold();
            flash_shown = -1;
            select_set = -1;
            play();
        }
        status.state = ART_RIGHT_SAVING;
        if (art_slots_erase(d[1], save_done)) {
            status.state = ART_RIGHT_SAVE_FAILED;
        }
        break;

    case ART_RELAY_SNAP: {
        // One byte: capture, the left reads it. ART_RELAY_SNAP_PUSH (#662): capture and push it.
        bool push = len == 2 && d[1] == ART_RELAY_SNAP_PUSH;
        if (len != 1 && !push) {
            return BT_GATT_ERR(BT_ATT_ERR_INVALID_ATTRIBUTE_LEN);
        }
        if (push) {
            screen_push_ask(conn); // before the capture can end
        }
        if (screen_snap_local() != SCREEN_OK) {
            if (push) {
                screen_push_cancel();
            }
            return BT_GATT_ERR(BT_ATT_ERR_UNLIKELY);
        }
        break;
    }

    case ART_RELAY_RGB:
        if (len < 2) {
            return BT_GATT_ERR(BT_ATT_ERR_INVALID_ATTRIBUTE_LEN);
        }
#if IS_ENABLED(CONFIG_NICE_VIEW_RGB_IDLE)
        // Byte 13 (#609): the left's own on/off, shown while the PC state holds this half.
        rgb_idle_remote_wanted(d[1] != 0, len < 3 || d[2] != 0,
                               len > RGB_OP_WANTED_AT ? d[RGB_OP_WANTED_AT] != 0 : d[1] != 0);
        // The left's look (#624): following it, the right shows the left's colour and effect,
        // whatever it saved before a reboot or missed while the link was down.
        if (len >= 3 + sizeof(struct battery_dim_config) + RGB_IDLE_LOOK_SIZE && d[2] != 0) {
            rgb_idle_remote_look(&d[3 + sizeof(struct battery_dim_config)]);
        }
#endif
        art_redraw_all(); // following the left's on/off switches the panel's power (#613)
#if IS_ENABLED(CONFIG_NICE_VIEW_RGB_BATTERY_DIM)
        if (len >= 3 + sizeof(struct battery_dim_config)) {
            struct battery_dim_config c;
            memcpy(&c, &d[3], sizeof(c));
            battery_dim_set_config(&c); // an invalid one is ignored; the rest still applies
        }
#endif
        break;

    case ART_RELAY_ACTIVITY: // write request or command: the same state, newest last (#603)
        if (len != 2) {
            return BT_GATT_ERR(BT_ATT_ERR_INVALID_ATTRIBUTE_LEN);
        }
        art_activity_remote(d[1] == ZMK_ACTIVITY_ACTIVE);
        pc_state_remote_keys(d[1] == ZMK_ACTIVITY_ACTIVE); // a left key counts for USB idle
        break;

    case ART_RELAY_PC_STATE: // the PC's state as the left has it (#609): write request or command
        if (len != PC_STATE_RELAY_SIZE) {
            return BT_GATT_ERR(BT_ATT_ERR_INVALID_ATTRIBUTE_LEN);
        }
#if IS_ENABLED(CONFIG_FOXGLOVE_PC_STATE)
        pc_state_remote(d[1], sys_get_le16(&d[2]));
#endif
        break;

    case ART_RELAY_RGB_HSB: {
        if (len != 5) {
            return BT_GATT_ERR(BT_ATT_ERR_INVALID_ATTRIBUTE_LEN);
        }
#if IS_ENABLED(CONFIG_ZMK_RGB_UNDERGLOW)
        // What &rgb_ug RGB_COLOR_HSB does, without ZMK's split queue; drawn on the next tick.
        struct zmk_led_hsb hsb = {.h = sys_get_le16(&d[1]), .s = d[3], .b = d[4]};
        if (zmk_rgb_underglow_set_hsb(hsb) < 0) {
            return BT_GATT_ERR(BT_ATT_ERR_VALUE_NOT_ALLOWED);
        }
#endif
        break;
    }
    }
    return len;
}

static ssize_t status_read(struct bt_conn *conn, const struct bt_gatt_attr *attr, void *buf,
                           uint16_t len, uint16_t offset) {
    status.set_count = custom_art_set_count;
#if IS_ENABLED(CONFIG_NICE_VIEW_RGB_IDLE)
    rgb_idle_state(&status.rgb);
#endif
    status.flash_slots = art_slots_count();
    status.flash_used = art_slots_used();
    // Status, then the slot table, the build info and the battery dim state (only built when a
    // read reaches past the status).
#define SLOTS_AT sizeof(status)
#define BUILD_AT (SLOTS_AT + ART_RELAY_FLASH_SLOTS * sizeof(struct art_relay_slot_info))
#define DIM_AT (BUILD_AT + sizeof(struct foxglove_build_info))
    BUILD_ASSERT(DIM_AT + sizeof(struct battery_dim_wire) == ART_RELAY_REDRAWS_AT, "art_relay.h");
    static uint8_t value[ART_RELAY_REDRAWS_AT + sizeof(uint32_t)];
    memcpy(value, &status, sizeof(status));
    if (offset + len > sizeof(status)) {
        for (int i = 0; i < ART_RELAY_FLASH_SLOTS; i++) {
            struct art_relay_slot_info info;
            art_slots_info(i, &info);
            memcpy(&value[SLOTS_AT + i * sizeof(info)], &info, sizeof(info));
        }
        struct foxglove_build_info build;
        foxglove_build_info(&build);
        memcpy(&value[BUILD_AT], &build, sizeof(build));
        struct battery_dim_wire dim = {0};
#if IS_ENABLED(CONFIG_NICE_VIEW_RGB_BATTERY_DIM)
        battery_dim_state(&dim);
#endif
        memcpy(&value[DIM_AT], &dim, sizeof(dim));
        sys_put_le32(art_redraw_count(), &value[ART_RELAY_REDRAWS_AT]);
    }
    return bt_gatt_attr_read(conn, attr, buf, len, offset, value, sizeof(value));
}

// The left subscribed to the doorbell (each link, after discovery): it hears this half's state at
// once (#605), not at its next read. On every write with notify on (ART_RELAY_RINGS_ON_SUBSCRIBE):
// a cfg_changed callback missed every re-link, since Zephyr 3.5 restores a bonded peer's CCC
// value at encryption (gatt.c update_ccc), before the left has subscribed, and the subscribe that
// follows doesn't change it (bt_gatt_attr_write_ccc). The ring runs on the system queue, after
// this write is stored, so the notify reaches the left.
static ssize_t doorbell_written(struct bt_conn *conn, const struct bt_gatt_attr *attr,
                                uint16_t value) {
    if (value & BT_GATT_CCC_NOTIFY) {
        art_relay_right_rang(ART_RELAY_RANG_RGB);
    }
    return sizeof(value);
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
    BT_GATT_CHARACTERISTIC(BT_UUID_DECLARE_128(ART_RELAY_STATUS_UUID),
                           BT_GATT_CHRC_READ | BT_GATT_CHRC_NOTIFY, BT_GATT_PERM_READ_ENCRYPT,
                           status_read, NULL, NULL),
    // Right after the status value: the left subscribes at its handle + 1 (#478).
    BT_GATT_CCC_MANAGED(((struct _bt_gatt_ccc[]){BT_GATT_CCC_INITIALIZER(NULL, doorbell_written,
                                                                          NULL)}),
                        BT_GATT_PERM_READ | BT_GATT_PERM_WRITE_ENCRYPT),
    BT_GATT_CHARACTERISTIC(BT_UUID_DECLARE_128(ART_RELAY_SCREEN_UUID), BT_GATT_CHRC_READ,
                           BT_GATT_PERM_READ_ENCRYPT, screen_read, NULL, NULL), );

/* The doorbell (#478): this half's underglow on/off or power source changed (following the left,
 * its own idle with sync off, a key, the cable, new levels). One notify of the status value, the bits of what changed;
 * the left reads again and tells the PC. Start/stop: starts on a change; stops once sent, when
 * nobody is subscribed (old left firmware, no link), or after RING_TRIES when the radio has no
 * buffer. */

#define STATUS_VALUE 4 // art_relay_svc.attrs: service, data decl, data, status decl, status
#define RING_RETRY K_MSEC(100)
#define RING_TRIES 5

static atomic_t rang;
static uint8_t ring_tries;

static void ring_cb(struct k_work *work) {
    uint8_t what = atomic_get(&rang);
    const struct bt_gatt_attr *status_attr = &art_relay_svc.attrs[STATUS_VALUE];
    if (what == 0 || bt_uuid_cmp(status_attr->uuid, BT_UUID_DECLARE_128(ART_RELAY_STATUS_UUID))) {
        return; // the table moved: never notify another attribute
    }
    // An RGB ring carries the state (#605): what, struct rgb_idle_wire, struct rgb_shown_wire.
    uint8_t ring[1 + sizeof(struct rgb_idle_wire) + sizeof(struct rgb_shown_wire)] = {what};
    uint16_t ring_len = 1;
#if IS_ENABLED(CONFIG_NICE_VIEW_RGB_IDLE)
    if (what & ART_RELAY_RANG_RGB) {
        struct rgb_idle_wire rgb;
        struct rgb_shown_wire shown;
        rgb_idle_state(&rgb);
        rgb_idle_shown(&shown);
        memcpy(&ring[1], &rgb, sizeof(rgb));
        memcpy(&ring[1 + sizeof(rgb)], &shown, sizeof(shown));
        ring_len = sizeof(ring);
    }
#endif
    int err = bt_gatt_notify(NULL, status_attr, ring, ring_len);
    if (err == -ENOMEM && ring_tries++ < RING_TRIES) {
        k_work_reschedule(k_work_delayable_from_work(work), RING_RETRY);
        return;
    }
    ring_tries = 0;
    atomic_and(&rang, ~(atomic_val_t)what); // a change since then rings again
    if (atomic_get(&rang)) {
        k_work_reschedule(k_work_delayable_from_work(work), K_NO_WAIT);
    }
}
static K_WORK_DELAYABLE_DEFINE(ring_work, ring_cb);

void art_relay_right_rang(uint8_t what) {
    // The underglow went on or off, or the cable came or went: the panel's power may be back.
    art_redraw_all(); // #613
    atomic_or(&rang, what);
    k_work_reschedule(&ring_work, K_NO_WAIT);
}

/* The screen push (#662): after a SNAP with ART_RELAY_SNAP_PUSH, the captured image goes to the
 * left as notifications of the status value: ART_RELAY_RANG_SCREEN, offset u16 LE, then up to
 * MTU - 6 bytes (art_relay.h). A notification takes no ATT response buffer, and a peripheral with
 * data to send skips its slave latency (ZMK's 30 x 7.5 ms), so the image comes in a few hundred
 * ms. The left's paced Read Blob requests took ~63 round trips, each waiting for this half to wake
 * (d265853: every snapshot timed out). The left reads any chunk that didn't come.
 *
 * Start/stop: starts when a capture asked for by such a SNAP ends (screen_snap_captured, display
 * queue); runs on the system queue, at most PUSH_OUT notifications unsent at a time, each one's
 * sent callback sends the next. It stops once the last byte is handed over, when the capture
 * failed, when the link refuses a notification (gone, or the left not subscribed), or after
 * PUSH_TRIES x PUSH_RETRY without a buffer. A new SNAP's capture starts it over from offset 0;
 * while that capture runs, the image can't be read and the push waits for its end. */

#define PUSH_OUT 2 // leaves the radio's other TX buffers to ZMK's key events and the rings
#define PUSH_RETRY K_MSEC(10)
#define PUSH_TRIES 50

static struct bt_conn *push_asked; // the SNAP's link, until its capture ends (under irq_lock)
static struct bt_conn *push_conn;  // the push's link (system queue only)
static atomic_t push_start;        // a capture for a push ended well: start over
static atomic_t push_gone;         // a link went: drop what the push holds
static atomic_t push_out;          // notifications handed to Zephyr, their sent callback not run
static atomic_t push_epoch;        // the push link the count is for
static uint16_t push_at;
static uint8_t push_tries;
static struct k_work_delayable push_work;

static void screen_push_ask(struct bt_conn *conn) {
    struct bt_conn *old;
    unsigned int key = irq_lock();
    old = push_asked;
    push_asked = bt_conn_ref(conn);
    irq_unlock(key);
    if (old != NULL) {
        bt_conn_unref(old);
    }
}

static void screen_push_cancel(void) {
    unsigned int key = irq_lock();
    struct bt_conn *old = push_asked;
    push_asked = NULL;
    irq_unlock(key);
    if (old != NULL) {
        bt_conn_unref(old);
    }
}

void screen_snap_captured(bool ok) {
    if (!ok) {
        screen_push_cancel();
        return;
    }
    unsigned int key = irq_lock();
    bool asked = push_asked != NULL;
    irq_unlock(key);
    if (asked) {
        atomic_set(&push_start, 1);
        k_work_reschedule(&push_work, K_NO_WAIT);
    }
}

static void push_end(void) {
    if (push_conn != NULL) {
        bt_conn_unref(push_conn);
        push_conn = NULL;
    }
    push_tries = 0;
}

static void push_sent(struct bt_conn *conn, void *user_data) {
    if ((atomic_val_t)(uintptr_t)user_data == atomic_get(&push_epoch) &&
        atomic_dec(&push_out) <= 0) {
        atomic_set(&push_out, 0);
    }
    if (push_conn != NULL) {
        k_work_reschedule(&push_work, K_NO_WAIT);
    }
}

static void push_cb(struct k_work *work) {
    if (atomic_get(&push_gone)) { // the link went: its ref goes, and the SNAP's
        atomic_clear(&push_gone);
        push_end();
        screen_push_cancel();
        atomic_clear(&push_start);
        return;
    }
    if (atomic_cas(&push_start, 1, 0)) {
        unsigned int key = irq_lock();
        struct bt_conn *conn = push_asked;
        push_asked = NULL;
        irq_unlock(key);
        if (conn == NULL) {
            return; // cancelled meanwhile
        }
        if (conn != push_conn) { // a new link: Zephyr drops the old one's sent callbacks
            atomic_inc(&push_epoch);
            atomic_set(&push_out, 0);
        }
        push_end();
        push_conn = conn;
        push_at = 0;
    }
    const struct bt_gatt_attr *status_attr = &art_relay_svc.attrs[STATUS_VALUE];
    if (push_conn == NULL ||
        bt_uuid_cmp(status_attr->uuid, BT_UUID_DECLARE_128(ART_RELAY_STATUS_UUID))) {
        push_end();
        return;
    }
    uint8_t chunk[ART_RELAY_SNAP_PUSH_HEADER + 20];
    int room = MIN((int)bt_gatt_get_mtu(push_conn) - 3 - ART_RELAY_SNAP_PUSH_HEADER,
                   (int)sizeof(chunk) - ART_RELAY_SNAP_PUSH_HEADER);
    while (push_at < SCREEN_SNAP_WIRE && atomic_get(&push_out) < PUSH_OUT && room > 0) {
        int n = screen_snap_wire_read(&chunk[ART_RELAY_SNAP_PUSH_HEADER], room, push_at);
        if (n == -EAGAIN) {
            return; // a new capture runs: its end starts the push over (or the image is gone)
        }
        if (n <= 0) {
            push_end();
            return;
        }
        chunk[0] = ART_RELAY_RANG_SCREEN;
        sys_put_le16(push_at, &chunk[1]);
        struct bt_gatt_notify_params params = {
            .attr = status_attr,
            .data = chunk,
            .len = ART_RELAY_SNAP_PUSH_HEADER + n,
            .func = push_sent,
            .user_data = (void *)(uintptr_t)atomic_get(&push_epoch),
        };
        atomic_inc(&push_out);
        int err = bt_gatt_notify_cb(push_conn, &params);
        if (err) {
            atomic_dec(&push_out);
            if ((err == -ENOMEM || err == -EAGAIN) && push_tries++ < PUSH_TRIES) {
                k_work_reschedule(&push_work, PUSH_RETRY); // no buffer: the radio catches up
                return;
            }
            LOG_WRN("art relay: screen push stopped at %d (%d)", push_at, err);
            push_end();
            return;
        }
        push_tries = 0;
        push_at += n;
    }
    if (push_at >= SCREEN_SNAP_WIRE) {
        push_end(); // the last sent callbacks only count down
    }
}
static K_WORK_DELAYABLE_DEFINE(push_work, push_cb);

// Never hold a link that has gone (the right takes one connection): the push work lets go of it.
static void push_disconnected(struct bt_conn *conn, uint8_t reason) {
    atomic_set(&push_gone, 1);
    k_work_reschedule(&push_work, K_NO_WAIT);
}

BT_CONN_CB_DEFINE(art_relay_push_conn_cb) = {
    .disconnected = push_disconnected,
};
