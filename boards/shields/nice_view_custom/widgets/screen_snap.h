/* Screen readback, native LVGL panel coordinates. SPDX-License-Identifier: MIT */
#pragma once
#include <stdbool.h>
#include <stdint.h>
#if __has_include(<lvgl.h>) // not in builds without a display (the ZMK simulator)
#include <lvgl.h>
#endif

#define SCREEN_SNAP_W 160
#define SCREEN_SNAP_H 68
#define SCREEN_SNAP_BYTES 1360
#define SCREEN_SNAP_HEADER 6
#define SCREEN_SNAP_WIRE (SCREEN_SNAP_HEADER + SCREEN_SNAP_BYTES)
enum screen_snap_status {
    SCREEN_OK, SCREEN_BUSY, SCREEN_NO_LINK, SCREEN_TIMEOUT, SCREEN_NOT_BUILT,
    SCREEN_BAD_REQUEST, SCREEN_NONE,
};
/* Called only on the display queue. Returns false before the widget exists. */
bool screen_snap_pixels(uint8_t out[SCREEN_SNAP_BYTES]);
#if __has_include(<lvgl.h>)
void screen_snap_canvas(uint8_t *out, const lv_color_t *pixels, int x);
#endif
void screen_snap_pixel(uint8_t *out, int x, int y, bool ink);
uint8_t screen_snap_local(void);
/* Copies only frozen bytes; safe on the Bluetooth RX thread. */
int screen_snap_wire_read(void *out, uint16_t len, uint16_t offset);
#if !defined(CONFIG_ZMK_SPLIT) || defined(CONFIG_ZMK_SPLIT_ROLE_CENTRAL)
void screen_snap_command(const uint8_t *data, int len);
void host_link_screen_reply(const uint8_t *data, int len);
void screen_snap_complete(uint8_t half, uint8_t status);
void screen_snap_remote_data(uint16_t offset, const void *data, uint16_t len);
bool screen_snap_remote_valid(void);
uint8_t art_relay_screen_snap(void);
#endif
