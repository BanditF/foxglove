/*
 * Simulator case for a single-screen board's local art target (tests/zmk-sim/art-local, #834): the
 * real art_store.c and art_local.c behind the real host_link.c, as on a board whose art target
 * sits on the host-link half (a Toucan2-sized 144 x 112 target, 32 KiB sets). No relay and no
 * BLE: each D6 reply is final when it comes. This plays the PC; the board's screens table, its
 * drawer and its flash slots are faked here (the drawer prints what it plays, the slots live in
 * RAM and finish their writes on the system queue, as art_slots.c's own queue would).
 * Printed: the PC's requests and each D6 reply decoded here, the art played, and D5 86 / D6 87 raw
 * (hostlink_sim.c).
 * SPDX-License-Identifier: MIT
 */

#include <string.h>

#include <zephyr/kernel.h>
#include <zephyr/sys/byteorder.h>
#include <zephyr/sys/crc.h>
#include <zephyr/sys/printk.h>

#include <raw_hid/events.h>
#include <zmk/event_manager.h>

#include "posix_board_if.h"

#include "art_pace.h"
#include "art_slots.h"
#include "art_store.h"
#include "custom_art.h"
#include "host_link.h"

/* The board: one 144 x 168 display on half 0, the PC's panel in its bottom third, and the art
 * target above it, on the same half (the host link's): local. The art-epaper case (#842) makes the
 * art target e-paper with partial refresh and a shortest frame of 1 s (v2 descriptor). */

BUILD_ASSERT(sizeof(struct screen_descriptor) == 24, "descriptor layout");

#define SIM_SLOTS 2 // the board's flash slots (faked in RAM below)

const struct screen_descriptor foxglove_screens[] = {
    {.id = 0, .kind = SCREEN_DISPLAY, .half = 0, .parent = SCREEN_UNSET, .width = 144,
     .height = 168, .stride = 18, .readback_half = SCREEN_UNSET},
    {.id = 1, .kind = SCREEN_PANEL, .half = 0, .parent = 0, .width = 144, .height = 56, .y = 112,
     .ops = SCREEN_OP_ROWS | SCREEN_OP_ROWS_AT, .stride = 18, .rows_per_report = 1,
     .readback_half = SCREEN_UNSET},
    // v2 (#832): the local store's largest blob and its slots, a fast screen with no frame limit.
    {.id = 2, .kind = SCREEN_ART, .half = 0, .parent = 0, .width = ART_STORE_W,
     .height = ART_STORE_H, .ops = SCREEN_OP_ART | SCREEN_OP_SLOTS, .stride = ART_STORE_MAX_SIZE,
     .readback_half = SCREEN_UNSET, .art_slots = SIM_SLOTS,
#if IS_ENABLED(CONFIG_FOXGLOVE_SIM_ART_EPAPER)
     // The art-epaper case (#842): e-paper with partial refresh, a frame a second at most.
     .art_refresh = SCREEN_REFRESH_EPAPER_PARTIAL, .art_min_frame_100ms = 10
#else
     .art_refresh = SCREEN_REFRESH_FAST
#endif
    },
};
const uint8_t foxglove_screen_count = ARRAY_SIZE(foxglove_screens);

/* The drawer: two built-in sets, a still one and an animated one (2 frames, 8 fps). It prints
 * what it plays and the pace it would play it at (art_pace.h, #842): a frame every n ms, or the
 * first frame only. */

static const uint8_t builtin_delta[] = {0x00};
static const uint8_t *const builtin_deltas[] = {builtin_delta};
static const uint8_t builtin_order[] = {0, 0};
const struct custom_art_set custom_art_sets[] = {
    {.name = "one", .deltas = builtin_deltas, .order = builtin_order, .frame_count = 1},
    {.name = "two", .deltas = builtin_deltas, .order = builtin_order, .frame_count = 2, .fps = 8},
};
const int custom_art_set_count = ARRAY_SIZE(custom_art_sets);

static const struct custom_art_set *on_screen;

static void print_pace(const struct custom_art_set *set) {
    uint32_t ms = art_pace_frame_ms(set);
    if (ms == 0) {
        printk("first frame only\n");
    } else {
        printk("every %u ms\n", ms);
    }
}

