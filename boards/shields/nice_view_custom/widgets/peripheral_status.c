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

#if CONFIG_NICE_VIEW_WIDGET_INVERTED
#define ART_PALETTE 0xff, 0xff, 0xff, 0xff, 0x00, 0x00, 0x00, 0xff
#else
#define ART_PALETTE 0x00, 0x00, 0x00, 0xff, 0xff, 0xff, 0xff, 0xff
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
    return (struct art_activity_state){.active = zmk_activity_get_state() == ZMK_ACTIVITY_ACTIVE};
}

ZMK_DISPLAY_WIDGET_LISTENER(widget_art_activity, struct art_activity_state,
                            art_activity_update_cb, art_activity_get_state)
ZMK_SUBSCRIPTION(widget_art_activity, zmk_activity_state_changed);
#endif

static void art_init(lv_obj_t *parent) {
    int index = CONFIG_NICE_VIEW_CUSTOM_ART_SET;
    if (index < 0 || index >= custom_art_set_count) {
        index = sys_rand32_get() % custom_art_set_count;
    }
    art_set = &custom_art_sets[index];
    art_frame = 0;

    art_img = lv_img_create(parent);
    art_show(0);
    lv_img_set_src(art_img, &art_dsc);
    lv_obj_align(art_img, LV_ALIGN_TOP_LEFT, 0, 0);

#if IS_ENABLED(CONFIG_NICE_VIEW_CUSTOM_ANIMATE)
    if (art_set->frame_count > 1 && art_set->fps > 0) {
        art_timer = lv_timer_create(art_timer_cb, 1000 / art_set->fps, NULL);
    }
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
