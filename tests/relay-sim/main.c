/*
 * relay-sim scenarios: the real art_relay_central.c (and its screen_snap_relay.inc) on the
 * simulated kernel and split link, driven the way foxglove-lab and the Foxglove app drive the
 * left half over USB. One scenario per run: relay-sim <scenario>. Prints one RESULT line and
 * exits 0 when every check passed, 1 when one failed.
 * SPDX-License-Identifier: MIT
 */
#include <stdarg.h>
#include <stddef.h>
#include <stdio.h>
#include <stdlib.h>

#include "art_relay.h"
#include "host_link.h"
#if __has_include("health.h") // bisect.ps1 builds widgets from before #705 too
#include "health.h"
#define SIM_HEALTH 1
#endif
#include "panel_polarity.h"
#include "screen_snap.h"
#include "sim_bt.h"
#include "sim_kernel.h"
#include "sim_stubs.h"
#ifdef SIM_TEST_HOOKS
#include "sim_hooks.h"
#include "test_hooks.h"
#endif

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

// D6 07 SLOT as the app reads a place back (art::slot, 3 s): used, length and name, and (#863)
// the CRC version byte and the CRC after it.
static uint8_t slot_crc_version;
static uint32_t slot_crc;
static int64_t slot_read_ms; // the last read's time to its reply
static bool lab_slot(uint8_t slot, bool *used, uint16_t *len, char name[17]) {
    int sends = sim_left.sends;
    if (art_relay_slot(slot) != 0) {
        return false;
    }
    int64_t start = k_uptime_get();
    while (sim_left.sends == sends && k_uptime_get() - start < 3000) {
        k_msleep(10);
    }
    slot_read_ms = k_uptime_get() - start;
    const uint8_t *r = sim_left.last_send;
    if (sim_left.sends == sends || r[1] != 0x87 || r[2] != 0 || r[3] != slot) {
        return false;
    }
    *used = r[4];
    *len = sys_get_le16(&r[9]);
    memcpy(name, &r[11], 16);
    name[16] = 0;
    slot_crc_version = r[HOST_LINK_SLOT_CRC_AT];
    slot_crc = sys_get_le32(&r[HOST_LINK_SLOT_CRC_AT + 1]);
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
    // Its CRC (#863): version 1, the CRC of the set as BEGIN announced it, so the app matches the
    // place to a piece by content.
    check("place-crc",
          read && slot_crc_version == HOST_LINK_SLOT_CRC_V1 && slot_crc == sim_right.flash[on].crc &&
              slot_crc != 0,
          "version %u, crc %08x, the place's %08x, in %lld ms", slot_crc_version, slot_crc,
          sim_right.flash[on].crc, (long long)slot_read_ms);
    // The CRC read is one more request under the link's latency: still well inside art::slot's
    // 3 s.
    check("slot-read-budget", slot_read_ms < 2000, "%lld ms", (long long)slot_read_ms);
    uint8_t empty = 0;
    while (empty < ART_RELAY_FLASH_SLOTS && sim_right.flash[empty].used) {
        empty++;
    }
    read = empty < ART_RELAY_FLASH_SLOTS && lab_slot(empty, &used, &len, name);
    check("empty-place-crc", read && !used && slot_crc_version == HOST_LINK_SLOT_CRC_V1 &&
                                 slot_crc == 0,
          "place %u: %s, version %u, crc %08x", empty, used ? "used" : "free", slot_crc_version,
          slot_crc);
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
    // Its slot reads back without a CRC (#863): version 0, and the left never asked for one, so
    // the app matches the place by name and length.
    int crc_reads = sim_bt_stats.crc_reads;
    bool used;
    uint16_t len;
    char name[17];
    bool read = lab_slot(6, &used, &len, name);
    check("old-right-no-crc",
          read && used && len == 3599 && slot_crc_version == 0 && slot_crc == 0 &&
              sim_bt_stats.crc_reads == crc_reads,
          "read %d, %s, %u bytes, version %u, crc %08x, %d CRC reads, in %lld ms", read,
          used ? "used" : "free", len, slot_crc_version, slot_crc,
          sim_bt_stats.crc_reads - crc_reads, (long long)slot_read_ms);
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

#if __has_include("art_toast.h")
// The on-board art step's toast (#852): &art_next / &art_prev without the app holding the keys
// (art_keys.c calls art_relay_step). The right holds its built-in (set 0) and kept places 0 and 2
// (sets 1 and 3), so the step list is 0, 1, 3. Every step that goes out shows the toast with its
// place in that list and the list's length, without the app and with its traffic (Foxglove
// running but paused); the right lands on the piece the toast names. A press while a SELECT is
// still going steps nothing and shows nothing, and so does one with the link down.
static bool board_step(int dir, int8_t want, char *why, size_t n) {
    int toasts = sim_left.toasts;
    int64_t start = k_uptime_get();
    art_relay_step(dir);
    // The step is a job on relay_q since #851: the toast comes once the right took the SELECT.
    while (sim_left.toasts == toasts && k_uptime_get() - start < 5000) {
        k_msleep(10);
    }
    if (sim_left.toasts != toasts + 1) {
        snprintf(why, n, "no toast (%d), right on set %d", sim_left.toasts - toasts, sim_right.set);
        return false;
    }
    while (sim_right.set != want && k_uptime_get() - start < 5000) {
        k_msleep(50);
    }
    k_msleep(1000); // the SELECT's status read lands, so the next step starts from here
    snprintf(why, n, "toast %d/%d dir %d %s; right on set %d after %lld ms",
             sim_left.toast_index + 1, sim_left.toast_total, sim_left.toast_dir,
             sim_left.toast_kept ? "kept" : "built-in", sim_right.set,
             (long long)(k_uptime_get() - start));
    return sim_right.set == want;
}

static void art_step_toast(void) {
    wait_relay(40);
    check("relay-up", art_relay_link() == 2, "link %d", art_relay_link());
    sim_right.flash[0].used = true;
    sim_right.flash[2].used = true;
    char why[200];
    check("start-on-built-in", lab_select(0, why, sizeof(why)), "%s", why);
    const int8_t list[] = {0, 1, 3};
    const int total = (int)ARRAY_SIZE(list);
    const struct {
        int8_t dir;
        bool app;
    } steps[] = {{1, false}, {1, false}, {1, false}, {-1, false}, {-1, true}, {1, true}, {1, true}};
    for (int i = 0; i < (int)ARRAY_SIZE(steps); i++) {
        app_on = steps[i].app;
        int at = 0;
        for (int k = 0; k < total; k++) {
            if (list[k] == sim_right.set) {
                at = k;
            }
        }
        int next = ((at + steps[i].dir) % total + total) % total;
        char name[24];
        snprintf(name, sizeof(name), "step-%d%s", i, steps[i].app ? "-app" : "");
        bool ok = board_step(steps[i].dir, list[next], why, sizeof(why));
        ok = ok && sim_left.toast_index == next && sim_left.toast_total == total &&
             sim_left.toast_dir == steps[i].dir && sim_left.toast_kept == (list[next] >= 1);
        check(name, ok, "want %d/%d (set %d): %s", next + 1, total, list[next], why);
    }
    app_on = false;

    // A press while a SELECT is still going: nothing steps, so no toast.
    int toasts = sim_left.toasts;
    art_relay_select(0);
    art_relay_step(1);
    check("busy-no-toast", sim_left.toasts == toasts, "%d toast(s)", sim_left.toasts - toasts);
    k_msleep(2000);

    // The link down: nothing steps, no toast.
    sim_bt_drop(2000000);
    k_msleep(500);
    toasts = sim_left.toasts;
    art_relay_step(1);
    check("no-link-no-toast", sim_left.toasts == toasts, "link %d, %d toast(s)", art_relay_link(),
          sim_left.toasts - toasts);
    wait_relay(40);
    debug_line("at the end");
}

// The wrap (#851): M = 2 built-ins and N = 4 kept places (0-3), so the step list is sets 0-5.
// From the last set, two full laps with &art_next and then two with &art_prev, Foxglove quit (no
// STATUS reads between presses), a press a little over a second after the last one landed. Each
// press must land on the next set round the list, the wrap included, with the toast's n/6.
// With lag, the right is one before #851: a built-in shows in its status 2 s after its SELECT
// (its display queue), so the left's read right after the SELECT still holds the set before.
// The left stepped from that set again (to the set already on screen): the stall at the wrap.
static void art_step_cycles(bool lag) {
    sim_right.set_count = 2;
    wait_relay(40);
    check("relay-up", art_relay_link() == 2, "link %d", art_relay_link());
    for (int i = 0; i < 4; i++) {
        sim_right.flash[i].used = true;
    }
    const int total = 6, last = total - 1;
    char why[200];
    check("start-on-last", lab_select((int8_t)last, why, sizeof(why)), "%s", why);
    sim_right.builtin_lag_us = lag ? 2000000 : 0;
    app_on = false;
    int fails = 0;
    for (int lap = 0; lap < 4; lap++) {
        int8_t dir = lap < 2 ? 1 : -1;
        for (int i = 0; i < total; i++) {
            int8_t want = (int8_t)(((sim_right.set + dir) % total + total) % total);
            bool ok = board_step(dir, want, why, sizeof(why));
            ok = ok && sim_left.toast_index == want && sim_left.toast_total == total &&
                 sim_left.toast_dir == dir && sim_left.toast_kept == (want >= 2);
            if (!ok || (want == 0 && dir > 0) || (want == last && dir < 0)) {
                char name[24];
                snprintf(name, sizeof(name), "lap-%d-%s-%d", lap, dir > 0 ? "next" : "prev", want);
                check(name, ok, "want %d/%d: %s", want + 1, total, why); // the wraps, and any miss
            }
            fails += !ok;
        }
    }
    check("four-laps", fails == 0, "%d of %d presses missed", fails, 4 * total);
    debug_line("at the end");
}
#endif

#if __has_include("art_toast.h")
// The first press after a boot (#868): the right starts on its built-in, and its status names it
// (set 0), as art_store_booted makes it say; before, it said -1 (a pushed set), and the first
// &art_next selected the emblem on screen again. Kept places 0 and 1 (list 0, 1, 2), Foxglove
// quit, no SELECT from anyone: the first next press steps to set 1 (2/3). Then the right restarts
// (an update that keeps the places) and the first previous press steps to the last set (3/3).
static void art_step_boot(void) {
    sim_right.flash[0].used = true;
    sim_right.flash[1].used = true;
    wait_relay(40);
    check("relay-up", art_relay_link() == 2, "link %d", art_relay_link());
    k_msleep(2000); // the left's first status read after the relay is found
    char why[200];
    check("boot-names-built-in", status().right.set == 0, "status set %d", status().right.set);
    bool ok = board_step(1, 1, why, sizeof(why));
    check("first-next-steps", ok && sim_left.toast_index == 1 && sim_left.toast_total == 3,
          "%s", why);
    sim_bt_right_update(false, 3000000);
    k_msleep(3000);
    wait_relay(40);
    k_msleep(2000);
    ok = board_step(-1, 2, why, sizeof(why));
    check("first-prev-after-restart",
          ok && sim_left.toast_index == 2 && sim_left.toast_total == 3 && sim_left.toast_dir == -1,
          "%s", why);
    debug_line("at the end");
}

/* #851, "Kept: 4 of 8" but the board steps between fewer: the places are kept the way the app
 * keeps them (D6 01/02/09 STORE over the relay, Screens -> Keep on keyboard), then Foxglove quits
 * and &art_next / &art_prev step on the board. Unlike art-step-cycles, which sets the right's
 * slots behind the left's back and lets a SELECT's read show them, nothing but the keeping itself
 * tells the left here. The list is the emblem (set 0) and places 0-3 (sets 1-4); two laps each
 * way, every press on the next set round the list, the wraps included, the toast's n/5 and m = 5.
 *   wait:    the app waits for each STORE to end (art::finish_slot_op) before it quits.
 *   quit:    the app quits while the last STORE is still writing (SAVING): no STATUS after it.
 *   relink:  then the split link drops and comes back (the right's sleep, a range drop).
 *   restart: then the right restarts (power switch, an update that keeps the places).
 *   bad:     place 1 (set 2) is listed but the right refuses it (BAD_BLOB, its CRC or header):
 *            every lap steps past it to the others and wraps; before, each press selected it
 *            again and the lap ended on set 1. The toast still counts the whole list (n/5). */
enum kept_after { KEPT_WAIT, KEPT_QUIT, KEPT_RELINK, KEPT_RESTART, KEPT_BAD };

static void art_step_kept(enum kept_after after) {
    sim_right.set_count = 1;
    wait_relay(40);
    check("relay-up", art_relay_link() == 2, "link %d", art_relay_link());
    app_on = true;
    char why[200];
    check("start-on-emblem", lab_select(0, why, sizeof(why)), "%s", why);
    static const char *names[] = {"hearth-moongarde", "constellations", "fury", "moon-garden"};
    static const uint16_t lens[] = {3281, 1756, 3599, 2048};
    const int kept = (int)ARRAY_SIZE(names);
    for (int i = 0; i < kept; i++) {
        char name[16];
        snprintf(name, sizeof(name), "keep-%d", i);
        if (after == KEPT_QUIT && i == kept - 1) {
            // The last Keep's D6 09 goes out and the app is gone before its STATUS reads.
            make_piece(lens[i], (uint8_t)(40 + i));
            uint8_t nm[16];
            name16(nm, names[i]);
            bool sent = art_relay_begin(lens[i], crc32_ieee(piece, lens[i])) == 0;
            for (int at = 0; sent && at < lens[i]; at += 27) {
                art_relay_data(at, &piece[at], (uint8_t)MIN(27, lens[i] - at));
                k_msleep(1);
            }
            app_on = false;
            sent = sent && art_relay_store((uint8_t)i, nm) == 0;
            k_msleep(3000); // the left's job ends; the right's flash write ends after it
            check(name, sent && place_holds((uint8_t)i, lens[i], (uint8_t)(40 + i), names[i]),
                  "STORE %s, place %d %s", sent ? "sent" : "refused", i,
                  sim_right.flash[i].used ? "used" : "empty");
        } else {
            check(name, lab_store((uint8_t)i, lens[i], (uint8_t)(40 + i), names[i], why,
                                  sizeof(why)),
                  "%s", why);
        }
    }
    app_on = false; // Foxglove quit: from here on no STATUS, no D6 07, nothing from the PC
    if (after == KEPT_RELINK) {
        sim_bt_drop(2000000);
        k_msleep(500);
        wait_relay(40);
        check("relinked", art_relay_link() == 2, "link %d", art_relay_link());
    } else if (after == KEPT_RESTART) {
        sim_bt_right_update(false, 3000000);
        k_msleep(3500);
        wait_relay(40);
        check("right-back", art_relay_link() == 2 && sim_right.set == 0, "link %d, right on set %d",
              art_relay_link(), sim_right.set);
    }
    uint8_t bad = 0; // sets the right refuses
    if (after == KEPT_BAD) {
        sim_right.bad_slots = BIT(1);
        bad = BIT(2);
    }
    k_msleep(2000);
    const int total = 1 + kept;
    int good = 0;
    for (int i = 0; i < total; i++) {
        good += !(bad & BIT(i));
    }
    int fails = 0, visited = 0;
    for (int lap = 0; lap < 4; lap++) {
        int8_t dir = lap < 2 ? 1 : -1;
        uint8_t seen = 0;
        for (int i = 0; i < good; i++) {
            int8_t want = sim_right.set;
            do {
                want = (int8_t)(((want + dir) % total + total) % total);
            } while (bad & BIT(want));
            bool ok = board_step(dir, want, why, sizeof(why));
            ok = ok && sim_left.toast_index == want && sim_left.toast_total == total &&
                 sim_left.toast_dir == dir && sim_left.toast_kept == (want >= 1);
            seen |= ok ? BIT(want) : 0;
            if (!ok || (want == 0 && dir > 0) || (want == total - 1 && dir < 0)) {
                char name[24];
                snprintf(name, sizeof(name), "lap-%d-%s-%d", lap, dir > 0 ? "next" : "prev", want);
                check(name, ok, "want %d/%d: %s", want + 1, total, why); // the wraps, and any miss
            }
            fails += !ok;
        }
        visited += seen == ((BIT(total) - 1) & ~bad);
    }
    check("all-kept-visited", fails == 0 && visited == 4,
          "%d of %d presses missed; %d of 4 laps visited all %d sets", fails, 4 * good, visited,
          good);
    debug_line("at the end");
}
#endif

#ifdef SIM_TEST_HOOKS
/* #851 after #867, the c9bf83b hardware trace (batch-c9bf83b/lab art-cycle-fwd, art-cycle-back):
 * the lab holds SYSTEM (D7 05 downs of 43 and 0 on the left, 46 on the right), then taps P (pos
 * 11) ten times, then O (pos 10) ten times, each tap's step awaited (up to 6 s) before the next.
 * The board has the emblem and 4 kept places (5 sets). The hardware stepped five or six times,
 * then a tap was refused busy (forward) or stepped nothing (back), about 10.5 s after the layer
 * keys went down every time: the left let its test-hook downs go 10 s after each down
 * (test_hooks.c), relaying 46's up and dropping SYSTEM mid-cycle. The keys stay held while the PC
 * keeps talking (up to HOLD_MAX_MS) since #851's fix. The keymap is modelled here: a P or O tap
 * steps the art only while all three layer keys are held. */
static int lab_press(uint8_t pos, uint8_t state) {
    test_hooks_host_seen();
    int replies = sim_left.position_replies;
    uint8_t r = test_hooks_press(pos, state);
    if (r != 0) {
        return r;
    }
    int64_t start = k_uptime_get();
    while (sim_left.position_replies == replies && k_uptime_get() - start < 4000) {
        k_msleep(10);
    }
    return sim_left.position_replies == replies ? -1 : sim_left.position_status;
}

static bool system_held(void) {
    return (sim_hooks_positions & ((uint64_t)1 << 43)) && (sim_hooks_positions & 1) &&
           (sim_right.held & ((uint64_t)1 << 46));
}

static void art_hold_cycles(void) {
    sim_right.set_count = 1;
    wait_relay(40);
    check("relay-up", art_relay_link() == 2, "link %d", art_relay_link());
    for (int i = 0; i < 4; i++) {
        sim_right.flash[i].used = true;
    }
    const int total = 5;
    char why[200];
    check("start-on-0", lab_select(0, why, sizeof(why)), "%s", why);
    app_on = false;
    k_msleep(1000);
    const struct {
        uint8_t pos;
        int8_t dir;
        const char *name;
    } runs[] = {{11, 1, "fwd"}, {10, -1, "back"}};
    for (int run = 0; run < 2; run++) {
        int64_t held_at = k_uptime_get();
        int st[3] = {lab_press(43, 1), lab_press(46, 1), lab_press(0, 1)};
        char name[32];
        snprintf(name, sizeof(name), "%s-hold", runs[run].name);
        check(name, st[0] == 0 && st[1] == 0 && st[2] == 0 && system_held(),
              "43 %d, 46 %d, 0 %d; SYSTEM %s", st[0], st[1], st[2], system_held() ? "on" : "off");
        int stepped = 0;
        for (int i = 0; i < 10; i++) {
            int8_t want = (int8_t)(((sim_right.set + runs[run].dir) % total + total) % total);
            int down = lab_press(runs[run].pos, 1);
            k_msleep(40);
            int up = lab_press(runs[run].pos, 0);
            bool layer = system_held();
            if (down == 0 && layer) {
                art_relay_step(runs[run].dir); // the keymap's &art_next / &art_prev on SYSTEM
            }
            int64_t start = k_uptime_get();
            while (sim_right.set != want && k_uptime_get() - start < 6000) {
                test_hooks_host_seen(); // the lab's D6 STATUS reads while it waits
                k_msleep(100);
            }
            bool ok = down == 0 && up == 0 && layer && sim_right.set == want;
            stepped += ok;
            if (!ok) {
                snprintf(name, sizeof(name), "%s-press-%d", runs[run].name, i + 1);
                check(name, false,
                      "%.1f s after the layer keys: tap %d/%d, SYSTEM %s, right on set %d (want %d)",
                      (k_uptime_get() - held_at) / 1000.0, down, up, layer ? "on" : "off",
                      sim_right.set, want);
                break;
            }
            k_msleep(750); // the lab's report between steps: about 1.75 s a step, as on hardware
        }
        snprintf(name, sizeof(name), "%s-ten-steps", runs[run].name);
        check(name, stepped == 10, "%d of 10 steps, %.1f s after the layer keys went down", stepped,
              (k_uptime_get() - held_at) / 1000.0);
        int rel[3] = {lab_press(0, 0), lab_press(46, 0), lab_press(43, 0)};
        snprintf(name, sizeof(name), "%s-release", runs[run].name);
        check(name, rel[0] == 0 && rel[1] == 0 && rel[2] == 0 && sim_hooks_positions == 0 &&
                        sim_right.held == 0,
              "0 %d, 46 %d, 43 %d; held left 0x%llx right 0x%llx", rel[0], rel[1], rel[2],
              (unsigned long long)sim_hooks_positions, (unsigned long long)sim_right.held);
        k_msleep(1000);
    }
    // The safety net stays: once the PC goes quiet, the left lets its held keys go.
    int st = lab_press(43, 1);
    int st2 = lab_press(46, 1);
    k_msleep(12000); // no host traffic
    check("quiet-releases", st == 0 && st2 == 0 && sim_hooks_positions == 0 && sim_right.held == 0,
          "after 12 s without the PC: held left 0x%llx right 0x%llx",
          (unsigned long long)sim_hooks_positions, (unsigned long long)sim_right.held);
    debug_line("at the end");
}
#endif

/* #873: the right reflashed with a different relay attribute table (the slot CRC characteristic
 * of #863 added, then gone again), both directions; the left finds the relay by itself, within
 * REFLASH_RELAY_S, with no left reboot. With `stall`, the left's TX stalls once the new link is
 * encrypted, before its discovery goes out (the c9bf83b flash: HELLO discovery 1 with no ends
 * for minutes, the split fallback for the bootloader not landing either): the discovery watch
 * has to drop that link for the relay to come back. */
#define REFLASH_RELAY_S 30
#define REFLASH_RELAY_STALL_S 75

static void right_reflash(bool stall) {
    wait_relay(40);
    check("relay-up", art_relay_link() == 2, "link %d", art_relay_link());
    const uint8_t versions[] = {ART_RELAY_STORE_STATUS_VERSION, ART_RELAY_SLOT_CRC_STATUS_VERSION};
    const char *names[] = {"to-old-table", "to-new-table"};
    for (int i = 0; i < 2; i++) {
        int drops = sim_bt_stats.link_drops;
        uint8_t was = sim_right.version;
        sim_right.version = versions[i]; // the new firmware's table, from its first boot
        int64_t start = k_uptime_get();
        sim_bt_right_update(false, 3000000);
        if (stall) {
            k_msleep(3000 + 500 + 1000); // the link is back and encrypted; discovery is due at +15 s
            sim_bt_stall_left_tx();
        }
        int limit = stall ? REFLASH_RELAY_STALL_S : REFLASH_RELAY_S;
        k_msleep(1000);
        while (art_relay_link() != 2 && k_uptime_get() - start < limit * 1000LL) {
            k_msleep(100);
        }
        int64_t took = k_uptime_get() - start;
        int8_t result;
        uint8_t tries;
        art_relay_discovery(&result, &tries);
        char name[40];
        snprintf(name, sizeof(name), "%s-relay", names[i]);
        check(name, art_relay_link() == 2 && result == ART_DISCOVERY_DONE,
              "version %d -> %d: link %d, discovery %d (tries %d) after %.1f s (limit %d s), %d "
              "link drop(s) by the left",
              was, versions[i], art_relay_link(), result, tries, took / 1000.0, limit,
              sim_bt_stats.link_drops - drops - 1);
        debug_line(names[i]);
        // The relay works on the new table: a slot read (with the CRC only from version 11 on)
        // and a SELECT. STATUS first, as Foxglove and the lab do: the left reads the right's
        // status (its version) on the first one after a re-link.
        status();
        k_msleep(500);
        status();
        bool used = false;
        uint16_t len = 0;
        char slot_name[17];
        bool read = lab_slot(0, &used, &len, slot_name);
        snprintf(name, sizeof(name), "%s-slot", names[i]);
        bool want_crc = versions[i] >= ART_RELAY_SLOT_CRC_STATUS_VERSION;
        check(name, read && (slot_crc_version == HOST_LINK_SLOT_CRC_V1) == want_crc,
              "read %s, CRC %s (want %s)", read ? "ok" : "failed",
              slot_crc_version == HOST_LINK_SLOT_CRC_V1 ? "present" : "absent",
              want_crc ? "present" : "absent");
        char why[200];
        snprintf(name, sizeof(name), "%s-select", names[i]);
        check(name, lab_select(0, why, sizeof(why)), "%s", why);
    }
}

/* D6 0B SLOT_READ (#878) as the app's art::read_slot drives it: one request at a time, each from
 * where the last reply ended, until the end (n 0) or an error. drop_after >= 0 drops the split
 * link just before that request. Returns the status that ended it (0: the whole set read; 0xfx
 * the sim's own: no reply in 3 s, a reply for another offset, an early end, too many bytes). */
static int64_t read_took_ms;
static int read_requests;
static uint8_t slot_read_once(uint8_t slot, uint16_t offset) {
    int before = sim_left.slot_read_replies;
    if (art_relay_slot_read(slot, offset) != 0) {
        return 1; // host_link.c answers busy itself
    }
    read_requests++;
    int64_t t0 = k_uptime_get();
    while (sim_left.slot_read_replies == before && k_uptime_get() - t0 < 3000) {
        k_msleep(2);
    }
    if (sim_left.slot_read_replies == before) {
        return 0xff;
    }
    const uint8_t *r = sim_left.slot_read;
    if (r[2] == 0 && (r[3] != slot || sys_get_le16(&r[4]) != offset)) {
        return 0xfe;
    }
    return r[2];
}

static uint8_t lab_read_set(uint8_t slot, uint16_t len, uint8_t *out, uint16_t *got,
                            int drop_after) {
    *got = 0;
    read_requests = 0;
    int64_t start = k_uptime_get();
    for (int req = 0;; req++) {
        if (req == drop_after) {
            sim_bt_drop(3000000);
        }
        uint8_t st = slot_read_once(slot, *got);
        read_took_ms = k_uptime_get() - start;
        if (st != 0) {
            return st;
        }
        uint8_t n = sim_left.slot_read[6];
        if (n == 0) {
            return *got == len ? 0 : 0xfd;
        }
        if (n > HOST_LINK_SLOT_READ_MAX || *got + n > len) {
            return 0xfc;
        }
        memcpy(&out[*got], &sim_left.slot_read[7], n);
        *got += n;
    }
}

// SLOT_READ (#878) under playlist-store's link (latency 30, the response-buffer rule, the right's
// busy system queue, the app's traffic): a kept place read back whole, in order, its bytes and CRC
// the place's; the errors: an offset past the end and an empty place (5), a read while a STORE
// writes (busy, 1), a right before the op (4); the split link lost mid-read (2 or 3), then a
// whole read again once it is back.
static void slot_read_scenario(void) {
    wait_relay(40);
    check("relay-up", art_relay_link() == 2, "link %d", art_relay_link());
    app_on = true;
    k_msleep(2000);
    sim_bt_faults.rsp_meta_rule = true;
    sim_bt_faults.right_redraw_busy_us = 20000;
    char why[200];
    const uint16_t len = 3599;
    check("keep", lab_store(2, len, 5, "radar", why, sizeof(why)), "%s", why);
    int drops = sim_bt_stats.link_drops;
    int windows = sim_bt_stats.slot_pushes;
    static uint8_t got_bytes[4096];
    uint16_t got;
    uint8_t st = lab_read_set(2, len, got_bytes, &got, -1);
    make_piece(len, 5);
    bool same = got == len && !memcmp(got_bytes, piece, len) &&
                !memcmp(got_bytes, sim_bt_flash_bytes(2), len);
    windows = sim_bt_stats.slot_pushes - windows;
    printf("  slot read: %u bytes in %lld ms (simulated), %d requests, %d windows pushed\n", got,
           (long long)read_took_ms, read_requests, windows);
    check("read-whole", st == 0 && same && crc32_ieee(got_bytes, got) == sim_right.flash[2].crc,
          "status %u, %u of %u bytes, %s", st, got, len, same ? "the place's bytes" : "differ");
    // The PC's 25-byte requests come from the left's window: one relay write per window.
    check("from-windows", windows == (len + 499) / 500 && read_requests == (len + 24) / 25 + 1,
          "%d windows for %d requests", windows, read_requests);
    check("read-budget", read_took_ms < 6000, "%lld ms for %u bytes", (long long)read_took_ms,
          len);
    st = slot_read_once(2, len + 1);
    check("past-the-end", st == HOST_LINK_SLOT_READ_BAD, "status %u", st);
    uint8_t empty = 0;
    while (empty < ART_RELAY_FLASH_SLOTS && sim_right.flash[empty].used) {
        empty++;
    }
    st = slot_read_once(empty, 0);
    check("empty-place", st == HOST_LINK_SLOT_READ_BAD, "place %u: status %u", empty, st);
    // A read while a STORE is under way: the right is writing the slot (or the left still
    // pushing), so busy; the store lands all the same.
    make_piece(1024, 7);
    uint8_t nm[16];
    name16(nm, "busy");
    int64_t start = k_uptime_get();
    bool stored = art_relay_begin(1024, crc32_ieee(piece, 1024)) == 0;
    for (int at = 0; stored && at < 1024; at += 27) {
        art_relay_data(at, &piece[at], (uint8_t)MIN(27, 1024 - at));
        k_msleep(1);
    }
    stored = stored && art_relay_store(5, nm) == 0;
    st = slot_read_once(2, 0);
    struct report r = {0};
    stored = stored && finish_slot_op(start, &r) && place_holds(5, 1024, 7, "busy");
    check("busy-while-storing", st == 1 && stored, "status %u, the store %s", st,
          stored ? "landed" : "failed");
    // A right before SLOT_READ (status version 11): 4, and nothing sent to it.
    sim_right.version = ART_RELAY_SLOT_READ_STATUS_VERSION - 1;
    status();
    k_msleep(1500);
    windows = sim_bt_stats.slot_pushes;
    st = slot_read_once(2, 0);
    check("old-right", st == 4 && sim_bt_stats.slot_pushes == windows, "status %u", st);
    sim_right.version = ART_RELAY_SLOT_READ_STATUS_VERSION;
    status();
    k_msleep(1500);
    check("no-relink", sim_bt_stats.link_drops == drops, "%d link drops",
          sim_bt_stats.link_drops - drops);
    check("none-dropped", sim_bt_stats.rsp_dropped == 0, "%d requests the right didn't answer",
          sim_bt_stats.rsp_dropped);
    // The split link lost mid-read: the read ends with no link (2) or no answer (3), not with
    // bytes that aren't the place's.
    // Request 60 starts a window (offset 1500): it finds the link gone.
    st = lab_read_set(2, len, got_bytes, &got, 60);
    make_piece(len, 5);
    check("link-lost", (st == 2 || st == 3) && got < len && !memcmp(got_bytes, piece, got),
          "status %u after %u bytes, the place's", st, got);
    wait_relay(40);
    k_msleep(3000); // the doorbell's subscription, which the push comes through
    st = lab_read_set(2, len, got_bytes, &got, -1);
    check("read-after-relink", st == 0 && got == len && !memcmp(got_bytes, piece, len),
          "status %u, %u bytes in %lld ms", st, got, (long long)read_took_ms);
    debug_line("at the end");
}

/* #943: every split link-up sends a burst (the RGB sync, the doorbell subscribe, status, polarity,
 * lamps, effect, activity) right after the discovery's last answer. Under the response-buffer rule
 * and a right whose system queue is busy (its art frames), a request that reaches the right
 * before its last response's buffer is free gets no answer: hardware showed one relay timeout on
 * every link-up and sometimes the unstick's link drop. Re-link six times; none may go unanswered. */
static void relink_busy(void) {
    wait_relay(40);
    check("relay-up", art_relay_link() == 2, "link %d", art_relay_link());
    app_on = true;
    k_msleep(2000);
    sim_bt_faults.rsp_meta_rule = true;
    sim_bt_faults.right_redraw_busy_us = 20000;
    sim_bt_faults.right_frame_us = 100000;
    sim_bt_faults.right_frame_busy_us = 20000;
    uint32_t timeouts = health_get(HEALTH_RELAY_TIMEOUTS);
    int drops = sim_bt_stats.link_drops, dropped = sim_bt_stats.rsp_dropped;
    int up = 0;
    for (int i = 0; i < 6; i++) {
        sim_bt_drop(2000000);
        k_msleep(100);
        wait_relay(40);
        up += art_relay_link() == 2;
        k_msleep(5000); // the link-up burst, the doorbell's subscribe (300 ms) and what follows
    }
    check("relay-back", up == 6, "%d of 6 re-links found the relay", up);
    check("no-timeouts", health_get(HEALTH_RELAY_TIMEOUTS) == timeouts, "%u relay requests timed out",
          (unsigned)(health_get(HEALTH_RELAY_TIMEOUTS) - timeouts));
    check("none-dropped", sim_bt_stats.rsp_dropped == dropped, "%d requests the right didn't answer",
          sim_bt_stats.rsp_dropped - dropped);
    check("no-unstick", sim_bt_stats.link_drops == drops + 6, "%d link drops beyond the 6",
          sim_bt_stats.link_drops - drops - 6);
    debug_line("at the end");
}

/* #945: "Right lights showed another colour than the left sent". The left counted a mismatch
 * whenever the right's report (a status read or a ring) held another look than the left's, even
 * before the right had taken the RGB op with that look. Every link-up where the right held an
 * older look (it missed a change while the link was down, or restarted with another saved look)
 * counted one: the doorbell subscribe's ring carries the right's state and comes before the
 * link-up RGB op. A drift is a right that confirmed the look and then shows another: that counts
 * once and gets the look again, each time it happens. */
static bool right_has_look(const uint8_t look[5]) { return !memcmp(sim_right.look, look, 5); }

static void look_step(const char *name, uint32_t before) {
    char ok[32], quiet[32];
    snprintf(ok, sizeof(ok), "%s-synced", name);
    snprintf(quiet, sizeof(quiet), "%s-no-mismatch", name);
    check(ok, right_has_look(sim_left.look), "right hue %u effect %u, left hue %u effect %u",
          sys_get_le16(sim_right.look), sim_right.look[4], sys_get_le16(sim_left.look),
          sim_left.look[4]);
    check(quiet, health_get(HEALTH_RGB_MISMATCHES) == before, "%u mismatches counted",
          (unsigned)(health_get(HEALTH_RGB_MISMATCHES) - before));
}

static void rgb_relink_look(void) {
    static const uint8_t teal[5] = {180, 0, 90, 60, 0}, red[5] = {0, 0, 100, 80, 1},
                         blue[5] = {240 & 0xff, 0, 100, 50, 2};
    wait_relay(40);
    check("relay-up", art_relay_link() == 2, "link %d", art_relay_link());
    app_on = true;
    k_msleep(3000);
    uint32_t m = health_get(HEALTH_RGB_MISMATCHES);
    look_step("start", m);
    // The left's look changes while the link is down: the right comes back with the old one.
    sim_bt_drop(2000000);
    k_msleep(100);
    memcpy(sim_left.look, teal, 5);
    art_relay_rgb_kick();
    wait_relay(40);
    k_msleep(5000);
    look_step("relink", m);
    // The right restarts with another saved look.
    sim_bt_right_update(false, 2000000);
    memcpy(sim_right.look, red, 5);
    wait_relay(40);
    k_msleep(5000);
    look_step("restart", m);
    // As the hardware's "1 request timed out" on connect: the link-up RGB op answered past the
    // 3 s GATT timeout, the app's STATUS and RGB_STATE reads going on meanwhile.
    sim_bt_drop(2000000);
    k_msleep(100);
    memcpy(sim_left.look, blue, 5);
    sim_bt_faults.late_op = ART_RELAY_RGB;
    sim_bt_faults.late_us = 4500000;
    sim_bt_faults.late_count = 1;
    wait_relay(40);
    k_msleep(9000);
    sim_bt_faults.late_op = 0;
    look_step("late-op", m);
    // As the hardware's link-up (#943's burst): the right's response buffer is busy when the RGB op
    // comes, so Zephyr ignores it; the doorbell's ring and the status reads still show its old
    // look until the left sends the op again.
    sim_bt_drop(2000000);
    k_msleep(100);
    memcpy(sim_left.look, teal, 5);
    sim_bt_faults.ignore_op = ART_RELAY_RGB;
    sim_bt_faults.ignore_count = 1;
    // The left lets the op go after 3 s and its unstick may re-link to free the request (the
    // hardware's "link dropped to free a stuck request"): up to 60 s for the relay and the look.
    int64_t lost_at = k_uptime_get();
    k_msleep(1000);
    while (k_uptime_get() - lost_at < 60000 &&
           (art_relay_link() != 2 || !right_has_look(sim_left.look))) {
        k_msleep(100);
    }
    printf("  lost op: relay and look back after %lld ms\n", (long long)(k_uptime_get() - lost_at));
    k_msleep(3000);
    check("lost-op-ignored", sim_bt_faults.ignore_count == 0, "%d left to ignore",
          sim_bt_faults.ignore_count);
    look_step("lost-op", m);
    // The PC's colour (#266), as at Foxglove's start: the right's ring (its LEDs going lit on the
    // PC's wake) is on its way with the colour it shows, when the left takes the PC's colour at
    // once (host_link.c sets its own after art_relay_rgb_hsb) and sends it. A report from before
    // the right acknowledged the colour is not a drift. (A guard: in the sim's timing the ring
    // lands before the left takes the colour, so main passed this step too.)
    int rings = sim_bt_stats.rings;
    sim_bt_right_ring_rgb();
    for (int i = 0; i < 100 && sim_bt_stats.rings == rings; i++) {
        k_msleep(1);
    }
    int rc = art_relay_rgb_hsb(30, 70, 90);
    check("pc-colour-taken", rc == 0 && sim_bt_stats.rings > rings, "art_relay_rgb_hsb %d, %d ring",
          rc, sim_bt_stats.rings - rings);
    sim_left.look[0] = 30;
    sim_left.look[1] = 0;
    sim_left.look[2] = 70;
    sim_left.look[3] = 90;
    k_msleep(3000);
    look_step("pc-colour", m);
    // A real drift: the right confirmed the look, then shows another one. Counted and resynced,
    // and again the second time.
    for (int i = 1; i <= 2; i++) {
        memcpy(sim_right.look, red, 5);
        sim_bt_right_ring_rgb();
        k_msleep(2000);
        char name[24];
        snprintf(name, sizeof(name), "drift%d-counted", i);
        check(name, health_get(HEALTH_RGB_MISMATCHES) == m + i, "%u mismatches counted",
              (unsigned)(health_get(HEALTH_RGB_MISMATCHES) - m));
        snprintf(name, sizeof(name), "drift%d-resynced", i);
        check(name, right_has_look(sim_left.look), "right hue %u", sys_get_le16(sim_right.look));
    }
    debug_line("at the end");
}

#ifdef ART_RELAY_KEYS_STATUS_VERSION
/* SET_EFFECT over the relay (#490): the right half's copy of the PC's effect, as host_effects.c
 * hands it to art_relay_effect, under the link's real timing (latency 30) with the app's traffic.
 * The right model (sim_bt.c) keeps the wire it is sent and anchors its effect clock on each GO,
 * as art_relay_peripheral.c and effect_run.c do; the left's clock is effect_run_clock (the sim's
 * uptime). Each half then renders with the real lamp_effect.c. */

#define FX_FRAME_MS 33 // CONFIG_FOXGLOVE_HOST_EFFECTS_FRAME_MS's default
// The bytes of the wire the left sends a right of version >= 9 (art_relay_central.c effect_send).
#define FX_SENT (offsetof(struct lamp_effect_wire, lamp) + 4 * ART_RELAY_LAMPS_MAX)

static const int16_t fx_room[4] = {0, 0, 328, 86};
static const int16_t fx_right[4] = {204, 0, 124, 86};

static void put24(uint32_t v, uint8_t *p) {
    p[0] = (uint8_t)v;
    p[1] = (uint8_t)(v >> 8);
    p[2] = (uint8_t)(v >> 16);
}

// The right half's copy of an effect, as host_effects.c's for_half makes it.
static void fx_wire(struct lamp_effect_wire *w, uint8_t kind, uint32_t period, int16_t right_x) {
    static const uint8_t rgb[2][3] = {{0xF0, 0x8A, 0x3C}, {0x06, 0x08, 0x0E}};
    memset(w, 0, sizeof(*w));
    w->kind = kind;
    w->n = 2;
    put24(period, w->period_ms);
    sys_put_le16(kind == LAMP_EFFECT_KEYS ? 0 : 9000, w->angle_cdeg); // a scanline downwards
    sys_put_le16(250, w->width_dmm);
    memcpy(w->rgb, rgb, sizeof(rgb));
    for (int i = 0; i < 4; i++) {
        sys_put_le16(fx_room[i], &w->room[2 * i]);
        sys_put_le16(i == 0 ? right_x : fx_right[i], &w->half[2 * i]);
    }
}

// How far the right's effect clock runs behind the left's, in us (both read the sim's clock).
static int64_t fx_lag_us(void) { return sim_right.effect_epoch_us; }

// Waits up to max_ms for the right to draw w: its copy byte for byte, running, and a GO of
// this send in.
static bool fx_right_draws(const struct lamp_effect_wire *w, int max_ms, int64_t *took) {
    int64_t start = k_uptime_get();
    while (k_uptime_get() - start < max_ms) {
        if (sim_right.effect_run && sim_right.effect_gos_since_send >= 1 &&
            memcmp(&sim_right.effect, w, FX_SENT) == 0) {
            *took = k_uptime_get() - start;
            k_msleep(600); // a GO answered late goes again (at most 3): the left's last one in
            return true;
        }
        k_msleep(5);
    }
    *took = k_uptime_get() - start;
    return false;
}

// The most any LED channel differs between two renders of the right half.
static int fx_diff(const uint8_t a[LAMP_EFFECT_LAMPS][3], const uint8_t b[LAMP_EFFECT_LAMPS][3]) {
    int worst = 0;
    for (int i = 0; i < LAMP_EFFECT_LAMPS; i++) {
        for (int c = 0; c < 3; c++) {
            int d = abs(a[i][c] - b[i][c]);
            worst = d > worst ? d : worst;
        }
    }
    return worst;
}

// The right half as the right draws it now (its copy, its clock) against the left's reference
// for that half (the wire it sent, the left's clock): within what one frame of the effect moves.
static bool fx_same_frame(const struct lamp_effect_wire *sent, int *got, int *bound) {
    static struct lamp_effect fx, rx;
    static uint8_t ref[LAMP_EFFECT_LAMPS][3], next[LAMP_EFFECT_LAMPS][3], right[LAMP_EFFECT_LAMPS][3];
    if (!lamp_effect_prepare(&fx, sent, true) || !lamp_effect_prepare(&rx, &sim_right.effect, true)) {
        return false;
    }
    int64_t now_us = sim_now_us();
    uint64_t left_ms = (uint64_t)(now_us / 1000);
    uint64_t right_ms = (uint64_t)((now_us - sim_right.effect_epoch_us) / 1000);
    lamp_effect_render(&fx, lamp_effect_phase(left_ms, fx.period_ms), ref);
    lamp_effect_render(&fx, lamp_effect_phase(left_ms + FX_FRAME_MS, fx.period_ms), next);
    lamp_effect_render(&rx, lamp_effect_phase(right_ms, rx.period_ms), right);
    *got = fx_diff(ref, right);
    *bound = fx_diff(ref, next);
    return *got <= *bound;
}

// The halves agree within one frame after every relay send (#490's acceptance): five effects,
// sent at different points of the right's latency sleep, with the app's traffic; then the wake's
// re-anchor (art_relay_effect_sync) alone, after the right slept.
static void effect_phase(void) {
    wait_relay(40);
    check("relay-up", art_relay_link() == 2, "link %d", art_relay_link());
    app_on = true;
    k_msleep(3000);
    int drops = sim_bt_stats.link_drops;
    int64_t worst = 0, worst_first = 0, slowest = 0;
    bool all = true, frames = true;
    int got = 0, bound = 0;
    struct lamp_effect_wire w;
    for (int i = 0; i < 5; i++) {
        fx_wire(&w, LAMP_EFFECT_SCANLINE, 2000 + 500 * i, fx_right[0]);
        art_relay_effect(true, true, &w);
        int64_t took;
        bool ok = fx_right_draws(&w, 3000, &took);
        all = all && ok;
        slowest = took > slowest ? took : slowest;
        int64_t lag = fx_lag_us();
        worst = lag > worst ? lag : worst;
        frames = frames && ok && fx_same_frame(&w, &got, &bound);
        printf("  effect %d: the right draws it after %lld ms; its clock %lld us behind\n", i,
               (long long)took, (long long)lag);
        k_msleep(1100 + 370 * i); // the right falls asleep again, more or less deep
    }
    check("right-draws-each", all, "slowest %lld ms", (long long)slowest);
    check("phase-within-a-frame", worst <= FX_FRAME_MS * 1000LL, "worst %lld us behind the left",
          (long long)worst);
    check("same-frame", frames, "last: right differs by %d, one frame moves %d", got, bound);
    // A wake from idle re-anchors the right's clock: GOs alone, with the right asleep in its
    // latency (or not) and its clock drifted 400 ms; the parameters stay. Six wakes at different
    // points of the right's sleep and of the app's traffic.
    int most_gos = 0;
    for (int i = 0; i < 6; i++) {
        k_msleep(4000 + 90 * i);
        int writes = sim_right.effect_writes, gos = sim_right.effect_gos;
        sim_right.effect_epoch_us += 400000;
        art_relay_effect_sync();
        int64_t start = k_uptime_get();
        while (sim_right.effect_gos < gos + 1 && k_uptime_get() - start < 3000) {
            k_msleep(5);
        }
        k_msleep(1000);
        int n = sim_right.effect_gos - gos;
        most_gos = n > most_gos ? n : most_gos;
        char name[24];
        snprintf(name, sizeof(name), "wake%d-resync", i + 1);
        check(name,
              n >= 1 && n <= 3 && sim_right.effect_writes == writes + n &&
                  memcmp(&sim_right.effect, &w, FX_SENT) == 0 &&
                  fx_lag_us() <= FX_FRAME_MS * 1000LL,
              "%d GO(s), %d write(s), the right %lld us behind", n,
              sim_right.effect_writes - writes, (long long)fx_lag_us());
    }
    printf("  wakes: at most %d GOs for one re-anchor\n", most_gos);
    check("no-relink", sim_bt_stats.link_drops == drops, "%d link drops",
          sim_bt_stats.link_drops - drops);
    check("none-dropped", sim_bt_stats.rsp_dropped == 0, "%d requests the right didn't answer",
          sim_bt_stats.rsp_dropped);
    debug_line("at the end");
}

// An effect lives through a re-link: the right's effect stops with the link (art_relay_peripheral.c)
// and the left sends it again on link-up, in phase; a PLACE while the link is down arrives as the
// newest; RELEASE then hands the right back to its own effect.
static void effect_relink(void) {
    wait_relay(40);
    check("relay-up", art_relay_link() == 2, "link %d", art_relay_link());
    app_on = true;
    struct lamp_effect_wire w;
    fx_wire(&w, LAMP_EFFECT_SCANLINE, 3000, fx_right[0]);
    art_relay_effect(true, true, &w);
    int64_t took;
    bool drawn = fx_right_draws(&w, 3000, &took);
    check("drawn", drawn, "after %lld ms", (long long)took);
    for (int i = 0; i < 2; i++) {
        sim_bt_drop(2000000);
        k_msleep(100);
        char name[32];
        snprintf(name, sizeof(name), "drop%d-right-stops", i + 1);
        check(name, !sim_right.effect_run && !sim_right.lamp_host, "right %s",
              sim_right.effect_run ? "still draws" : "own effect");
        snprintf(name, sizeof(name), "drop%d-left-says-no-link", i + 1);
        check(name, art_relay_effect_right() == ART_LAMPS_NO_LINK, "state %d",
              art_relay_effect_right());
        if (i == 1) { // the PC moves the halves meanwhile: the newest place goes on link-up
            fx_wire(&w, LAMP_EFFECT_SCANLINE, 3000, 214);
            art_relay_effect(true, true, &w);
        }
        k_msleep(3000);
        wait_relay(40);
        bool ok = fx_right_draws(&w, 5000, &took);
        k_msleep(100); // the GO's answer back at the left: its state
        int got = 0, bound = 0;
        bool frame = ok && fx_same_frame(&w, &got, &bound);
        snprintf(name, sizeof(name), "relink%d-drawn-in-phase", i + 1);
        check(name, ok && frame && fx_lag_us() <= FX_FRAME_MS * 1000LL,
              "after %lld ms, %lld us behind, differs by %d (a frame moves %d)", (long long)took,
              (long long)fx_lag_us(), got, bound);
        snprintf(name, sizeof(name), "relink%d-state-ok", i + 1);
        check(name, art_relay_effect_right() == ART_LAMPS_OK, "state %d", art_relay_effect_right());
    }
    int stops = sim_right.effect_stops;
    art_relay_effect(false, true, NULL); // RELEASE
    k_msleep(2000);
    check("release-hands-back", !sim_right.effect_run && !sim_right.lamp_host &&
                                    sim_right.effect_stops == stops + 1,
          "right %s, %d stop(s)", sim_right.effect_run ? "still draws" : "own effect",
          sim_right.effect_stops - stops);
    check("none-dropped", sim_bt_stats.rsp_dropped == 0, "%d requests the right didn't answer",
          sim_bt_stats.rsp_dropped);
    debug_line("at the end");
}

// The PC's frames keep priority: SET_FRAME takes the right from the effect (LAMP_HOST, no GO 0, so
// no flash of the right's own effect between), and the PC's next effect takes it back the same way
// (no LAMP_HOST 0 between), in phase.
static void effect_frames(void) {
    wait_relay(40);
    check("relay-up", art_relay_link() == 2, "link %d", art_relay_link());
    app_on = true;
    struct lamp_effect_wire w;
    fx_wire(&w, LAMP_EFFECT_SCANLINE, 2500, fx_right[0]);
    art_relay_effect(true, true, &w);
    int64_t took;
    bool drawn = fx_right_draws(&w, 3000, &took);
    check("drawn", drawn, "after %lld ms", (long long)took);
    // host_frames.c SHOW: host_effects_yield (a quiet stop), then the frame to the right.
    static uint8_t frame[ART_RELAY_LAMPS_MAX][3];
    memset(frame, 0x40, sizeof(frame));
    int stops = sim_right.effect_stops, hosts = sim_right.lamp_host_writes;
    art_relay_effect(false, false, NULL);
    art_relay_lamps(true, frame);
    int64_t start = k_uptime_get();
    while ((sim_right.effect_run || !sim_right.lamp_host) && k_uptime_get() - start < 3000) {
        k_msleep(5);
    }
    check("frames-take-over", !sim_right.effect_run && sim_right.lamp_host &&
                                  sim_right.effect_stops == stops,
          "right effect %s, frames %s, %d GO 0", sim_right.effect_run ? "runs" : "stopped",
          sim_right.lamp_host ? "on" : "off", sim_right.effect_stops - stops);
    k_msleep(1500);
    // The PC's effect again: host_frames_yield (frames released), then the effect.
    hosts = sim_right.lamp_host_writes;
    fx_wire(&w, LAMP_EFFECT_SCANLINE, 3500, fx_right[0]);
    art_relay_lamps(false, frame);
    art_relay_effect(true, true, &w);
    bool ok = fx_right_draws(&w, 3000, &took);
    k_msleep(500);
    check("effect-takes-back", ok && sim_right.lamp_host_writes == hosts &&
                                   fx_lag_us() <= FX_FRAME_MS * 1000LL,
          "after %lld ms, %d LAMP_HOST write(s) between, %lld us behind", (long long)took,
          sim_right.lamp_host_writes - hosts, (long long)fx_lag_us());
    check("none-dropped", sim_bt_stats.rsp_dropped == 0, "%d requests the right didn't answer",
          sim_bt_stats.rsp_dropped);
    debug_line("at the end");
}

// Key rings (LAMP_EFFECT_KEYS) and the right's firmware: a right before them (status version 12)
// is never sent the kind (it would refuse it and draw black): it keeps its own effect, one it drew
// before stops, and the left reports it can't; other effects still go. After an update to 13 the
// right gets the rings' parameters once, and the relay stays quiet while they run (each half
// starts its own rings from its own keys, effect_run.c).
static void effect_keys_old(void) {
    sim_right.version = ART_RELAY_SLOT_READ_STATUS_VERSION;
    wait_relay(40);
    check("relay-up", art_relay_link() == 2, "link %d", art_relay_link());
    app_on = true;
    struct lamp_effect_wire w, keys;
    fx_wire(&w, LAMP_EFFECT_SCANLINE, 2500, fx_right[0]);
    art_relay_effect(true, true, &w);
    int64_t took;
    bool drawn = fx_right_draws(&w, 3000, &took);
    check("old-right-draws-a-scanline", drawn, "after %lld ms", (long long)took);
    int stops = sim_right.effect_stops, writes = sim_right.effect_writes;
    fx_wire(&keys, LAMP_EFFECT_KEYS, 1200, fx_right[0]);
    art_relay_effect(true, true, &keys);
    k_msleep(2000);
    check("old-right-not-sent-rings", sim_right.effect_writes == writes &&
                                          sim_right.effect.kind != LAMP_EFFECT_KEYS &&
                                          sim_right.effect_black == 0,
          "%d writes, kind %d, %d black", sim_right.effect_writes - writes, sim_right.effect.kind,
          sim_right.effect_black);
    check("old-right-own-effect", !sim_right.effect_run && sim_right.effect_stops == stops + 1,
          "right %s, %d stop(s)", sim_right.effect_run ? "still draws" : "own effect",
          sim_right.effect_stops - stops);
    check("old-right-unsupported", art_relay_effect_right() == ART_LAMPS_UNSUPPORTED, "state %d",
          art_relay_effect_right());
    art_relay_effect(true, true, &w);
    bool again = fx_right_draws(&w, 3000, &took);
    k_msleep(100); // the GO's answer back at the left: its state
    check("old-right-scanline-again", again && art_relay_effect_right() == ART_LAMPS_OK,
          "after %lld ms, state %d", (long long)took, art_relay_effect_right());
    // The right updated to key rings; the left sends them on the new link.
    art_relay_effect(true, true, &keys);
    k_msleep(1000);
    sim_right.version = ART_RELAY_KEYS_STATUS_VERSION;
    sim_bt_right_update(false, 3000000);
    k_msleep(4000);
    wait_relay(40);
    bool ok = fx_right_draws(&keys, 5000, &took);
    k_msleep(100); // the GO's answer back at the left: its state
    check("new-right-gets-rings", ok && sim_right.effect_black == 0 &&
                                      art_relay_effect_right() == ART_LAMPS_OK,
          "after %lld ms, kind %d, state %d", (long long)took, sim_right.effect.kind,
          art_relay_effect_right());
    writes = sim_right.effect_writes;
    int gos = sim_right.effect_gos, reqs = sim_bt_stats.requests_sent;
    app_on = false;
    k_msleep(10000);
    check("rings-quiet", sim_right.effect_writes == writes && sim_right.effect_gos == gos,
          "%d parameter writes, %d GOs in 10 s (%d requests in all)",
          sim_right.effect_writes - writes, sim_right.effect_gos - gos,
          sim_bt_stats.requests_sent - reqs);
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
    } else if (!strcmp(scenario, "relink-busy")) {
        relink_busy();
    } else if (!strcmp(scenario, "rgb-relink-look")) {
        rgb_relink_look();
    } else if (!strcmp(scenario, "playlist-store")) {
        playlist_store();
    } else if (!strcmp(scenario, "playlist-store-old")) {
        playlist_store_old();
    } else if (!strcmp(scenario, "restore-update")) {
        restore_update();
    } else if (!strcmp(scenario, "slot-read")) {
        slot_read_scenario();
#ifdef SIM_TEST_HOOKS
    } else if (!strcmp(scenario, "art-hold-cycles")) {
        art_hold_cycles();
#endif
#ifdef ART_RELAY_KEYS_STATUS_VERSION
    } else if (!strcmp(scenario, "effect-phase")) {
        effect_phase();
    } else if (!strcmp(scenario, "effect-relink")) {
        effect_relink();
    } else if (!strcmp(scenario, "effect-frames")) {
        effect_frames();
    } else if (!strcmp(scenario, "effect-keys-old")) {
        effect_keys_old();
#endif
    } else if (!strcmp(scenario, "right-reflash")) {
        right_reflash(false);
    } else if (!strcmp(scenario, "right-reflash-stall")) {
        right_reflash(true);
#if __has_include("art_toast.h")
    } else if (!strcmp(scenario, "art-step-toast")) {
        art_step_toast();
    } else if (!strcmp(scenario, "art-step-boot")) {
        art_step_boot();
    } else if (!strcmp(scenario, "art-step-cycles")) {
        art_step_cycles(false);
    } else if (!strcmp(scenario, "art-step-cycles-lag")) {
        art_step_cycles(true);
    } else if (!strcmp(scenario, "art-step-kept")) {
        art_step_kept(KEPT_WAIT);
    } else if (!strcmp(scenario, "art-step-kept-quit")) {
        art_step_kept(KEPT_QUIT);
    } else if (!strcmp(scenario, "art-step-kept-relink")) {
        art_step_kept(KEPT_RELINK);
    } else if (!strcmp(scenario, "art-step-kept-restart")) {
        art_step_kept(KEPT_RESTART);
    } else if (!strcmp(scenario, "art-step-kept-bad")) {
        art_step_kept(KEPT_BAD);
#endif
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