void art_play(const struct custom_art_set *set) {
    on_screen = set;
    printk("art: play %s, %d frame(s), %d fps, ", set->name, set->frame_count, set->fps);
    print_pace(set);
}
void art_repace(void) {
    if (on_screen != NULL) {
        printk("art: repace %s, ", on_screen->name);
        print_pace(on_screen);
    }
}
void art_hold(void) {}
void art_redraw_all(void) {}
// Out of range (-1) picks set 0 here, not a random one: the trace stays the same every run.
int art_builtin_index(int index) { return index >= 0 && index < custom_art_set_count ? index : 0; }

/* Flash slots, in RAM: two of them. A write ends 50 ms later on the system queue. */

static uint8_t flash[SIM_SLOTS][ART_STORE_MAX_SIZE];
static struct art_relay_slot_info info[SIM_SLOTS];
static int job_slot;
static const uint8_t *job_blob;
static uint16_t job_len;
static char job_name[16];
static void (*job_done)(int err);
static atomic_t busy;

static void job_cb(struct k_work *work) {
    struct art_relay_slot_info *i = &info[job_slot];
    i->writes++;
    i->used = job_blob != NULL;
    i->len = job_blob != NULL ? job_len : 0;
    memset(i->name, 0, sizeof(i->name));
    if (job_blob != NULL) {
        memcpy(flash[job_slot], job_blob, job_len);
        memcpy(i->name, job_name, sizeof(i->name));
    }
    printk("art: slot %d %s\n", job_slot, job_blob != NULL ? "written" : "erased");
    atomic_clear(&busy);
    job_done(0);
}
static K_WORK_DELAYABLE_DEFINE(job_work, job_cb);

static int start(int slot, const uint8_t *blob, uint16_t len, const char *name,
                 void (*done)(int err)) {
    if (slot < 0 || slot >= SIM_SLOTS) {
        return -EINVAL;
    }
    if (!atomic_cas(&busy, 0, 1)) {
        return -EBUSY;
    }
    job_slot = slot;
    job_blob = blob;
    job_len = len;
    memset(job_name, 0, sizeof(job_name));
    if (name != NULL) {
        memcpy(job_name, name, sizeof(job_name));
    }
    job_done = done;
    k_work_reschedule(&job_work, K_MSEC(50));
    return 0;
}

int art_slots_count(void) { return SIM_SLOTS; }
uint8_t art_slots_used(void) {
    uint8_t used = 0;
    for (int i = 0; i < SIM_SLOTS; i++) {
        used |= info[i].used ? BIT(i) : 0;
    }
    return used;
}
void art_slots_info(int slot, struct art_relay_slot_info *out) { *out = info[slot]; }
// The header's CRC as art_slots.c keeps it: the set's, as BEGIN announced it (#863).
uint32_t art_slots_crc(int slot) {
    return slot >= 0 && slot < SIM_SLOTS && info[slot].used ? crc32_ieee(flash[slot], info[slot].len)
                                                            : 0;
}
const uint8_t *art_slots_blob(int slot, uint16_t *len) {
    if (slot < 0 || slot >= SIM_SLOTS || !info[slot].used) {
        return NULL;
    }
    *len = info[slot].len;
    return flash[slot];
}
int art_slots_save(int slot, const uint8_t *blob, uint16_t len, const char name[16],
                   void (*done)(int err)) {
    return start(slot, blob, len, name, done);
}
int art_slots_erase(int slot, void (*done)(int err)) { return start(slot, NULL, 0, NULL, done); }

/* The PC */

static void note(const char *what) { printk("artlocal: -- %s\n", what); }

