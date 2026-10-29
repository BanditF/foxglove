/*
 * Left half: collects an art blob from the PC (host_link.c), then pushes it to the right half's
 * art relay service (art_relay_peripheral.c) over the split connection.
 * SPDX-License-Identifier: MIT
 *
 * The service isn't part of ZMK's split protocol, so this finds it on its own: well after the
 * split connection is secured (once ZMK's own discovery is done), it discovers the service and
 * its characteristics. Pushes run on their own work queue, where blocking on the radio is fine;
 * every request there has a timeout, and a push holds its own reference to the connection so the
 * link dropping mid-push only fails the push.
 */

#include <string.h>

#include <zephyr/init.h>
#include <zephyr/kernel.h>
#include <zephyr/bluetooth/bluetooth.h>
#include <zephyr/bluetooth/conn.h>
#include <zephyr/bluetooth/hci.h>
#include <zephyr/bluetooth/gatt.h>
#include <zephyr/bluetooth/uuid.h>
#include <zephyr/sys/byteorder.h>
#include <zephyr/sys/crc.h>

#include <zephyr/logging/log.h>
LOG_MODULE_DECLARE(zmk, CONFIG_ZMK_LOG_LEVEL);

#if IS_ENABLED(CONFIG_NICE_VIEW_RGB_IDLE)
#include <zmk/activity.h>
#include <zmk/rgb_underglow.h>

#include "rgb_idle.h"
#endif

#include "art_relay.h"
#include "art_relay_doorbell.h"
#if IS_ENABLED(CONFIG_FOXGLOVE_HOST_EFFECTS)
#include "effect_run.h"
#endif
#include "half_marker.h"
#include "host_link.h"
#include "screen_snap.h"
#if IS_ENABLED(CONFIG_FOXGLOVE_TEST_HOOKS)
#include <zmk/behavior.h>
#include <zmk/split/central.h>

#include "test_hooks.h"
#endif

// ZMK discovers and subscribes to its own split service once the link is secured; a discovery
// of ours running alongside that wedged both (requests never answered), so wait it out.
#define DISCOVER_DELAY K_SECONDS(15)
#define DISCOVER_RETRY K_SECONDS(5)
#define DISCOVER_TRIES 20
#define GATT_TIMEOUT_MS 3000
#define GATT_TIMEOUT K_MSEC(GATT_TIMEOUT_MS)

static uint8_t blob[CONFIG_NICE_VIEW_ART_RELAY_MAX_SIZE];
static uint16_t blob_len;
static uint32_t blob_crc;
static uint16_t pushed;
static uint8_t state = ART_LEFT_IDLE;
static uint8_t error;
static struct art_relay_status right;
static uint16_t right_len;   // bytes of it the last status read filled
static int64_t right_read_at; // uptime of that read, 0 = never
// What the right's LEDs show, from its last doorbell on this link (#605), while valid.
static struct rgb_shown_wire right_shown;
static atomic_t right_shown_valid;
static void rgb_reconcile(void);

static struct bt_conn *peer;
static int64_t peer_secure_at; // uptime peer was first seen encrypted, 0 = not yet
static uint16_t data_handle, status_handle, screen_handle;

static const struct bt_uuid_128 service_uuid = BT_UUID_INIT_128(ART_RELAY_SERVICE_UUID);
static const struct bt_uuid_128 data_uuid = BT_UUID_INIT_128(ART_RELAY_DATA_UUID);
static const struct bt_uuid_128 screen_uuid = BT_UUID_INIT_128(ART_RELAY_SCREEN_UUID);
static const struct bt_uuid_128 status_uuid = BT_UUID_INIT_128(ART_RELAY_STATUS_UUID);

// Counters and last results, reported to the PC in the HELLO reply (art_relay_debug).
static struct {
    uint8_t chrcs;     // characteristics seen by discovery
    uint8_t ends;      // discovery procedures that finished
    int8_t write_ret;  // last bt_gatt_write / bt_gatt_read return value
    uint8_t att_err;   // last ATT error from a write or read callback
    uint8_t timeouts;  // requests that got no callback within GATT_TIMEOUT
    uint8_t answered;  // callbacks for requests
} dbg;

/* Discovery */

static struct bt_gatt_discover_params discover_params;
static int8_t discovery = ART_DISCOVERY_IDLE;
static uint8_t discovery_tries;
static struct k_work_delayable discover_work;
static void rgb_sync_link_up(void);
static void lamps_link_up(void);
static void effect_link_up(void);
static void doorbell_link_up(void);
static void activity_link_up(void);

// Tries again a little later, in case ZMK's own setup of the link got in the way.
static void discovery_failed(int8_t result) {
    discovery = result;
    if (peer != NULL && discovery_tries < DISCOVER_TRIES) {
        k_work_reschedule(&discover_work, DISCOVER_RETRY);
    }
}

static atomic_t discovering; // discover_params belong to Zephyr until the procedure ends

static uint8_t chrc_found(struct bt_conn *conn, const struct bt_gatt_attr *attr,
                          struct bt_gatt_discover_params *params) {
    if (conn != peer) { // the end of a discovery on a link that has gone
        atomic_clear(&discovering);
        return BT_GATT_ITER_STOP;
    }
    if (attr == NULL) {
        atomic_clear(&discovering);
    }
    if (attr == NULL) {
        dbg.ends++;
        LOG_INF("art relay: right half %s", data_handle ? "ready" : "has no relay");
        if (data_handle && status_handle) {
            discovery = ART_DISCOVERY_DONE;
            rgb_sync_link_up();
            lamps_link_up();
            effect_link_up();
            doorbell_link_up();
            activity_link_up();
            host_link_art_status();
        } else {
            discovery_failed(ART_DISCOVERY_NO_CHRC);
        }
        return BT_GATT_ITER_STOP;
    }
    dbg.chrcs++;
    const struct bt_gatt_chrc *chrc = attr->user_data;
    if (bt_uuid_cmp(chrc->uuid, &data_uuid.uuid) == 0) {
        data_handle = chrc->value_handle;
    } else if (bt_uuid_cmp(chrc->uuid, &status_uuid.uuid) == 0) {
        status_handle = chrc->value_handle;
    } else if (bt_uuid_cmp(chrc->uuid, &screen_uuid.uuid) == 0) {
        screen_handle = chrc->value_handle;
    }
    if (data_handle && status_handle &&
        (screen_handle || chrc->value_handle == params->end_handle)) {
        // Older firmware ends at status; new firmware ends at screen.
        // Stop here: at MTU 23 each response holds one characteristic, and the request Zephyr
        // sends to look past the last one was never answered, which stalled every request
        // queued after it on the split link (ZMK's own included).
        dbg.ends++;
        discovery = ART_DISCOVERY_DONE;
        atomic_clear(&discovering); // Zephyr gets no callback after a STOP
        rgb_sync_link_up();
        lamps_link_up();
        effect_link_up();
        doorbell_link_up();
        activity_link_up();
        host_link_art_status();
        return BT_GATT_ITER_STOP;
    }
    return BT_GATT_ITER_CONTINUE;
}

static uint8_t service_found(struct bt_conn *conn, const struct bt_gatt_attr *attr,
                             struct bt_gatt_discover_params *params) {
    if (conn != peer || attr == NULL) {
        atomic_clear(&discovering);
    }
    if (conn != peer) {
        return BT_GATT_ITER_STOP;
    }
    if (attr == NULL) {
        dbg.ends++;
        LOG_INF("art relay: right half has no relay service");
        discovery_failed(ART_DISCOVERY_NO_SERVICE);
        return BT_GATT_ITER_STOP;
    }
    const struct bt_gatt_service_val *svc = attr->user_data;
    discover_params.uuid = NULL;
    discover_params.func = chrc_found;
    discover_params.start_handle = attr->handle + 1;
    discover_params.end_handle = svc->end_handle;
    discover_params.type = BT_GATT_DISCOVER_CHARACTERISTIC;
    int err = bt_gatt_discover(conn, &discover_params);
    if (err) {
        atomic_clear(&discovering);
        LOG_ERR("art relay: characteristic discovery failed (%d)", err);
        discovery_failed(MAX(err, INT8_MIN));
    }
    return BT_GATT_ITER_STOP;
}

static void discover_cb(struct k_work *work) {
    if (peer == NULL || (data_handle && status_handle)) {
        return;
    }
    if (!atomic_cas(&discovering, 0, 1)) { // the last one (maybe on the old link) hasn't ended
        k_work_reschedule(&discover_work, DISCOVER_RETRY);
        return;
    }
    discovery_tries++;
    discovery = ART_DISCOVERY_RUNNING;
    discover_params.uuid = &service_uuid.uuid;
    discover_params.func = service_found;
    discover_params.start_handle = 0x0001;
    discover_params.end_handle = 0xffff;
    discover_params.type = BT_GATT_DISCOVER_PRIMARY;
    int err = bt_gatt_discover(peer, &discover_params);
    if (err) {
        atomic_clear(&discovering);
        LOG_ERR("art relay: service discovery failed (%d)", err);
        discovery_failed(MAX(err, INT8_MIN));
    }
}
static K_WORK_DELAYABLE_DEFINE(discover_work, discover_cb);

// Forgets the split connection and everything found on it.
static void drop_peer(void) {
    k_work_cancel_delayable(&discover_work);
    if (peer != NULL) {
        bt_conn_unref(peer);
        peer = NULL;
    }
    peer_secure_at = 0;
    data_handle = status_handle = screen_handle = 0;
    discovery = ART_DISCOVERY_IDLE;
    discovery_tries = 0;
    right_read_at = 0;
    atomic_clear(&right_shown_valid);
    memset(&dbg, 0, sizeof(dbg));
}

static void guard_arm(void);
static void guard_cancel(bool encrypted);

static bool still_connected(struct bt_conn *conn) {
    struct bt_conn_info info;
    return bt_conn_get_info(conn, &info) == 0 && info.state == BT_CONN_STATE_CONNECTED;
}

static void connected(struct bt_conn *conn, uint8_t err) {
    struct bt_conn_info info;
    if (err || bt_conn_get_info(conn, &info) || info.role != BT_CONN_ROLE_CENTRAL) {
        return; // only the split link has us as central; host links have us as peripheral
    }
    if (peer != NULL && peer != conn && still_connected(peer)) {
        return; // a second link with us as central isn't the split link we know
    }
    // A new split link, even if the old one's disconnect hasn't come yet (the right rebooted):
    // holding on to the dead one left the relay pointed at it and never discovered the new one.
    bool was_ready = art_relay_link() == 2;
    drop_peer();
    peer = bt_conn_ref(conn);
    guard_arm();
    if (bt_conn_get_security(conn) >= BT_SECURITY_L2) { // already encrypted: no security_changed
        peer_secure_at = k_uptime_get();
        k_work_reschedule(&discover_work, DISCOVER_DELAY);
    }
    if (was_ready) {
        host_link_art_status();
        host_link_rgb_right_changed(); // the right's state isn't known any more (#478)
    }
}

