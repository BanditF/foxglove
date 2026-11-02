/*
 * relay-sim scenarios: the real art_relay_central.c (and its screen_snap_relay.inc) on the
 * simulated kernel and split link, driven the way foxglove-lab and the Foxglove app drive the
 * left half over USB. One scenario per run: relay-sim <scenario>. Prints one RESULT line and
 * exits 0 when every check passed, 1 when one failed.
 * SPDX-License-Identifier: MIT
 */
#include <stdarg.h>
#include <stdio.h>
#include <stdlib.h>

#include "art_relay.h"
#if __has_include("health.h") // bisect.ps1 builds widgets from before #705 too
#include "health.h"
#define SIM_HEALTH 1
#endif
#include "panel_polarity.h"
#include "screen_snap.h"
#include "sim_bt.h"
#include "sim_kernel.h"
#include "sim_stubs.h"

extern int (*const sim_sys_init_art_relay_init)(void);
extern int sim_log_level;

static char results[1024];
static bool failed;

static void check(const char *name, bool ok, const char *fmt, ...) {
    char detail[256] = "";
    va_list ap;
    va_start(ap, fmt);
    vsnprintf(detail, sizeof(detail), fmt, ap);
    va_end(ap);
    printf("%s %-22s %s (t=%.1f s)\n", ok ? "PASS" : "FAIL", name, detail, sim_now_us() / 1e6);
    size_t n = strlen(results);
    snprintf(results + n, sizeof(results) - n, " %s=%s", name, ok ? "PASS" : "FAIL");
    failed |= !ok;
}

static void debug_line(const char *when) {
    uint8_t d[ART_RELAY_DEBUG_SIZE];
    art_relay_debug(d);
    printf("  HELLO relay debug %s: [%d,%d,%d,%d,%d,%d,%d,%d,%d,%d]; link %d; sim: %d requests "
           "sent, %d answered, %d ATT timeouts, %d stuck in att->reqs, %d queued now, %s, %d "
           "link drops, %d rings, %d notifies dropped, %d responses dropped\n",
           when, d[0], d[1], (int8_t)d[2], d[3], d[4], d[5], d[6], d[7], d[8], d[9],
           art_relay_link(), sim_bt_stats.requests_sent, sim_bt_stats.responses,
           sim_bt_stats.att_timeouts, sim_bt_stats.stuck_requests, sim_bt_queued_requests(),
           sim_bt_request_on_air() ? "one on the air" : "none on the air",
           sim_bt_stats.link_drops, sim_bt_stats.rings, sim_bt_stats.notifies_dropped,
           sim_bt_stats.rsp_dropped);
}

/* What the PC does over USB (host_link.c calls these on its thread) */

struct report {
    uint8_t state, error;
    uint16_t len, pushed;
    uint8_t link;
    struct art_relay_status right;
};

static struct report status(void) {
    uint8_t out[ART_RELAY_REPORT_SIZE];
    art_relay_report(out);
    struct report r;
    r.state = out[0];
    r.error = out[1];
    r.len = sys_get_le16(&out[2]);
    r.pushed = sys_get_le16(&out[4]);
    r.link = out[6];
    memcpy(&r.right, &out[7], sizeof(r.right));
    return r;
}

static void wait_relay(int max_s) {
    for (int i = 0; i < max_s * 10 && art_relay_link() != 2; i++) {
        k_msleep(100);
    }
}

// D6 04, then STATUS until it settles, as foxglove-lab's art_select (within 10 s).
static bool lab_select(int8_t set, char *why, size_t n) {
    if (art_relay_select(set) != 0) {
        snprintf(why, n, "refused (busy)");
        return false;
    }
    int64_t start = k_uptime_get();
    while (k_uptime_get() - start < 10000) {
        k_msleep(100);
        struct report r = status();
        if (r.state == ART_LEFT_DONE) {
            snprintf(why, n, "done in %lld ms, right set %d", (long long)(k_uptime_get() - start),
                     r.right.set);
            return r.right.set == set;
        }
        if (r.state == ART_LEFT_FAILED) {
            snprintf(why, n, "failed after %lld ms, error %d", (long long)(k_uptime_get() - start),
                     r.error);
            return false;
        }
    }
    snprintf(why, n, "still pushing after 10 s");
    return false;
}

// BEGIN, DATA (27 bytes a report), COMMIT, then STATUS every push_poll_ms (the lab: 500) for 60 s.
static int push_poll_ms = 500;
static int64_t push_took_ms; // COMMIT to DONE or FAILED, as the last lab_push saw it
static bool lab_push(uint16_t len, char *why, size_t n) {
    static uint8_t blob[4096];
    for (int i = 0; i < len; i++) {
        blob[i] = (uint8_t)(i * 13 + 7);
    }
    int pushes_before = sim_right.pushes_ok;
    if (art_relay_begin(len, crc32_ieee(blob, len)) != 0) {
        snprintf(why, n, "BEGIN refused");
        return false;
    }
    for (int at = 0; at < len; at += 27) {
        art_relay_data(at, &blob[at], (uint8_t)MIN(27, len - at));
        k_msleep(1);
    }
    if (art_relay_commit() != 0) {
        snprintf(why, n, "COMMIT refused");
        return false;
    }
    int64_t start = k_uptime_get();
    while (k_uptime_get() - start < 60000) {
        k_msleep(push_poll_ms);
        struct report r = status();
        if (r.state == ART_LEFT_DONE || r.state == ART_LEFT_FAILED) {
            push_took_ms = k_uptime_get() - start;
            snprintf(why, n, "%s after %lld ms, %d/%d bytes pushed, error %d",
                     r.state == ART_LEFT_DONE ? "done" : "failed",
                     (long long)(k_uptime_get() - start), r.pushed, r.len, r.error);
            return r.state == ART_LEFT_DONE && sim_right.pushes_ok == pushes_before + 1;
        }
    }
    struct report r = status();
    snprintf(why, n, "still pushing after 60 s, %d/%d bytes", r.pushed, r.len);
    return false;
}

// D7 06 half 1 as foxglove-lab's snapshot: BUSY is asked again every 100 ms (50 times), a relay
// timeout is taken once more. The reply comes from the firmware (screen_snap_complete).
enum { SNAP_OK, SNAP_TIMEOUT, SNAP_BUSY, SNAP_OTHER };
static int snapshot_once(void) {
    for (int tries = 0; tries < 50; tries++) {
        sim_left.snap_done = false;
        uint8_t s = art_relay_screen_snap();
        if (s == SCREEN_BUSY) {
            k_msleep(100);
            continue;
        }
        if (s != SCREEN_OK) {
            return SNAP_OTHER;
        }
        int64_t start = k_uptime_get();
        while (!sim_left.snap_done && k_uptime_get() - start < 6000) {
            k_msleep(10);
        }
        if (!sim_left.snap_done) {
            return SNAP_TIMEOUT;
        }
        return sim_left.snap_status == SCREEN_OK      ? SNAP_OK
               : sim_left.snap_status == SCREEN_TIMEOUT ? SNAP_TIMEOUT
                                                        : SNAP_OTHER;
    }
    return SNAP_BUSY;
}

