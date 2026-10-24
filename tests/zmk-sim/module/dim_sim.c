/*
 * Simulator driver for the underglow's battery cap (tests/zmk-sim/battery-dim): a fake strip that
 * prints the frames it gets, and a fake cable. It draws the same frame through the real
 * dim_strip.c on battery and on USB power, and checks the caller's buffer is left alone.
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

static bool usb_powered;

bool dim_strip_usb_powered(void) { return usb_powered; }

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
    int err = led_strip_update_rgb(dim, frame, ARRAY_SIZE(frame));
    printk("dim: err %d, caller's frame %s\n", err,
           memcmp(copy, frame, sizeof(frame)) == 0 ? "unchanged" : "CHANGED");
}

static void run_cb(struct k_work *work) {
    printk("dim: ready %d\n", device_is_ready(dim));
    usb_powered = false;
    draw("battery");
    usb_powered = true;
    draw("usb");
    usb_powered = false;
    draw("battery again");
}
static K_WORK_DELAYABLE_DEFINE(run_work, run_cb);

static int dim_sim_init(void) {
    k_work_schedule(&run_work, K_MSEC(10));
    return 0;
}
SYS_INIT(dim_sim_init, APPLICATION, 99);
