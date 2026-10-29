/* relay-sim: the fake split link (sim_bt.c). SPDX-License-Identifier: MIT */
#pragma once

#include "zephyr_sim.h"

// Zephyr 3.5 with ZMK's split central defaults: BT_L2CAP_TX_BUF_COUNT 5 (ZMK, central), so
// BT_CONN_TX_MAX 5 and ATT's TX meta pool 5; MTU 23; ZMK's 7.5 ms connection interval.
#define SIM_TX_BUFS 5
#define SIM_TX_CTX 5
#define SIM_MTU 23
#define SIM_CONN_INTERVAL_US 7500
// ZMK_SPLIT_BLE_PREF_LATENCY (ZMK v0.3.0 default): the right may sleep through 30 events in a row.
#define SIM_PERIPH_LATENCY 30
#define SIM_PDUS_PER_EVENT 4
#define SIM_ATT_TIMEOUT_US 30000000LL

struct sim_bt_stats {
    int requests_sent;    // ATT requests the left put on the air
    int responses;        // responses the left got
    int att_timeouts;     // ATT bearer timeouts on the left (30 s, the bearer then is dead)
    int stuck_requests;   // requests left queued with nothing on the air (sent from the syswq
                          // without a TX context; Zephyr retries only on the next sent PDU)
    int cmds_sent;        // write commands put on the air
    int link_drops;
    int notifies_dropped; // notifications the left had no subscription for
    int rings;            // rings the right sent
    int rsp_dropped;      // responses the right couldn't send ("already processing a REQ/RSP")
    int screen_pushes;    // screen images the right pushed as notifications
    int screen_chunks;    // their chunks
};
extern struct sim_bt_stats sim_bt_stats;

// Faults a scenario can arm.
struct sim_bt_faults {
    // The right answers the next request matching (op, any if 0) this late (us); -1 never.
    int late_op;
    int64_t late_us;
    int late_count;
    uint8_t late_err; // the late response carries this ATT error (0: none)
    // The right's system work queue is busy this long after each redraw (wake, SNAP), and its
    // response buffer is freed only there (Zephyr 3.5 rsp_meta): a request in that window gets no
    // response at all.
    bool rsp_meta_rule;
    int64_t right_redraw_busy_us;
    // From this time until the link next drops, nothing the left sends goes on the air (its
    // requests get no response and no ATT timeout, since that starts only once a request is
    // sent), while the right's PDUs still arrive: HELLO's "in flight 1, timeouts 1" with the
    // bearer alive, which only a re-link ended (#618 soak and gate runs). 0 = off.
    int64_t left_tx_stall_at_us;
    // The right listens at every connection event (no peripheral latency). Off by default: the
    // real link has ZMK's latency 30, which the old relay-sim didn't model (#662's d265853).
    bool no_latency;
    // The right's art animates every right_frame_us (default 250 ms) while lit; each frame holds
    // its system work queue right_frame_busy_us (0: not at all).
    int64_t right_frame_us;
    int64_t right_frame_busy_us;
    // A right before the screen push (#662): it refuses SNAP with the push byte (ATT 0x0d) and
    // only answers reads.
    bool right_no_push;
};
extern struct sim_bt_faults sim_bt_faults;

void sim_bt_init(void);
// Starts the left TX stall now (sim_bt_faults.left_tx_stall_at_us).
void sim_bt_stall_left_tx(void);
// Brings the link up at once (as after both halves booted and bonded): connected, encrypted
// after secure_after_us.
void sim_bt_connect(int64_t secure_after_us);
// Drops the link (as the guard or a re-link would); it comes back after back_after_us.
void sim_bt_drop(int64_t back_after_us);
bool sim_bt_att_alive(void);
int sim_bt_queued_requests(void);
bool sim_bt_request_on_air(void);

/* The fake right half */
struct sim_right {
    int8_t set;
    uint8_t state;
    uint16_t received;
    uint16_t blob_len;
    uint32_t blob_crc;
    bool rgb_on;     // the left's on/off as the right last took it (RGB op)
    bool follow;
    uint8_t left_activity; // ACTIVITY op
    bool lit;
    uint8_t look[5];
    uint32_t redraws;
    int snap_ready;   // 0 none, 1 capturing, 2 ready
    uint32_t frame;   // the right's art frame counter (it animates while lit)
    uint16_t ccc_cfg; // the left's CCC entry (bonded: kept over links)
    uint16_t ccc_value;
    int pushes_ok;
    int polarity;        // the PANEL_POLARITY op's value the right last took (#666), -1 none
    int polarity_writes; // PANEL_POLARITY ops it took
};
extern struct sim_right sim_right;
