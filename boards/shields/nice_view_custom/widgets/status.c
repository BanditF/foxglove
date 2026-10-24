/*
 *
 * Copyright (c) 2025 The ZMK Contributors
 * SPDX-License-Identifier: MIT
 *
 */

#include <zephyr/kernel.h>

#include <zephyr/logging/log.h>
LOG_MODULE_DECLARE(zmk, CONFIG_ZMK_LOG_LEVEL);

#include <zmk/battery.h>
#include <zmk/display.h>
#include "status.h"
#include "screen_snap.h"
#include "host_link.h"
#include <zmk/events/usb_conn_state_changed.h>
#include <zmk/event_manager.h>
#include <zmk/events/battery_state_changed.h>
#include <zmk/events/ble_active_profile_changed.h>
#include <zmk/events/endpoint_changed.h>
#include <zmk/events/wpm_state_changed.h>
#include <zmk/events/layer_state_changed.h>
#include <zmk/events/keycode_state_changed.h>
#include <zmk/events/hid_indicators_changed.h>
#include <zmk/hid_indicators.h>
#include <dt-bindings/zmk/hid_usage.h>
#include <dt-bindings/zmk/hid_usage_pages.h>
#include <zmk/usb.h>
#include <zmk/ble.h>
#include <zmk/endpoints.h>
#include <zmk/keymap.h>
#include <zmk/wpm.h>

static sys_slist_t widgets = SYS_SLIST_STATIC_INIT(&widgets);

struct output_status_state {
    struct zmk_endpoint_instance selected_endpoint;
    int active_profile_index;
    bool active_profile_connected;
    bool active_profile_bonded;
    bool profiles_connected[NICEVIEW_PROFILE_COUNT];
    bool profiles_bonded[NICEVIEW_PROFILE_COUNT];
};

struct layer_status_state {
    zmk_keymap_layer_index_t index;
    const char *label;
};

struct wpm_status_state {
    uint8_t wpm;
};

struct mods_status_state {
    uint8_t mods;
    bool caps_lock;
};

/*
 * The screen is mounted portrait: 68 wide x 160 tall. It is backed by three 68x68 canvases that
 * show portrait rows 0-67 (top), 68-135 (middle) and 136-159 (the first 24 rows of bottom).
 * draw_screen() draws the whole portrait layout into each canvas, shifted up by that canvas's
 * offset; LVGL clips what falls outside, so shapes line up across the seams.
 */
#define SCREEN_W 68
#define SCREEN_H 160

static void rect(lv_obj_t *c, int oy, int x, int y, int w, int h, int radius, bool filled,
                 bool outline, lv_color_t color) {
    lv_draw_rect_dsc_t dsc;
    lv_draw_rect_dsc_init(&dsc);
    dsc.radius = radius;
    dsc.bg_color = color;
    dsc.bg_opa = filled ? LV_OPA_COVER : LV_OPA_TRANSP;
    dsc.border_color = color;
    dsc.border_width = outline ? 1 : 0;
    canvas_draw_rect(c, x, y - oy, w, h, &dsc);
}

static void text(lv_obj_t *c, int oy, int x, int y, int w, const char *t, lv_color_t color,
                 lv_text_align_t align) {
    lv_draw_label_dsc_t dsc;
    init_label_dsc(&dsc, color, &lv_font_unscii_8, align);
    canvas_draw_text(c, x, y - oy, w, &dsc, t);
}

// Rounded pill with a centred label: outlined when off, solid when on.
static void pill(lv_obj_t *c, int oy, int x, int y, int w, int h, const char *t, bool on) {
    rect(c, oy, x, y, w, h, h / 2, on, true, LVGL_FOREGROUND);
    text(c, oy, x, y + (h - 8) / 2, w, t, on ? LVGL_BACKGROUND : LVGL_FOREGROUND,
         LV_TEXT_ALIGN_CENTER);
}

