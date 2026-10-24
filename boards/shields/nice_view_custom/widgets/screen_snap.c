/* Frozen screen images; no work or allocation until requested. SPDX-License-Identifier: MIT */
#include <string.h>
#include <zephyr/kernel.h>
#include <zephyr/spinlock.h>
#include <zephyr/sys/byteorder.h>
#include <zephyr/sys/crc.h>
#include <zmk/display.h>
#include "screen_snap.h"

#if IS_ENABLED(CONFIG_NICE_VIEW_HOST_LINK) && \
    (!IS_ENABLED(CONFIG_ZMK_SPLIT) || IS_ENABLED(CONFIG_ZMK_SPLIT_ROLE_CENTRAL))
#define SNAP_CENTRAL 1
#endif
struct snapshot {
    uint8_t wire[SCREEN_SNAP_WIRE];
    bool busy;
    bool valid;
};
static struct snapshot local;
#ifdef SNAP_CENTRAL
static struct snapshot remote;
#endif
static struct k_spinlock snap_lock;

void screen_snap_pixel(uint8_t *out, int x, int y, bool ink) {
    uint8_t mask = 0x80 >> (x & 7);
    if (ink) {
        out[y * 20 + x / 8] |= mask;
    } else {
        out[y * 20 + x / 8] &= ~mask;
    }
}

void screen_snap_canvas(uint8_t *out, const uint8_t *pixels, int ox) {
    // The canvases are 8-bit grey (CANVAS_COLOR_FORMAT, LVGL 9); dark is ink.
    const uint32_t stride = lv_draw_buf_width_to_stride(68, LV_COLOR_FORMAT_L8);
    for (int y = 0; y < 68; y++) {
        for (int x = MAX(0, -ox); x < MIN(68, 160 - ox); x++) {
            screen_snap_pixel(out, ox + x, y, pixels[y * stride + x] < 128);
        }
    }
}

static void capture(struct k_work *work) {
    bool ok = screen_snap_pixels(local.wire + SCREEN_SNAP_HEADER);
    if (ok) {
        sys_put_le16(sys_get_le16(local.wire) + 1, local.wire);
        sys_put_le32(crc32_ieee(local.wire + SCREEN_SNAP_HEADER, SCREEN_SNAP_BYTES),
                     local.wire + 2);
    }
    k_spinlock_key_t key = k_spin_lock(&snap_lock);
    local.valid = ok;
    local.busy = false;
    k_spin_unlock(&snap_lock, key);
#ifdef SNAP_CENTRAL
    screen_snap_complete(0, ok ? SCREEN_OK : SCREEN_NOT_BUILT);
#endif
}
static K_WORK_DEFINE(capture_work, capture);

uint8_t screen_snap_local(void) {
    if (!zmk_display_is_initialized()) {
        return SCREEN_NOT_BUILT;
    }
    k_spinlock_key_t key = k_spin_lock(&snap_lock);
    bool busy = local.busy;
    if (!busy) {
        local.busy = true;
    }
    k_spin_unlock(&snap_lock, key);
    if (busy) {
        return SCREEN_BUSY;
    }
    if (k_work_submit_to_queue(zmk_display_work_q(), &capture_work) < 0) {
        key = k_spin_lock(&snap_lock);
        local.busy = false;
        k_spin_unlock(&snap_lock, key);
        return SCREEN_NOT_BUILT;
    }
    return SCREEN_OK;
}

int screen_snap_wire_read(void *out, uint16_t len, uint16_t offset) {
    k_spinlock_key_t key = k_spin_lock(&snap_lock);
    int n = -EAGAIN;
    if (!local.busy && local.valid && offset <= SCREEN_SNAP_WIRE) {
        n = MIN(len, SCREEN_SNAP_WIRE - offset);
        memcpy(out, local.wire + offset, n);
    }
    k_spin_unlock(&snap_lock, key);
    return n;
}

#ifdef SNAP_CENTRAL
void screen_snap_complete(uint8_t half, uint8_t status) {
    struct snapshot *s = half ? &remote : &local;
    uint8_t r[14] = {0xd7, 0x86, status, half};
    k_spinlock_key_t key = k_spin_lock(&snap_lock);
    if (half) {
        if (status == SCREEN_OK) {
            s->valid = true;
        }
        s->busy = false;
    }
    if (status == SCREEN_OK) {
        memcpy(r + 4, s->wire, 2);
        r[6] = SCREEN_SNAP_W;
        r[7] = SCREEN_SNAP_H;
        sys_put_le16(SCREEN_SNAP_BYTES, r + 8);
        memcpy(r + 10, s->wire + 2, 4);
    }
    k_spin_unlock(&snap_lock, key);
    host_link_screen_reply(r, sizeof(r));
}

void screen_snap_remote_data(uint16_t offset, const void *data, uint16_t len) {
    k_spinlock_key_t key = k_spin_lock(&snap_lock);
    if (remote.busy && offset + len <= SCREEN_SNAP_WIRE) {
        remote.valid = false;
        memcpy(remote.wire + offset, data, len);
    }
    k_spin_unlock(&snap_lock, key);
}

bool screen_snap_remote_valid(void) {
    return crc32_ieee(remote.wire + SCREEN_SNAP_HEADER, SCREEN_SNAP_BYTES) ==
           sys_get_le32(remote.wire + 2);
}

void screen_snap_command(const uint8_t *d, int len) {
    uint8_t half = len >= 3 ? d[2] : 0xff;
    uint8_t status = SCREEN_BAD_REQUEST;
    bool snap = d[1] == 6;
    uint16_t offset = len >= 5 ? sys_get_le16(d + 3) : 0;
    uint8_t r[CONFIG_RAW_HID_REPORT_SIZE] = {0xd7, 0x87, 0, half};
    if (half <= 1 && len >= (snap ? 3 : 5) && (snap || offset <= SCREEN_SNAP_BYTES)) {
        struct snapshot *s = half ? &remote : &local;
        if (snap && !half) {
            status = screen_snap_local();
            if (status == SCREEN_OK) {
                return;
            }
        } else {
            k_spinlock_key_t key = k_spin_lock(&snap_lock);
            status = s->busy ? SCREEN_BUSY : SCREEN_OK;
            if (snap && status == SCREEN_OK) {
                s->busy = true;
            } else if (!snap && status == SCREEN_OK) {
                status = s->valid ? SCREEN_OK : SCREEN_NONE;
                if (status == SCREEN_OK) {
                    r[6] = MIN(SCREEN_SNAP_BYTES - offset, sizeof(r) - 7);
                    memcpy(r + 7, s->wire + SCREEN_SNAP_HEADER + offset, r[6]);
                }
            }
            k_spin_unlock(&snap_lock, key);
            if (snap && status == SCREEN_OK) {
#if IS_ENABLED(CONFIG_NICE_VIEW_ART_RELAY)
                status = art_relay_screen_snap();
#else
                status = SCREEN_NOT_BUILT;
#endif
                if (status == SCREEN_OK) {
                    return;
                }
                screen_snap_complete(half, status);
                return;
            }
        }
    }
    if (snap) {
        /* A rejected request must not clear another request's busy state. */
        uint8_t error[14] = {0xd7, 0x86, status, half};
        host_link_screen_reply(error, sizeof(error));
    } else {
        r[2] = status;
        sys_put_le16(offset, r + 4);
        host_link_screen_reply(r, 7 + r[6]);
    }
}
#endif
