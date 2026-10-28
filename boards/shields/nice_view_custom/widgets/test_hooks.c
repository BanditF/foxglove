/* Position injection follows the same event path as kscan. SPDX-License-Identifier: MIT */
#include <zephyr/kernel.h>

// The half that talks to the PC: a split central, or a keyboard that isn't split at all.
#define FOXGLOVE_CENTRAL (!IS_ENABLED(CONFIG_ZMK_SPLIT) || IS_ENABLED(CONFIG_ZMK_SPLIT_ROLE_CENTRAL))
#include <zephyr/random/random.h>
#include <zephyr/sys/byteorder.h>
#include <zephyr/sys/reboot.h>
#include <dt-bindings/zmk/reset.h>
#include <zmk/behavior.h>
#include <zmk/event_manager.h>
#include <zmk/events/position_state_changed.h>
#include <zmk/matrix.h>
#if FOXGLOVE_CENTRAL
#include <zmk/keymap.h>
#include "behavior_safety.h"
#include "host_link.h"
#endif
#include "art_relay.h"
#include "test_hooks.h"
#include "half_marker.h"

FOXGLOVE_TEST_HOOK_MARK(position);
FOXGLOVE_TEST_HOOK_MARK(bootloader);

BUILD_ASSERT(ZMK_KEYMAP_LEN <= 64, "Injection bitmap has 64 positions");
/* Eyelash's map entries encode row << 8 | column. Right kscan adds col-offset=7
 * before the transform, so columns >=7 belong to the right. The transformed index
 * is already global: split/peripheral.c forwards pos_ev->position unchanged.
 * Do not subtract 24 or apply the column offset again on the peripheral. */
static const uint32_t position_map[] = DT_PROP(DT_NODELABEL(default_transform), map);
BUILD_ASSERT(ARRAY_SIZE(position_map) == ZMK_KEYMAP_LEN, "Unexpected Eyelash layout");
static bool is_right(uint8_t pos) {
    return IS_ENABLED(CONFIG_ZMK_SPLIT) && (position_map[pos] & 0xff) >= 7;
}

/* Only system queue callbacks access this bitmap and its deadlines. */
static uint64_t downs;
static int64_t expires[64];
static atomic_t busy;
static atomic_t host_seen_at;
static uint8_t request_pos, request_state, relay_status;
static bool automatic;
static void safety_cb(struct k_work *work);
static K_WORK_DELAYABLE_DEFINE(safety_work, safety_cb);

void test_hooks_host_seen(void) { atomic_set(&host_seen_at, k_uptime_get_32()); }

#if FOXGLOVE_CENTRAL
static bool position_refused(uint8_t pos) {
    /* Match v0.3's layer order, including reordered Studio layers and the default
     * layer floor. Only &trans falls through; &none consumes the position. */
    for (int idx = ZMK_KEYMAP_LAYERS_LEN - 1; idx >= 0; idx--) {
        uint8_t id = zmk_keymap_layer_index_to_id(idx);
        if (id == ZMK_KEYMAP_LAYER_ID_INVAL) {
            continue;
        }
        bool at_default = id == zmk_keymap_layer_default();
        if (zmk_keymap_layer_active(id)) {
            const struct zmk_behavior_binding *b = zmk_keymap_get_layer_binding_at_idx(id, pos);
            if (!b || !b->behavior_dev) {
                return true;
            }
#define IS_TRANS(node) if (strcmp(b->behavior_dev, DEVICE_DT_NAME(node)) == 0) { trans = true; }
            bool trans = false;
            DT_FOREACH_STATUS_OKAY(zmk_behavior_transparent, IS_TRANS)
#undef IS_TRANS
            if (!trans) {
                return foxglove_behavior_refused(b->behavior_dev);
            }
        }
        if (at_default) {
            break;
        }
    }
    return false;
}
#endif

static void finish(uint8_t status) {
#if FOXGLOVE_CENTRAL
    if (!automatic) {
        host_link_position_reply(status, request_pos);
    }
#endif
    atomic_clear(&busy);
    if (downs) {
        k_work_reschedule(&safety_work, K_MSEC(100));
    }
}

static void emit(uint8_t pos, bool state) {
    raise_zmk_position_state_changed((struct zmk_position_state_changed){
        .source = ZMK_POSITION_STATE_CHANGE_SOURCE_LOCAL,
        .position = pos, .state = state, .timestamp = k_uptime_get(),
    });
}

static void relay_done_cb(struct k_work *work) {
    uint64_t bit = UINT64_C(1) << request_pos;
    if (!request_state && relay_status == 0) {
        downs &= ~bit;
    }
    /* A timed-out down may have arrived: retain ownership until a release succeeds.
     * The right also expires its own downs even if the split link disappears. */
    if (request_state && relay_status != 0 && relay_status != 3) {
        downs &= ~bit;
    }
    finish(relay_status);
}
static K_WORK_DEFINE(relay_done_work, relay_done_cb);
void test_hooks_relay_done(uint8_t status) {
    relay_status = status;
    k_work_submit(&relay_done_work);
}

