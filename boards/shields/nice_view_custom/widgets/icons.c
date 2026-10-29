/*
 * The status strip's icons, shared by both halves (#610). Layout in icons.h.
 * SPDX-License-Identifier: MIT
 */

#include <zephyr/kernel.h>

#include "icons.h"

#define STRIP_W 68

void icons_rect(lv_obj_t *c, int oy, int x, int y, int w, int h, int radius, bool filled,
                bool outline, lv_color_t color) {
    lv_draw_rect_dsc_t dsc;
    lv_draw_rect_dsc_init(&dsc);
    dsc.radius = radius;
    dsc.bg_color = color;
    dsc.bg_opa = filled ? LV_OPA_COVER : LV_OPA_TRANSP;
    dsc.border_color = color;
    dsc.border_width = outline ? 1 : 0;
    lv_canvas_draw_rect(c, x, y - oy, w, h, &dsc);
}

void icons_text(lv_obj_t *c, int oy, int x, int y, int w, const char *t, lv_color_t color,
                lv_text_align_t align) {
    lv_draw_label_dsc_t dsc;
    init_label_dsc(&dsc, color, &lv_font_unscii_8, align);
    lv_canvas_draw_text(c, x, y - oy, w, &dsc, t);
}

void icons_battery(lv_obj_t *c, int oy, uint8_t level, bool charging) {
    lv_color_t fg = LVGL_FOREGROUND;
    icons_rect(c, oy, 1, 3, 22, 10, 2, false, true, fg);
    icons_rect(c, oy, 23, 6, 2, 4, 0, true, false, fg);
    int fill = (17 * MIN(level, 100) + 50) / 100;
    if (fill > 0) {
        icons_rect(c, oy, 3, 5, fill, 6, 0, true, false, fg);
    }
    if (charging) {
        // Row by row: lv_canvas_draw_polygon fills a concave bolt with gaps (the left's
        // readback showed it broken up), so it is drawn from 1 px tall runs instead.
        static const struct {
            uint8_t x, y, w;
        } bolt[] = {{30, 2, 2}, {29, 3, 2}, {28, 4, 2}, {27, 5, 5}, {26, 6, 5},
                    {28, 7, 2}, {27, 8, 2}, {26, 9, 2}, {26, 10, 1}};
        for (size_t i = 0; i < ARRAY_SIZE(bolt); i++) {
            icons_rect(c, oy, bolt[i].x, bolt[i].y, bolt[i].w, 1, 0, true, false, fg);
        }
    }
}

void icons_output(lv_obj_t *c, int oy, const char *label, bool connected) {
    lv_color_t fg = LVGL_FOREGROUND;
    icons_text(c, oy, 34, 4, STRIP_W - 35, label, fg, LV_TEXT_ALIGN_RIGHT);
    if (!connected) {
        icons_rect(c, oy, 42, 8, STRIP_W - 44, 1, 0, true, false, fg); // struck through
    }
}

/* Two 9x10 boxes (the halves, as tall as the battery) at x 43 and 57, joined by a 2 px bar.
 * Linked: the boxes filled and the bar whole. Not linked: outlined, the bar broken in the middle. */
#define LINK_LEFT_X 43
#define LINK_RIGHT_X 57
#define LINK_BOX_W 9

void icons_split_link(lv_obj_t *c, int oy, bool connected) {
    lv_color_t fg = LVGL_FOREGROUND;
    icons_rect(c, oy, LINK_LEFT_X, 3, LINK_BOX_W, 10, 2, connected, true, fg);
    icons_rect(c, oy, LINK_RIGHT_X, 3, LINK_BOX_W, 10, 2, connected, true, fg);
    int bar_x = LINK_LEFT_X + LINK_BOX_W, bar_w = LINK_RIGHT_X - bar_x;
    if (connected) {
        icons_rect(c, oy, bar_x, 7, bar_w, 2, 0, true, false, fg);
    } else {
        icons_rect(c, oy, bar_x, 7, 1, 2, 0, true, false, fg);
        icons_rect(c, oy, LINK_RIGHT_X - 1, 7, 1, 2, 0, true, false, fg);
    }
}