static int snapshot(uint16_t *frame) {
    int r = snapshot_once();
    if (r == SNAP_TIMEOUT) {
        printf("  lab: right screen snapshot: right half timed out; taking it once more\n");
        r = snapshot_once();
    }
    *frame = sim_left.snap_frame;
    return r;
}

static const char *snap_name(int r) {
    static const char *n[] = {"ok", "right half timed out", "still busy after retries", "error"};
    return n[r];
}

// A key on the left after idle, or the left going idle: ZMK's activity, then rgb_idle.
static void left_activity(enum zmk_activity_state s) {
    sim_left.activity = s;
    art_relay_activity_kick();
    art_relay_rgb_kick();
}

static void rgb_state(bool *right_on, bool *shown_valid, bool *shown_lit) {
    struct rgb_idle_wire w;
    uint8_t age = art_relay_right_rgb(&w);
    *right_on = age != 0xff && w.on;
#ifdef RGB_SHOWN_LIT
    struct rgb_shown_wire s;
    *shown_valid = art_relay_right_shown(&s);
    *shown_lit = *shown_valid && (s.flags & RGB_SHOWN_LIT);
#else
    *shown_valid = *shown_lit = false; // a left before #605 has no shown block
#endif
}

/* The app's background traffic (Foxglove running): STATUS and RGB_STATE about once a second,
 * the right's dim state every few seconds (D7 0E with refresh). */
static volatile bool app_on;
static void app_thread(void *arg) {
    (void)arg;
    for (int i = 0;; i++) {
        k_msleep(1000);
        if (!app_on) {
            continue;
        }
        status();
        bool a, b, c;
        rgb_state(&a, &b, &c);
        if (i % 3 == 0) {
            struct battery_dim_wire d;
            art_relay_right_dim(&d, true);
        }
    }
}

/* Scenarios */

static const char *scenario;

// resilience: right-follows-left-activity, then the pushes (r2b-r2d) and a SELECT, Foxglove quit.
// With stall, the left's TX stalls at the wake (sim_bt_faults.left_tx_stall_at_us): the checks
// after it give the relay 45 s to come back on its own, as nothing on the PC side can.
static void wake_and_push(bool app, bool stall) {
    wait_relay(40);
    check("relay-up", art_relay_link() == 2, "link %d", art_relay_link());
    app_on = app;
    char why[160];
    check("select-3", lab_select(3, why, sizeof(why)), "%s", why);
    left_activity(ZMK_ACTIVITY_ACTIVE);
    k_msleep(400);
    bool on, valid, lit;
    rgb_state(&on, &valid, &lit);
    // The left idles: both dark; the right's art holds still.
    k_msleep(1000);
    left_activity(ZMK_ACTIVITY_IDLE);
    k_msleep(3000);
    uint16_t f1 = 0, f2 = 0;
    int s1 = snapshot(&f1);
    k_msleep(1400);
    int s2 = snapshot(&f2);
    check("snapshot-idle", s1 == SNAP_OK && s2 == SNAP_OK && f1 == f2, "%s, %s; frames %u %u",
          snap_name(s1), snap_name(s2), f1, f2);
    // A key on the left: the right lights and its art moves again.
    if (stall) {
        sim_bt_stall_left_tx();
    }
    left_activity(ZMK_ACTIVITY_ACTIVE);
    k_msleep(250);
    rgb_state(&on, &valid, &lit);
    int s3 = SNAP_OK;
    uint16_t f3 = f2;
    int64_t start = k_uptime_get();
    while (k_uptime_get() - start < 10000) {
        s3 = snapshot(&f3);
        if (s3 != SNAP_OK || f3 != f2) {
            break;
        }
        k_msleep(300);
    }
    if (!stall) {
        check("snapshot-wake", s3 == SNAP_OK && f3 != f2, "%s; frame %u -> %u after %lld ms",
              snap_name(s3), f2, f3, (long long)(k_uptime_get() - start));
    } else {
        printf("  lab: snapshot during the stall: %s\n", snap_name(s3));
    }
    debug_line("after the snapshots");
    if (stall) {
        // Nothing on the PC side can end the stall; the relay must come back on its own.
        uint8_t d[ART_RELAY_DEBUG_SIZE];
        int64_t t0 = k_uptime_get();
        for (;;) {
            art_relay_debug(d);
            if ((art_relay_link() == 2 && d[6] == 0 && sim_bt_att_alive() &&
                 sim_bt_faults.left_tx_stall_at_us == 0) ||
                k_uptime_get() - t0 > 45000) {
                break;
            }
            k_msleep(500);
        }
        check("relay-recovers", art_relay_link() == 2 && d[6] == 0 &&
                                    sim_bt_faults.left_tx_stall_at_us == 0,
              "link %d, in flight %d, timeouts %d, %s, after %lld ms", art_relay_link(), d[6],
              d[4], sim_bt_faults.left_tx_stall_at_us ? "the left's TX still stalled" : "re-linked",
              (long long)(k_uptime_get() - t0));
#ifdef SIM_HEALTH
        // The self-report (#705) says so after the re-link, where HELLO's per-link debug can't:
        // the timeouts and the unstick since boot, the drop and the second time the relay was found.
        struct health_wire h;
        health_fill(&h);
        check("health-after-stall",
              h.relay_timeouts >= 1 && h.relay_unsticks >= 1 && h.split_drops >= 1 &&
                  h.relay_ready >= 2 && h.split_connected == 1,
              "timeouts %u, unsticks %u, split drops %u, relay found %u times, connected %u",
              h.relay_timeouts, h.relay_unsticks, h.split_drops, h.relay_ready, h.split_connected);
#endif
        debug_line("after the wait");
    }
    check("push", lab_push(3599, why, sizeof(why)), "%s", why);
    check("push-2", lab_push(3190, why, sizeof(why)), "%s", why);
    check("select-0", lab_select(0, why, sizeof(why)), "%s", why);
    uint16_t f4;
    int s4 = snapshot(&f4);
    check("snapshot-end", s4 == SNAP_OK, "%s", snap_name(s4));
    debug_line("at the end");
}

// One request of the background answered late (after our 3 s timeout): user commands right then.
static void late_answer(void) {
    wait_relay(40);
    check("relay-up", art_relay_link() == 2, "link %d", art_relay_link());
    k_msleep(2000);
    sim_bt_faults.late_op = 0;
    sim_bt_faults.late_us = 4500000;
    sim_bt_faults.late_count = 1;
    status(); // queues a status read: its answer comes 4.5 s late
    k_msleep(3200);
    debug_line("with the late request out");
    char why[160];
    check("select-late", lab_select(5, why, sizeof(why)), "%s", why);
    check("push-late", lab_push(3599, why, sizeof(why)), "%s", why);
    uint16_t f;
    int s = snapshot(&f);
    check("snapshot-late", s == SNAP_OK, "%s", snap_name(s));
    debug_line("at the end");
}