static void press_cb(struct k_work *work) {
    uint8_t pos = request_pos;
    uint64_t bit = UINT64_C(1) << pos;
    if (!!(downs & bit) == !!request_state) {
        finish(0);
        return;
    }
#if FOXGLOVE_CENTRAL
    if (request_state && position_refused(pos)) {
        finish(6);
        return;
    }
#endif
    if (request_state) {
        downs |= bit;
        expires[pos] = k_uptime_get() + 10000;
    }
#if FOXGLOVE_CENTRAL && IS_ENABLED(CONFIG_NICE_VIEW_ART_RELAY)
    if (is_right(pos)) {
        art_relay_press(pos, request_state);
        return;
    }
#elif FOXGLOVE_CENTRAL
    if (is_right(pos)) {
        downs &= ~bit;
        finish(2);
        return;
    }
#endif
    if (!request_state) {
        downs &= ~bit;
    }
    emit(pos, request_state);
    finish(0);
}
static K_WORK_DEFINE(press_work, press_cb);

uint8_t test_hooks_press(uint8_t pos, uint8_t state) {
    if (pos >= ZMK_KEYMAP_LEN || state > 1) {
        return 5;
    }
#if !FOXGLOVE_CENTRAL
    if (!is_right(pos)) {
        return 5;
    }
#endif
    if (!atomic_cas(&busy, 0, 1)) {
        return 2;
    }
    request_pos = pos;
    request_state = state;
    automatic = false;
    k_work_submit(&press_work);
    return 0;
}

static void safety_cb(struct k_work *work) {
    int64_t now = k_uptime_get();
    bool quiet = false;
#if FOXGLOVE_CENTRAL
    quiet = (uint32_t)(k_uptime_get_32() - atomic_get(&host_seen_at)) >= 10000;
#endif
    for (int pos = 0; pos < ZMK_KEYMAP_LEN; pos++) {
        uint64_t bit = UINT64_C(1) << pos;
        if (!(downs & bit) || (!quiet && now < expires[pos])) {
            continue;
        }
#if FOXGLOVE_CENTRAL
        if (is_right(pos)) {
            if (atomic_cas(&busy, 0, 1)) {
                request_pos = pos;
                request_state = 0;
                automatic = true;
                press_cb(NULL);
            }
            continue;
        }
#endif
        /* Release all expired local keys together, even while a relay request waits. */
        downs &= ~bit;
        emit(pos, false);
    }
    if (downs) {
        k_work_reschedule(&safety_work, K_MSEC(100));
    }
}

/* BOOTLOADER (D7 08): reboot into the UF2 bootloader without a double tap. */

#define BOOT_ARM_MS 5000
// Long enough for the reply (USB report or ATT response) to leave before the reset.
#define BOOT_DELAY K_MSEC(300)

static void reboot_cb(struct k_work *work) { sys_reboot(RST_UF2); }
static K_WORK_DELAYABLE_DEFINE(reboot_work, reboot_cb);

void test_hooks_reboot_soon(void) { k_work_schedule(&reboot_work, BOOT_DELAY); }

#if FOXGLOVE_CENTRAL
static uint32_t boot_nonce; // 0 = not armed
static int64_t boot_armed_at;
static atomic_t boot_relaying;

void test_hooks_boot_relay_done(uint8_t status, uint8_t route) {
    atomic_clear(&boot_relaying);
    host_link_bootloader_reply(status, 1, route);
}

bool test_hooks_bootloader(const uint8_t *d, int len, uint8_t r[7]) {
    r[2] = 5;
    if (len >= 3 && d[2] == 0) { // ARM
        do {
            boot_nonce = sys_rand32_get();
        } while (boot_nonce == 0);
        boot_armed_at = k_uptime_get();
        r[2] = 0;
        sys_put_le32(boot_nonce, &r[3]);
        return false;
    }
    if (len >= 3 && d[2] == 1) {
        // Any TRIGGER uses the nonce up, a malformed one too: no second try after a bad guess.
        uint32_t armed = boot_nonce;
        boot_nonce = 0;
        if (len < 8 || d[3] > 1) {
            r[3] = len >= 4 ? d[3] : 0;
            return false; // status 5
        }
        boot_nonce = armed; // checked below, then used up there
    }
    if (len < 8 || d[2] != 1) {
        return false;
    }
    uint8_t half = d[3];
    r[3] = half;
    bool good = boot_nonce != 0 && sys_get_le32(&d[4]) == boot_nonce &&
                k_uptime_get() - boot_armed_at <= BOOT_ARM_MS;
    boot_nonce = 0; // one TRIGGER per ARM, good or bad
    if (!good) {
        r[2] = 6;
        return false;
    }
    if (half == 0) {
        r[2] = 0;
        test_hooks_reboot_soon();
        return false;
    }
#if IS_ENABLED(CONFIG_NICE_VIEW_ART_RELAY)
    // Without a relay link too: the relay queue falls back to ZMK's own split link (#545).
    if (atomic_cas(&boot_relaying, 0, 1)) {
        art_relay_boot();
        return true;
    }
#endif
    r[2] = 2;
    return false;
}
#endif
