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
    sim_bt_connect(500000);
    if (!sim_run(1000LL * 1000000, is_done, NULL)) {
        check("progress", false, "every thread blocked and no timer left");
    }
    if (!lab_done) {
        check("time", false, "the scenario didn't end in 1000 s");
    }
    printf("RESULT %s%s\n", scenario, results);
    return failed ? 1 : 0;
}
