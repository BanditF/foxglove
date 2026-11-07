/*
 * relay-sim hooks-real-keys (#851): test_hooks.c's hold timeouts act only on the downs the hooks
 * own (D7 05 on the left, ART_RELAY_PRESS on the right), never on a key pressed for real.
 * SPDX-License-Identifier: MIT
 *
 * Built twice: as the left (CONFIG_ZMK_SPLIT_ROLE_CENTRAL) and as the right (a split peripheral).
 * A real key is a position event raised the way kscan raises it, straight into ZMK; test_hooks.c
 * never sees it. Every position event is recorded with who raised it. While the real key is held
 * the safety paths fire: the PC quiet for over 10 s and the 60 s cap (left), the 65 s cap and a
 * split-link disconnect (right). Passes when no event for the real key comes from anyone but its
 * own press and release, and each hook-owned down in the same run is released.
 */
#include <stdarg.h>
#include <stdio.h>
#include <stdlib.h>

#include "sim_hooks.h"
#include "sim_kernel.h"
#include "art_relay.h"
#include "host_link.h"
#include "test_hooks.h"

#define FOXGLOVE_CENTRAL (!IS_ENABLED(CONFIG_ZMK_SPLIT) || IS_ENABLED(CONFIG_ZMK_SPLIT_ROLE_CENTRAL))

static char results[1024];
static bool failed;

static void check(const char *name, bool ok, const char *fmt, ...) {
    char detail[256] = "";
    va_list ap;
    va_start(ap, fmt);
    vsnprintf(detail, sizeof(detail), fmt, ap);
    va_end(ap);
    printf("%s %-26s %s (t=%.1f s)\n", ok ? "PASS" : "FAIL", name, detail, sim_now_us() / 1e6);
    size_t n = strlen(results);
    snprintf(results + n, sizeof(results) - n, " %s=%s", name, ok ? "PASS" : "FAIL");
    failed |= !ok;
}

/* Position events, by who raised them */

static bool kscan; // set while the harness raises a real key's event
static int real_events, foreign_events; // the real key's: from kscan, from anyone else
static uint8_t real_pos;
static bool real_down;
uint64_t sim_hooks_positions; // held, by events from the hooks

int raise_zmk_position_state_changed(struct zmk_position_state_changed ev) {
    if (ev.position == real_pos) {
        if (kscan) {
            real_events++;
            real_down = ev.state;
        } else {
            foreign_events++;
            printf("  !! %s for the real key %u, not from kscan\n", ev.state ? "press" : "release",
                   (unsigned)ev.position);
        }
        return 0;
    }
    uint64_t bit = (uint64_t)1 << ev.position;
    sim_hooks_positions = ev.state ? sim_hooks_positions | bit : sim_hooks_positions & ~bit;
    return 0;
}

static void kscan_press(uint8_t pos, bool state) {
    kscan = true;
    raise_zmk_position_state_changed((struct zmk_position_state_changed){
        .source = ZMK_POSITION_STATE_CHANGE_SOURCE_LOCAL,
        .position = pos,
        .state = state,
        .timestamp = k_uptime_get()});
    kscan = false;
}

uint32_t sys_rand32_get(void) { return 0x5eed1234; }

/* The left's PC link and relay, as far as test_hooks.c calls them */

static int replies;
static uint8_t reply_status;
void host_link_position_reply(uint8_t status, uint8_t pos) {
    (void)pos;
    replies++;
    reply_status = status;
}
void host_link_bootloader_reply(uint8_t status, uint8_t half) {
    (void)status;
    (void)half;
}

static uint64_t right_held; // what the left's relay told the right to hold
static uint8_t relay_pos, relay_state;
static void relay_cb(struct k_work *w) {
    (void)w;
    uint64_t bit = (uint64_t)1 << relay_pos;
    right_held = relay_state ? right_held | bit : right_held & ~bit;
    test_hooks_relay_done(0);
}
static K_WORK_DEFINE(relay_work, relay_cb);
void art_relay_press(uint8_t pos, uint8_t state) {
    relay_pos = pos;
    relay_state = state;
    k_work_submit(&relay_work);
}
#if FOXGLOVE_CENTRAL
void art_relay_boot(void) { test_hooks_boot_relay_done(2); }
#endif

// D7 05 from the PC (the left), or ART_RELAY_PRESS from the left (the right): 0 when taken and,
// on the left, answered 0.
static int hook_press(uint8_t pos, uint8_t state) {
#if FOXGLOVE_CENTRAL
    test_hooks_host_seen();
    int before = replies;
    if (test_hooks_press(pos, state) != 0) {
        return -1;
    }
    for (int i = 0; i < 100 && replies == before; i++) {
        k_msleep(10);
    }
    return replies == before ? -2 : reply_status;
#else
    int r = test_hooks_press(pos, state);
    k_msleep(10); // its press work runs on the system queue
    return r;
#endif
}

static bool hook_held(uint8_t pos) {
    uint64_t bit = (uint64_t)1 << pos;
    return (sim_hooks_positions | right_held) & bit;
}

// Waits ms, the PC talking every second when `talking` (the left; the right can't see it).
static void wait_ms(int ms, bool talking) {
    for (int t = 0; t < ms; t += 100) {
        if (talking && t % 1000 == 0) {
            test_hooks_host_seen();
        }
        k_msleep(100);
    }
}

