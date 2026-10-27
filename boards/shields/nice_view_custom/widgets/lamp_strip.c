/*
 * This half's underglow strip under the PC's control (lamp_strip.h).
 * SPDX-License-Identifier: MIT
 *
 * Strip writes run on ZMK's low-priority queue, the one ZMK's own underglow off work uses. Taking
 * the strip turns ZMK's underglow off, which queues a work item that zeroes the strip; the first
 * host draw is queued behind it, so the PC's frame is what stays. Handing back turns ZMK's
 * underglow off once more (zeroing the host's last frame) and lets rgb_idle put the user's
 * on/off back. Every pixel goes through the chosen zmk,underglow, which is dim_strip.c on the
 * Corne, so frames dim on battery like ZMK's own effects (#465). While the PC draws, dim_strip drops
 * ZMK's own writes, so an &rgb_ug key that turns ZMK's underglow on can't draw over the PC's
 * colours (SET_EFFECT runs without a timeout).
 */

#include <zephyr/device.h>
#include <zephyr/drivers/led_strip.h>
#include <zephyr/kernel.h>

#include <drivers/ext_power.h>
#include <zmk/rgb_underglow.h>
#include <zmk/workqueue.h>

#include "lamp_strip.h"
#include "rgb_idle.h"
#if IS_ENABLED(CONFIG_NICE_VIEW_RGB_BATTERY_DIM)
#include "battery_dim.h"
#endif

#define EXT_POWER                                                                                  \
    (IS_ENABLED(CONFIG_ZMK_RGB_UNDERGLOW_EXT_POWER) &&                                             \
     DT_HAS_COMPAT_STATUS_OKAY(zmk_ext_power_generic))

static const struct device *const strip = DEVICE_DT_GET(DT_CHOSEN(zmk_underglow));
#if EXT_POWER
static const struct device *const ext_power = DEVICE_DT_GET(DT_INST(0, zmk_ext_power_generic));
#endif

static struct led_rgb pixels[LAMP_STRIP_LEN];
static atomic_t want_host;
static atomic_t want_dark;
static bool host;    // only touched on the low-priority queue
static __maybe_unused bool unpowered; // likewise: a dark effect cut the LEDs' power

static void hold(bool on) {
#if IS_ENABLED(CONFIG_NICE_VIEW_RGB_BATTERY_DIM)
    dim_strip_hold(on); // ZMK's own writes are dropped while the PC draws
#endif
}

static void draw(void) {
#if EXT_POWER
    bool dark = atomic_get(&want_dark);
    if (!dark && unpowered && device_is_ready(ext_power)) {
        ext_power_enable(ext_power);
        unpowered = false;
    }
#endif
#if IS_ENABLED(CONFIG_NICE_VIEW_RGB_BATTERY_DIM)
    dim_strip_update_host(pixels, LAMP_STRIP_LEN);
#else
    led_strip_update_rgb(strip, pixels, LAMP_STRIP_LEN);
#endif
#if EXT_POWER
    if (dark && !unpowered && device_is_ready(ext_power)) {
        ext_power_disable(ext_power); // after the black frame, so they don't keep a colour
        unpowered = true;
    }
#endif
}

static void strip_cb(struct k_work *work);
static K_WORK_DEFINE(strip_work, strip_cb);

static void strip_cb(struct k_work *work) {
    bool want = atomic_get(&want_host);
    if (want != host) {
        host = want;
        if (want) {
            rgb_idle_host_control(true); // first, so idle and sync don't turn ZMK back on
            zmk_rgb_underglow_off();     // stops ZMK's animation; queues its zeroing
            hold(true);                  // and drops ZMK's writes (that zeroing too) from here
#if EXT_POWER
            if (device_is_ready(ext_power)) {
                ext_power_enable(ext_power); // ZMK's off cut the LEDs' power
            }
            unpowered = false;
#endif
            // Behind ZMK's zeroing work, so the first frame isn't wiped.
            k_work_submit_to_queue(zmk_workqueue_lowprio_work_q(), &strip_work);
            return;
        }
        hold(false);
        zmk_rgb_underglow_off();      // clears the PC's last frame
        rgb_idle_host_control(false); // re-applies the user's on/off (and idle, sync)
        return;
    }
    if (host) {
        draw();
    }
}

void lamp_strip_show(void) { k_work_submit_to_queue(zmk_workqueue_lowprio_work_q(), &strip_work); }

void lamp_strip_host(bool on) {
    if (atomic_set(&want_host, on) != on) {
        lamp_strip_show();
    }
}

void lamp_strip_dark(bool dark) { atomic_set(&want_dark, dark); }

void lamp_strip_set(int index, uint8_t r, uint8_t g, uint8_t b) {
    if (index >= 0 && index < LAMP_STRIP_LEN) {
        pixels[index] = (struct led_rgb){.r = r, .g = g, .b = b};
    }
}