static void security_changed(struct bt_conn *conn, bt_security_t level, enum bt_security_err err) {
    if (conn == peer && !err && level >= BT_SECURITY_L2) {
        if (peer_secure_at == 0) {
            peer_secure_at = k_uptime_get();
        }
        guard_cancel(true);
        if (!(data_handle && status_handle)) {
            k_work_reschedule(&discover_work, DISCOVER_DELAY);
        }
    }
}

static void disconnected(struct bt_conn *conn, uint8_t reason) {
    if (conn != peer) {
        return;
    }
    bool was_ready = art_relay_link() == 2;
    guard_cancel(false);
    drop_peer();
    if (was_ready) {
        host_link_art_status();
        host_link_rgb_right_changed(); // the right's state isn't known any more (#478)
    }
}

BT_CONN_CB_DEFINE(art_relay_conn_cb) = {
    .connected = connected,
    .disconnected = disconnected,
    .security_changed = security_changed,
};

/* GATT calls, made synchronous for the push work queue */

/* A Bluetooth call on relay_q that doesn't return (#618). In the soak (#644) HELLO's relay debug
 * read "in flight 1, answered 23, timeouts 1" twice, 41 s apart: relay_q was inside a
 * bt_gatt_read or bt_gatt_write, after in_flight was set and before wait_done's timed wait. Zephyr
 * 3.5 takes the PDU, its ATT meta data and the connection's TX context with K_FOREVER off the
 * system workqueue (bt_att_chan_create_pdu, tx_meta_data_alloc, conn_tx_alloc), and gives the
 * last two back on the system workqueue. When they run out, relay_q sleeps in the call, and
 * nothing behind it runs: art selects and pushes, status reads, the colour, the screen readback.
 * The app saw that as "Bluetooth write failed" and a right snapshot that timed out.
 *
 * Moving the calls to the system workqueue (where those two take K_NO_WAIT) isn't safe in 3.5:
 * the ACL buffer itself is still taken with K_FOREVER there (net_buf_alloc has no syswq guard),
 * and a system workqueue asleep on it would never free the TX contexts either. The call can't be
 * taken back, and its params are Zephyr's until it returns, so in_flight can't be freed under
 * it. What frees the buffers is the link going: the soak's re-link ended it (the count read 78
 * after). So the rescue does exactly that, once: a call on relay_q still in progress RELAY_STUCK
 * after it began drops the split link, logged. Zephyr flushes the link's queue, the call returns
 * with an error, relay_q goes on, and ZMK reconnects (the right's keys pause for that, some 15 s,
 * which is why it is a last resort and not a timeout).
 *
 * Start/stop: armed when a relay_q call begins, cancelled when it returns (no timer otherwise).
 * One drop per stuck call; at most RESCUE_MAX_IN_ROW drops until a call succeeds again, then it
 * only logs. lane_q's write commands aren't watched: one stuck there delays only the head start,
 * and the request on relay_q still carries the state. */
#define RELAY_STUCK K_SECONDS(10)
#define RESCUE_MAX_IN_ROW 3

K_THREAD_STACK_DEFINE(rescue_stack, 768);
static struct k_work_q rescue_q; // its own: relay_q is the one stuck, and the system queue
                                 // mustn't wait for bt_conn_disconnect
static atomic_t calling;         // a relay_q Bluetooth call is in progress
static atomic_t rescue_in_row;

static void rescue_cb(struct k_work *work) {
    if (!atomic_get(&calling)) {
        return;
    }
    struct bt_conn *conn = NULL;
    unsigned int key = irq_lock(); // disconnected() clears peer from the Bluetooth thread
    if (peer != NULL) {
        conn = bt_conn_ref(peer);
    }
    irq_unlock(key);
    if (conn == NULL) {
        return; // the link is gone already: that frees the call
    }
    if (atomic_get(&rescue_in_row) < RESCUE_MAX_IN_ROW) {
        atomic_inc(&rescue_in_row);
        LOG_WRN("art relay: a Bluetooth call on the relay queue hasn't returned in 10 s (no TX "
                "buffer?); dropping the split link to free it (%d in a row)",
                (int)atomic_get(&rescue_in_row));
        bt_conn_disconnect(conn, BT_HCI_ERR_REMOTE_USER_TERM_CONN);
    } else {
        LOG_WRN("art relay: the relay queue is stuck in a Bluetooth call again; leaving the link");
    }
    bt_conn_unref(conn);
}
static K_WORK_DELAYABLE_DEFINE(rescue_work, rescue_cb);

static void call_begin(void) {
    atomic_set(&calling, 1);
    k_work_reschedule_for_queue(&rescue_q, &rescue_work, RELAY_STUCK);
}

static int call_end(int ret) {
    atomic_clear(&calling);
    k_work_cancel_delayable(&rescue_work);
    if (ret == 0) {
        atomic_clear(&rescue_in_row);
    }
    return ret;
}

// A Bluetooth call on relay_q, watched by the rescue above.
#define BT_CALL(expr) (call_begin(), call_end(expr))

static K_SEM_DEFINE(gatt_done, 0, 1);
static uint8_t gatt_err;
// A request whose callback hasn't come yet; its params must not be reused until it does.
static atomic_t in_flight;

static int wait_done(int ret) {
    dbg.write_ret = MAX(ret, INT8_MIN);
    if (ret) {
        atomic_set(&in_flight, 0);
        return -EIO;
    }
    if (k_sem_take(&gatt_done, GATT_TIMEOUT)) {
        dbg.timeouts++;
        return -ETIMEDOUT;
    }
    dbg.att_err = gatt_err;
    return gatt_err ? -EIO : 0;
}

static void request_done(uint8_t err) {
    gatt_err = err;
    dbg.answered++;
    atomic_set(&in_flight, 0);
    k_sem_give(&gatt_done);
}

static void write_cb(struct bt_conn *conn, uint8_t err, struct bt_gatt_write_params *params) {
    request_done(err);
}

static int write_req(struct bt_conn *conn, const uint8_t *data, uint16_t len) {
    static struct bt_gatt_write_params params;
    static uint8_t copy[20]; // SAVE is the longest: op, slot, name[16]
    if (atomic_get(&in_flight) || len > sizeof(copy)) {
        return -EBUSY;
    }
    gatt_err = 0;
    memcpy(copy, data, len); // the request may outlive the caller's buffer
    params.func = write_cb;
    params.handle = data_handle;
    params.offset = 0;
    params.data = copy;
    params.length = len;
    k_sem_reset(&gatt_done);
    atomic_set(&in_flight, 1); // before the call: the callback can come before it returns
    return wait_done(BT_CALL(bt_gatt_write(conn, &params)));
}

static int write_cmd(struct bt_conn *conn, const uint8_t *data, uint16_t len) {
    for (int tries = 0; tries < 50; tries++) {
        int err = BT_CALL(bt_gatt_write_without_response(conn, data_handle, data, len, false));
        if (err != -ENOMEM && err != -EAGAIN) {
            return err;
        }
        k_msleep(10); // out of buffers: let the radio catch up
    }
    return -ETIMEDOUT;
}

// Whether the last status read holds the right's underglow (status version >= 2).
static bool right_rgb_known(void) {
    return right_read_at != 0 && right.version >= 2 && right_len >= sizeof(right);
}

static uint8_t read_cb(struct bt_conn *conn, uint8_t err, struct bt_gatt_read_params *params,
                       const void *data, uint16_t length) {
    if (data != NULL) {
        // The value goes on with the flash slot table; the first chunk has all of the status.
        bool had = right_rgb_known();
        uint8_t was_on = right.rgb.on;
        memcpy(&right, data, MIN(length, sizeof(right)));
        right_len = length;
        right_read_at = k_uptime_get();
        request_done(0);
        if (right_rgb_known() != had || (had && right.rgb.on != was_on)) {
            host_link_rgb_right_changed(); // D7 83 (#478); nothing if the PC already has it
        }
        rgb_reconcile();
        return BT_GATT_ITER_STOP;
    }
    request_done(err);
    return BT_GATT_ITER_STOP;
}

static int read_status(struct bt_conn *conn) {
    static struct bt_gatt_read_params params;
    if (atomic_get(&in_flight)) {
        return -EBUSY;
    }
    gatt_err = 0; // as write_req: a call that fails reads as never sent (user_req)
    params.func = read_cb;
    params.handle_count = 1;
    params.single.handle = status_handle;
    params.single.offset = 0;
    k_sem_reset(&gatt_done);
    atomic_set(&in_flight, 1);
    return wait_done(BT_CALL(bt_gatt_read(conn, &params)));
}

/* Someone's own command (#618): a SELECT, SAVE or ERASE, a push, the screen readback. The relay
 * also carries Foxglove's background traffic (status, dim and slot reads, the colour, the
 * activity and underglow state), and a request of that which timed out stays out on the link
 * (in_flight) until its late answer. A user's command used to fail at once on it, as "Bluetooth
 * write failed" in the app. Now it waits for that answer first, and a write that didn't get out
 * (that request still in the way, or no buffer for the call) is tried once more, logged.
 *
 * Start/stop: starts with the command's job on relay_q. The wait ends on the late request's
 * callback (request_done gives gatt_done) or the job's deadline, USER_JOB_MS after it started,
 * below the PC's 10 s wait for a SELECT. One retry, only while the deadline leaves room for it.
 * A link that drops ends the request in the way with an error, so the wait ends then too. */
#define USER_JOB_MS 8000

// Waits, on its callback, for a request still out on the link; -EBUSY if it's there at `end`.
static int wait_idle(int64_t end) {
    while (atomic_get(&in_flight)) {
        int64_t left = end - k_uptime_get();
        if (left <= 0) {
            return -EBUSY;
        }
        k_sem_take(&gatt_done, K_MSEC(left)); // request_done gives it; then look again
    }
    return 0;
}

// The request never went out: another was in the way, or the call itself failed (no buffer).
// An ATT error is the right's answer and a timeout went out; neither is "never sent".
static bool not_sent(int err) { return err == -EBUSY || (err == -EIO && gatt_err == 0); }