// The PC's panel (host_link.c), 1 bit per pixel, MSB first, 1 = ink.
static void draw_host_panel(lv_obj_t *c, int oy, const uint8_t *panel) {
    for (int y = MAX(0, oy - HOST_PANEL_Y); y < HOST_PANEL_H; y++) {
        int sy = HOST_PANEL_Y + y - oy;
        if (sy >= CANVAS_SIZE) {
            break;
        }
        for (int x = 0; x < HOST_PANEL_W; x++) {
            if (panel[y * HOST_PANEL_STRIDE + x / 8] & (0x80 >> (x % 8))) {
                lv_canvas_set_px(c, HOST_PANEL_X + x, sy, LVGL_FOREGROUND, LV_OPA_COVER);
            }
        }
    }
}

// Bluetooth profiles, so the one in use is obvious (wrong-profile mix-ups): numbered boxes, the
// active one filled, connected ones outlined; below, the active profile's state, or USB when the
// output is USB. Inverted while the SYSTEM layer (Bluetooth keys) is up and just after a change.
static void draw_profiles(lv_obj_t *c, int oy, const struct status_state *state) {
    bool ble = state->selected_endpoint.transport == ZMK_TRANSPORT_BLE;
    bool invert = state->profile_flash ||
                  (state->layer_label != NULL && strcmp(state->layer_label, "SYSTEM") == 0);
    lv_color_t fg = invert ? LVGL_BACKGROUND : LVGL_FOREGROUND;
    lv_color_t bg = invert ? LVGL_FOREGROUND : LVGL_BACKGROUND;
    if (invert) {
        rect(c, oy, 2, 84, SCREEN_W - 4, 20, 3, true, false, bg);
    }
    for (int i = 0; i < NICEVIEW_PROFILE_COUNT; i++) {
        int x = 4 + i * 12;
        char digit[2] = {'1' + i, 0};
        bool active = i == state->active_profile_index;
        if (active) {
            rect(c, oy, x, 85, 11, 10, 2, true, false, fg);
        } else if (state->profiles_connected[i]) {
            rect(c, oy, x, 85, 11, 10, 2, false, true, fg);
        }
        text(c, oy, x, 86, 11, digit, active ? bg : fg, LV_TEXT_ALIGN_CENTER);
    }
    const char *word = !ble                              ? "USB"
                       : state->active_profile_connected ? "LINKED"
                       : state->active_profile_bonded    ? "SEARCH"
                                                         : "PAIR";
    text(c, oy, 2, 96, SCREEN_W - 4, word, fg, LV_TEXT_ALIGN_CENTER);
}