// Each D6 reply, decoded: the "left" state and error (the relay's names), then the store's status.
// HELLO's art bits and D5 88's word 2 as well.
static int reply_listener(const zmk_event_t *eh) {
    const struct raw_hid_sent_event *ev = as_raw_hid_sent_event(eh);
    if (ev == NULL || ev->length < 32) {
        return ZMK_EV_EVENT_BUBBLE;
    }
    const uint8_t *r = ev->data;
    if (r[0] == HOST_LINK_PANEL && r[1] == 0x81) {
        uint32_t caps = sys_get_le32(&r[21]);
        printk("artlocal: HELLO art push %u, slots %u, store %u, status events %u\n",
               (caps >> 1) & 1, (caps >> 10) & 1, (caps >> 30) & 1, (caps >> 3) & 1);
    } else if (r[0] == HOST_LINK_PANEL && r[1] == 0x88) {
        uint32_t w2 = sys_get_le32(&r[3]);
        printk("artlocal: CAPS words %u, word 2 0x%08x (ART_LOCAL %u, ART_PACE %u, SLOT_READ "
               "%u)\n",
               r[2], w2, w2 & HOST_LINK_CAPS2_ART_LOCAL ? 1 : 0,
               w2 & HOST_LINK_CAPS2_ART_PACE ? 1 : 0, w2 & HOST_LINK_CAPS2_SLOT_READ ? 1 : 0);
    } else if (r[0] == HOST_LINK_ART && r[1] != 0x87 && r[1] != 0x8B) {
        static const char *const left[] = {"idle", "receiving", "pushing", "done", "failed"};
        struct art_relay_status st;
        memcpy(&st, &r[9], sizeof(st));
        printk("artlocal: reply %02X %s error %u, %u/%u, link %u; v%u store %u received %u of "
               "%u, set %d of %u, slots %u used 0x%02x\n",
               r[1], r[2] < ARRAY_SIZE(left) ? left[r[2]] : "?", r[3], sys_get_le16(&r[6]),
               sys_get_le16(&r[4]), r[8], st.version, st.state, st.received, st.capacity,
               st.set, st.set_count, st.flash_slots, st.flash_used);
    }
    return ZMK_EV_EVENT_BUBBLE;
}

ZMK_LISTENER(art_local_sim, reply_listener);
ZMK_SUBSCRIPTION(art_local_sim, raw_hid_sent_event);

static void pc(const uint8_t *d, int len) {
    static uint8_t report[32];
    memset(report, 0, sizeof(report));
    memcpy(report, d, len);
    printk("artlocal: pc sends");
    for (int i = 0; i < MIN(len, 8); i++) {
        printk(" %02X", d[i]);
    }
    printk(len > 8 ? " ...\n" : "\n");
    raise_raw_hid_received_event((struct raw_hid_received_event){.data = report, .length = 32});
    k_msleep(200); // replies go out from the system work queue
}

#define PC(...)                                                                                    \
    do {                                                                                           \
        const uint8_t d_[] = {__VA_ARGS__};                                                        \
        pc(d_, sizeof(d_));                                                                        \
    } while (0)

// An NVA1 set of w x h: 2 frames of one unique delta (4 bytes), 4 fps, ink. 20 bytes.
static uint16_t blob(uint8_t *b, uint8_t w, uint8_t h) {
    const uint8_t set[] = {'N', 'V', 'A', '1', w, h, 4, 1, 1, 0, 2, 0, // header
                           0, 0,                                         // order
                           16, 0,                                        // delta 0 at 16
                           0x01, 0x02, 0x03, 0x04};
    memcpy(b, set, sizeof(set));
    return sizeof(set);
}

static void begin(uint16_t len, uint32_t crc) {
    uint8_t d[8] = {HOST_LINK_ART, 0x01};
    sys_put_le16(len, &d[2]);
    sys_put_le32(crc, &d[4]);
    pc(d, sizeof(d));
}

static void data(uint16_t at, const uint8_t *b, uint8_t n) {
    uint8_t d[32] = {HOST_LINK_ART, 0x02};
    sys_put_le16(at, &d[2]);
    d[4] = n;
    memcpy(&d[5], b, n);
    pc(d, 5 + n);
}

// BEGIN, DATA (one report: the sets here are 20 bytes), with crc_xor spoiling the CRC.
static void send_set(uint8_t w, uint8_t h, uint32_t crc_xor) {
    uint8_t b[32];
    uint16_t n = blob(b, w, h);
    begin(n, crc32_ieee(b, n) ^ crc_xor);
    data(0, b, n);
}

static void store(uint8_t slot, const char *name) {
    uint8_t d[19] = {HOST_LINK_ART, 0x09, slot};
    strncpy((char *)&d[3], name, 16);
    pc(d, sizeof(d));
}

static void animate(uint8_t flags) {
    uint8_t d[3] = {HOST_LINK_ART, 0x0A, flags};
    pc(d, sizeof(d));
}