// A request's answer comes after the left gave up on it and went on with the next one (#618):
// that answer must be dropped, never taken for the next request's. The late answer is an ATT
// error, so a left that matched it to the SELECT queued next would fail that SELECT; and it comes
// well before the unstick, so the link stays up.
static void late_ignored(void) {
    wait_relay(40);
    check("relay-up", art_relay_link() == 2, "link %d", art_relay_link());
    char why[160];
    check("select-3", lab_select(3, why, sizeof(why)), "%s", why);
    k_msleep(2000);
    int drops = sim_bt_stats.link_drops;
    sim_bt_faults.late_op = 0;
    sim_bt_faults.late_us = 4500000;
    sim_bt_faults.late_err = BT_ATT_ERR_UNLIKELY;
    sim_bt_faults.late_count = 1;
    status(); // queues a status read: its answer comes 4.5 s late, an error
    k_msleep(3200); // the left gives up on it after 3 s
    debug_line("with the late request let go");
    uint8_t d[ART_RELAY_DEBUG_SIZE];
    art_relay_debug(d);
    check("slot-let-go", d[4] == 1 && d[6] == 1, "timeouts %d, held %d", d[4], d[6]);
    // The SELECT goes at once on the other slot and queues behind the late read on the link; the
    // late error lands while it waits.
    check("select-5", lab_select(5, why, sizeof(why)), "%s", why);
    k_msleep(2000);
    struct report r = status();
    k_msleep(500);
    r = status();
    art_relay_debug(d);
    check("late-dropped", r.right.set == 5 && sim_right.set == 5 && d[6] == 0,
          "left sees set %d, right on %d, held %d", r.right.set, sim_right.set, d[6]);
    check("push-after", lab_push(3599, why, sizeof(why)), "%s", why);
    check("no-relink", sim_bt_stats.link_drops == drops, "%d link drops",
          sim_bt_stats.link_drops - drops);
    debug_line("at the end");
}

// A request the right never answers (until the bearer times out): what's left afterwards.
static void lost_answer(void) {
    wait_relay(40);
    check("relay-up", art_relay_link() == 2, "link %d", art_relay_link());
    k_msleep(2000);
    sim_bt_faults.late_op = 0;
    sim_bt_faults.late_us = -1;
    sim_bt_faults.late_count = 1;
    status();
    k_msleep(3200);
    char why[160];
    check("select-lost", lab_select(5, why, sizeof(why)), "%s", why);
    debug_line("after the select");
    k_msleep(30000);
    check("push-after-att-timeout", lab_push(3599, why, sizeof(why)), "%s", why);
    debug_line("at the end");
}

// rgb-state-follows-the-leds after a re-link: the right's shown block must come back.
static void relink_shown(void) {
    wait_relay(40);
    bool on, valid, lit;
    k_msleep(1000);
    rgb_state(&on, &valid, &lit);
    printf("  first link: right shown block %s\n", valid ? "present" : "missing");
    sim_bt_drop(2000000);
    k_msleep(3000);
    wait_relay(40);
    k_msleep(2000);
    left_activity(ZMK_ACTIVITY_ACTIVE); // the lab's tap: no change on the right (already lit)
    int64_t start = k_uptime_get();
    while (k_uptime_get() - start < 3000) {
        rgb_state(&on, &valid, &lit);
        if (valid && lit && on) {
            break;
        }
        k_msleep(100);
    }
    check("shown-after-relink", valid && lit && on, "right on %d, shown %s, lit %d", on,
          valid ? "present" : "missing", lit);
    debug_line("at the end");
}

// The resilience suite's order: pushes (r2b-r2d), a push with the split link dropped mid-relay
// (r5a), then rgb-state-follows-the-leds: tap the left, RGB_STATE must show the right's shown
// block (lit) within the step's 3 s.
static void push_relink_tap(void) {
    wait_relay(40);
    char why[160];
    check("push", lab_push(3599, why, sizeof(why)), "%s", why);
    bool on, valid, lit;
    rgb_state(&on, &valid, &lit);
    printf("  after the push: right shown block %s\n", valid ? "present" : "missing");
    // r5a: BEGIN, DATA, COMMIT, and the link drops while the left pushes.
    static uint8_t blob[3599];
    art_relay_begin(sizeof(blob), crc32_ieee(blob, sizeof(blob)));
    for (int at = 0; at < (int)sizeof(blob); at += 27) {
        art_relay_data(at, &blob[at], (uint8_t)MIN(27, (int)sizeof(blob) - at));
    }
    art_relay_commit();
    k_msleep(50);
    sim_bt_drop(2000000);
    k_msleep(3000);
    wait_relay(40);
    check("relay-back", art_relay_link() == 2, "link %d", art_relay_link());
    k_msleep(2000); // the case's own steps before its tap
    left_activity(ZMK_ACTIVITY_ACTIVE);
    int64_t start = k_uptime_get();
    while (k_uptime_get() - start < 3000) {
        rgb_state(&on, &valid, &lit);
        if (on && valid && lit) {
            break;
        }
        k_msleep(100);
    }
    check("rgb-state-shown", on && valid && lit, "right on %d, shown %s, lit %d after %lld ms", on,
          valid ? "present" : "missing", lit, (long long)(k_uptime_get() - start));
    debug_line("at the end");
}

// right-art-select-golden with the app running (#662): art.select set 0, a snapshot, set 1, a
// snapshot. The right's ATT server follows Zephyr 3.5's response-buffer rule (a request that
// comes before the right's system queue has run its last response's sent callback gets no
// response at all), and that queue is busy for 20 ms after each redraw (SELECT, SNAP). A left that
// sends its next request the moment an answer comes (the SELECT's status read, the first read
// after the SNAP write, each Read Blob of the image) loses it: on hardware, "right half timed
// out", then "still busy after retries" and the unstick's link drop (5a50426 gate).
static void select_snap(void) {
    wait_relay(40);
    check("relay-up", art_relay_link() == 2, "link %d", art_relay_link());
    app_on = true;
    k_msleep(2000);
    sim_bt_faults.rsp_meta_rule = true;
    sim_bt_faults.right_redraw_busy_us = 20000; // more than a request turnaround (2 events)
    int drops = sim_bt_stats.link_drops;
    char why[160];
    int64_t worst = 0;
    for (int8_t set = 0; set < 2; set++) {
        char name[24];
        snprintf(name, sizeof(name), "select-%d", set);
        check(name, lab_select(set, why, sizeof(why)), "%s", why);
        uint16_t f;
        int64_t t0 = k_uptime_get();
        int r = snapshot(&f);
        int64_t took = k_uptime_get() - t0;
        worst = MAX(worst, took);
        snprintf(name, sizeof(name), "snapshot-set%d", set);
        check(name, r == SNAP_OK, "%s in %lld ms", snap_name(r), (long long)took);
    }
    check("snapshot-budget", worst < 5000, "slowest %lld ms (firmware budget 5 s, lab waits 6 s)",
          (long long)worst);
    check("no-relink", sim_bt_stats.link_drops == drops, "%d link drops",
          sim_bt_stats.link_drops - drops);
    check("none-dropped", sim_bt_stats.rsp_dropped == 0, "%d requests the right didn't answer",
          sim_bt_stats.rsp_dropped);
    debug_line("at the end");
}

