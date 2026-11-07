/*
 * relay-sim: what test_hooks.c (the D7 05 position press, #851) needs from Zephyr, the
 * devicetree and ZMK. The keymap has no layers here (nothing is refused); the position events it
 * raises land in sim_hooks_positions (sim_stubs.c), which the scenarios read as the left's held
 * keys. SPDX-License-Identifier: MIT
 */
#pragma once
#include "zephyr_sim.h"

/* The Eyelash Corne's default_transform (eyelash_corne.dtsi): RC(row, col) = row << 8 | col. */
#define SIM_RC(r, c) (((r) << 8) | (c))
#define SIM_DT_default_transform_map                                                              \
    {SIM_RC(0, 0),  SIM_RC(0, 1),  SIM_RC(0, 2),  SIM_RC(0, 3),  SIM_RC(0, 4),  SIM_RC(0, 5),     \
     SIM_RC(0, 12), SIM_RC(0, 13), SIM_RC(0, 7),  SIM_RC(0, 8),  SIM_RC(0, 9),  SIM_RC(0, 10),    \
     SIM_RC(0, 11), SIM_RC(1, 0),  SIM_RC(1, 1),  SIM_RC(1, 2),  SIM_RC(1, 3),  SIM_RC(1, 4),     \
     SIM_RC(1, 5),  SIM_RC(2, 12), SIM_RC(4, 12), SIM_RC(3, 12), SIM_RC(1, 13), SIM_RC(1, 7),     \
     SIM_RC(1, 8),  SIM_RC(1, 9),  SIM_RC(1, 10), SIM_RC(1, 11), SIM_RC(2, 0),  SIM_RC(2, 1),     \
     SIM_RC(2, 2),  SIM_RC(2, 3),  SIM_RC(2, 4),  SIM_RC(2, 5),  SIM_RC(3, 2),  SIM_RC(1, 12),    \
     SIM_RC(2, 13), SIM_RC(2, 7),  SIM_RC(2, 8),  SIM_RC(2, 9),  SIM_RC(2, 10), SIM_RC(2, 11),    \
     SIM_RC(3, 3),  SIM_RC(3, 4),  SIM_RC(3, 5),  SIM_RC(3, 13), SIM_RC(3, 7),  SIM_RC(3, 8)}
#define DT_NODELABEL(label) label
#define DT_PROP(node, prop) SIM_DT_PROP2(node, prop)
#define SIM_DT_PROP2(node, prop) SIM_DT_##node##_##prop
#define DT_FOREACH_STATUS_OKAY(compat, fn)
#define DEVICE_DT_NAME(node) ""

/* ZMK */
#define ZMK_KEYMAP_LEN 48
#define ZMK_KEYMAP_LAYERS_LEN 1
#define ZMK_KEYMAP_LAYER_ID_INVAL UINT8_MAX
#define ZMK_POSITION_STATE_CHANGE_SOURCE_LOCAL UINT8_MAX
struct zmk_behavior_binding;
struct zmk_position_state_changed {
    uint8_t source;
    uint32_t position;
    bool state;
    int64_t timestamp;
};
int raise_zmk_position_state_changed(struct zmk_position_state_changed ev);
static inline uint8_t zmk_keymap_layer_index_to_id(int idx) { return (uint8_t)idx; }
static inline uint8_t zmk_keymap_layer_default(void) { return 0; }
static inline bool zmk_keymap_layer_active(uint8_t id) { (void)id; return false; }
static inline const struct zmk_behavior_binding *zmk_keymap_get_layer_binding_at_idx(uint8_t id,
                                                                                    uint8_t pos) {
    (void)id;
    (void)pos;
    return NULL;
}

/* Zephyr */
static inline uint32_t k_uptime_get_32(void) { return (uint32_t)k_uptime_get(); }
#define RST_UF2 0x57
static inline void sys_reboot(int type) { (void)type; }
uint32_t sys_rand32_get(void);

/* The left's held keys as test_hooks.c's events left them (bit = position). */
extern uint64_t sim_hooks_positions;
