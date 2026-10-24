/*
 * The PC's panel on a board's own display (foxglove_link shield, #160/#75): host_link.c's front
 * buffer shown as a 1-bit image on LVGL's top layer, over the board's widgets, while the panel is
 * alive; hidden otherwise. Runs on the display work queue (host_link.c's redraw()).
 * SPDX-License-Identifier: MIT
 */

#include <string.h>

#include <lvgl.h>
#include <zephyr/kernel.h>

#include "host_link.h"

// Indexed 1 bit: an 8-byte palette (0 = paper, 1 = ink), then rows of HOST_PANEL_STRIDE bytes,
// MSB first: the host link's own format, so the buffer is copied as is.
static uint8_t image[8 + HOST_PANEL_BYTES] = {0xff, 0xff, 0xff, 0xff, 0x00, 0x00, 0x00, 0xff};
static const lv_img_dsc_t image_dsc = {.header.cf = LV_COLOR_FORMAT_I1,
                                       .header.w = HOST_PANEL_W,
                                       .header.h = HOST_PANEL_H,
                                       .data_size = sizeof(image),
                                       .data = image};
static lv_obj_t *panel;

void host_panel_redraw(void) {
    bool alive = host_panel_get(&image[8]);
    if (panel == NULL) {
        if (!alive) {
            return; // made the first time the PC presents a frame; nothing at boot
        }
        panel = lv_image_create(lv_layer_top());
        lv_image_set_src(panel, &image_dsc);
        lv_obj_set_pos(panel, HOST_PANEL_X, HOST_PANEL_Y);
    }
    if (alive) {
        lv_obj_clear_flag(panel, LV_OBJ_FLAG_HIDDEN);
        lv_image_cache_drop(&image_dsc);
        lv_obj_invalidate(panel);
    } else {
        lv_obj_add_flag(panel, LV_OBJ_FLAG_HIDDEN);
    }
}