// The snapshot under ZMK's real link timing (#662's d265853 hardware failure): 7.5 ms interval,
// peripheral latency 30 (the right listens after an event with data, when it has data, else every
// 31st event), the right's response-buffer rule, its system queue busy 20 ms after each redraw and
// 20 ms on every art frame (10 per second), the app's traffic. d265853 read the image with ~63
// Read Blob requests spaced REQ_GAP_MS: the right fell asleep between them, each waited for its
// next listen, and every snapshot ran past the 5 s budget ("right half timed out"). With push the
// right sends the image as notifications. Passes when every snapshot is OK within 40% of the
// budget, nothing went unanswered and the link never dropped; prints the push time of an art blob.
static void snap_latency(bool old_right) {
    wait_relay(40);
    check("relay-up", art_relay_link() == 2, "link %d", art_relay_link());
    app_on = true;
    k_msleep(2000);
    sim_bt_faults.rsp_meta_rule = true;
    sim_bt_faults.right_redraw_busy_us = 20000;
    sim_bt_faults.right_frame_us = 100000;
    sim_bt_faults.right_frame_busy_us = 20000;
    sim_bt_faults.right_no_push = old_right;
    int drops = sim_bt_stats.link_drops;
    char why[160];
    int64_t worst = 0;
    for (int i = 0; i < 4; i++) {
        char name[24];
        if (i < 2) {
            snprintf(name, sizeof(name), "select-%d", i);
            check(name, lab_select((int8_t)i, why, sizeof(why)), "%s", why);
        } else {
            k_msleep(700); // the right animates between snapshots
        }
        uint16_t f;
        int64_t t0 = k_uptime_get();
        int r = snapshot(&f);
        int64_t took = k_uptime_get() - t0;
        worst = MAX(worst, took);
        snprintf(name, sizeof(name), "snapshot-%d", i);
        check(name, r == SNAP_OK, "%s in %lld ms (simulated)", snap_name(r), (long long)took);
    }
    // The firmware's budget is 5 s (SCREEN_BUDGET_MS); a push should take well under half of it.
    check("snapshot-time", worst < 2000,
          "slowest %lld ms simulated, budget 5000 ms, limit 2000 ms (40%%); lab waits 6 s",
          (long long)worst);
    push_poll_ms = 10;
    for (int i = 0; i < 2; i++) {
        uint16_t len = i ? 3190 : 3599;
        check(i ? "push-2" : "push", lab_push(len, why, sizeof(why)), "%u B: %s", len, why);
        printf("  art push of %u bytes under latency: %lld ms from COMMIT to DONE (simulated)\n",
               len, (long long)push_took_ms);
    }
    check("select-end", lab_select(2, why, sizeof(why)), "%s", why);
    check("no-relink", sim_bt_stats.link_drops == drops, "%d link drops",
          sim_bt_stats.link_drops - drops);
    check("none-dropped", sim_bt_stats.rsp_dropped == 0, "%d requests the right didn't answer",
          sim_bt_stats.rsp_dropped);
    printf("  sim: %d screen pushes, %d chunks\n", sim_bt_stats.screen_pushes,
           sim_bt_stats.screen_chunks);
    debug_line("at the end");
}

// Waits up to max_ms for the right to have polarity p and the left to report it acknowledged.
static bool polarity_reaches(uint8_t p, int max_ms, int64_t *took) {
    int64_t start = k_uptime_get();
    while (k_uptime_get() - start < max_ms) {
        if (sim_right.polarity == p && art_relay_right_polarity() == p) {
            *took = k_uptime_get() - start;
            return true;
        }
        k_msleep(20);
    }
    *took = k_uptime_get() - start;
    return false;
}

// The panel polarity (#666): the left sends the right its value on link-up, on each change (the
// PC's D7 12: panel_polarity_set kicks the relay), and again after a re-link, newest wins; a flip
// made while the link is down reaches the right when it comes back. With the app's traffic, the
// right's response-buffer rule and its busy system queue after each redraw (select-snap's
// conditions, #662): no request may go unanswered and the link must not drop.
static void polarity_relink(void) {
    wait_relay(40);
    check("relay-up", art_relay_link() == 2, "link %d", art_relay_link());
    int64_t took;
    bool ok = polarity_reaches(PANEL_POLARITY_DARK, 3000, &took);
    check("sent-on-link-up", ok, "right %d, left sees %d, after %lld ms", sim_right.polarity,
          art_relay_right_polarity(), (long long)took);
    app_on = true;
    k_msleep(2000);
    sim_bt_faults.rsp_meta_rule = true;
    sim_bt_faults.right_redraw_busy_us = 20000;
    int drops = sim_bt_stats.link_drops;

    sim_left.polarity = PANEL_POLARITY_LIGHT;
    art_relay_polarity_kick();
    ok = polarity_reaches(PANEL_POLARITY_LIGHT, 3000, &took);
    check("flip-light", ok, "right %d after %lld ms", sim_right.polarity, (long long)took);

    // A burst of flips, a SELECT in the middle: the newest value wins, every request answered.
    for (int i = 0; i < 4; i++) {
        sim_left.polarity = (uint8_t)(i % 2 == 0 ? PANEL_POLARITY_LIGHT : PANEL_POLARITY_DARK);
        art_relay_polarity_kick();
        k_msleep(5);
    }
    char why[160];
    check("select-during", lab_select(2, why, sizeof(why)), "%s", why);
    ok = polarity_reaches(PANEL_POLARITY_DARK, 3000, &took);
    check("burst-newest-wins", ok, "right %d, %d writes", sim_right.polarity,
          sim_right.polarity_writes);
    check("no-relink", sim_bt_stats.link_drops == drops, "%d link drops",
          sim_bt_stats.link_drops - drops);
    check("none-dropped", sim_bt_stats.rsp_dropped == 0, "%d requests the right didn't answer",
          sim_bt_stats.rsp_dropped);

    // The link drops; the PC flips to light meanwhile (the kick finds no link); the re-link sends
    // the newest value, and the left says unknown until the right has it.
    sim_bt_faults.rsp_meta_rule = false;
    sim_bt_drop(2000000);
    k_msleep(100);
    check("unknown-while-down", art_relay_right_polarity() == PANEL_POLARITY_UNKNOWN, "left sees %d",
          art_relay_right_polarity());
    sim_left.polarity = PANEL_POLARITY_LIGHT;
    art_relay_polarity_kick();
    int writes = sim_right.polarity_writes;
    k_msleep(3000);
    wait_relay(40);
    check("relay-back", art_relay_link() == 2, "link %d", art_relay_link());
    ok = polarity_reaches(PANEL_POLARITY_LIGHT, 3000, &took);
    check("resent-on-relink", ok && sim_right.polarity_writes == writes + 1,
          "right %d, %d write(s) since the drop, after %lld ms", sim_right.polarity,
          sim_right.polarity_writes - writes, (long long)took);

    // A re-link with no change still sends it once (a right that rebooted meanwhile, #666).
    writes = sim_right.polarity_writes;
    sim_bt_drop(2000000);
    k_msleep(3000);
    wait_relay(40);
    ok = polarity_reaches(PANEL_POLARITY_LIGHT, 3000, &took);
    check("resent-unchanged", ok && sim_right.polarity_writes == writes + 1, "%d write(s)",
          sim_right.polarity_writes - writes);
    k_msleep(3000);
    check("then-quiet", sim_right.polarity_writes == writes + 1, "%d write(s) 3 s later",
          sim_right.polarity_writes - writes);
    debug_line("at the end");
}