static void draw_screen(lv_obj_t *c, int oy, const struct status_state *state,
                        const uint8_t *panel) {
    lv_color_t fg = LVGL_FOREGROUND;
    lv_color_t bg = LVGL_BACKGROUND;

    lv_canvas_fill_bg(c, bg, LV_OPA_COVER);

    // Status strip, like the right half: battery on the left, connection on the right.
    rect(c, oy, 1, 3, 22, 10, 2, false, true, fg);
    rect(c, oy, 23, 6, 2, 4, 0, true, false, fg);
    int fill = (17 * MIN(state->battery, 100) + 50) / 100;
    if (fill > 0) {
        rect(c, oy, 3, 5, fill, 6, 0, true, false, fg);
    }
    if (state->charging) {
        // LVGL 9 has no polygon: the bolt as four triangles.
        static const int8_t tris[][6] = {{29, 2, 26, 8, 29, 6}, {26, 8, 28, 8, 29, 6},
                                         {28, 8, 27, 13, 31, 6}, {28, 8, 31, 6, 29, 6}};
        lv_layer_t layer;
        lv_canvas_init_layer(c, &layer);
        for (int i = 0; i < ARRAY_SIZE(tris); i++) {
            lv_draw_triangle_dsc_t tri;
            lv_draw_triangle_dsc_init(&tri);
            tri.color = fg;
            for (int j = 0; j < 3; j++) {
                tri.p[j].x = tris[i][2 * j];
                tri.p[j].y = tris[i][2 * j + 1] - oy;
            }
            lv_draw_triangle(&layer, &tri);
        }
        lv_canvas_finish_layer(c, &layer);
    }
    char conn[16] = "USB";
    if (state->selected_endpoint.transport == ZMK_TRANSPORT_BLE) {
        snprintf(conn, sizeof(conn), "BT%d", state->active_profile_index + 1);
    }
    text(c, oy, 34, 4, SCREEN_W - 35, conn, fg, LV_TEXT_ALIGN_RIGHT);
    if (state->selected_endpoint.transport == ZMK_TRANSPORT_BLE &&
        !state->active_profile_connected) {
        rect(c, oy, 42, 8, SCREEN_W - 44, 1, 0, true, false, fg); // struck through: not connected
    }

    // Frame with the layer name in a tab, like the Hammerbeam art on the right.
    rect(c, oy, 0, 17, SCREEN_W, SCREEN_H - 17, 6, false, true, fg);
    char layer[12];
    if (state->layer_label == NULL || strlen(state->layer_label) == 0) {
        snprintf(layer, sizeof(layer), "LAYER %d", state->layer_index);
    } else {
        snprintf(layer, sizeof(layer), "%s", state->layer_label);
    }
    int tab_w = MIN(8 * (int)strlen(layer) + 8, SCREEN_W - 4);
    rect(c, oy, 2, 19, tab_w, 13, 6, true, false, fg);
    text(c, oy, 2, 22, tab_w, layer, bg, LV_TEXT_ALIGN_CENTER);

    // Held modifiers (either side) as pills, Caps Lock below.
    const uint8_t m = state->mods;
    pill(c, oy, 4, 37, 29, 14, "CTL", m & (BIT(0) | BIT(4)));
    pill(c, oy, 35, 37, 29, 14, "SFT", m & (BIT(1) | BIT(5)));
    pill(c, oy, 4, 53, 29, 14, "ALT", m & (BIT(2) | BIT(6)));
    pill(c, oy, 35, 53, 29, 14, "WIN", m & (BIT(3) | BIT(7)));
    pill(c, oy, 4, 69, 60, 14, "CAPS", state->caps_lock);

    draw_profiles(c, oy, state);
    rect(c, oy, 6, 104, SCREEN_W - 12, 1, 0, true, false, fg);

    if (panel != NULL) {
        draw_host_panel(c, oy, panel);
        return;
    }

    // WPM: label, number badge, and a dithered filled-area chart of the last 10 samples.
    text(c, oy, 5, 110, 30, "WPM", fg, LV_TEXT_ALIGN_LEFT);
    char wpm_text[6];
    snprintf(wpm_text, sizeof(wpm_text), "%d", state->wpm[9]);
    int badge_w = 8 * (int)strlen(wpm_text) + 8;
    rect(c, oy, SCREEN_W - 5 - badge_w, 108, badge_w, 12, 6, true, false, fg);
    text(c, oy, SCREEN_W - 5 - badge_w, 110, badge_w, wpm_text, bg, LV_TEXT_ALIGN_CENTER);

    const int top = 124, bot = 152;
    int lo = 255, hi = 0;
    for (int i = 0; i < 10; i++) {
        lo = MIN(lo, state->wpm[i]);
        hi = MAX(hi, state->wpm[i]);
    }
    int range = MAX(hi - lo, 1);
    lv_point_t pts[10];
    for (int i = 0; i < 10; i++) {
        pts[i].x = 5 + i * 58 / 9;
        pts[i].y = bot - (state->wpm[i] - lo) * (bot - top) / range;
    }
    for (int i = 0; i < 9; i++) {
        for (int x = pts[i].x; x <= pts[i + 1].x; x++) {
            int span = MAX(pts[i + 1].x - pts[i].x, 1);
            int y_line = pts[i].y + (pts[i + 1].y - pts[i].y) * (x - pts[i].x) / span;
            for (int y = MAX(y_line + 1, oy); y <= MIN(bot, oy + CANVAS_SIZE - 1); y++) {
                if (((x + y) & 1) == 0) {
                    lv_canvas_set_px(c, x, y - oy, fg, LV_OPA_COVER);
                }
            }
        }
    }
    for (int i = 0; i < 10; i++) {
        pts[i].y -= oy;
    }
    lv_draw_line_dsc_t line_dsc;
    init_line_dsc(&line_dsc, fg, 1);
    canvas_draw_line(c, pts, 10, &line_dsc);
}

