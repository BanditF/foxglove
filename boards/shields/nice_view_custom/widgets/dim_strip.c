/* SPDX-License-Identifier: MIT */
// The underglow on battery: an LED strip between ZMK and the WS2812s that scales every pixel to
// CONFIG_NICE_VIEW_RGB_BATTERY_CAP percent while the half has no USB power (#218). ZMK's own
// brightness, effects and saved state don't change, nothing is written to flash, and each half
// goes by its own cable. ZMK redraws every 50 ms while the underglow is on, so plugging in or out
// shows on the next frame; there's no timer or listener of our own.
#define DT_DRV_COMPAT foxglove_dim_strip

#include <zephyr/device.h>
#include <zephyr/drivers/led_strip.h>
#include <zephyr/kernel.h>
#include <zephyr/sys/util.h>

#if IS_ENABLED(CONFIG_USB_DEVICE_STACK)
#include <zmk/usb.h>
#endif

#define LEN DT_INST_PROP(0, chain_length)

static const struct device *const strip = DEVICE_DT_GET(DT_INST_PHANDLE(0, led_strip));
static struct led_rgb scaled[LEN]; // the WS2812 driver may write into the buffer it's given
static K_MUTEX_DEFINE(scaled_lock); // the underglow and the LampArray (#115) both draw

static bool on_battery(void) {
#if IS_ENABLED(CONFIG_USB_DEVICE_STACK)
    return !zmk_usb_is_powered();
#else
    return true;
#endif
}

static int dim_update_rgb(const struct device *dev, struct led_rgb *pixels, size_t n) {
    const unsigned int cap = CONFIG_NICE_VIEW_RGB_BATTERY_CAP;
    if (cap >= 100 || !on_battery()) {
        return led_strip_update_rgb(strip, pixels, n);
    }
    n = MIN(n, LEN);
    k_mutex_lock(&scaled_lock, K_FOREVER);
    for (size_t i = 0; i < n; i++) {
        scaled[i] = pixels[i];
        scaled[i].r = pixels[i].r * cap / 100;
        scaled[i].g = pixels[i].g * cap / 100;
        scaled[i].b = pixels[i].b * cap / 100;
    }
    int err = led_strip_update_rgb(strip, scaled, n);
    k_mutex_unlock(&scaled_lock);
    return err;
}

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