/* The art playlist (#522) and art across firmware updates (#521). STORE (D6 09) keeps a piece
 * in one of the right's flash places without showing it: the playlist stages its next piece in a
 * place it was lent ahead of the step, and the step is then a SELECT of that place; after an
 * update Foxglove puts the places the keyboard lost back the same way. */

static const char *right_state_name(uint8_t s) {
    static const char *n[] = {"built-in", "receiving", "playing pushed", "bad CRC", "bad blob",
                              "too big",  "saving",    "saved",          "save failed"};
    return s < ARRAY_SIZE(n) ? n[s] : "?";
}

static uint8_t piece[4096];
static void make_piece(uint16_t len, uint8_t seed) {
    for (int i = 0; i < len; i++) {
        piece[i] = (uint8_t)(i * 13 + 7 + seed * 29);
    }
}

static void name16(uint8_t out[16], const char *name) {
    memset(out, 0, 16);
    memcpy(out, name, MIN(strlen(name), 16));
}

// Whether place `slot` holds the piece made from (len, seed) under `name`, as the right has it.
static bool place_holds(uint8_t slot, uint16_t len, uint8_t seed, const char *name) {
    make_piece(len, seed);
    const struct sim_flash_slot *f = &sim_right.flash[slot];
    return f->used && f->len == len && f->crc == crc32_ieee(piece, len) &&
           !strncmp(f->name, name, 16);
}

// Ends a SAVE or STORE as the app's art::finish_slot_op does: STATUS every 250 ms until the left
// is done and the right has left SAVING, at most 20 s. The right state at the end.
static bool finish_slot_op(int64_t start, struct report *r) {
    while (k_uptime_get() - start < 20000) {
        k_msleep(250);
        *r = status();
        if (r->state == ART_LEFT_FAILED) {
            return false;
        }
        if (r->state == ART_LEFT_DONE && r->right.state != ART_RIGHT_SAVING) {
            return r->right.state != ART_RIGHT_SAVE_FAILED;
        }
    }
    return false;
}

// D6 01, 02 x n, 09 as the app's art::store sends them, then finish_slot_op. Passes when the
// left is done, the right didn't fail it, and the place holds these very bytes.
static int64_t store_took_ms;
static bool lab_store(uint8_t slot, uint16_t len, uint8_t seed, const char *name, char *why,
                      size_t n) {
    make_piece(len, seed);
    uint8_t nm[16];
    name16(nm, name);
    int64_t start = k_uptime_get();
    if (art_relay_begin(len, crc32_ieee(piece, len)) != 0) {
        snprintf(why, n, "BEGIN refused");
        return false;
    }
    for (int at = 0; at < len; at += 27) {
        art_relay_data(at, &piece[at], (uint8_t)MIN(27, len - at));
        k_msleep(1);
    }
    struct report r = {0};
    bool ok = art_relay_store(slot, nm) == 0 && finish_slot_op(start, &r);
    if (!ok && r.state == 0) {
        r = status();
    }
    store_took_ms = k_uptime_get() - start;
    bool landed = place_holds(slot, len, seed, name);
    snprintf(why, n, "%s in %lld ms (left error %d, right %s), place %u %s", ok ? "done" : "failed",
             (long long)store_took_ms, r.error, right_state_name(r.right.state), slot,
             landed ? "holds it" : "doesn't hold it");
    return ok && landed;
}

// D6 06 SAVE of the pushed set on screen (the keep path before STORE), then finish_slot_op.
static bool lab_save(uint8_t slot, const char *name, char *why, size_t n) {
    uint8_t nm[16];
    name16(nm, name);
    int64_t start = k_uptime_get();
    struct report r = {0};
    bool ok = art_relay_save(slot, nm) == 0 && finish_slot_op(start, &r);
    snprintf(why, n, "%s in %lld ms, right %s", ok ? "saved" : "failed",
             (long long)(k_uptime_get() - start), right_state_name(r.right.state));
    return ok && sim_right.flash[slot].used;
}

// D6 07 SLOT as the app reads a place back (art::slot, 3 s): used, length and name.
static bool lab_slot(uint8_t slot, bool *used, uint16_t *len, char name[17]) {
    int sends = sim_left.sends;
    if (art_relay_slot(slot) != 0) {
        return false;
    }
    int64_t start = k_uptime_get();
    while (sim_left.sends == sends && k_uptime_get() - start < 3000) {
        k_msleep(10);
    }
    const uint8_t *r = sim_left.last_send;
    if (sim_left.sends == sends || r[1] != 0x87 || r[2] != 0 || r[3] != slot) {
        return false;
    }
    *used = r[4];
    *len = sys_get_le16(&r[9]);
    memcpy(name, &r[11], 16);
    name[16] = 0;
    return true;
}

// The art the right shows didn't change: its set and no push played.
static bool screen_kept(int8_t set, int pushes) {
    return sim_right.set == set && sim_right.pushes_ok == pushes;
}