static void draw_all(struct zmk_widget_status *widget) {
    const uint8_t *panel = NULL;
#if IS_ENABLED(CONFIG_NICE_VIEW_HOST_LINK)
    static uint8_t panel_buf[HOST_PANEL_BYTES];
    if (host_panel_get(panel_buf)) {
        panel = panel_buf;
    }
#endif
    for (int i = 0; i < 3; i++) {
        lv_obj_t *canvas = lv_obj_get_child(widget->obj, i);
        draw_screen(canvas, i * CANVAS_SIZE, &widget->state, panel);
        rotate_canvas(canvas);
    }
}

void host_panel_redraw(void) {
    struct zmk_widget_status *widget;
    SYS_SLIST_FOR_EACH_CONTAINER(&widgets, widget, node) { draw_all(widget); }
}

static void set_battery_status(struct zmk_widget_status *widget,
                               struct battery_status_state state) {
#if IS_ENABLED(CONFIG_USB_DEVICE_STACK)
    widget->state.charging = state.usb_present;
#endif /* IS_ENABLED(CONFIG_USB_DEVICE_STACK) */

    widget->state.battery = state.level;

    draw_all(widget);
}

static void battery_status_update_cb(struct battery_status_state state) {
    struct zmk_widget_status *widget;
    SYS_SLIST_FOR_EACH_CONTAINER(&widgets, widget, node) { set_battery_status(widget, state); }
}

static struct battery_status_state battery_status_get_state(const zmk_event_t *eh) {
    const struct zmk_battery_state_changed *ev = as_zmk_battery_state_changed(eh);

