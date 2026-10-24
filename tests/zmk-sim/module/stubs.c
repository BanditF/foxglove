/* SPDX-License-Identifier: MIT */
#define DT_DRV_COMPAT foxglove_sim_noop_two
#include <zephyr/device.h>
#include <zephyr/sys/printk.h>
#include <drivers/behavior.h>
#include <zmk/behavior.h>
#include <zmk/pm.h>

static int noop(struct zmk_behavior_binding *binding,
                struct zmk_behavior_binding_event event) {
    ARG_UNUSED(binding);
    ARG_UNUSED(event);
    return ZMK_BEHAVIOR_OPAQUE;
}
static const struct behavior_driver_api noop_api = {
    .binding_pressed = noop,
    .binding_released = noop,
};
#define NOOP_INST(n) \
    BEHAVIOR_DT_INST_DEFINE(n, NULL, NULL, NULL, NULL, POST_KERNEL, \
                            CONFIG_KERNEL_INIT_PRIORITY_DEFAULT, &noop_api);
DT_INST_FOREACH_STATUS_OKAY(NOOP_INST)

int zmk_pm_soft_off(void) {
    printk("SIM_SOFT_OFF\n");
    return 0;
}