/* The art-epaper case (#842): an e-paper target, partial refresh, a frame a second at most. */
static void epaper_driver(void) {
    note("hello: word 2's ART_PACE; the art target's v2 descriptor: e-paper (1), 1 s (10)");
    PC(0xD5, 0x01, 0x01);
    PC(0xD5, 0x08);
    PC(0xD5, 0x06, 0x02);

    note("built-in 1 (2 frames, 8 fps): e-paper without the PC's go, first frame only");
    PC(0xD6, 0x04, 0x01);
    note("animate off (as each connect sends it): no change, nothing re-timed");
    animate(0x00);

    note("the PC lets art animate (D6 0A 01): re-timed, held to 1 s (8 fps would be 125 ms)");
    animate(0x01);
    note("the same again: nothing re-timed");
    animate(0x01);
    note("built-in 0, a still set: stays still");
    PC(0xD6, 0x04, 0x00);

    note("push a 4 fps set: held to 1 s");
    send_set(144, 112, 0);
    PC(0xD6, 0x03);
    note("keep it in slot 0 and play it from there: held to 1 s");
    {
        uint8_t d[19] = {HOST_LINK_ART, 0x06, 0};
        strncpy((char *)&d[3], "kept", 16);
        pc(d, sizeof(d));
    }
    PC(0xD6, 0x05);
    PC(0xD6, 0x04, 0x02);

    note("animate off (D6 0A 00): the kept set shows its first frame only");
    animate(0x00);

    note("ANIMATE mid-push: the receive goes on; the set plays, still");
    {
        uint8_t b[32];
        uint16_t n = blob(b, 144, 112);
        begin(n, crc32_ieee(b, n));
        animate(0x00);
        data(0, b, n);
        PC(0xD6, 0x03);
    }

    note("done");
    posix_exit(0); // the end of the run: nothing may print after it
}

void art_local_sim_driver(void) {
    if (IS_ENABLED(CONFIG_FOXGLOVE_SIM_ART_EPAPER)) {
        epaper_driver();
        return;
    }
    note("hello: the art bits, word 2's ART_LOCAL, the screens (art target on half 0, v2)");
    PC(0xD5, 0x01, 0x01);
    PC(0xD5, 0x08);
    for (uint8_t i = 0; i <= 3; i++) {
        PC(0xD5, 0x06, i);
    }

    note("status: link 2 at once, no relay to wait for");
    PC(0xD6, 0x05);

    note("select built-in 1: done in the reply");
    PC(0xD6, 0x04, 0x01);

    note("push a 144x112 set: COMMIT's reply is final, the set plays");
    send_set(144, 112, 0);
    PC(0xD6, 0x03);

    note("push the Corne's 140x68: bad blob (error 7, store 4); the set on screen stays");
    send_set(140, 68, 0);
    PC(0xD6, 0x03);
    note("push with a wrong CRC: error 5, store 3");
    send_set(144, 112, 1);
    PC(0xD6, 0x03);
    note("too big (65535 over 32768): error 2, store 5");
    begin(0xFFFF, 0);
    note("data past the announced length: error 4");
    begin(4, 0);
    {
        uint8_t b[8] = {0};
        data(2, b, 8);
    }

    note("save the set on screen into slot 0: saving, then saved");
    {
        uint8_t d[19] = {HOST_LINK_ART, 0x06, 0};
        strncpy((char *)&d[3], "pushed", 16);
        pc(d, sizeof(d));
    }
    PC(0xD6, 0x05);
    note("slot 0, then a slot the board doesn't have: D6 87 at once");
    PC(0xD6, 0x07, 0x00);
    PC(0xD6, 0x07, 0x05);
    note("SLOT_READ (#878): slot 0's set back at once, then its end (n 0)");
    PC(0xD6, 0x0B, 0x00, 0x00, 0x00);
    PC(0xD6, 0x0B, 0x00, 0x14, 0x00);
    note("SLOT_READ refused (5): past the end, an empty slot, a slot the board doesn't have");
    PC(0xD6, 0x0B, 0x00, 0x15, 0x00);
    PC(0xD6, 0x0B, 0x01, 0x00, 0x00);
    PC(0xD6, 0x0B, 0x05, 0x00, 0x00);
    note("play slot 0 (select set_count + 0)");
    PC(0xD6, 0x04, 0x02);

    note("store a set into slot 1 without showing it: saving, then back to idle");
    send_set(144, 112, 0);
    store(1, "staged");
    PC(0xD6, 0x05);
    note("store into slot 0, on screen: refused (error 6)");
    send_set(144, 112, 0);
    store(0, "nope");

    note("erase slot 0, on screen: back to a built-in, saving, then saved");
    PC(0xD6, 0x08, 0x00);
    PC(0xD6, 0x05);

    note("done");
    posix_exit(0); // the end of the run: nothing may print after it
}
