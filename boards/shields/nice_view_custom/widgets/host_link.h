/*
 * PC link: the host panel on the left screen and art pushes to the right half.
 * SPDX-License-Identifier: MIT
 *
 * Raw HID reports are 32 bytes; byte 0 picks the protocol, byte 1 the command. Ours don't collide
 * with KeyPeek (0xFF, 0xF1), VIA (0x01-0x14, 0xFE) or zzeneg's hid host (0xAA-0xB1, 0xCC-0xCD).
 * 0xD7-0xDF are reserved for host-link protocols (0xD7 is settings; 0xD8-0xDF unassigned).
 * Replies come back as input reports with the same byte 0 and byte 1 = command | 0x80.
 *
 * 0xD5 host panel, host -> keyboard:
 *   01 HELLO   ver                  reply: ver, panel w, h, x, y, right link, right battery %,
 *                                   relay discovery result (int8), discovery tries,
 *                                   10 relay debug bytes, capabilities u32 LE (report bytes 21-24),
 *                                   panel timeout seconds u16 LE (bytes 25-26; 0 = never)
 *      Capabilities: bit 0 panel, 1 art relay, 2 settings/RGB, 3 unsolicited art STATUS,
 *                    4 settings CONN_STATE, 5 POSITION (only when test hooks are built),
 *                    6 screen snapshots (both commands).
 *      Old firmware pads appended fields with zero; version and prior fields stay unchanged.
 *      After host traffic, a right battery change sends an unsolicited HELLO with the same payload.
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
 * on error. Once a host has sent any host-link report since boot, relay discovery completion
 * and loss of a ready relay also send 0xD6 0x85, with exactly the STATUS reply payload.
 * These best-effort events use the same reply queue; hosts may still request STATUS to resync.
 *
 * 0xD7 keyboard settings, host -> keyboard; saved on the keyboard, so the PC needn't resend them:
 *   01 RGB_SYNC  0 = off, 1 = on, 0xff = just read     reply: sync (0/1, 0xff = not built in),
 *                                                     left underglow on, right link
 *      On: the right half's underglow follows the left's on/off (default). Off: each half turns
 *      its underglow off on its own idle.
 *   02 INVOKE    behavior local id u16, param1 u32, param2 u32, hold ms u16 (0 = tap)
 *                                                     reply: status, local id u16
 *      Presses and releases the binding as a key would, on the left; allowed global behaviors
 *      (&rgb_ug, ...) reach the right half too. The ids are ZMK Studio's local ids.
 *      hold ms keeps it pressed that long. Status:
 *      0 ok, 1 unknown id, 2 busy (a held invoke is still running), 3 behavior failed,
 *      4 not built in (no local ids), 5 bad length, 6 refused.
 *      Refused before scheduling: &bt (BT_SEL/BT_CLR/BT_CLR_ALL/BT_NXT/BT_PRV/BT_DISC),
 *      &out (all commands), &bootloader, &sys_reset and &soft_off. Matches resolved device
 *      names of enabled zmk,behavior-{bluetooth,outputs,reset,soft-off} nodes, including
 *      renamed instances; all Bluetooth parameters are refused.
 *   03 RGB_STATE                                      reply: left struct rgb_idle_wire (7 B),
 *                                                     right age s (0xff = none), right (7 B)
 *      The right's state comes from the last relay status read (each request asks for a new one),
 *      so poll it; age says how old it is.
 *   04 CONN_STATE  no payload (request D7 04)          reply: D7 84, active BLE profile index u8,
 *                                                     connected (0/1), bonded (0/1),
 *                                                     selected endpoint (0 USB, 1 BLE),
 *                                                     USB HID ready (0/1), BLE profile count u8
 *      Bonded means the active profile is not open. Endpoint is the currently selected transport,
 *      not the preferred transport. Without BLE, index/connected/bonded/count are zero; without
 *      USB, ready is zero. State is sampled on request only; no boot work or unsolicited replies.
 *   05 POSITION  byte 0 D7, byte 1 05, byte 2 pos u8, byte 3 state u8 (1 down, 0 up)
 *                reply: byte 0 D7, byte 1 85, byte 2 status u8, byte 3 pos u8
 *      Positions are keymap indices 0..ZMK_KEYMAP_LEN-1 (48). Remaining report bytes are padding.
 *      Status: 0 ok, 2 busy/no relay link, 3 relay timeout (3 s), 4 not built in,
 *      5 short payload/invalid position/state, 6 refused (same devices as INVOKE).
 *      Active layers resolve top-down through &trans, stopping at the default layer.
 *      Left events run on the system queue; right events go over ART_RELAY_PRESS then
 *      return through ZMK's normal split link. Duplicate downs and unowned ups are no-op ok.
 *      Owned downs release after 10 s or after 10 s without a host-link report.
 *      CONFIG_FOXGLOVE_TEST_HOOKS=n omits the handler and returns status 4.
 *      Replies use the existing nonblocking queue and are zero-padded to the report size.
 *   06 SCREEN_SNAP half u8 (0 left, 1 right), request bytes 0..2 = D7 06 half.
 *      Reply bytes: 0 D7, 1 86, 2 status, 3 half, 4..5 frame u16, 6 w (160), 7 h (68),
 *                   8..9 bytes u16 (1360), 10..13 crc32 u32. All multibyte fields are LE.
 *      Metadata is zero on error. Frame increments per captured image (wraps at 65536).
 *      CRC is crc32_ieee over the 1360 image bytes. Right reply is asynchronous, after relay
 *      transfer and CRC verification; USB never waits for the other half.
 *   07 SCREEN_READ half u8, offset u16 LE, request bytes 0..4 = D7 07 half offset.
 *      Reply: 0 D7, 1 87, 2 status, 3 half, 4..5 echoed offset u16 LE, 6 n u8, 7.. data[n].
 *      n = min(1360-offset, report size-7); offset=1360 succeeds with n=0; larger is invalid.
 *      Reads the frozen local cache, including for half 1. Error replies have n=0.
 *      Status (both): 0 ok, 1 busy (retry), 2 no right relay/screen link, 3 right timeout
 *      (3 seconds including queue wait), 4 not built in, 5 bad request, 6 no snapshot yet (READ).
 *      Capture/transfer makes READ busy. A failed partial transfer invalidates the cache;
 *      a request rejected before copying pixels preserves the previous snapshot.
 *      Image: native 160x68 panel coordinates, row-major, 20 bytes/row, MSB first, 1=black ink,
 *      0=white, after widget inversion. Native top is y=0. On both portrait-mounted halves,
 *      rotate the returned image 90 degrees counterclockwise for the person's 68x160 view:
 *      portrait (x,y) = (native y,159-native x). Battery/status is at the portrait top.
 *      Left joins the three already rotated TRUE_COLOR canvases (including the displayed host
 *      panel). Right overlays the INDEXED_1BIT art, using its palette, on the status canvas.
 *      Both captures run on zmk_display_work_q(), without touching LVGL from USB/BT RX.
 */

#pragma once

#include <stdbool.h>
#include <stdint.h>

#define HOST_LINK_PANEL 0xD5
#define HOST_LINK_ART 0xD6
#define HOST_LINK_SETTINGS 0xD7
#define HOST_LINK_VERSION 1

#define HOST_LINK_CAP_PANEL (1U << 0)
#define HOST_LINK_CAP_ART_RELAY (1U << 1)
#define HOST_LINK_CAP_SETTINGS_RGB (1U << 2)
#define HOST_LINK_CAP_ART_STATUS_EVENT (1U << 3)
#define HOST_LINK_CAP_CONN_STATE (1U << 4)
#define HOST_LINK_CAP_POSITION (1U << 5)
#define HOST_LINK_CAP_SCREEN (1U << 6)

// Nonblocking POSITION response; also used by the test-hooks worker.
void host_link_position_reply(uint8_t status, uint8_t pos);

// Called by relay connection/discovery callbacks; queues a report only after host traffic.
void host_link_art_status(void);

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
