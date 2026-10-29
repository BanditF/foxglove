/*
 *
 * Copyright (c) 2023 The ZMK Contributors
 * SPDX-License-Identifier: MIT
 *
 */

#include <string.h>

#include <zephyr/kernel.h>
#include <zephyr/random/random.h>

#include <zephyr/logging/log.h>
LOG_MODULE_DECLARE(zmk, CONFIG_ZMK_LOG_LEVEL);

#include <zmk/battery.h>
#include <zmk/display.h>
#include <zmk/events/usb_conn_state_changed.h>
#include <zmk/event_manager.h>
#include <zmk/events/battery_state_changed.h>
#include <zmk/split/bluetooth/peripheral.h>
#include <zmk/events/split_peripheral_status_changed.h>
#include <zmk/usb.h>
#include <zmk/ble.h>

#include <zmk/events/activity_state_changed.h>
#include <zmk/activity.h>

#include "peripheral_status.h"
#include "screen_snap.h"
#include "custom_art.h"

static sys_slist_t widgets = SYS_SLIST_STATIC_INIT(&widgets);

struct peripheral_status_state {
    bool connected;
};

static void draw_top(lv_obj_t *widget, lv_color_t cbuf[], const struct status_state *state) {
    lv_obj_t *canvas = lv_obj_get_child(widget, 0);

    lv_draw_label_dsc_t label_dsc;
    init_label_dsc(&label_dsc, LVGL_FOREGROUND, &lv_font_montserrat_16, LV_TEXT_ALIGN_RIGHT);
    lv_draw_rect_dsc_t rect_black_dsc;
    init_rect_dsc(&rect_black_dsc, LVGL_BACKGROUND);

    // Fill background
    lv_canvas_draw_rect(canvas, 0, 0, CANVAS_SIZE, CANVAS_SIZE, &rect_black_dsc);

    // Draw battery
    draw_battery(canvas, state);

    // Draw output status
    lv_canvas_draw_text(canvas, 0, 0, CANVAS_SIZE, &label_dsc,
                        state->connected ? LV_SYMBOL_WIFI : LV_SYMBOL_CLOSE);

    // Rotate canvas
    rotate_canvas(canvas, cbuf);
}

static void set_battery_status(struct zmk_widget_status *widget,
                               struct battery_status_state state) {
#if IS_ENABLED(CONFIG_USB_DEVICE_STACK)
    widget->state.charging = state.usb_present;
#endif /* IS_ENABLED(CONFIG_USB_DEVICE_STACK) */

    widget->state.battery = state.level;

    draw_top(widget->obj, widget->cbuf, &widget->state);
}

static void battery_status_update_cb(struct battery_status_state state) {
    struct zmk_widget_status *widget;
    SYS_SLIST_FOR_EACH_CONTAINER(&widgets, widget, node) { set_battery_status(widget, state); }
}

static struct battery_status_state battery_status_get_state(const zmk_event_t *eh) {
    return (struct battery_status_state){
        .level = zmk_battery_state_of_charge(),
#if IS_ENABLED(CONFIG_USB_DEVICE_STACK)
        .usb_present = zmk_usb_is_powered(),
#endif /* IS_ENABLED(CONFIG_USB_DEVICE_STACK) */
    };
}

ZMK_DISPLAY_WIDGET_LISTENER(widget_battery_status, struct battery_status_state,
                            battery_status_update_cb, battery_status_get_state)

ZMK_SUBSCRIPTION(widget_battery_status, zmk_battery_state_changed);
#if IS_ENABLED(CONFIG_USB_DEVICE_STACK)
ZMK_SUBSCRIPTION(widget_battery_status, zmk_usb_conn_state_changed);
#endif /* IS_ENABLED(CONFIG_USB_DEVICE_STACK) */

static struct peripheral_status_state get_state(const zmk_event_t *_eh) {
    return (struct peripheral_status_state){.connected = zmk_split_bt_peripheral_is_connected()};
}

