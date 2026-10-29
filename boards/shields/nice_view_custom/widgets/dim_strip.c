/* SPDX-License-Identifier: MIT */
// The underglow's brightness by power state: an LED strip between ZMK and the WS2812s that scales
// every pixel to battery_dim_level() percent (battery_dim.c: on USB, on battery, low battery). ZMK's
// own brightness, effects and saved state don't change, and each half goes by its own cable and
// charge. ZMK redraws every 50 ms while the underglow is on, so a new level shows on the next
// frame; the level itself only changes on ZMK's USB and battery events (#218).
#define DT_DRV_COMPAT foxglove_dim_strip

#include <zephyr/device.h>
#include <zephyr/drivers/led_strip.h>
#include <zephyr/kernel.h>
#include <zephyr/sys/util.h>
#include <zephyr/toolchain.h>

#include "battery_dim.h"
#if IS_ENABLED(CONFIG_NICE_VIEW_RGB_IDLE)
#include "rgb_idle.h"
#endif

#define LEN DT_INST_PROP(0, chain_length)
// The underglow's chain is the board's strip's (#561): this wrapper's chain-length
// (dim_strip.dtsi) has to say the same, or LEDs go undrawn or SET_FRAME misses them.
#if DT_NODE_HAS_PROP(DT_INST_PHANDLE(0, led_strip), chain_length)
BUILD_ASSERT(LEN == DT_PROP(DT_INST_PHANDLE(0, led_strip), chain_length),
             "dim_strip.dtsi's chain-length must match the board's led_strip");
#endif

static const struct device *const strip = DEVICE_DT_GET(DT_INST_PHANDLE(0, led_strip));
static struct led_rgb scaled[LEN]; // the WS2812 driver may write into the buffer it's given
static K_MUTEX_DEFINE(scaled_lock); // the underglow and the LampArray (#115) both draw

// A lit channel stays at least 1, so the lowest level dims the lights but never turns them off.
static uint8_t scale(uint8_t v, unsigned int cap) { return v == 0 ? 0 : MAX(v * cap / 100, 1); }

static atomic_t held; // the PC draws (dim_strip_hold)

static int draw(struct led_rgb *pixels, size_t n, bool host) {
    const unsigned int cap = battery_dim_level();
#if IS_ENABLED(CONFIG_NICE_VIEW_RGB_IDLE)
    rgb_idle_frame(host, pixels, MIN(n, LEN), cap); // what the LEDs show, for D7 83 (#605)
#endif
    if (cap >= 100) {
        return led_strip_update_rgb(strip, pixels, n);
    }
    n = MIN(n, LEN);
    k_mutex_lock(&scaled_lock, K_FOREVER);
    for (size_t i = 0; i < n; i++) {
        scaled[i] = pixels[i];
        scaled[i].r = scale(pixels[i].r, cap);
        scaled[i].g = scale(pixels[i].g, cap);
        scaled[i].b = scale(pixels[i].b, cap);
    }
    int err = led_strip_update_rgb(strip, scaled, n);
    k_mutex_unlock(&scaled_lock);
    return err;
}

static int dim_update_rgb(const struct device *dev, struct led_rgb *pixels, size_t n) {
    return atomic_get(&held) ? 0 : draw(pixels, n, false);
}

void dim_strip_hold(bool on) { atomic_set(&held, on); }

int dim_strip_update_host(struct led_rgb *pixels, size_t n) { return draw(pixels, n, true); }

static int dim_update_channels(const struct device *dev, uint8_t *channels, size_t n) {
    return -ENOTSUP;
}

static const struct led_strip_driver_api dim_api = {
    .update_rgb = dim_update_rgb,
    .update_channels = dim_update_channels,
};

static int dim_init(const struct device *dev) { return device_is_ready(strip) ? 0 : -ENODEV; }

// After the WS2812 driver, before ZMK's underglow (APPLICATION) looks for its strip.
DEVICE_DT_INST_DEFINE(0, dim_init, NULL, NULL, NULL, POST_KERNEL, 99, &dim_api);
BUILD_ASSERT(CONFIG_LED_STRIP_INIT_PRIORITY < 99, "the WS2812 must init first");
