/* SPDX-License-Identifier: MIT */
#pragma once
#include <stdbool.h>
#include <string.h>
#include <zephyr/device.h>
#include <zephyr/devicetree.h>

/*
 * Behaviors the PC may not press (INVOKE, POSITION): Bluetooth, output selection, reset,
 * bootloader and soft off. Two checks, either one refuses:
 * - every enabled node of those compatibles, by device name (covers renamed nodes; both reset
 *   behaviors share one compatible);
 * - ZMK v0.3's own node names, because the devicetree check alone missed &bt in the simulator
 *   (no enabled zmk,behavior-bluetooth instance there, yet a "bluetooth" device to press).
 */
static inline bool foxglove_behavior_refused(const char *name) {
    if (!name) {
        return false;
    }
    static const char *const zmk_names[] = {"bluetooth", "outputs", "sysreset", "bootload",
                                            "z_so_off"};
    for (size_t i = 0; i < ARRAY_SIZE(zmk_names); i++) {
        if (strcmp(name, zmk_names[i]) == 0) {
            return true;
        }
    }
#define REFUSE_NODE(node) if (strcmp(name, DEVICE_DT_NAME(node)) == 0) { return true; }
    DT_FOREACH_STATUS_OKAY(zmk_behavior_bluetooth, REFUSE_NODE)
    DT_FOREACH_STATUS_OKAY(zmk_behavior_outputs, REFUSE_NODE)
    DT_FOREACH_STATUS_OKAY(zmk_behavior_reset, REFUSE_NODE)
    DT_FOREACH_STATUS_OKAY(zmk_behavior_soft_off, REFUSE_NODE)
#undef REFUSE_NODE
    return false;
}