// A user's request (write_req, or read_status when `data` is NULL): after the request in the way,
// and once more on a send that didn't go out. `again_after_timeout` also retries one that went out
// unanswered, for commands that are the same done twice (SELECT of a set, a status read).
static int user_req(struct bt_conn *conn, const uint8_t *data, uint16_t len, int64_t end,
                    bool again_after_timeout) {
    int err = 0;
    for (int attempt = 0; attempt < 2; attempt++) {
        err = wait_idle(end);
        if (!err) {
            err = data != NULL ? write_req(conn, data, len) : read_status(conn);
        }
        bool retry = not_sent(err) || (again_after_timeout && err == -ETIMEDOUT);
        if (!err || !retry || attempt > 0 || k_uptime_get() + GATT_TIMEOUT_MS > end) {
            break;
        }
        LOG_WRN("art relay: %s to the right failed (%d, in flight %d), trying once more",
                data != NULL ? "write" : "status read", err, (int)atomic_get(&in_flight));
    }
    return err;
}

static int64_t user_deadline(void) { return k_uptime_get() + USER_JOB_MS; }

/* Pushes */

K_THREAD_STACK_DEFINE(relay_stack, 1024);
static struct k_work_q relay_q;
static int8_t select_set;

static void fail(uint8_t err) {
    error = err;
    state = ART_LEFT_FAILED;
}

// The PC went quiet mid-blob (app killed, cable pulled): drop what came so far.
static void receive_timeout_cb(struct k_work *work) {
    if (state == ART_LEFT_RECEIVING) {
        LOG_WRN("art relay: no data from the PC for a while, dropping the blob");
        blob_len = 0;
        error = ART_ERR_TIMEOUT;
        state = ART_LEFT_IDLE;
    }
}
static K_WORK_DELAYABLE_DEFINE(receive_timeout, receive_timeout_cb);

// A reference to the split connection for one job on the relay queue, or NULL if the right half
// isn't there or has no relay service. If the link drops mid-job, its requests fail with an error
// instead of touching a connection that is gone.
static struct bt_conn *job_conn(void) {
    struct bt_conn *conn = NULL;
    unsigned int key = irq_lock(); // disconnected() clears peer from the Bluetooth thread
    if (art_relay_link() == 2) {
        conn = bt_conn_ref(peer);
    }
    irq_unlock(key);
    return conn;
}

#include "screen_snap_relay.inc"
#if IS_ENABLED(CONFIG_FOXGLOVE_TEST_HOOKS)
static uint8_t press_pos, press_state;
static void press_cb(struct k_work *work) {
    uint8_t status = 2;
    struct bt_conn *conn = job_conn();
    if (conn != NULL) {
        const uint8_t d[] = {ART_RELAY_PRESS, press_pos, press_state};
        int err = write_req(conn, d, sizeof(d));
        status = err == 0 ? 0 : err == -ETIMEDOUT ? 3 : 2;
        /* Application ATT errors 0x82/84/85 preserve the peripheral's rejection. */
        if (err == -EIO && gatt_err >= 0x82 && gatt_err <= 0x86) {
            status = gatt_err - 0x80;
        }
        bt_conn_unref(conn);
    }
    test_hooks_relay_done(status);
}
static K_WORK_DEFINE(press_work, press_cb);
void art_relay_press(uint8_t pos, uint8_t state) {
    /* test_hooks owns one request until completion; relay_q serializes GATT params. */
    press_pos = pos;
    press_state = state;
    k_work_submit_to_queue(&relay_q, &press_work);
}

/* BOOT through ZMK's own split link (#545), for when the relay can't take it: no relay link yet
 * (an older right whose relay the left can't use, or the ~20-40 s before discovery finds it), or a
 * request that failed or went unanswered. Every right firmware has ZMK's &bootloader ("bootload",
 * reset.dtsi, invoked on the event's source), and the split's run-behavior write needs nothing of
 * ours. It is a write without response, so nothing confirms it: the right's bootloader drive
 * does. ZMK finds its own characteristics after the link is encrypted and drops the write until
 * then, so it is only sent once the link has been encrypted for SPLIT_SETTLE_MS; before that the
 * PC hears status 2 and asks again (on D6 85, or after a pause). One send per D7 08, no retry.
 * ZMK's split run queue holds 5: when it is full, the call waits up to 100 ms here (relay_q) and
 * then drops the oldest entry, as any of ZMK's own split invokes would. */
#define SPLIT_SETTLE_MS 5000
#define SPLIT_RIGHT_SOURCE 0 // the right is ZMK's first (only) peripheral

static bool boot_via_split(void) {
    unsigned int key = irq_lock(); // connected()/disconnected() run on the Bluetooth thread
    bool settled = peer != NULL && peer_secure_at != 0 &&
                   k_uptime_get() - peer_secure_at >= SPLIT_SETTLE_MS;
    irq_unlock(key);
    if (!settled) {
        return false;
    }
    struct zmk_behavior_binding binding = {.behavior_dev = "bootload"};
    struct zmk_behavior_binding_event event = {
        .position = 0,
        .timestamp = k_uptime_get(),
        .source = SPLIT_RIGHT_SOURCE,
    };
    int err = zmk_split_central_invoke_behavior(SPLIT_RIGHT_SOURCE, &binding, event, true);
    LOG_INF("art relay: BOOT through ZMK's split (%d)", err);
    return err == 0;
}

static void boot_cb(struct k_work *work) {
    uint8_t status = 2;
    struct bt_conn *conn = job_conn();
    if (conn != NULL) {
        const uint8_t d[] = {ART_RELAY_BOOT, 'B', 'O', 'O', 'T'};
        int err = write_req(conn, d, sizeof(d));
        status = err == 0 ? 0 : err == -ETIMEDOUT ? 3 : 2;
        if (err == -EIO && gatt_err == 0x84) {
            status = 4; // the right's firmware has no test hooks: it said no, so no fallback
        }
        bt_conn_unref(conn);
    }
    // A timeout may have landed too; a second ask of a right already rebooting is harmless.
    if ((status == 2 || status == 3) && boot_via_split()) {
        status = 7; // sent, unacknowledged: the PC watches for the drive (host_link.h)
    }
    test_hooks_boot_relay_done(status);
}
static K_WORK_DEFINE(boot_work, boot_cb);
void art_relay_boot(void) { k_work_submit_to_queue(&relay_q, &boot_work); }
#endif

static int push(struct bt_conn *conn) {
    uint8_t buf[3 + 244];
    int64_t end = user_deadline();
    buf[0] = ART_RELAY_BEGIN;
    sys_put_le16(blob_len, &buf[1]);
    sys_put_le32(blob_crc, &buf[3]);
    // BEGIN starts the right's receive over, so one that went out unanswered can go again.
    if (user_req(conn, buf, 7, end, true)) {
        return ART_ERR_BLE;
    }
    int chunk = CLAMP((int)bt_gatt_get_mtu(conn) - 3 - 3, 16, (int)sizeof(buf) - 3);
    for (pushed = 0; pushed < blob_len;) {
        int n = MIN(chunk, blob_len - pushed);
        buf[0] = ART_RELAY_DATA;
        sys_put_le16(pushed, &buf[1]);
        memcpy(&buf[3], &blob[pushed], n);
        if (write_cmd(conn, buf, 3 + n)) {
            return ART_ERR_BLE;
        }
        pushed += n;
    }
    buf[0] = ART_RELAY_COMMIT;
    end = user_deadline(); // the data took its own time
    if (user_req(conn, buf, 1, end, false) || user_req(conn, NULL, 0, end, true)) {
        return ART_ERR_BLE;
    }
    return right.state == ART_RIGHT_PLAYING ? ART_ERR_NONE : ART_ERR_RIGHT;
}

static void push_cb(struct k_work *work) {
    struct bt_conn *conn = job_conn();
    if (conn == NULL) {
        fail(ART_ERR_NO_LINK);
        return;
    }
    int err = push(conn);
    bt_conn_unref(conn);
    if (err) {
        fail(err);
        return;
    }
    state = ART_LEFT_DONE;
}
static K_WORK_DEFINE(push_work, push_cb);

static void select_cb(struct k_work *work) {
    struct bt_conn *conn = job_conn();
    if (conn == NULL) {
        fail(ART_ERR_NO_LINK);
        return;
    }
    uint8_t buf[] = {ART_RELAY_SELECT, select_set};
    int64_t end = user_deadline();
    bool err = user_req(conn, buf, sizeof(buf), end, true) || user_req(conn, NULL, 0, end, true);
    bt_conn_unref(conn);
    if (err) {
        fail(ART_ERR_BLE);
        return;
    }
    state = ART_LEFT_DONE;
}
static K_WORK_DEFINE(select_work, select_cb);

/* Flash slots on the right */

static uint8_t slot_op[18]; // the relay write for SAVE or ERASE
static uint8_t slot_op_len;

static void slot_op_cb(struct k_work *work) {
    struct bt_conn *conn = job_conn();
    if (conn == NULL) {
        fail(ART_ERR_NO_LINK);
        return;
    }
    // SAVE and ERASE aren't sent again once out: a second one finds the right saving (0x82).
    int64_t end = user_deadline();
    bool err = user_req(conn, slot_op, slot_op_len, end, false) ||
               user_req(conn, NULL, 0, end, true);
    bt_conn_unref(conn);
    if (err) {
        fail(ART_ERR_RIGHT);
        return;
    }
    state = ART_LEFT_DONE;
}
static K_WORK_DEFINE(slot_op_work, slot_op_cb);

static int start_slot_op(const uint8_t *op, uint8_t len) {
    if (state == ART_LEFT_PUSHING) {
        return -EBUSY;
    }
    k_work_cancel_delayable(&receive_timeout);
    memcpy(slot_op, op, len);
    slot_op_len = len;
    state = ART_LEFT_PUSHING;
    error = ART_ERR_NONE;
    pushed = blob_len = 0;
    k_work_submit_to_queue(&relay_q, &slot_op_work);
    return 0;
}

int art_relay_save(uint8_t slot, const uint8_t name[16]) {
    uint8_t op[18] = {ART_RELAY_SAVE, slot};
    memcpy(&op[2], name, 16);
    return start_slot_op(op, sizeof(op));
}

int art_relay_erase(uint8_t slot) {
    uint8_t op[] = {ART_RELAY_ERASE, slot};
    return start_slot_op(op, sizeof(op));
}

// D6 07 SLOT: one entry of the right's slot table, by an offset read of the status value.
static uint8_t slot_index;
static uint8_t slot_info[sizeof(struct art_relay_slot_info)];
static atomic_t slot_busy;
// Where an offset read of the status value lands (one at a time: they run on relay_q, and a new
// request is never sent while one is in flight).
static uint8_t *ofs_buf;
static uint16_t ofs_len, slot_got;