// The playlist (#522) with two places lent to it, under the link's real timing (latency 30, the
// right's response-buffer rule and its busy system queue, the app's traffic): two pieces staged
// ahead, then four steps, each a SELECT of the staged place followed by staging the next piece
// into the other place. The screen changes only at the SELECTs; a STORE into the place on screen
// is refused and leaves the screen and the link alone.
static void playlist_store(void) {
    wait_relay(40);
    check("relay-up", art_relay_link() == 2, "link %d", art_relay_link());
    app_on = true;
    k_msleep(2000);
    sim_bt_faults.rsp_meta_rule = true;
    sim_bt_faults.right_redraw_busy_us = 20000;
    int drops = sim_bt_stats.link_drops;
    char why[200];
    const uint8_t lent[2] = {6, 7};
    const uint16_t lens[] = {3599, 3190, 2048, 3999, 3333, 2777};
    const char *names[] = {"radar", "orbit", "uplink", "skyline", "vitals", "signal"};
    int8_t shown = sim_right.set;
    int pushes = sim_right.pushes_ok;
    // Ahead of the first step: piece 0 into the first lent place, the screen untouched.
    bool ok = lab_store(lent[0], lens[0], 0, names[0], why, sizeof(why));
    check("stage-ahead", ok && screen_kept(shown, pushes), "%s; screen on set %d", why,
          sim_right.set);
    int64_t worst = store_took_ms, worst_step = 0;
    // Each step: a SELECT of the staged place, then the next piece into the other one.
    for (int step = 0; step < 5; step++) {
        uint8_t on = lent[step % 2], other = lent[(step + 1) % 2];
        char name[24];
        snprintf(name, sizeof(name), "step-%d", step);
        int64_t t0 = k_uptime_get();
        ok = lab_select((int8_t)(1 + on), why, sizeof(why)); // set_count 1 + place
        int64_t took = k_uptime_get() - t0;
        worst_step = MAX(worst_step, took);
        check(name, ok && place_holds(on, lens[step], (uint8_t)step, names[step]), "%s", why);
        shown = sim_right.set;
        pushes = sim_right.pushes_ok;
        snprintf(name, sizeof(name), "stage-next-%d", step);
        int next = step + 1;
        ok = lab_store(other, lens[next], (uint8_t)next, names[next], why, sizeof(why));
        check(name, ok && screen_kept(shown, pushes), "%s; screen on set %d", why, sim_right.set);
        worst = MAX(worst, store_took_ms);
    }
    printf("  playlist: slowest stage %lld ms, slowest step %lld ms (simulated)\n",
           (long long)worst, (long long)worst_step);
    check("stage-budget", worst < 4000,
          "slowest stage %lld ms (staged right after a step; the next is a minute away at least)",
          (long long)worst);
    check("step-budget", worst_step < 1500, "slowest step %lld ms", (long long)worst_step);
    // The place on screen is never written: the right refuses, the screen and the link stay.
    uint8_t on = (uint8_t)(sim_right.set - 1);
    bool refused = !lab_store(on, 1024, 9, "nope", why, sizeof(why));
    check("store-on-screen-refused",
          refused && screen_kept(shown, pushes) && sim_right.flash[on].crc != 0 &&
              !place_holds(on, 1024, 9, "nope"),
          "%s", why);
    check("store-after-refusal", lab_store(lent[0] == on ? lent[1] : lent[0], 2222, 10, "after", why,
                                           sizeof(why)),
          "%s", why);
    // The places read back as the app reads them.
    bool used;
    uint16_t len;
    char name[17];
    bool read = lab_slot(on, &used, &len, name);
    check("place-reads-back", read && used && len == sim_right.flash[on].len &&
                                  !strncmp(name, sim_right.flash[on].name, 16),
          "place %u: %s, %u bytes, \"%s\"", on, used ? "used" : "free", len, read ? name : "?");
    check("no-relink", sim_bt_stats.link_drops == drops, "%d link drops",
          sim_bt_stats.link_drops - drops);
    check("none-dropped", sim_bt_stats.rsp_dropped == 0, "%d requests the right didn't answer",
          sim_bt_stats.rsp_dropped);
    printf("  sim: %d stores, %d saves on the right; place writes:", sim_right.stores_ok,
           sim_right.saves_ok);
    for (int i = 0; i < ART_RELAY_FLASH_SLOTS; i++) {
        printf(" %u", sim_right.flash[i].writes);
    }
    printf("\n");
    debug_line("at the end");
}

// A right before STORE (status version 9): the left fails it before sending anything, so the
// screen and the link are untouched, and the keep path that shows the piece (push, SAVE) works
// as before: what Foxglove falls back to there.
static void playlist_store_old(void) {
    sim_right.version = ART_RELAY_STORE_STATUS_VERSION - 1;
    wait_relay(40);
    check("relay-up", art_relay_link() == 2, "link %d", art_relay_link());
    app_on = true;
    k_msleep(2000);
    int drops = sim_bt_stats.link_drops;
    int requests = sim_bt_stats.requests_sent;
    int8_t shown = sim_right.set;
    int pushes = sim_right.pushes_ok;
    char why[200];
    bool ok = lab_store(6, 3599, 0, "radar", why, sizeof(why));
    struct report r = status();
    check("old-right-refused", !ok && r.error == ART_ERR_RIGHT && screen_kept(shown, pushes) &&
                                   sim_right.received == 0,
          "%s; left error %d, right received %u bytes", why, r.error, sim_right.received);
    check("refused-fast", store_took_ms < 1000, "%lld ms, %d requests meanwhile (the app's)",
          (long long)store_took_ms, sim_bt_stats.requests_sent - requests);
    check("fallback-push", lab_push(3599, why, sizeof(why)), "%s", why);
    check("fallback-save", lab_save(6, "radar", why, sizeof(why)), "%s", why);
    check("no-relink", sim_bt_stats.link_drops == drops, "%d link drops",
          sim_bt_stats.link_drops - drops);
    debug_line("at the end");
}

// Art across a firmware update (#521): three kept places, then the right restarts with new
// firmware. An update keeps them (they live above the image); one that wiped them (a settings
// reset) is restored with STORE, the screen staying on the right's own art, and the link drops
// once in the middle of the restore: the first pass restores 2 of 3, the next (on the relay
// coming up) the last one. Foxglove's health row says "restored N of M" from the reads back.
static int count_restored(const uint8_t *places, const uint16_t *lens, const char **names,
                          int m) {
    int n = 0;
    for (int i = 0; i < m; i++) {
        bool used;
        uint16_t len;
        char name[17];
        if (lab_slot(places[i], &used, &len, name) && used && len == lens[i] &&
            !strncmp(name, names[i], 16) && place_holds(places[i], lens[i], (uint8_t)(20 + i),
                                                        names[i])) {
            n++;
        }
    }
    return n;
}