static void set_connection_status(struct zmk_widget_status *widget,
                                  struct peripheral_status_state state) {
    widget->state.connected = state.connected;

    draw_top(widget->obj, widget->cbuf, &widget->state);
}

static void output_status_update_cb(struct peripheral_status_state state) {
    struct zmk_widget_status *widget;
    SYS_SLIST_FOR_EACH_CONTAINER(&widgets, widget, node) { set_connection_status(widget, state); }
}

ZMK_DISPLAY_WIDGET_LISTENER(widget_peripheral_status, struct peripheral_status_state,
                            output_status_update_cb, get_state)
ZMK_SUBSCRIPTION(widget_peripheral_status, zmk_split_peripheral_status_changed);

// Art: one set chosen per power-on; animated sets step through their frames while the
// keyboard is active (CONFIG_NICE_VIEW_CUSTOM_ANIMATE) and hold still while idle.
static const struct custom_art_set *art_set;
static lv_obj_t *art_img;
static int art_frame;
static int art_unique = -1; // unique frame currently unpacked in art_buf
// Set while art_set's data is being replaced (art_relay_peripheral.c): hold the current frame.
static atomic_t art_held;
// The left is active (art_activity_remote, #603): set from the relay, cleared when the left
// idles or the link drops.
static atomic_t remote_active;

// The art moves while either half is active: on a split keyboard most keys go to the left, so
// this half's own activity alone left it still while the left typed or after the left woke.
static __maybe_unused bool art_active(void) {
    return zmk_activity_get_state() == ZMK_ACTIVITY_ACTIVE || atomic_get(&remote_active);
}

// Art is stored with background bits set. This palette shows it light-on-dark, like the status
// strip and the left screen; NICE_VIEW_WIDGET_INVERTED flips it along with them.
#if CONFIG_NICE_VIEW_WIDGET_INVERTED
#define ART_PALETTE 0x00, 0x00, 0x00, 0xff, 0xff, 0xff, 0xff, 0xff
#else
#define ART_PALETTE 0xff, 0xff, 0xff, 0xff, 0x00, 0x00, 0x00, 0xff
#endif

static uint8_t art_buf[8 + CUSTOM_ART_STRIDE * CUSTOM_ART_H] = {ART_PALETTE};
static const lv_img_dsc_t art_dsc = {.header.cf = LV_IMG_CF_INDEXED_1BIT,
                                     .header.w = CUSTOM_ART_W,
                                     .header.h = CUSTOM_ART_H,
                                     .data_size = sizeof(art_buf),
                                     .data = art_buf};

// XOR one RLE-packed delta into the pixels (format in scripts/flipper_art.py:rle).
static void art_apply(const uint8_t *src) {
    uint8_t *dst = art_buf + 8;
    uint8_t *end = art_buf + sizeof(art_buf);
    while (dst < end) {
        uint8_t ctrl = *src++;
        int n = MIN((ctrl & 0x3f) + 1, end - dst);
        switch (ctrl >> 6) {
        case 0:
            for (int i = 0; i < n; i++) {
                dst[i] ^= src[i];
            }
            src += n;
            break;
        case 1:
            break;
        case 2:
            for (int i = 0; i < n; i++) {
                dst[i] ^= 0xff;
            }
            break;
        default:
            for (int i = 0; i < n; i++) {
                dst[i] ^= *src;
            }
            src++;
            break;
        }
        dst += n;
    }
}

static void art_show(int frame) {
    int target = art_set->order[frame];
    if (target < art_unique - target) {
        // Rewinding from scratch is shorter than stepping back.
        memset(art_buf + 8, 0, sizeof(art_buf) - 8);
        art_unique = -1;
    }
    while (art_unique < target) {
        art_apply(art_set->deltas[++art_unique]);
    }
    while (art_unique > target) {
        art_apply(art_set->deltas[art_unique--]);
    }
    lv_img_cache_invalidate_src(&art_dsc);
    lv_obj_invalidate(art_img);
}

