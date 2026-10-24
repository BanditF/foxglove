/* SPDX-License-Identifier: MIT */
#pragma once
#include <stdbool.h>
#include <string.h>
#include <zephyr/device.h>
#include <zephyr/devicetree.h>

/* Device names also cover renamed nodes; both reset behaviors share one compatible. */
static inline bool foxglove_behavior_refused(const char *name) {
    if (!name) {
        return false;
    }
#define REFUSE_NODE(node) if (strcmp(name, DEVICE_DT_NAME(node)) == 0) { return true; }
    DT_FOREACH_STATUS_OKAY(zmk_behavior_bluetooth, REFUSE_NODE)
    DT_FOREACH_STATUS_OKAY(zmk_behavior_outputs, REFUSE_NODE)
    DT_FOREACH_STATUS_OKAY(zmk_behavior_reset, REFUSE_NODE)
    DT_FOREACH_STATUS_OKAY(zmk_behavior_soft_off, REFUSE_NODE)
#undef REFUSE_NODE
    return false;
}