static void restore_update(void) {
    wait_relay(40);
    check("relay-up", art_relay_link() == 2, "link %d", art_relay_link());
    app_on = true;
    k_msleep(2000);
    char why[200];
    const uint8_t places[] = {0, 1, 2};
    const uint16_t lens[] = {3599, 3190, 2048};
    const char *names[] = {"fury", "radar", "gem"};
    const int m = 3;
    for (int i = 0; i < m; i++) {
        char name[24];
        snprintf(name, sizeof(name), "keep-%d", i);
        check(name, lab_store(places[i], lens[i], (uint8_t)(20 + i), names[i], why, sizeof(why)),
              "%s", why);
    }
    // An update that keeps the places: nothing to restore.
    sim_bt_right_update(false, 3000000);
    k_msleep(3000);
    wait_relay(40);
    check("relay-back", art_relay_link() == 2, "link %d", art_relay_link());
    k_msleep(1000);
    struct report r = status();
    k_msleep(500);
    r = status();
    int kept = count_restored(places, lens, names, m);
    check("update-keeps-places", kept == m && (r.right.flash_used & 0x7) == 0x7,
          "%d of %d still there, used bits 0x%02x", kept, m, r.right.flash_used);

    // One that wipes them: the left reports them free; Foxglove stores each one back.
    sim_bt_right_update(true, 3000000);
    k_msleep(3000);
    wait_relay(40);
    check("relay-back-2", art_relay_link() == 2, "link %d", art_relay_link());
    k_msleep(1000);
    status();
    k_msleep(500);
    r = status();
    check("places-lost", (r.right.flash_used & 0x7) == 0, "used bits 0x%02x",
          r.right.flash_used);
    int8_t shown = sim_right.set;
    int pushes = sim_right.pushes_ok;
    int drops = sim_bt_stats.link_drops;
    // First pass: the link drops while the second piece goes.
    for (int i = 0; i < m; i++) {
        bool ok;
        if (i == 1) {
            make_piece(lens[i], (uint8_t)(20 + i));
            uint8_t nm[16];
            name16(nm, names[i]);
            art_relay_begin(lens[i], crc32_ieee(piece, lens[i]));
            for (int at = 0; at < lens[i]; at += 27) {
                art_relay_data(at, &piece[at], (uint8_t)MIN(27, lens[i] - at));
            }
            art_relay_store(places[i], nm);
            k_msleep(30);
            sim_bt_drop(2000000);
            struct report q;
            ok = finish_slot_op(k_uptime_get(), &q) && place_holds(places[i], lens[i],
                                                                   (uint8_t)(20 + i), names[i]);
            printf("  restore %s with the link dropped: %s\n", names[i], ok ? "landed" : "lost");
            k_msleep(3000);
            wait_relay(40);
        } else {
            ok = lab_store(places[i], lens[i], (uint8_t)(20 + i), names[i], why, sizeof(why));
            printf("  restore %s: %s\n", names[i], why);
        }
    }
    int first = count_restored(places, lens, names, m);
    printf("  art restored %d/%d after the first pass\n", first, m);
    check("first-pass-partial", first == m - 1 && sim_bt_stats.link_drops == drops + 1,
          "restored %d/%d, %d link drop(s)", first, m, sim_bt_stats.link_drops - drops);
    // The relay came up again: the missing one goes.
    for (int i = 0; i < m; i++) {
        if (!place_holds(places[i], lens[i], (uint8_t)(20 + i), names[i])) {
            char name[24];
            snprintf(name, sizeof(name), "retry-%d", i);
            check(name, lab_store(places[i], lens[i], (uint8_t)(20 + i), names[i], why,
                                  sizeof(why)),
                  "%s", why);
        }
    }
    int all = count_restored(places, lens, names, m);
    printf("  art restored %d/%d\n", all, m);
    check("restored-all", all == m, "art restored %d/%d", all, m);
    check("screen-untouched", screen_kept(shown, pushes), "set %d, %d pushes played",
          sim_right.set, sim_right.pushes_ok - pushes);
    check("none-dropped", sim_bt_stats.rsp_dropped == 0, "%d requests the right didn't answer",
          sim_bt_stats.rsp_dropped);
    debug_line("at the end");
}

#ifdef SIM_HEALTH
// D7 13 HEALTH half 1 as host_link.c starts it: the relay read, then the reply it sends.
static int health_right(struct health_wire *out, int64_t *took) {
    int before = sim_left.health_replies;
    int64_t t0 = k_uptime_get();
    if (art_relay_health_read() != 0) {
        return 1; // busy
    }
    while (sim_left.health_replies == before && k_uptime_get() - t0 < 20000) {
        k_msleep(10);
    }
    *took = k_uptime_get() - t0;
    if (sim_left.health_replies == before) {
        return -1;
    }
    if (sim_left.health_status == 0) {
        art_relay_right_health((uint8_t *)out, sizeof(*out));
    }
    return sim_left.health_status;
}

static const char *state_name(uint8_t r) {
    static const char *names[] = {"pending", "pass", "fail", "n/a"};
    return names[r & 3];
}