#if IS_ENABLED(CONFIG_NICE_VIEW_CUSTOM_ANIMATE)
static lv_timer_t *art_timer;

static void art_timer_cb(lv_timer_t *timer) {
    if (atomic_get(&art_held)) {
        return;
    }
    art_frame = (art_frame + 1) % art_set->frame_count;
    art_show(art_frame);
}

struct art_activity_state {
    bool active;
};

static void art_activity_update_cb(struct art_activity_state state) {
    if (art_timer == NULL) {
        return;
    }
    if (state.active) {
        lv_timer_resume(art_timer);
    } else {
        lv_timer_pause(art_timer);
    }
}

static struct art_activity_state art_activity_get_state(const zmk_event_t *eh) {
    return (struct art_activity_state){.active = art_active()};
}

ZMK_DISPLAY_WIDGET_LISTENER(widget_art_activity, struct art_activity_state,
                            art_activity_update_cb, art_activity_get_state)
ZMK_SUBSCRIPTION(widget_art_activity, zmk_activity_state_changed);

// The left's activity changed: the same update on the display queue, where the timer lives.
static void art_remote_cb(struct k_work *work) {
    art_activity_update_cb((struct art_activity_state){.active = art_active()});
}
static K_WORK_DEFINE(art_remote_work, art_remote_cb);
#endif

/* Start/stop (#603): no timer of its own. Each relayed change (activity_relay.h: the left sends
 * one per activity change, and its state again on link-up) resumes or pauses the art timer once;
 * a repeat of the same state does nothing. The link dropping clears it. */
void art_activity_remote(bool active) {
    if ((bool)atomic_set(&remote_active, active) == active) {
        return;
    }
    if (!zmk_display_is_initialized()) {
        return; // art_init reads remote_active when it starts the art
    }
#if IS_ENABLED(CONFIG_NICE_VIEW_CUSTOM_ANIMATE)
    k_work_submit_to_queue(zmk_display_work_q(), &art_remote_work);
#endif
    if (active) {
        art_redraw_all(); // as on this half's own wake (#613)
    }
}

void art_hold(void) { atomic_set(&art_held, 1); }

void art_play(const struct custom_art_set *set) {
    art_set = set;
    art_frame = 0;
    art_unique = -1;
    memset(art_buf + 8, 0, sizeof(art_buf) - 8);
    atomic_set(&art_held, 0);
    if (art_img != NULL) {
        art_show(0);
    }

#if IS_ENABLED(CONFIG_NICE_VIEW_CUSTOM_ANIMATE)
    if (art_timer != NULL) {
        lv_timer_del(art_timer);
        art_timer = NULL;
    }
    if (set->frame_count > 1 && set->fps > 0) {
        art_timer = lv_timer_create(art_timer_cb, 1000 / set->fps, NULL);
        if (!art_active()) {
            lv_timer_pause(art_timer);
        }
    }
#endif
}

/* Whole-screen redraw (#613). The nice!view runs off the nice!nano's switched VCC, the rail ZMK's
 * underglow off cuts (CONFIG_ZMK_RGB_UNDERGLOW_EXT_POWER): idle-off, sync from the left, the PC
 * taking the LEDs and handing them back (lamp_strip.c) all switch it. A Sharp memory LCD that
 * loses power comes back up full of noise, and LVGL only flushes what it knows is dirty, so a
 * still picture (a 1-frame set, or any set while idle) stayed noise until something happened to
 * redraw. Every event that can bring the panel's power back, and every art change, calls this:
 * the whole screen is invalidated so the next flush rewrites every pixel.
 * Start: art_redraw_all() (the events above). Stop: after REDRAW_SHOTS rewrites; a new event
 * starts over (reschedule, so a burst of events is one redraw). The first shot comes once the
 * work that switches the power has run on its own queue, the second catches a slow one. */
#define REDRAW_FIRST K_MSEC(100)
#define REDRAW_AGAIN K_MSEC(900)
#define REDRAW_SHOTS 2

