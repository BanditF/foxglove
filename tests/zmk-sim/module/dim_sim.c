/*
 * Simulator driver for the underglow's levels by power state (tests/zmk-sim/battery-dim): a fake
 * strip that prints the frames it gets, and fake USB and charge readings. It draws the same frame
 * through the real dim_strip.c and battery_dim.c before any event, then after each USB or battery
 * change (battery_dim_update, what ZMK's events call), and checks the caller's buffer is left
 * alone.
 * SPDX-License-Identifier: MIT
 */
#define DT_DRV_COMPAT foxglove_sim_strip

#include <stdbool.h>
#include <string.h>

#include <zephyr/device.h>
#include <zephyr/drivers/led_strip.h>
#include <zephyr/init.h>
#include <zephyr/kernel.h>
#include <zephyr/sys/printk.h>

#include "battery_dim.h"

// What ZMK reads before its first event: no cable, no charge reading yet.
bool battery_dim_read_usb(void) { return false; }
uint8_t battery_dim_read_soc(void) { return 0; }

static int sim_update_rgb(const struct device *dev, struct led_rgb *pixels, size_t n) {
    char line[96];
    int len = snprintk(line, sizeof(line), "dim: strip");
    for (size_t i = 0; i < n && len < sizeof(line); i++) {
        len += snprintk(line + len, sizeof(line) - len, " %u,%u,%u", pixels[i].r, pixels[i].g,
                        pixels[i].b);
    }
    printk("%s\n", line); // one printk, so log output can't land inside the line
    return 0;
}

static int sim_update_channels(const struct device *dev, uint8_t *channels, size_t n) {
    return -ENOTSUP;
}

static const struct led_strip_driver_api sim_api = {
    .update_rgb = sim_update_rgb,
    .update_channels = sim_update_channels,
};

DEVICE_DT_INST_DEFINE(0, NULL, NULL, NULL, NULL, POST_KERNEL, CONFIG_KERNEL_INIT_PRIORITY_DEFAULT,
                      &sim_api);

static const struct device *const dim = DEVICE_DT_GET(DT_CHOSEN(zmk_underglow));

static void draw(const char *label) {
    struct led_rgb frame[] = {{.r = 255, .g = 128, .b = 10}, {.r = 100, .g = 0, .b = 200}};
    struct led_rgb copy[ARRAY_SIZE(frame)];
    memcpy(copy, frame, sizeof(frame));
    printk("dim: %s\n", label);
    printk("dim: level %u\n", battery_dim_level());
    int err = led_strip_update_rgb(dim, frame, ARRAY_SIZE(frame));
    printk("dim: err %d, caller's frame %s\n", err,
           memcmp(copy, frame, sizeof(frame)) == 0 ? "unchanged" : "CHANGED");
}

static void event(bool usb, uint8_t soc, const char *label) {
    battery_dim_update(usb, soc);
    draw(label);
}

static void config(uint8_t enabled, uint8_t power, uint8_t battery, uint8_t low, uint8_t below,
                   const char *label) {
    struct battery_dim_config c = {enabled, power, battery, low, below};
    printk("dim: config %s: %d\n", label, battery_dim_set_config(&c));
    draw(label);
}

static void run_cb(struct k_work *work) {
    printk("dim: ready %d\n", device_is_ready(dim));
    draw("first frame, no event yet");
    event(true, 25, "usb at 25%");
    event(false, 25, "battery 25%");
    event(false, 20, "battery 20%");
    event(false, 15, "battery 15%");
    event(false, 21, "battery 21%, still low");
    event(false, 22, "battery 22%");
    event(false, 19, "battery 19%");
    event(true, 19, "usb at 19%");
    event(false, 19, "unplugged at 19%");
    config(0, 100, 80, 12, 20, "switched off");
    config(1, 0, 80, 12, 20, "level 0 refused");
    config(1, 100, 50, 5, 30, "own levels");
    event(false, 40, "battery 40%, own levels");
}
static K_WORK_DELAYABLE_DEFINE(run_work, run_cb);

static int dim_sim_init(void) {
    k_work_schedule(&run_work, K_MSEC(10));
    return 0;
}
SYS_INIT(dim_sim_init, APPLICATION, 99);
