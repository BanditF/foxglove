/*
 * PC link: the host panel on the left screen and art pushes to the right half.
 * SPDX-License-Identifier: MIT
 *
 * Raw HID reports are 32 bytes; byte 0 picks the protocol, byte 1 the command. Ours don't collide
 * with KeyPeek (0xFF, 0xF1), VIA (0x01-0x14, 0xFE) or zzeneg's hid host (0xAA-0xB1, 0xCC-0xCD).
 * Replies come back as input reports with the same byte 0 and byte 1 = command | 0x80.
 *
 * 0xD5 host panel, host -> keyboard:
 *   01 HELLO   ver                  reply: ver, panel w, h, x, y, right link, right battery %,
 *                                   relay discovery result (int8), discovery tries
 *   02 FILL    colour (0 or 1)      fill the back buffer
 *   03 ROWS    y, n (1-3), n rows   write rows of HOST_PANEL_STRIDE bytes, MSB first, 1 = ink
 *   04 PRESENT                      show the back buffer
 *   05 RELEASE                      give the area back to the WPM chart now
 * The panel takes the area at the first PRESENT and keeps the last presented frame until RELEASE.
 * With CONFIG_NICE_VIEW_HOST_PANEL_TIMEOUT_S > 0, the WPM chart also comes back after that many
 * seconds without a 0xD5 report.
 *
 * 0xD6 right-hand art (see art_relay.h for the blob), host -> keyboard:
 *   01 BEGIN   len u16, crc32 u32   start receiving an art blob on the left
 *   02 DATA    offset u16, n, n bytes (n <= 27)
 *   03 COMMIT                       check the CRC, then push the blob to the right half
 *   04 SELECT  set (int8, -1 = random built-in)  show a built-in set again
 *   05 STATUS
 * Every command but DATA replies with the relay status (art_relay_central.c); DATA replies only
 * on error.
 */

#pragma once

#include <stdbool.h>
#include <stdint.h>

#define HOST_LINK_PANEL 0xD5
#define HOST_LINK_ART 0xD6
#define HOST_LINK_VERSION 1

// The panel covers the WPM block of the portrait layout (status.c), inside the frame.
#define HOST_PANEL_X 2
#define HOST_PANEL_Y 106
#define HOST_PANEL_W 64
#define HOST_PANEL_H 52
#define HOST_PANEL_STRIDE (HOST_PANEL_W / 8)
#define HOST_PANEL_BYTES (HOST_PANEL_STRIDE * HOST_PANEL_H)

// Copies the shown panel into buf and returns true while the panel holds the area.
bool host_panel_get(uint8_t buf[HOST_PANEL_BYTES]);

// Redraws the left screen; status.c, called on the display work queue.
void host_panel_redraw(void);