static uint8_t slot_read_cb(struct bt_conn *conn, uint8_t err, struct bt_gatt_read_params *params,
                            const void *data, uint16_t length) {
    if (data != NULL) {
        uint16_t n = MIN(length, ofs_len - slot_got);
        memcpy(&ofs_buf[slot_got], data, n);
        slot_got += n;
        if (slot_got < ofs_len) {
            return BT_GATT_ITER_CONTINUE; // Zephyr reads on from where this chunk ended
        }
        request_done(0);
        return BT_GATT_ITER_STOP;
    }
    request_done(err ? err : BT_ATT_ERR_UNLIKELY); // the value ended early
    return BT_GATT_ITER_STOP;
}

static void slot_read_job(struct k_work *work) {
    static struct bt_gatt_read_params params;
    uint8_t status = 2;
    struct bt_conn *conn = job_conn();
    if (conn != NULL) {
        status = 4; // the right's firmware has no slots
        if (right.version >= 3 && slot_index < right.flash_slots) {
            status = 3;
            if (!atomic_get(&in_flight)) {
                slot_got = 0;
                ofs_buf = slot_info;
                ofs_len = sizeof(slot_info);
                params.func = slot_read_cb;
                params.handle_count = 1;
                params.single.handle = status_handle;
                params.single.offset =
                    sizeof(struct art_relay_status) + slot_index * sizeof(slot_info);
                k_sem_reset(&gatt_done);
                atomic_set(&in_flight, 1);
                if (wait_done(BT_CALL(bt_gatt_read(conn, &params))) == 0) {
                    status = 0;
                }
            }
        }
        bt_conn_unref(conn);
    }
    uint8_t r[4 + sizeof(slot_info)] = {HOST_LINK_ART, 0x87, status, slot_index};
    if (status == 0) {
        memcpy(&r[4], slot_info, sizeof(slot_info));
    }
    host_link_send(r, sizeof(r));
    atomic_clear(&slot_busy);
}
static K_WORK_DEFINE(slot_read_work, slot_read_job);

static atomic_t build_busy;

static void build_read_job(struct k_work *work) {
    static struct bt_gatt_read_params params;
    static struct foxglove_build_info build;
    uint8_t status = 2;
    struct bt_conn *conn = job_conn();
    if (conn != NULL) {
        status = 4; // the right's firmware doesn't report it
        if (right.version >= 4) {
            status = 3;
            if (!atomic_get(&in_flight)) {
                slot_got = 0;
                ofs_buf = (uint8_t *)&build;
                ofs_len = sizeof(build);
                params.func = slot_read_cb;
                params.handle_count = 1;
                params.single.handle = status_handle;
                params.single.offset = sizeof(struct art_relay_status) +
                                       ART_RELAY_FLASH_SLOTS * sizeof(struct art_relay_slot_info);
                k_sem_reset(&gatt_done);
                atomic_set(&in_flight, 1);
                if (wait_done(BT_CALL(bt_gatt_read(conn, &params))) == 0) {
                    status = 0;
                }
            }
        }
        bt_conn_unref(conn);
    }
    host_link_build_info_reply(status, 1, status == 0 ? &build : NULL);
    atomic_clear(&build_busy);
}
static K_WORK_DEFINE(build_read_work, build_read_job);

int art_relay_build_info(void) {
    if (!atomic_cas(&build_busy, 0, 1)) {
        return -EBUSY;
    }
    k_work_submit_to_queue(&relay_q, &build_read_work);
    return 0;
}

/* D7 0E for the right: its battery dim state, by an offset read past the build info (status
 * version >= 6). Started by a PC request or a new config reaching the right; one read, no retry. */

static struct battery_dim_wire right_dim;
static int64_t right_dim_at; // uptime of the last good read, 0 = never
static atomic_t dim_busy;
static uint32_t right_redraws; // the right's whole-screen rewrites, as last read (#618)
static bool right_redraws_ok;

static void dim_read_job(struct k_work *work) {
    static struct bt_gatt_read_params params;
    static struct battery_dim_wire got;
    int err = -ENOTSUP;
    struct bt_conn *conn = job_conn();
    if (conn != NULL) {
        if (right.version >= 6 && !atomic_get(&in_flight)) {
            slot_got = 0;
            ofs_buf = (uint8_t *)&got;
            ofs_len = sizeof(got);
            params.func = slot_read_cb;
            params.handle_count = 1;
            params.single.handle = status_handle;
            params.single.offset = sizeof(struct art_relay_status) +
                                   ART_RELAY_FLASH_SLOTS * sizeof(struct art_relay_slot_info) +
                                   sizeof(struct foxglove_build_info);
            k_sem_reset(&gatt_done);
            atomic_set(&in_flight, 1);
            err = wait_done(BT_CALL(bt_gatt_read(conn, &params)));
        }
        bt_conn_unref(conn);
    }
    if (err == 0 && (conn = job_conn()) != NULL) {
        // Its redraw count, right after the dim state; a right before it ends the value there.
        static uint8_t count[sizeof(uint32_t)];
        slot_got = 0;
        ofs_buf = count;
        ofs_len = sizeof(count);
        params.single.offset = ART_RELAY_REDRAWS_AT;
        k_sem_reset(&gatt_done);
        atomic_set(&in_flight, 1);
        right_redraws_ok = wait_done(BT_CALL(bt_gatt_read(conn, &params))) == 0;
        right_redraws = right_redraws_ok ? sys_get_le32(count) : 0;
        bt_conn_unref(conn);
    }
    if (err == 0) {
        bool differs = right_dim_at == 0 || memcmp(&got, &right_dim, sizeof(got)) != 0;
        right_dim = got;
        right_dim_at = k_uptime_get();
        if (differs) {
            host_link_battery_dim_changed();
        }
    }
    atomic_clear(&dim_busy);
}
static K_WORK_DEFINE(dim_read_work, dim_read_job);

static void dim_read(void) {
    if (state != ART_LEFT_PUSHING && atomic_cas(&dim_busy, 0, 1)) {
        k_work_submit_to_queue(&relay_q, &dim_read_work);
    }
}

int art_relay_slot(uint8_t slot) {
    if (!atomic_cas(&slot_busy, 0, 1)) {
        return -EBUSY;
    }
    slot_index = slot;
    k_work_submit_to_queue(&relay_q, &slot_read_work);
    return 0;
}

static void status_cb(struct k_work *work) {
    struct bt_conn *conn = job_conn();
    if (conn != NULL) {
        read_status(conn);
        bt_conn_unref(conn);
    }
}
static K_WORK_DEFINE(status_work, status_cb);

/* Called from host_link.c on the USB thread */

int art_relay_begin(uint16_t len, uint32_t crc) {
    if (state == ART_LEFT_PUSHING) {
        return -EBUSY; // leave the running push's state alone
    }
    if (len > sizeof(blob)) {
        k_work_cancel_delayable(&receive_timeout);
        fail(ART_ERR_TOO_BIG);
        return -ENOMEM;
    }
    // A BEGIN also replaces a receive that is stuck or still running.
    k_work_reschedule(&receive_timeout, ART_RELAY_RECEIVE_TIMEOUT);
    blob_len = len;
    blob_crc = crc;
    pushed = 0;
    error = ART_ERR_NONE;
    state = ART_LEFT_RECEIVING;
    return 0;
}

int art_relay_data(uint16_t offset, const uint8_t *data, uint8_t len) {
    if (state != ART_LEFT_RECEIVING) {
        return -EINVAL;
    }
    if (offset + len > blob_len) {
        k_work_cancel_delayable(&receive_timeout);
        fail(ART_ERR_RANGE);
        return -EINVAL;
    }
    memcpy(&blob[offset], data, len);
    k_work_reschedule(&receive_timeout, ART_RELAY_RECEIVE_TIMEOUT);
    return 0;
}

int art_relay_commit(void) {
    if (state != ART_LEFT_RECEIVING) {
        return -EINVAL;
    }
    k_work_cancel_delayable(&receive_timeout);
    if (crc32_ieee(blob, blob_len) != blob_crc) {
        fail(ART_ERR_BAD_CRC);
        return -EINVAL;
    }
    state = ART_LEFT_PUSHING;
    k_work_submit_to_queue(&relay_q, &push_work);
    return 0;
}

int art_relay_select(int8_t set) {
    if (state == ART_LEFT_PUSHING) {
        return -EBUSY;
    }
    k_work_cancel_delayable(&receive_timeout);
    select_set = set;
    state = ART_LEFT_PUSHING;
    error = ART_ERR_NONE;
    pushed = blob_len = 0;
    k_work_submit_to_queue(&relay_q, &select_work);
    return 0;
}

void art_relay_step(int dir) {
    if (art_relay_link() < 2 || right_read_at == 0) {
        return;
    }
    int8_t ids[INT8_MAX];
    int total = 0;
    for (int i = 0; i < right.set_count && total < (int)ARRAY_SIZE(ids); i++) {
        ids[total++] = i;
    }
    uint8_t used = right.version >= 3 ? right.flash_used : 0;
    for (int n = 0; n < 8 && right.set_count + n < INT8_MAX; n++) {
        if (used & BIT(n)) {
            ids[total++] = right.set_count + n;
        }
    }
    if (total == 0) {
        return;
    }
    int at = dir > 0 ? -1 : 0; // a pushed set (-1) isn't in the list: start at either end
    for (int i = 0; i < total; i++) {
        if (ids[i] == right.set) {
            at = i;
        }
    }
    art_relay_select(ids[((at + dir) % total + total) % total]);
}

void art_relay_discovery(int8_t *result, uint8_t *tries) {
    // DONE only with the handles in hand, on the link we have now.
    *result = discovery == ART_DISCOVERY_DONE && !(data_handle && status_handle)
                  ? ART_DISCOVERY_IDLE
                  : discovery;
    *tries = discovery_tries;
}

/* #122 guard: a split link stuck below encryption gets dropped so ZMK reconnects */

#if IS_ENABLED(CONFIG_FOXGLOVE_SPLIT_GUARD)
#define GUARD_AFTER K_SECONDS(20)
#define GUARD_MAX_IN_ROW 3

static uint8_t guard_drops;  // since boot, for HELLO
static uint8_t guard_in_row; // drops since the last link that encrypted
static atomic_t guard_forced; // D7 0B: treat the next check's link as stuck (dev builds)