static atomic_t redraw_left;

static void redraw_cb(struct k_work *work) {
    lv_obj_t *screen = lv_scr_act();
    if (screen != NULL) {
        lv_obj_invalidate(screen);
    }
    if (atomic_dec(&redraw_left) > 1) {
        k_work_reschedule_for_queue(zmk_display_work_q(), k_work_delayable_from_work(work),
                                    REDRAW_AGAIN);
    }
}
static K_WORK_DELAYABLE_DEFINE(redraw_work, redraw_cb);

void art_redraw_all(void) {
    if (!zmk_display_is_initialized()) {
        return; // the first load draws everything anyway
    }
    atomic_set(&redraw_left, REDRAW_SHOTS);
    k_work_reschedule_for_queue(zmk_display_work_q(), &redraw_work, REDRAW_FIRST);
}

// This half waking (a key: rgb_idle turns the underglow, and so the panel, back on) or its cable
// coming or going.
static int redraw_listener(const zmk_event_t *eh) {
    const struct zmk_activity_state_changed *activity = as_zmk_activity_state_changed(eh);
    if (activity == NULL || activity->state == ZMK_ACTIVITY_ACTIVE) {
        art_redraw_all();
    }
    return ZMK_EV_EVENT_BUBBLE;
}

ZMK_LISTENER(art_redraw, redraw_listener);
ZMK_SUBSCRIPTION(art_redraw, zmk_activity_state_changed);
#if IS_ENABLED(CONFIG_USB_DEVICE_STACK)
ZMK_SUBSCRIPTION(art_redraw, zmk_usb_conn_state_changed);
#endif

int art_builtin_index(int index) {
    if (index < 0 || index >= custom_art_set_count) {
        index = sys_rand32_get() % custom_art_set_count;
    }
    return index;
}

static void art_init(lv_obj_t *parent) {
    art_img = lv_img_create(parent);
    lv_img_set_src(art_img, &art_dsc);
    lv_obj_align(art_img, LV_ALIGN_TOP_LEFT, 0, 0);
    art_play(&custom_art_sets[art_builtin_index(CONFIG_NICE_VIEW_CUSTOM_ART_SET)]);

#if IS_ENABLED(CONFIG_NICE_VIEW_CUSTOM_ANIMATE)
    widget_art_activity_init();
#endif
}

int zmk_widget_status_init(struct zmk_widget_status *widget, lv_obj_t *parent) {
    widget->obj = lv_obj_create(parent);
    lv_obj_set_size(widget->obj, 160, 68);
    lv_obj_t *top = lv_canvas_create(widget->obj);
    lv_obj_align(top, LV_ALIGN_TOP_RIGHT, 0, 0);
    lv_canvas_set_buffer(top, widget->cbuf, CANVAS_SIZE, CANVAS_SIZE, LV_IMG_CF_TRUE_COLOR);

    art_init(widget->obj);

    sys_slist_append(&widgets, &widget->node);
    widget_battery_status_init();
    widget_peripheral_status_init();

    return 0;
}

lv_obj_t *zmk_widget_status_obj(struct zmk_widget_status *widget) { return widget->obj; }

bool screen_snap_pixels(uint8_t out[SCREEN_SNAP_BYTES]) {
    struct zmk_widget_status *widget;
    SYS_SLIST_FOR_EACH_CONTAINER(&widgets, widget, node) {
        screen_snap_canvas(out, widget->cbuf, 92);
        /* The later art child covers x=0..139, including part of the status canvas. */
        for (int y = 0; y < CUSTOM_ART_H; y++) {
            for (int x = 0; x < CUSTOM_ART_W; x++) {
                int index = !!(art_buf[8 + y * CUSTOM_ART_STRIDE + x / 8] &
                              (0x80 >> (x & 7)));
                screen_snap_pixel(out, x, y, art_buf[index * 4] < 128);
            }
        }
        return true;
    }
    return false;
}
