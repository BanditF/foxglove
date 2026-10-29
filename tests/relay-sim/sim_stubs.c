/*
 * relay-sim: the left half around art_relay_central.c (host_link.c, rgb_idle.c, battery_dim.c,
 * screen_snap.c, ZMK's activity), as far as the relay calls into them.
 * SPDX-License-Identifier: MIT
 */
#include <stdio.h>

#include "sim_stubs.h"

#include "art_relay.h"
#include "host_link.h"
#include "panel_polarity.h"
#include "screen_snap.h"

struct sim_left sim_left = {.on = true, .activity = ZMK_ACTIVITY_ACTIVE, .look = {23, 0, 100, 42, 3}};

enum zmk_activity_state zmk_activity_get_state(void) { return sim_left.activity; }

int rgb_idle_get_on(bool *on) {
    *on = sim_left.on && sim_left.activity == ZMK_ACTIVITY_ACTIVE;
    return 0;
}
int rgb_idle_relay_on(bool *on, bool *own) { // the RGB op's on and byte 13 (#609)
    *own = sim_left.on;
    return rgb_idle_get_on(on);
}
bool rgb_idle_sync(void) { return true; }
#ifdef RGB_IDLE_LOOK_SIZE
void rgb_idle_look(uint8_t out[RGB_IDLE_LOOK_SIZE]) { memcpy(out, sim_left.look, 5); }
#endif
void battery_dim_get_config(struct battery_dim_config *out) {
    struct battery_dim_config c = {1, 100, 80, 12, 20};
    *out = c;
}
uint32_t effect_run_clock(void) { return (uint32_t)k_uptime_get(); }
void panel_polarity_relay_op(uint8_t out[PANEL_POLARITY_RELAY_SIZE]) {
    out[0] = PANEL_POLARITY_RELAY_OP;
    out[1] = sim_left.polarity;
}

void host_link_art_status(void) { sim_left.art_status_events++; }
void host_link_rgb_right_changed(void) { sim_left.rgb_events++; }
void host_link_rgb_right_followed(void) { sim_left.rgb_followed++; }
void host_link_battery_dim_changed(void) { sim_left.dim_events++; }
void host_link_send(const uint8_t *data, int len) {
    (void)data;
    (void)len;
    sim_left.sends++;
}
void host_link_build_info_reply(uint8_t status, uint8_t half,
                                const struct foxglove_build_info *info) {
    (void)status;
    (void)half;
    (void)info;
}

/* screen_snap.c's side of the right's readback */
static uint8_t snap_wire[SCREEN_SNAP_WIRE];
void screen_snap_complete(uint8_t half, uint8_t status) {
    (void)half;
    sim_left.snap_status = status;
    sim_left.snap_done = true;
    if (status == SCREEN_OK) {
        sim_left.snap_frame = sys_get_le16(snap_wire);
    }
}
void screen_snap_remote_data(uint16_t offset, const void *data, uint16_t len) {
    if (offset + len <= sizeof(snap_wire)) {
        memcpy(snap_wire + offset, data, len);
    }
}
bool screen_snap_remote_valid(void) {
    return crc32_ieee(snap_wire + SCREEN_SNAP_HEADER, SCREEN_SNAP_BYTES) ==
           sys_get_le32(snap_wire + 2);
}

/* Test hooks (dev builds); the relay's press and boot jobs answer here. */
void test_hooks_relay_done(uint8_t status) { (void)status; }
void test_hooks_boot_relay_done(uint8_t status) { (void)status; }