static void guard_cb(struct k_work *work) {
    struct bt_conn *conn = NULL;
    unsigned int key = irq_lock();
    if (peer != NULL) {
        conn = bt_conn_ref(peer);
    }
    irq_unlock(key);
    if (conn == NULL) {
        return;
    }
    bool forced = atomic_cas(&guard_forced, 1, 0); // D7 0B: from guard_test_q, the same work
    if (still_connected(conn) && (bt_conn_get_security(conn) < BT_SECURITY_L2 || forced)) {
        if (guard_in_row < GUARD_MAX_IN_ROW) {
            guard_in_row++;
            guard_drops = MIN(guard_drops + 1, UINT8_MAX);
            LOG_WRN("split: link to the right not encrypted after 20 s (security %d, MTU %d), "
                    "dropping it (%d in a row)",
                    bt_conn_get_security(conn), bt_gatt_get_mtu(conn), guard_in_row);
            bt_conn_disconnect(conn, BT_HCI_ERR_REMOTE_USER_TERM_CONN);
        } else {
            LOG_WRN("split: link to the right still not encrypted; leaving it (re-pair?)");
        }
    }
    bt_conn_unref(conn);
}
// On relay_q: bt_conn_disconnect waits for the controller, which the system queue mustn't.
static K_WORK_DELAYABLE_DEFINE(guard_work, guard_cb);

static void guard_arm(void) { k_work_reschedule_for_queue(&relay_q, &guard_work, GUARD_AFTER); }

static void guard_cancel(bool encrypted) {
    k_work_cancel_delayable(&guard_work);
    if (encrypted) {
        guard_in_row = 0;
    }
}

uint8_t art_relay_guard_drops(void) { return guard_drops; }

#if IS_ENABLED(CONFIG_FOXGLOVE_TEST_HOOKS)
FOXGLOVE_TEST_HOOK_MARK(guard_test);
#endif

#if IS_ENABLED(CONFIG_FOXGLOVE_TEST_HOOKS)
// Its own queue (dev builds only): on relay_q the drop would wait behind the very relay job a
// test wants to cut, and the system queue mustn't wait for bt_conn_disconnect.
K_THREAD_STACK_DEFINE(guard_test_stack, 768);
static struct k_work_q guard_test_q;
static atomic_t guard_test_q_started;

static void guard_test_cb(struct k_work *work) {
    atomic_set(&guard_forced, 1);
    guard_cb(work);
}
static K_WORK_DELAYABLE_DEFINE(guard_test_work, guard_test_cb);
#endif

int art_relay_guard_test(uint16_t delay_ms) {
#if IS_ENABLED(CONFIG_FOXGLOVE_TEST_HOOKS)
    if (peer == NULL) {
        return -ENOTCONN;
    }
    // The real recovery path (drop, ZMK reconnects) on a link that's actually fine; a genuine
    // stuck-before-encryption link can't be made on purpose without deleting the bond. Started
    // on first use, so nothing runs at boot.
    if (atomic_cas(&guard_test_q_started, 0, 1)) {
        k_work_queue_start(&guard_test_q, guard_test_stack, K_THREAD_STACK_SIZEOF(guard_test_stack),
                           K_LOWEST_APPLICATION_THREAD_PRIO, NULL);
    }
    k_work_reschedule_for_queue(&guard_test_q, &guard_test_work, K_MSEC(delay_ms));
    return 0;
#else
    return -ENOTSUP;
#endif
}
#else
int art_relay_guard_test(uint16_t delay_ms) { return -ENOTSUP; }
static void guard_arm(void) {}
static void guard_cancel(bool encrypted) {}
uint8_t art_relay_guard_drops(void) { return 0; }
#endif

void art_relay_debug(uint8_t out[ART_RELAY_DEBUG_SIZE]) {
    out[0] = dbg.chrcs;
    out[1] = dbg.ends;
    out[2] = (uint8_t)dbg.write_ret;
    out[3] = dbg.att_err;
    out[4] = dbg.timeouts;
    out[5] = dbg.answered;
    out[6] = atomic_get(&in_flight);
    out[7] = peer ? bt_conn_get_security(peer) : 0;
    sys_put_le16(peer ? bt_gatt_get_mtu(peer) : 0, &out[8]);
}

uint8_t art_relay_link(void) {
    if (peer == NULL) {
        return 0;
    }
    return data_handle && status_handle ? 2 : 1;
}

uint8_t art_relay_right_dim(struct battery_dim_wire *out, bool refresh) {
    if (refresh && state != ART_LEFT_PUSHING && right_read_at == 0) {
        k_work_submit_to_queue(&relay_q, &status_work); // the version, before the dim read
    }
    if (refresh) {
        dim_read();
    }
    if (art_relay_link() < 2 || right_dim_at == 0) {
        memset(out, 0, sizeof(*out));
        return 0xff;
    }
    *out = right_dim;
    return MIN((k_uptime_get() - right_dim_at) / 1000, 254);
}

bool art_relay_right_shown(struct rgb_shown_wire *out) {
    if (art_relay_link() < 2 || !atomic_get(&right_shown_valid)) {
        return false;
    }
    *out = right_shown;
    return true;
}

bool art_relay_right_redraws(uint32_t *out) {
    *out = right_redraws;
    return right_redraws_ok && art_relay_link() == 2 && right_dim_at != 0;
}

uint8_t art_relay_right_rgb(struct rgb_idle_wire *out) {
    if (state != ART_LEFT_PUSHING) {
        k_work_submit_to_queue(&relay_q, &status_work);
    }
    if (art_relay_link() < 2 || !right_rgb_known()) {
        memset(out, 0, sizeof(*out));
        return 0xff;
    }
    *out = right.rgb;
    return MIN((k_uptime_get() - right_read_at) / 1000, 254);
}

void art_relay_report(uint8_t out[ART_RELAY_REPORT_SIZE]) {
    out[0] = state;
    out[1] = error;
    sys_put_le16(blob_len, &out[2]);
    sys_put_le16(pushed, &out[4]);
    out[6] = art_relay_link();
    memcpy(&out[7], &right, sizeof(right));
    if (state != ART_LEFT_PUSHING) {
        k_work_submit_to_queue(&relay_q, &status_work); // fresher right status for next time
    }
}

/* The right's doorbell (#478): the left subscribes to the status value's notify once the relay
 * is found. A ring says the right's on/off or power source changed on its own; the left reads it
 * again, and those reads tell the PC (D7 83 when the on/off differs, D7 8E when the dim state
 * does). A right without the CCC answers the subscribe with an ATT error and is just read as before.
 *
 * Start/stop: the subscribe starts on link-up: one write, not retried when refused (an older
 * right) or lost (that only costs the events). Only while the old link's subscription hasn't
 * ended yet (the right rebooted: the new link came first) does it wait, DOORBELL_WAITS x 1 s at
 * most. A ring starts one status read (and a dim read for power); one that comes during an art
 * push is dropped (the push's own status read has the on/off). The subscription goes with the
 * link (volatile). */

#define DOORBELL_WAITS 3

static struct bt_gatt_subscribe_params doorbell;
static atomic_t doorbell_live; // doorbell belongs to Zephyr until its subscription ends
static uint8_t doorbell_waits;

static uint8_t doorbell_rang(struct bt_conn *conn, struct bt_gatt_subscribe_params *params,
                             const void *data, uint16_t length) {
    if (data == NULL) { // unsubscribed: the link went, or a right without the CCC refused
        atomic_clear(&doorbell_live);
        return BT_GATT_ITER_STOP;
    }
    const uint8_t *d = data;
    uint8_t what = length >= 1 ? d[0] : ART_RELAY_RANG_RGB;
    // Since #605 an RGB ring carries the right's state: no read needed, so no wait on relay_q.
    // Its shown block is kept even before the first status read on this link (#618).
    bool carried = false;
    if (what & ART_RELAY_RANG_RGB) {
        uint8_t got = art_relay_ring_rgb(d, length, right_rgb_known(), &right.rgb, &right_shown,
                                         &right_shown_valid);
        carried = got & ART_RELAY_RING_CARRIED;
        if (carried) {
            right_read_at = k_uptime_get();
        }
        if (got & ART_RELAY_RING_TELL) {
            host_link_rgb_right_changed(); // D7 83 at once (deduped)
        }
        if (carried) {
            rgb_reconcile();
        }
    }
    if (carried && what == ART_RELAY_RANG_RGB) {
        return BT_GATT_ITER_CONTINUE;
    }
    if (state != ART_LEFT_PUSHING) {
        k_work_submit_to_queue(&relay_q, &status_work); // the on/off (and the version for the dim)
        if (what & ART_RELAY_RANG_POWER) {
            dim_read();
        }
    }
    return BT_GATT_ITER_CONTINUE;
}

static void doorbell_subscribed(struct bt_conn *conn, uint8_t err,
                                struct bt_gatt_subscribe_params *params) {
    dbg.att_err = err; // older right firmware has no CCC there: an ATT error, nothing else
}

static void doorbell_cb(struct k_work *work) {
    struct bt_conn *conn = job_conn();
    if (conn == NULL) {
        return;
    }
    if (!atomic_cas(&doorbell_live, 0, 1)) {
        if (doorbell_waits++ < DOORBELL_WAITS) { // the old link's subscription hasn't ended
            k_work_reschedule_for_queue(&relay_q, k_work_delayable_from_work(work), K_SECONDS(1));
        }
    } else {
        doorbell_waits = 0;
        doorbell = (struct bt_gatt_subscribe_params){
            .notify = doorbell_rang,
            .subscribe = doorbell_subscribed,
            .value_handle = status_handle,
            .ccc_handle = status_handle + 1, // the CCC follows the value (art_relay_peripheral.c)
            .value = BT_GATT_CCC_NOTIFY,
        };
        atomic_set_bit(doorbell.flags, BT_GATT_SUBSCRIBE_FLAG_VOLATILE);
        if (BT_CALL(bt_gatt_subscribe(conn, &doorbell)) != 0) {
            atomic_clear(&doorbell_live);
        }
    }
    bt_conn_unref(conn);
}
static K_WORK_DELAYABLE_DEFINE(doorbell_work, doorbell_cb);

// From the discovery callback (Bluetooth thread). After the RGB sync's first write.
static void doorbell_link_up(void) {
    doorbell_waits = 0;
    k_work_reschedule_for_queue(&relay_q, &doorbell_work, K_MSEC(300));
}

/* The left's activity (#603, #605; activity_relay.h). Each change goes to the right twice:
 *  - at once, as a write command from its own small queue (lane_q). It goes out at the next
 *    connection event, ahead of whatever waits on relay_q: there every request (art pushes,
 *    slot, build and dim reads, screen readback, the PC's frames and effects, the colour) takes
 *    a round trip of up to the split link's slave latency (ZMK's default 30 x 7.5 ms), one after
 *    the other, so a busy queue held the right's wake back for seconds. Best effort: one try,
 *    nothing more when the radio has no buffer (only lane_q waits for one, never ZMK or relay_q).
 *  - as a write request on relay_q, whose response confirms it. It sends the state as it is
 *    when the job runs (newest wins), so a command and a request that cross can't leave the
 *    right on an old state: the kick behind the newer state queues the request again.
 * The underglow on/off (art_relay_rgb_kick) takes the same head start.
 *
 * Start/stop:
 *   lane     starts on a kick (an activity or underglow change); one write command for each
 *            thing kicked since it last ran, then nothing until the next kick.
 *   request  starts on a kick or link-up; stops once the right has acknowledged the current
 *            state, on link loss, or after ACTIVITY_RETRIES failed tries ACTIVITY_RETRY apart; a
 *            request still out on the link (a timed-out one) is waited for ACTIVITY_BUSY_WAITS x
 *            ACTIVITY_BUSY_RETRY at most. */