    return (struct battery_status_state){
        .level = (ev != NULL) ? ev->state_of_charge : zmk_battery_state_of_charge(),
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

// After a profile or output change the profile block stays inverted this long (one-shot, on the
// display queue; no polling).
#define PROFILE_FLASH K_SECONDS(3)

static void profile_flash_end(struct k_work *work) {
    struct zmk_widget_status *widget;
    SYS_SLIST_FOR_EACH_CONTAINER(&widgets, widget, node) {
        widget->state.profile_flash = false;
        draw_all(widget);
    }
}
static K_WORK_DELAYABLE_DEFINE(profile_flash_work, profile_flash_end);

static void set_output_status(struct zmk_widget_status *widget,
                              const struct output_status_state *state) {
    static bool seen; // the first update is the boot state, not a change
    bool changed = state->active_profile_index != widget->state.active_profile_index ||
                   state->selected_endpoint.transport != widget->state.selected_endpoint.transport;
    if (seen && changed) {
        widget->state.profile_flash = true;
        k_work_reschedule_for_queue(zmk_display_work_q(), &profile_flash_work, PROFILE_FLASH);
    }
    seen = true;
    widget->state.selected_endpoint = state->selected_endpoint;
    widget->state.active_profile_index = state->active_profile_index;
    widget->state.active_profile_connected = state->active_profile_connected;
    widget->state.active_profile_bonded = state->active_profile_bonded;
    for (int i = 0; i < NICEVIEW_PROFILE_COUNT; ++i) {
        widget->state.profiles_connected[i] = state->profiles_connected[i];
        widget->state.profiles_bonded[i] = state->profiles_bonded[i];
    }

    draw_all(widget);
}

static void output_status_update_cb(struct output_status_state state) {
    struct zmk_widget_status *widget;
    SYS_SLIST_FOR_EACH_CONTAINER(&widgets, widget, node) { set_output_status(widget, &state); }
}

static struct output_status_state output_status_get_state(const zmk_event_t *_eh) {
    struct output_status_state state = {
        .selected_endpoint = zmk_endpoints_selected(),
        .active_profile_index = zmk_ble_active_profile_index(),
        .active_profile_connected = zmk_ble_active_profile_is_connected(),
        .active_profile_bonded = !zmk_ble_active_profile_is_open(),
    };
    for (int i = 0; i < MIN(NICEVIEW_PROFILE_COUNT, ZMK_BLE_PROFILE_COUNT); ++i) {
        state.profiles_connected[i] = zmk_ble_profile_is_connected(i);
        state.profiles_bonded[i] = !zmk_ble_profile_is_open(i);
    }
    return state;
}

ZMK_DISPLAY_WIDGET_LISTENER(widget_output_status, struct output_status_state,
                            output_status_update_cb, output_status_get_state)
ZMK_SUBSCRIPTION(widget_output_status, zmk_endpoint_changed);

#if IS_ENABLED(CONFIG_USB_DEVICE_STACK)
ZMK_SUBSCRIPTION(widget_output_status, zmk_usb_conn_state_changed);
#endif
#if defined(CONFIG_ZMK_BLE)
ZMK_SUBSCRIPTION(widget_output_status, zmk_ble_active_profile_changed);
#endif

static void set_layer_status(struct zmk_widget_status *widget, struct layer_status_state state) {
    widget->state.layer_index = state.index;
    widget->state.layer_label = state.label;

    draw_all(widget);
}

static void layer_status_update_cb(struct layer_status_state state) {
    struct zmk_widget_status *widget;
    SYS_SLIST_FOR_EACH_CONTAINER(&widgets, widget, node) { set_layer_status(widget, state); }
}

static struct layer_status_state layer_status_get_state(const zmk_event_t *eh) {
    zmk_keymap_layer_index_t index = zmk_keymap_highest_layer_active();
    return (struct layer_status_state){
        .index = index, .label = zmk_keymap_layer_name(zmk_keymap_layer_index_to_id(index))};
}

ZMK_DISPLAY_WIDGET_LISTENER(widget_layer_status, struct layer_status_state, layer_status_update_cb,
                            layer_status_get_state)

ZMK_SUBSCRIPTION(widget_layer_status, zmk_layer_state_changed);

static void set_wpm_status(struct zmk_widget_status *widget, struct wpm_status_state state) {
    for (int i = 0; i < 9; i++) {
        widget->state.wpm[i] = widget->state.wpm[i + 1];
    }
    widget->state.wpm[9] = state.wpm;

    draw_all(widget);
}

static void wpm_status_update_cb(struct wpm_status_state state) {
    struct zmk_widget_status *widget;
    SYS_SLIST_FOR_EACH_CONTAINER(&widgets, widget, node) { set_wpm_status(widget, state); }
}

struct wpm_status_state wpm_status_get_state(const zmk_event_t *eh) {
    return (struct wpm_status_state){.wpm = zmk_wpm_get_state()};
};

ZMK_DISPLAY_WIDGET_LISTENER(widget_wpm_status, struct wpm_status_state, wpm_status_update_cb,
                            wpm_status_get_state)
ZMK_SUBSCRIPTION(widget_wpm_status, zmk_wpm_state_changed);

static void set_mods_status(struct zmk_widget_status *widget, struct mods_status_state state) {
    widget->state.mods = state.mods;
    widget->state.caps_lock = state.caps_lock;

    draw_all(widget);
}

static void mods_status_update_cb(struct mods_status_state state) {
    struct zmk_widget_status *widget;
    SYS_SLIST_FOR_EACH_CONTAINER(&widgets, widget, node) { set_mods_status(widget, state); }
}

// Tracks physically held modifier keys from the key events themselves, so the
// result doesn't depend on whether the HID report was updated before us.
static struct mods_status_state mods_status_get_state(const zmk_event_t *eh) {
    static uint8_t held_mods;

    const struct zmk_keycode_state_changed *kc =
        eh != NULL ? as_zmk_keycode_state_changed(eh) : NULL;
    if (kc != NULL && kc->usage_page == HID_USAGE_KEY &&
        kc->keycode >= HID_USAGE_KEY_KEYBOARD_LEFTCONTROL &&
        kc->keycode <= HID_USAGE_KEY_KEYBOARD_RIGHT_GUI) {
        WRITE_BIT(held_mods, kc->keycode - HID_USAGE_KEY_KEYBOARD_LEFTCONTROL, kc->state);
    }

    const struct zmk_hid_indicators_changed *ind =
        eh != NULL ? as_zmk_hid_indicators_changed(eh) : NULL;
    zmk_hid_indicators_t indicators =
        ind != NULL ? ind->indicators : zmk_hid_indicators_get_current_profile();

    return (struct mods_status_state){
        .mods = held_mods,
        .caps_lock = indicators & BIT(1), // HID LED report: bit 1 = Caps Lock
    };
}

ZMK_DISPLAY_WIDGET_LISTENER(widget_mods_status, struct mods_status_state, mods_status_update_cb,
                            mods_status_get_state)
ZMK_SUBSCRIPTION(widget_mods_status, zmk_keycode_state_changed);
ZMK_SUBSCRIPTION(widget_mods_status, zmk_hid_indicators_changed);

int zmk_widget_status_init(struct zmk_widget_status *widget, lv_obj_t *parent) {
    widget->obj = lv_obj_create(parent);
    lv_obj_set_size(widget->obj, 160, 68);
    lv_obj_t *top = lv_canvas_create(widget->obj);
    lv_obj_align(top, LV_ALIGN_TOP_RIGHT, 0, 0);
    lv_canvas_set_buffer(top, widget->cbuf, CANVAS_SIZE, CANVAS_SIZE, CANVAS_COLOR_FORMAT);
    lv_obj_t *middle = lv_canvas_create(widget->obj);
    lv_obj_align(middle, LV_ALIGN_TOP_LEFT, 24, 0);
    lv_canvas_set_buffer(middle, widget->cbuf2, CANVAS_SIZE, CANVAS_SIZE, CANVAS_COLOR_FORMAT);
    lv_obj_t *bottom = lv_canvas_create(widget->obj);
    lv_obj_align(bottom, LV_ALIGN_TOP_LEFT, -44, 0);
    lv_canvas_set_buffer(bottom, widget->cbuf3, CANVAS_SIZE, CANVAS_SIZE, CANVAS_COLOR_FORMAT);

    sys_slist_append(&widgets, &widget->node);
    widget_battery_status_init();
    widget_output_status_init();
    widget_layer_status_init();
    widget_wpm_status_init();
    widget_mods_status_init();

    return 0;
}

lv_obj_t *zmk_widget_status_obj(struct zmk_widget_status *widget) { return widget->obj; }

bool screen_snap_pixels(uint8_t out[SCREEN_SNAP_BYTES]) {
    struct zmk_widget_status *widget;
    SYS_SLIST_FOR_EACH_CONTAINER(&widgets, widget, node) {
        screen_snap_canvas(out, widget->cbuf, 92);
        screen_snap_canvas(out, widget->cbuf2, 24);
        screen_snap_canvas(out, widget->cbuf3, -44);
        return true;
    }
    return false;
}