#if !FOXGLOVE_CENTRAL
extern const struct bt_conn_cb test_hooks_conn_cb;
#endif

static void scenario(void) {
    char name[40];
#if FOXGLOVE_CENTRAL
    const char *half = "left";
    real_pos = 1; // a left key (Q), held for real throughout
    const uint8_t hook_left = 43, hook_right = 46;
#else
    const char *half = "right";
    real_pos = 11; // a right key (P), held for real throughout
    const uint8_t hook_left = 46, hook_right = 47; // both the right's own, relayed to it
#endif
    kscan_press(real_pos, true);
    snprintf(name, sizeof(name), "%s-real-down", half);
    check(name, real_down && real_events == 1, "key %u down from kscan", real_pos);

#if FOXGLOVE_CENTRAL
    // 1. The PC goes quiet: the left lets its own downs go after 10 s, the real key stays.
    int a = hook_press(hook_left, 1), b = hook_press(hook_right, 1);
    wait_ms(12000, false);
    check("left-quiet-hooks-released", a == 0 && b == 0 && !hook_held(hook_left) && !hook_held(hook_right),
          "hook downs %u (local) and %u (relayed): %s, %s after 12 s without the PC", hook_left,
          hook_right, hook_held(hook_left) ? "held" : "released",
          hook_held(hook_right) ? "held" : "released");
    check("left-quiet-real-kept", real_down && foreign_events == 0, "key %u %s, %d foreign events",
          real_pos, real_down ? "still down" : "released", foreign_events);
    // 2. The PC keeps talking: the 60 s cap lets the hook's downs go, the real key stays.
    a = hook_press(hook_left, 1);
    b = hook_press(hook_right, 1);
    wait_ms(30000, true);
    bool mid = hook_held(hook_left) && hook_held(hook_right);
    wait_ms(31000, true);
    check("left-cap-hooks-released", a == 0 && b == 0 && mid && !hook_held(hook_left) && !hook_held(hook_right),
          "held at 30 s: %s; after 61 s with the PC talking: %s", mid ? "yes" : "no",
          hook_held(hook_left) || hook_held(hook_right) ? "still held" : "released");
    check("left-cap-real-kept", real_down && foreign_events == 0, "key %u %s, %d foreign events",
          real_pos, real_down ? "still down" : "released", foreign_events);
#else
    // 1. The 65 s cap: the right lets the relayed down go, the real key stays.
    int a = hook_press(hook_left, 1);
    wait_ms(30000, false);
    bool mid = hook_held(hook_left);
    wait_ms(36000, false);
    check("right-cap-hook-released", a == 0 && mid && !hook_held(hook_left),
          "held at 30 s: %s; after 66 s: %s", mid ? "yes" : "no",
          hook_held(hook_left) ? "still held" : "released");
    check("right-cap-real-kept", real_down && foreign_events == 0, "key %u %s, %d foreign events",
          real_pos, real_down ? "still down" : "released", foreign_events);
    // 2. The split link goes: the right lets the relayed downs go, the real key stays.
    a = hook_press(hook_left, 1);
    int b = hook_press(hook_right, 1);
    k_msleep(500);
    test_hooks_conn_cb.disconnected(NULL, 0x13);
    k_msleep(500);
    check("right-link-hooks-released", a == 0 && b == 0 && !hook_held(hook_left) && !hook_held(hook_right),
          "after the disconnect: %u %s, %u %s", hook_left,
          hook_held(hook_left) ? "held" : "released", hook_right,
          hook_held(hook_right) ? "held" : "released");
    check("right-link-real-kept", real_down && foreign_events == 0, "key %u %s, %d foreign events",
          real_pos, real_down ? "still down" : "released", foreign_events);
#endif
    // The real release is the real key's only release.
    kscan_press(real_pos, false);
    wait_ms(2000, false);
    snprintf(name, sizeof(name), "%s-real-up", half);
    check(name, !real_down && real_events == 2 && foreign_events == 0,
          "key %u: %d events from kscan (down, up), %d from anyone else", real_pos, real_events,
          foreign_events);
}

static bool done;
static void thread(void *arg) {
    (void)arg;
    scenario();
    done = true;
    for (;;) {
        k_msleep(1000000);
    }
}

static bool is_done(void *arg) {
    (void)arg;
    return done;
}

int main(int argc, char **argv) {
    setvbuf(stdout, NULL, _IONBF, 0);
    if (argc > 2) {
        extern int sim_log_level;
        sim_log_level = atoi(argv[2]);
    }
    printf("relay-sim hooks-real-keys (%s)\n", FOXGLOVE_CENTRAL ? "left" : "right");
    sim_kernel_init();
    k_work_queue_start(&k_sys_work_q, NULL, 0, -1, NULL);
    sim_thread_create("usb", 5, thread, NULL);
    if (!sim_run(1000LL * 1000000, is_done, NULL)) {
        check("progress", false, "every thread blocked and no timer left");
    }
    printf("RESULT hooks-real-keys-%s%s\n", FOXGLOVE_CENTRAL ? "left" : "right", results);
    return failed ? 1 : 0;
}