#define ACTIVITY_SETTLE K_MSEC(100) // after link-up, as the underglow's RGB_SETTLE
#define ACTIVITY_RETRY K_SECONDS(1)
#define ACTIVITY_RETRIES 10
#define ACTIVITY_BUSY_RETRY K_MSEC(20)
#define ACTIVITY_BUSY_WAITS 160 // 3.2 s: a little past GATT_TIMEOUT

K_THREAD_STACK_DEFINE(lane_stack, 768);
static struct k_work_q lane_q;

#define LANE_ACTIVITY BIT(0)
#define LANE_RGB BIT(1)
static atomic_t lane_want;

static void lane_rgb(struct bt_conn *conn, uint16_t handle);

static void lane_cb(struct k_work *work) {
    atomic_val_t want = atomic_clear(&lane_want);
    struct bt_conn *conn = NULL;
    uint16_t handle = 0;
    unsigned int key = irq_lock(); // disconnected() clears peer and the handles
    if (art_relay_link() == 2) {
        conn = bt_conn_ref(peer);
        handle = data_handle;
    }
    irq_unlock(key);
    if (conn == NULL) {
        return; // link-up sends the state as a request
    }
    if (want & LANE_ACTIVITY) {
        uint8_t op[2];
        art_relay_activity_op(op);
        bt_gatt_write_without_response(conn, handle, op, sizeof(op), false);
    }
    if (want & LANE_RGB) {
        lane_rgb(conn, handle);
    }
    bt_conn_unref(conn);
}
static K_WORK_DEFINE(lane_work, lane_cb);

static void lane(atomic_val_t what) {
    atomic_or(&lane_want, what);
    k_work_submit_to_queue(&lane_q, &lane_work);
}

// On relay_q only:
static int8_t activity_sent = -1; // the state the right last acknowledged, -1 unknown
static uint8_t activity_tries, activity_busy;
static atomic_t activity_reset; // a new link: activity_cb starts over

static void activity_cb(struct k_work *work) {
    struct k_work_delayable *dwork = k_work_delayable_from_work(work);
    if (atomic_cas(&activity_reset, 1, 0)) {
        activity_sent = -1; // a new right, or one that rebooted
        activity_tries = 0;
        activity_busy = 0;
    }
    struct bt_conn *conn = job_conn();
    if (conn == NULL) {
        return; // activity_link_up starts over
    }
    uint8_t op[2];
    art_relay_activity_op(op);
    int err = 0;
    if (op[1] != activity_sent) {
        err = write_req(conn, op, sizeof(op));
        if (!err) {
            activity_sent = op[1];
        }
    }
    bt_conn_unref(conn);
    if (err == -EBUSY && activity_busy++ < ACTIVITY_BUSY_WAITS) {
        k_work_reschedule_for_queue(&relay_q, dwork, ACTIVITY_BUSY_RETRY);
        return;
    }
    activity_busy = 0;
    if (err && activity_tries++ < ACTIVITY_RETRIES) {
        k_work_reschedule_for_queue(&relay_q, dwork, ACTIVITY_RETRY);
        return;
    }
    activity_tries = 0;
    // Nothing else until the next change (art_relay_activity_kick) or link-up.
}
static K_WORK_DELAYABLE_DEFINE(activity_work, activity_cb);

void art_relay_activity_kick(void) {
    if (art_relay_link() == 2) {
        lane(LANE_ACTIVITY);
        k_work_reschedule_for_queue(&relay_q, &activity_work, K_NO_WAIT);
    }
}

// From the discovery callback (Bluetooth thread); the relay queue's state resets in activity_cb.
static void activity_link_up(void) {
    atomic_set(&activity_reset, 1);
    k_work_reschedule_for_queue(&relay_q, &activity_work, ACTIVITY_SETTLE);
}

static int art_relay_init(void) {
    k_work_queue_start(&relay_q, relay_stack, K_THREAD_STACK_SIZEOF(relay_stack),
                       K_LOWEST_APPLICATION_THREAD_PRIO, NULL);
    // Just above relay_q, so a head start never waits behind a relay job.
    k_work_queue_start(&lane_q, lane_stack, K_THREAD_STACK_SIZEOF(lane_stack),
                       K_LOWEST_APPLICATION_THREAD_PRIO - 1, NULL);
    // Above relay_q too: it runs while relay_q sleeps in a call (only then has it work).
    k_work_queue_start(&rescue_q, rescue_stack, K_THREAD_STACK_SIZEOF(rescue_stack),
                       K_LOWEST_APPLICATION_THREAD_PRIO - 1, NULL);
    return 0;
}

SYS_INIT(art_relay_init, APPLICATION, CONFIG_APPLICATION_INIT_PRIORITY);

/* Underglow: the right half follows ours (rgb_idle.c) */

#if IS_ENABLED(CONFIG_NICE_VIEW_RGB_IDLE)
// Right after the link comes up, ZMK's own setup traffic goes first.
#define RGB_SETTLE K_MSEC(100)
// A send that failed is retried this often, idle or not, up to RGB_RETRIES times per change.
#define RGB_RETRY K_SECONDS(1)
#define RGB_RETRIES 10

// The last RGB op the right half confirmed: on, follow, the battery dim config and the look.
static uint8_t rgb_sent[3 + sizeof(struct battery_dim_config) + RGB_IDLE_LOOK_SIZE];
#define RGB_LOOK_AT (3 + sizeof(struct battery_dim_config))
static bool rgb_sent_valid;
// A status read showed the right with another look than the left's (#624): send it again, once
// per look of the left's (a right that ignores the look, older firmware, isn't asked forever).
static atomic_t rgb_force;
static uint8_t look_forced[RGB_IDLE_LOOK_SIZE];
static atomic_t look_forced_valid;
static uint8_t rgb_retries;
static atomic_t hsb_sent = ATOMIC_INIT(-1); // the PC colour the right last acknowledged (#266)

// The RGB op as the right should have it now; false when the on/off can't be read. Any thread.
static bool rgb_op(uint8_t buf[sizeof(rgb_sent)], uint8_t *len) {
    bool on;
    // Under the PC's frames the user's on/off, not ZMK's: a right that can't draw frames keeps
    // its own effect (and one that can comes back lit).
    if (rgb_idle_get_on(&on) != 0) {
        return false;
    }
    memset(buf, 0, sizeof(rgb_sent));
    buf[0] = ART_RELAY_RGB;
    buf[1] = on;
    buf[2] = rgb_idle_sync();
    *len = 3;
#if IS_ENABLED(CONFIG_NICE_VIEW_RGB_BATTERY_DIM)
    struct battery_dim_config dim;
    battery_dim_get_config(&dim);
    memcpy(&buf[3], &dim, sizeof(dim)); // the right takes the left's levels (status v6)
#endif
    // The left's look (#624): with follow on the right shows it, whatever it saved before. An
    // older right stops reading at the dim config.
    rgb_idle_look(&buf[RGB_LOOK_AT]);
    *len = sizeof(rgb_sent);
    return true;
}

// The head start (lane_q, above): the same op as a write command, ahead of relay_q (#605).
static void lane_rgb(struct bt_conn *conn, uint16_t handle) {
    uint8_t buf[sizeof(rgb_sent)];
    uint8_t len;
    if (rgb_op(buf, &len)) {
        bt_gatt_write_without_response(conn, handle, buf, len, false);
    }
}

static void rgb_sync_cb(struct k_work *work) {
    struct k_work_delayable *dwork = k_work_delayable_from_work(work);
    bool synced = true;
    if (atomic_clear(&rgb_force)) {
        rgb_sent_valid = false; // the right doesn't show what it confirmed (#624)
    }
    uint8_t buf[sizeof(rgb_sent)];
    uint8_t len;
    if (rgb_op(buf, &len)) {
        if (!rgb_sent_valid || memcmp(buf, rgb_sent, sizeof(buf)) != 0) {
            // A write request, so the right's answer confirms it applied the state: one small
            // request per change, with write_req's timeout.
            struct bt_conn *conn = job_conn();
            synced = conn != NULL && write_req(conn, buf, len) == 0;
            if (conn != NULL) {
                bt_conn_unref(conn);
            }
            if (synced) {
                bool dim_changed = !rgb_sent_valid || memcmp(&buf[3], &rgb_sent[3], 5) != 0;
                memcpy(rgb_sent, buf, sizeof(buf));
                rgb_sent_valid = true;
                if (dim_changed && IS_ENABLED(CONFIG_NICE_VIEW_RGB_BATTERY_DIM)) {
                    dim_read(); // the PC hears once the right has the new levels
                }
            }
        }
    }
    if (art_relay_link() < 2) {
        return; // rgb_sync_link_up starts over
    }
    if (!synced && rgb_retries < RGB_RETRIES) {
        rgb_retries++;
        k_work_schedule_for_queue(&relay_q, dwork, RGB_RETRY);
        return;
    }
    if (synced) {
        rgb_retries = 0;
    }
    // Nothing else to do until the next change (art_relay_rgb_kick) or link-up (#280).
}
static K_WORK_DELAYABLE_DEFINE(rgb_sync_work, rgb_sync_cb);

static void rgb_sync_link_up(void) {
    rgb_sent_valid = false;
    rgb_retries = 0;
    atomic_clear(&look_forced_valid); // a new link may need the look again
    atomic_set(&hsb_sent, -1); // a new link: the right may have rebooted
    k_work_reschedule_for_queue(&relay_q, &rgb_sync_work, RGB_SETTLE);
}

// Bluetooth thread (a status read or a ring): with sync on, a right whose stored look isn't the
// left's gets the RGB op again (#624). A reconcile on the right's reports, not a timer.
static void rgb_reconcile(void) {
    if (!rgb_idle_sync() || !right_rgb_known() || art_relay_link() < 2) {
        return;
    }
    uint8_t look[RGB_IDLE_LOOK_SIZE];
    rgb_idle_look(look);
    if (sys_get_le16(look) == right.rgb.hue && look[2] == right.rgb.saturation &&
        look[3] == right.rgb.brightness && look[4] == right.rgb.effect) {
        return;
    }
    if (atomic_get(&look_forced_valid) && memcmp(look, look_forced, sizeof(look)) == 0) {
        return; // sent once for this look already
    }
    memcpy(look_forced, look, sizeof(look));
    atomic_set(&look_forced_valid, 1);
    atomic_set(&rgb_force, 1);
    k_work_reschedule_for_queue(&relay_q, &rgb_sync_work, K_NO_WAIT);
}