// The self-report (#705): each counter driven by its real path, read the way the PC reads it.
// The right comes up 70 s after boot, past CONFIG_FOXGLOVE_HEALTH_RELAY_S (60): the boot self-test
// says pending, then failed (no link), then failed (found late). Then the right's block over the
// relay under ZMK's latency and the right's response-buffer rule (spaced reads, nothing dropped),
// a forced relay timeout, a push the right refuses and one it takes, a right without the block,
// and a re-link.
static void health_scenario(void) {
    struct health_wire h;
    health_fill(&h);
    check("block", h.version == HEALTH_VERSION && h.half == 0 && h.length == HEALTH_SIZE &&
                       (h.flags & HEALTH_F_RELAY_CENTRAL),
          "version %u, half %u, length %u, flags 0x%02x", h.version, h.half, h.length, h.flags);
    uint8_t relay = h.selftest[HEALTH_CHECK_RELAY];
    check("selftest-pending", relay == HEALTH_RESULT(HEALTH_PENDING, 0), "relay check %s code %u",
          state_name(relay), relay >> 2);
    k_msleep(61000);
    health_fill(&h);
    relay = h.selftest[HEALTH_CHECK_RELAY];
    check("selftest-no-link", relay == HEALTH_RESULT(HEALTH_FAIL, 2), "relay check %s code %u",
          state_name(relay), relay >> 2);
    k_msleep(9000);
    sim_bt_connect(500000); // the right switched on 70 s after the left
    wait_relay(40);
    check("relay-up", art_relay_link() == 2, "link %d", art_relay_link());
    health_fill(&h);
    relay = h.selftest[HEALTH_CHECK_RELAY];
    check("selftest-late", relay == HEALTH_RESULT(HEALTH_FAIL, 1) && h.relay_ready == 1 &&
                               h.split_connected == 1 && h.split_up_at_s >= 70,
          "relay check %s code %u, found %u, connected %u, link up at %u s", state_name(relay),
          relay >> 2, h.relay_ready, h.split_connected, h.split_up_at_s);
    check("debug-folded-in", h.relay_discovery == ART_DISCOVERY_DONE && h.relay_debug[9] == 0 &&
                                 h.relay_debug[8] == SIM_MTU,
          "discovery %d, MTU %u", h.relay_discovery, h.relay_debug[8] | h.relay_debug[9] << 8);

    // The right's block, spaced: the response-buffer rule and a busy right (select-snap's).
    app_on = true;
    k_msleep(2000);
    sim_bt_faults.rsp_meta_rule = true;
    sim_bt_faults.right_redraw_busy_us = 20000;
    int drops = sim_bt_stats.link_drops;
    int sent = sim_bt_stats.requests_sent;
    struct health_wire r;
    int64_t took;
    int st = health_right(&r, &took);
    check("right-block", st == 0 && r.version == HEALTH_VERSION && r.half == 1 &&
                             r.length == HEALTH_SIZE && r.relay_ready == 1 && r.leds == 21,
          "status %d in %lld ms, version %u, half %u, found %u, LEDs %u", st, (long long)took,
          r.version, r.half, r.relay_ready, r.leds);
    check("right-block-time", took < 5000, "%lld ms simulated (the lab waits 8 s)",
          (long long)took);
    printf("  health read of the right: %d requests (the app's included)\n",
           sim_bt_stats.requests_sent - sent);

    // A forced relay timeout: a status read answered 4.5 s late.
    health_fill(&h);
    uint16_t timeouts = h.relay_timeouts;
    sim_bt_faults.late_op = 0;
    sim_bt_faults.late_us = 4500000;
    sim_bt_faults.late_count = 1;
    status();
    k_msleep(3300);
    health_fill(&h);
    check("relay-timeout", h.relay_timeouts == timeouts + 1, "timeouts %u -> %u", timeouts,
          h.relay_timeouts);
    k_msleep(2000); // the late answer comes and frees its slot

    // A push the right refuses, then one it takes.
    char why[160];
    uint16_t ok0 = h.art_ok, failed0 = h.art_failed;
    sim_bt_faults.right_reject_commit = true;
    bool pushed = lab_push(3599, why, sizeof(why));
    health_fill(&h);
    check("art-push-failed", !pushed && h.art_failed == failed0 + 1 &&
                                 h.art_last_error == ART_ERR_RIGHT,
          "%s; failed %u -> %u, last error %u", why, failed0, h.art_failed, h.art_last_error);
    check("art-push-ok", lab_push(3190, why, sizeof(why)), "%s", why);
    health_fill(&h);
    check("art-ok-counted", h.art_ok == ok0 + 1 && h.art_set == -1, "ok %u -> %u, set %d", ok0,
          h.art_ok, h.art_set);
    st = health_right(&r, &took);
    check("right-art-counts", st == 0 && r.art_failed == 1 && r.art_ok == 1 &&
                                  r.art_last_error == ART_RIGHT_BAD_CRC,
          "status %d, the right: ok %u, failed %u, last %u", st, r.art_ok, r.art_failed,
          r.art_last_error);
    check("no-relink", sim_bt_stats.link_drops == drops, "%d link drops",
          sim_bt_stats.link_drops - drops);
    check("none-dropped", sim_bt_stats.rsp_dropped == 0, "%d requests the right didn't answer",
          sim_bt_stats.rsp_dropped);

    // A right without the block: D7 93 says 4, and nothing is counted as a refusal.
    sim_bt_faults.rsp_meta_rule = false;
    health_fill(&h);
    uint16_t refused = h.relay_refused;
    sim_bt_faults.right_no_health = true;
    st = health_right(&r, &took);
    health_fill(&h);
    check("right-without-block", st == 4 && h.relay_refused == refused, "status %d, refused %u",
          st, h.relay_refused - refused);
    sim_bt_faults.right_no_health = false;

    // A re-link: one more drop, the relay found again, the link-up time moves.
    uint16_t split0 = h.split_drops, ready0 = h.relay_ready;
    uint32_t up0 = h.split_up_at_s;
    sim_bt_drop(2000000);
    k_msleep(3000);
    wait_relay(40);
    health_fill(&h);
    check("relink-counted", h.split_drops == split0 + 1 && h.relay_ready == ready0 + 1 &&
                                h.split_up_at_s > up0 && h.split_connected == 1,
          "drops %u -> %u, found %u -> %u, up at %u -> %u s", split0, h.split_drops, ready0,
          h.relay_ready, up0, h.split_up_at_s);
    debug_line("at the end");
}
#endif

static bool lab_done;
static void lab_thread(void *arg) {
    (void)arg;
    if (!strcmp(scenario, "wake") || !strcmp(scenario, "wake-busy")) {
        wake_and_push(false, false);
    } else if (!strcmp(scenario, "wake-app")) {
        wake_and_push(true, false);
    } else if (!strcmp(scenario, "stall")) {
        wake_and_push(false, true);
    } else if (!strcmp(scenario, "stall-app")) {
        wake_and_push(true, true);
    } else if (!strcmp(scenario, "late")) {
        late_answer();
    } else if (!strcmp(scenario, "late-ignored")) {
        late_ignored();
    } else if (!strcmp(scenario, "lost")) {
        lost_answer();
    } else if (!strcmp(scenario, "push-relink-tap")) {
        push_relink_tap();
    } else if (!strcmp(scenario, "relink-shown")) {
        relink_shown();
    } else if (!strcmp(scenario, "select-snap")) {
        select_snap();
    } else if (!strcmp(scenario, "snap-latency")) {
        snap_latency(false);
    } else if (!strcmp(scenario, "snap-latency-old")) {
        snap_latency(true);
    } else if (!strcmp(scenario, "polarity-relink")) {
        polarity_relink();
    } else if (!strcmp(scenario, "playlist-store")) {
        playlist_store();
    } else if (!strcmp(scenario, "playlist-store-old")) {
        playlist_store_old();
    } else if (!strcmp(scenario, "restore-update")) {
        restore_update();
#ifdef SIM_HEALTH
    } else if (!strcmp(scenario, "health")) {
        health_scenario();
#endif
    } else {
        printf("unknown scenario %s\n", scenario);
        failed = true;
    }
    lab_done = true;
    for (;;) {
        k_msleep(1000000);
    }
}

static bool is_done(void *arg) {
    (void)arg;
    return lab_done;
}

int main(int argc, char **argv) {
    setvbuf(stdout, NULL, _IONBF, 0);
    scenario = argc > 1 ? argv[1] : "wake";
    if (argc > 2) {
        sim_log_level = atoi(argv[2]);
    }
    if (!strcmp(scenario, "wake-busy")) {
        // The right's display redraw holds its system work queue, where Zephyr frees its
        // response buffer: a request that comes meanwhile gets no response (att.c rsp_meta).
        sim_bt_faults.rsp_meta_rule = true;
        sim_bt_faults.right_redraw_busy_us = 40000;
    }
    printf("relay-sim %s\n", scenario);
    sim_kernel_init();
    k_work_queue_start(&k_sys_work_q, NULL, 0, -1, NULL);
    sim_bt_init();
    sim_sys_init_art_relay_init();
    sim_thread_create("usb", 5, lab_thread, NULL);
    sim_thread_create("app", 6, app_thread, NULL);
    if (strcmp(scenario, "health") != 0) { // health brings the right up late itself
        sim_bt_connect(500000);
    }
    if (!sim_run(1000LL * 1000000, is_done, NULL)) {
        check("progress", false, "every thread blocked and no timer left");
    }
    if (!lab_done) {
        check("time", false, "the scenario didn't end in 1000 s");
    }
    printf("RESULT %s%s\n", scenario, results);
    return failed ? 1 : 0;
}
