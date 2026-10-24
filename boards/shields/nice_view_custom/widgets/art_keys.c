/*
 * &art_next / &art_prev (#60): a key steps the right screen's art. The press is the event: with
 * the app subscribed (D7 0D ART_KEYS) it goes to the app as D7 8D, and the app steps its hotlist
 * (files too); otherwise the left steps through what the right holds itself, the built-in sets
 * and then the saved slots, over the art relay. Central only, so a key on either half works.
 * SPDX-License-Identifier: MIT
 */
#define DT_DRV_COMPAT foxglove_behavior_art_step

#include <zephyr/device.h>
#include <drivers/behavior.h>
#include <zmk/behavior.h>

#include "art_relay.h"
#include "host_link.h"

struct art_step_config {
    int8_t dir;
};

static int art_step_pressed(struct zmk_behavior_binding *binding,
                            struct zmk_behavior_binding_event event) {
    const struct device *dev = zmk_behavior_get_binding(binding->behavior_dev);
    const struct art_step_config *cfg = dev->config;
    if (host_link_art_key(cfg->dir) != 0) {
        art_relay_step(cfg->dir);
    }
    return ZMK_BEHAVIOR_OPAQUE;
}

static int art_step_released(struct zmk_behavior_binding *binding,
                             struct zmk_behavior_binding_event event) {
    return ZMK_BEHAVIOR_OPAQUE;
}

static const struct behavior_driver_api art_step_api = {
    .locality = BEHAVIOR_LOCALITY_CENTRAL,
    .binding_pressed = art_step_pressed,
    .binding_released = art_step_released,
};

#define ART_STEP_INST(n)                                                                           \
    static const struct art_step_config art_step_config_##n = {                                   \
        .dir = DT_INST_PROP(n, previous) ? -1 : 1};                                               \
    BEHAVIOR_DT_INST_DEFINE(n, NULL, NULL, NULL, &art_step_config_##n, POST_KERNEL,               \
                            CONFIG_KERNEL_INIT_PRIORITY_DEFAULT, &art_step_api);

DT_INST_FOREACH_STATUS_OKAY(ART_STEP_INST)