void art_relay_rgb_kick(void) {
    if (art_relay_link() == 2) {
        rgb_retries = 0;
        lane(LANE_RGB); // at once (#605); the request below confirms it
        k_work_reschedule_for_queue(&relay_q, &rgb_sync_work, K_NO_WAIT);
    }
}

/* The PC's colour (#266). &rgb_ug RGB_COLOR_HSB through ZMK's split queue reached the right
 * seconds late or not at all under streaming (that queue drops its oldest entry when full, and
 * the PC only saw the right through the next status read). This sends the newest colour as one
 * write request instead: the write response is the right's ack, the PC's RGB_STATE takes the
 * right's colour from it, and a value that changes while one is in flight goes next (last wins). */

#define HSB_RETRY K_MSEC(100)
#define HSB_BUSY_RETRY K_MSEC(20) // another request on the relay (a status read) is in flight
#define HSB_RETRIES 5
// The request in the way ends with its answer or GATT_TIMEOUT (3 s); this many waits is past that.
#define BUSY_WAITS 160
#define RGB_VERIFY_AFTER K_MSEC(300) // ZMK's split write, then the right's next underglow tick

static atomic_t hsb_want = ATOMIC_INIT(-1); // hue << 16 | saturation << 8 | brightness, -1 none
static uint8_t hsb_retries, hsb_busy, verify_busy;

static void hsb_cb(struct k_work *work) {
    struct k_work_delayable *dwork = k_work_delayable_from_work(work);
    atomic_val_t want = atomic_get(&hsb_want);
    if (want < 0 || want == atomic_get(&hsb_sent)) {
        return;
    }
    struct bt_conn *conn = job_conn();
    if (conn == NULL) {
        return; // link gone: rgb_sync_link_up starts over, the PC re-sends on its own
    }
    uint8_t buf[5] = {ART_RELAY_RGB_HSB};
    sys_put_le16(want >> 16, &buf[1]);
    buf[3] = (want >> 8) & 0xff;
    buf[4] = want & 0xff;
    int err = write_req(conn, buf, sizeof(buf));
    bt_conn_unref(conn);
    if (err == -EBUSY && hsb_busy++ < BUSY_WAITS) {
        k_work_reschedule_for_queue(&relay_q, dwork, HSB_BUSY_RETRY);
        return;
    }
    hsb_busy = 0;
    if (err) {
        if (hsb_retries++ < HSB_RETRIES) {
            k_work_reschedule_for_queue(&relay_q, dwork, HSB_RETRY);
        }
        return;
    }
    hsb_retries = 0;
    atomic_set(&hsb_sent, want);
    // The ack says the right has it: that's the right's colour now, for RGB_STATE.
    right.rgb.hue = want >> 16;
    right.rgb.saturation = (want >> 8) & 0xff;
    right.rgb.brightness = want & 0xff;
    right_read_at = k_uptime_get();
    if (atomic_get(&hsb_want) != want) {
        k_work_reschedule_for_queue(&relay_q, dwork, K_NO_WAIT); // a newer one came meanwhile
    } else {
        host_link_rgb_right_followed();
    }
}
static K_WORK_DELAYABLE_DEFINE(hsb_work, hsb_cb);

int art_relay_rgb_hsb(uint16_t hue, uint8_t saturation, uint8_t brightness) {
    if (art_relay_link() < 2 || right_read_at == 0 || right.version < 5) {
        return -ENOTSUP;
    }
    atomic_set(&hsb_want, (atomic_val_t)hue << 16 | saturation << 8 | brightness);
    hsb_retries = 0;
    hsb_busy = 0;
    k_work_reschedule_for_queue(&relay_q, &hsb_work, K_NO_WAIT);
    return 0;
}

static void verify_cb(struct k_work *work) {
    struct bt_conn *conn = job_conn();
    if (conn == NULL) {
        return;
    }
    int err = read_status(conn);
    bt_conn_unref(conn);
    if (err == -EBUSY && verify_busy++ < BUSY_WAITS) {
        k_work_reschedule_for_queue(&relay_q, k_work_delayable_from_work(work), HSB_BUSY_RETRY);
        return;
    }
    verify_busy = 0;
    if (!err) {
        host_link_rgb_right_followed();
    }
}
static K_WORK_DELAYABLE_DEFINE(verify_work, verify_cb);

void art_relay_rgb_verify(void) {
    if (art_relay_link() == 2) {
        verify_busy = 0;
        k_work_reschedule_for_queue(&relay_q, &verify_work, RGB_VERIFY_AFTER);
    }
}

/* SET_FRAME (#272): the PC's frames for the right half. host_frames.c on the left hands over
 * each frame at most every 100 ms (newest wins); this sends what changed since the frame the
 * right last acknowledged. The take-over is a write request (ART_RELAY_LAMP_HOST); a frame's lamps
 * go as write commands with the last one, or a one-colour fill, as a write request: ATT keeps
 * their order, so its response acknowledges the whole frame. A static frame sends nothing.
 *
 * Start/stop: a job starts on a frame, a take-over or hand-back, or link-up while the PC has the
 * lights; it stops when the right has acknowledged, the link is gone, or after LAMPS_RETRIES
 * failed tries (the next frame starts over in full). */

#define EFFECT_STATUS_VERSION 8 // right status version with ART_RELAY_EFFECT / EFFECT_GO
#define EFFECT_LAMPS_STATUS_VERSION 9 // ... whose copy of the wire holds the LED points (#550)
BUILD_ASSERT(sizeof(struct lamp_effect_wire) <= 255, "ART_RELAY_EFFECT's offset is a u8");
BUILD_ASSERT(LAMP_PARTS_MAX <= LAMP_PARTS_RELAY_MAX,
             "a half's LEDs past what the relay carries (lamp_parts.h)");
static bool effect_takes_right(void);

#if IS_ENABLED(CONFIG_FOXGLOVE_HOST_FRAMES)
#define LAMPS_RETRY K_MSEC(100)
#define LAMPS_RETRIES 3 // a frame: the next frame heals a lost one anyway
// A hand-back has no next frame to heal it: the RGB sync's budget (10 x 1 s).
#define LAMPS_BACK_RETRY RGB_RETRY
#define LAMPS_BACK_RETRIES RGB_RETRIES
#define LAMPS_STATUS_VERSION 7 // right status version with ART_RELAY_LAMPS / LAMP_HOST
#define LAMPS_REQ_MAX 5        // lamps in one write request (write_req copies up to 20 bytes)
BUILD_ASSERT(3 + 3 * LAMPS_REQ_MAX <= 20, "write_req's copy holds the last chunk");

static struct k_spinlock lamps_lock;
static uint8_t lamps_want[ART_RELAY_LAMPS_MAX][3]; // under lamps_lock
static atomic_t lamps_host;
// On relay_q only:
static uint8_t lamps_acked[ART_RELAY_LAMPS_MAX][3];
static bool lamps_acked_valid;
static int8_t lamps_host_sent = -1; // the LAMP_HOST the right last acknowledged, -1 unknown
static uint8_t lamps_tries, lamps_busy;
static atomic_t lamps_right; // ART_LAMPS_*
static atomic_t lamps_reset; // a new link: lamps_cb starts over

// The lamps start..start+n-1 of frame: all but the last as commands, then the last (up to
// LAMPS_REQ_MAX of them) as a request; or a fill as one request.
static int lamps_send(struct bt_conn *conn, const uint8_t frame[][3], int start, int n,
                      bool uniform) {
    // Static: relay_q's alone (its stack is 1 KB), sized by the right's LEDs (lamp_parts.h).
    static uint8_t buf[3 + 3 * ART_RELAY_LAMPS_MAX] = {ART_RELAY_LAMPS};
    if (uniform && n > 1) {
        buf[1] = start;
        buf[2] = 0x80 | n;
        memcpy(&buf[3], frame[start], 3);
        return write_req(conn, buf, 6);
    }
    int per = CLAMP(((int)bt_gatt_get_mtu(conn) - 3 - 3) / 3, 1, ART_RELAY_LAMPS_MAX);
    int tail = MIN(n, LAMPS_REQ_MAX);
    for (int at = start; at < start + n - tail; at += per) {
        int k = MIN(per, start + n - tail - at);
        buf[1] = at;
        buf[2] = k;
        memcpy(&buf[3], frame[at], 3 * k);
        int err = write_cmd(conn, buf, 3 + 3 * k);
        if (err) {
            return err;
        }
    }
    buf[1] = start + n - tail;
    buf[2] = tail;
    memcpy(&buf[3], frame[start + n - tail], 3 * tail);
    return write_req(conn, buf, 3 + 3 * tail);
}

static void lamps_cb(struct k_work *work) {
    struct k_work_delayable *dwork = k_work_delayable_from_work(work);
    if (atomic_cas(&lamps_reset, 1, 0)) {
        lamps_host_sent = -1; // the right handed its LEDs back when the old link dropped
        lamps_acked_valid = false;
        lamps_tries = 0;
        lamps_busy = 0;
    }
    struct bt_conn *conn = job_conn();
    if (conn == NULL) {
        return; // lamps_link_up starts over
    }
    int err = 0;
    if (right_read_at == 0) {
        err = read_status(conn); // the right's version says whether it draws frames
    }
    if (!err && right.version < LAMPS_STATUS_VERSION) {
        atomic_set(&lamps_right, ART_LAMPS_UNSUPPORTED); // its own effect stays
        bt_conn_unref(conn);
        return;
    }
    bool host = atomic_get(&lamps_host);
    if (!err && !host && lamps_host_sent == 1 && effect_takes_right()) {
        // SET_EFFECT took over from the frames: its GO takes the right's LEDs as they are, so
        // handing them back first would only flash the right's own effect.
        lamps_host_sent = 0;
    }
    if (!err && (host ? lamps_host_sent != 1 : lamps_host_sent == 1)) {
        const uint8_t op[] = {ART_RELAY_LAMP_HOST, host};
        err = write_req(conn, op, sizeof(op));
        if (!err) {
            lamps_host_sent = host;
            lamps_acked_valid = false; // the right starts from black: send it all
        }
    }
    if (!err) {
        atomic_set(&lamps_right, ART_LAMPS_OK);
    }
    if (!err && host) {
        static uint8_t frame[ART_RELAY_LAMPS_MAX][3]; // relay_q's alone, off its 1 KB stack
        k_spinlock_key_t key = k_spin_lock(&lamps_lock);
        memcpy(frame, lamps_want, sizeof(frame));
        k_spin_unlock(&lamps_lock, key);
        int start = 0;
        bool uniform = true;
        int n = ART_RELAY_LAMPS_MAX;
        if (lamps_acked_valid) {
            n = art_relay_lamp_span(lamps_acked, frame, ART_RELAY_LAMPS_MAX, &start, &uniform);
        } else {
            for (int i = 1; i < n && uniform; i++) {
                uniform = memcmp(frame[i], frame[0], 3) == 0;
            }
        }
        if (n > 0) {
            lamps_acked_valid = false; // until the right acknowledges this one
            err = lamps_send(conn, frame, start, n, uniform);
            if (!err) {
                memcpy(lamps_acked, frame, sizeof(frame));
                lamps_acked_valid = true;
            }
        }
    }
    bt_conn_unref(conn);
    if (err == -EBUSY && lamps_busy++ < BUSY_WAITS) {
        k_work_reschedule_for_queue(&relay_q, dwork, HSB_BUSY_RETRY); // a status read is out
        return;
    }
    lamps_busy = 0;
    if (err && lamps_tries++ < (host ? LAMPS_RETRIES : LAMPS_BACK_RETRIES)) {
        k_work_reschedule_for_queue(&relay_q, dwork, host ? LAMPS_RETRY : LAMPS_BACK_RETRY);
        return;
    }
    lamps_tries = 0;
}
static K_WORK_DELAYABLE_DEFINE(lamps_work, lamps_cb);

void art_relay_lamps(bool host, const uint8_t rgb[ART_RELAY_LAMPS_MAX][3]) {
    k_spinlock_key_t key = k_spin_lock(&lamps_lock);
    memcpy(lamps_want, rgb, sizeof(lamps_want));
    k_spin_unlock(&lamps_lock, key);
    atomic_set(&lamps_host, host);
    if (art_relay_link() == 2) {
        // Not reschedule: a retry that's waiting keeps its slot and then sends the newest frame.
        k_work_schedule_for_queue(&relay_q, &lamps_work, K_NO_WAIT);
    }
}

uint8_t art_relay_lamps_right(void) {
    return art_relay_link() == 2 ? atomic_get(&lamps_right) : ART_LAMPS_NO_LINK;
}

// From the discovery callback (Bluetooth thread); the relay queue's state resets in lamps_cb.
static void lamps_link_up(void) {
    atomic_set(&lamps_reset, 1);
    atomic_set(&lamps_right, ART_LAMPS_NO_LINK);
    if (atomic_get(&lamps_host)) {
        k_work_reschedule_for_queue(&relay_q, &lamps_work, RGB_SETTLE);
    }
}
#else
static void lamps_link_up(void) {}
#endif

/* SET_EFFECT (#490): the right half's copy of the PC's effect (host_effects.c). Its parameters go
 * as write commands (ART_RELAY_EFFECT, offset + bytes), then one write request (EFFECT_GO) with
 * the left's clock read just before it goes, so the right's phase is the left's within the link's
 * latency; ATT keeps the order, so GO's response acknowledges the whole effect. A stop is a GO
 * with run 0, unless SET_FRAME took over (its LAMP_HOST stops the right's effect).
 *
 * Start/stop: a job starts on an effect, a PLACE, a stop, link-up while an effect runs, or the
 * left waking from idle (the clock alone, so long runs don't drift); it stops when the right has
 * acknowledged, the link is gone, or after EFFECT_RETRIES failed tries. */

#if IS_ENABLED(CONFIG_FOXGLOVE_HOST_EFFECTS)
#define EFFECT_RETRY RGB_RETRY // nothing re-sends an effect: the RGB sync's budget (10 x 1 s)
#define EFFECT_RETRIES RGB_RETRIES

static struct k_spinlock effect_lock;
static struct lamp_effect_wire effect_want; // under effect_lock
static atomic_t effect_run;   // the right should draw effect_want
static atomic_t effect_tell;  // a stop goes to the right (not when SET_FRAME took over)
static atomic_t effect_dirty; // effect_want or effect_run changed since the right's last ack
static atomic_t effect_right; // ART_LAMPS_*
static atomic_t effect_reset; // a new link: effect_cb starts over
static atomic_t effect_resync; // the running effect's clock again, alone
// On relay_q only:
static int8_t effect_sent = -1; // the GO the right last acknowledged: -1 unknown, 0 stop, 1 run
static uint8_t effect_tries, effect_busy;

static bool effect_takes_right(void) {
    return atomic_get(&effect_run) && right.version >= EFFECT_STATUS_VERSION;
}

// The left's clock, read just before it goes, so the right starts where the left is.
static int effect_go(struct bt_conn *conn) {
    uint8_t go[6] = {ART_RELAY_EFFECT_GO, 1};
    sys_put_le32(effect_run_clock(), &go[2]);
    return write_req(conn, go, sizeof(go));
}

static int effect_send(struct bt_conn *conn) {
    // Static: relay_q's alone, and its stack is 1 KB (the wire is 126 bytes since #550).
    static struct lamp_effect_wire w;
    static uint8_t buf[2 + sizeof(w)] = {ART_RELAY_EFFECT};
    k_spinlock_key_t key = k_spin_lock(&effect_lock);
    w = effect_want;
    k_spin_unlock(&effect_lock, key);
    // A right before #550 has no room for the LED points: it keeps its own table. Else the
    // right's own LEDs (#561): the whole wire on halves that mirror each other.
    int size = right.version >= EFFECT_LAMPS_STATUS_VERSION
                   ? (int)(offsetof(struct lamp_effect_wire, lamp) + 4 * ART_RELAY_LAMPS_MAX)
                   : (int)offsetof(struct lamp_effect_wire, lamps_set);
    int per = CLAMP((int)bt_gatt_get_mtu(conn) - 3 - 2, 1, size);
    for (int at = 0; at < size; at += per) {
        int n = MIN(per, size - at);
        buf[1] = at;
        memcpy(&buf[2], (const uint8_t *)&w + at, n);
        int err = write_cmd(conn, buf, 2 + n);
        if (err) {
            return err;
        }
    }
    return effect_go(conn);
}

static void effect_cb(struct k_work *work) {
    struct k_work_delayable *dwork = k_work_delayable_from_work(work);
    if (atomic_cas(&effect_reset, 1, 0)) {
        effect_sent = -1; // the right handed its LEDs back when the old link dropped
        effect_tries = 0;
        effect_busy = 0;
    }
    struct bt_conn *conn = job_conn();
    if (conn == NULL) {
        return; // effect_link_up starts over
    }
    int err = 0;
    if (right_read_at == 0) {
        err = read_status(conn); // the right's version says whether it draws effects
    }
    if (!err && right.version < EFFECT_STATUS_VERSION) {
        atomic_set(&effect_right, ART_LAMPS_UNSUPPORTED); // its own effect stays
        bt_conn_unref(conn);
        return;
    }
    bool run = atomic_get(&effect_run);
    bool dirty = atomic_cas(&effect_dirty, 1, 0);
    bool resync = atomic_cas(&effect_resync, 1, 0);
    if (!err && run && (dirty || effect_sent != 1)) {
        effect_sent = -1;
        err = effect_send(conn);
        if (!err) {
            effect_sent = 1;
        }
    } else if (!err && run && resync) {
        err = effect_go(conn); // the right has the effect: only its clock again
        if (err) {
            atomic_set(&effect_resync, 1);
        }
    } else if (!err && !run && effect_sent != 0 && atomic_get(&effect_tell)) {
        const uint8_t go[6] = {ART_RELAY_EFFECT_GO, 0};
        err = write_req(conn, go, sizeof(go));
        if (!err) {
            effect_sent = 0;
        }
    } else if (!err && !run) {
        effect_sent = 0; // SET_FRAME's take-over stops it on the right
    }
    if (!err) {
        atomic_set(&effect_right, ART_LAMPS_OK);
    } else if (dirty) {
        atomic_set(&effect_dirty, 1); // not sent: the retry sends it
    }
    bt_conn_unref(conn);
    if (err == -EBUSY && effect_busy++ < BUSY_WAITS) {
        k_work_reschedule_for_queue(&relay_q, dwork, HSB_BUSY_RETRY); // a status read is out
        return;
    }
    effect_busy = 0;
    if (err && effect_tries++ < EFFECT_RETRIES) {
        k_work_reschedule_for_queue(&relay_q, dwork, EFFECT_RETRY);
        return;
    }
    effect_tries = 0;
}
static K_WORK_DELAYABLE_DEFINE(effect_work, effect_cb);

void art_relay_effect(bool run, bool tell, const struct lamp_effect_wire *right_half) {
    if (run) {
        k_spinlock_key_t key = k_spin_lock(&effect_lock);
        effect_want = *right_half;
        k_spin_unlock(&effect_lock, key);
    }
    atomic_set(&effect_run, run);
    atomic_set(&effect_tell, tell);
    atomic_set(&effect_dirty, 1);
    if (art_relay_link() == 2) {
        k_work_reschedule_for_queue(&relay_q, &effect_work, K_NO_WAIT);
    }
}

uint8_t art_relay_effect_right(void) {
    return art_relay_link() == 2 ? atomic_get(&effect_right) : ART_LAMPS_NO_LINK;
}

void art_relay_effect_sync(void) {
    if (atomic_get(&effect_run) && art_relay_link() == 2) {
        atomic_set(&effect_resync, 1);
        k_work_reschedule_for_queue(&relay_q, &effect_work, K_NO_WAIT);
    }
}

// From the discovery callback (Bluetooth thread); the relay queue's state resets in effect_cb.
static void effect_link_up(void) {
    atomic_set(&effect_reset, 1);
    atomic_set(&effect_right, ART_LAMPS_NO_LINK);
    if (atomic_get(&effect_run)) {
        k_work_reschedule_for_queue(&relay_q, &effect_work, RGB_SETTLE);
    }
}
#else
static __maybe_unused bool effect_takes_right(void) { return false; }
static void effect_link_up(void) {}
#endif
#else
static void lane_rgb(struct bt_conn *conn, uint16_t handle) {}
static void rgb_sync_link_up(void) {}
static void rgb_reconcile(void) {}
static void lamps_link_up(void) {}
static void effect_link_up(void) {}
void art_relay_rgb_kick(void) {}
int art_relay_rgb_hsb(uint16_t hue, uint8_t saturation, uint8_t brightness) { return -ENOTSUP; }
void art_relay_rgb_verify(void) {}
#endif
