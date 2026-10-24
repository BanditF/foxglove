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
#include "host_link.h"
#include "screen_snap.h"

// ZMK discovers and subscribes to its own split service once the link is secured; a discovery
// of ours running alongside that wedged both (requests never answered), so wait it out.
#define DISCOVER_DELAY K_SECONDS(15)
#define DISCOVER_RETRY K_SECONDS(5)
#define DISCOVER_TRIES 20
#define GATT_TIMEOUT K_SECONDS(3)

static uint8_t blob[CONFIG_NICE_VIEW_ART_RELAY_MAX_SIZE];
static uint16_t blob_len;
static uint32_t blob_crc;
static uint16_t pushed;
static uint8_t state = ART_LEFT_IDLE;
static uint8_t error;
static struct art_relay_status right;
static uint16_t right_len;   // bytes of it the last status read filled
static int64_t right_read_at; // uptime of that read, 0 = never

static struct bt_conn *peer;
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

// Tries again a little later, in case ZMK's own setup of the link got in the way.
static void discovery_failed(int8_t result) {
    discovery = result;
    if (peer != NULL && discovery_tries < DISCOVER_TRIES) {
        k_work_reschedule(&discover_work, DISCOVER_RETRY);
    }
}

static uint8_t chrc_found(struct bt_conn *conn, const struct bt_gatt_attr *attr,
                          struct bt_gatt_discover_params *params) {
    if (attr == NULL) {
        dbg.ends++;
        LOG_INF("art relay: right half %s", data_handle ? "ready" : "has no relay");
        if (data_handle && status_handle) {
            discovery = ART_DISCOVERY_DONE;
            rgb_sync_link_up();
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
        rgb_sync_link_up();
        host_link_art_status();
        return BT_GATT_ITER_STOP;
    }
    return BT_GATT_ITER_CONTINUE;
}

static uint8_t service_found(struct bt_conn *conn, const struct bt_gatt_attr *attr,
                             struct bt_gatt_discover_params *params) {
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
        LOG_ERR("art relay: characteristic discovery failed (%d)", err);
        discovery_failed(MAX(err, INT8_MIN));
    }
    return BT_GATT_ITER_STOP;
}

static void discover_cb(struct k_work *work) {
    if (peer == NULL || (data_handle && status_handle)) {
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
        LOG_ERR("art relay: service discovery failed (%d)", err);
        discovery_failed(MAX(err, INT8_MIN));
    }
}
static K_WORK_DELAYABLE_DEFINE(discover_work, discover_cb);

static void connected(struct bt_conn *conn, uint8_t err) {
    struct bt_conn_info info;
    if (err || peer != NULL || bt_conn_get_info(conn, &info) || info.role != BT_CONN_ROLE_CENTRAL) {
        return; // only the split link has us as central; host links have us as peripheral
    }
    peer = bt_conn_ref(conn);
    data_handle = status_handle = screen_handle = 0;
    discovery_tries = 0;
}

static void security_changed(struct bt_conn *conn, bt_security_t level, enum bt_security_err err) {
    if (conn == peer && !err && level >= BT_SECURITY_L2 && !(data_handle && status_handle)) {
        k_work_reschedule(&discover_work, DISCOVER_DELAY);
    }
}

static void disconnected(struct bt_conn *conn, uint8_t reason) {
    if (conn != peer) {
        return;
    }
    bool was_ready = art_relay_link() == 2;
    k_work_cancel_delayable(&discover_work);
    bt_conn_unref(peer);
    peer = NULL;
    data_handle = status_handle = screen_handle = 0;
    right_read_at = 0;
    if (was_ready) {
        host_link_art_status();
    }
}

BT_CONN_CB_DEFINE(art_relay_conn_cb) = {
    .connected = connected,
    .disconnected = disconnected,
    .security_changed = security_changed,
};

/* GATT calls, made synchronous for the push work queue */

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
    static uint8_t copy[8];
    if (atomic_get(&in_flight) || len > sizeof(copy)) {
        return -EBUSY;
    }
    memcpy(copy, data, len); // the request may outlive the caller's buffer
    params.func = write_cb;
    params.handle = data_handle;
    params.offset = 0;
    params.data = copy;
    params.length = len;
    k_sem_reset(&gatt_done);
    atomic_set(&in_flight, 1); // before the call: the callback can come before it returns
    return wait_done(bt_gatt_write(conn, &params));
}

static int write_cmd(struct bt_conn *conn, const uint8_t *data, uint16_t len) {
    for (int tries = 0; tries < 50; tries++) {
        int err = bt_gatt_write_without_response(conn, data_handle, data, len, false);
        if (err != -ENOMEM && err != -EAGAIN) {
            return err;
        }
        k_msleep(10); // out of buffers: let the radio catch up
    }
    return -ETIMEDOUT;
}

static uint8_t read_cb(struct bt_conn *conn, uint8_t err, struct bt_gatt_read_params *params,
                       const void *data, uint16_t length) {
    if (data != NULL) {
        memcpy(&right, data, MIN(length, sizeof(right)));
        right_len = length;
        right_read_at = k_uptime_get();
        return BT_GATT_ITER_CONTINUE;
    }
    request_done(err);
    return BT_GATT_ITER_STOP;
}

static int read_status(struct bt_conn *conn) {
    static struct bt_gatt_read_params params;
    if (atomic_get(&in_flight)) {
        return -EBUSY;
    }
    params.func = read_cb;
    params.handle_count = 1;
    params.single.handle = status_handle;
    params.single.offset = 0;
    k_sem_reset(&gatt_done);
    atomic_set(&in_flight, 1);
    return wait_done(bt_gatt_read(conn, &params));
}

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

static int push(struct bt_conn *conn) {
    uint8_t buf[3 + 244];
    buf[0] = ART_RELAY_BEGIN;
    sys_put_le16(blob_len, &buf[1]);
    sys_put_le32(blob_crc, &buf[3]);
    if (write_req(conn, buf, 7)) {
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
    if (write_req(conn, buf, 1) || read_status(conn)) {
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
    bool err = write_req(conn, buf, sizeof(buf)) || read_status(conn);
    bt_conn_unref(conn);
    if (err) {
        fail(ART_ERR_BLE);
        return;
    }
    state = ART_LEFT_DONE;
}
static K_WORK_DEFINE(select_work, select_cb);

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

void art_relay_discovery(int8_t *result, uint8_t *tries) {
    *result = discovery;
    *tries = discovery_tries;
}

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

uint8_t art_relay_right_rgb(struct rgb_idle_wire *out) {
    if (state != ART_LEFT_PUSHING) {
        k_work_submit_to_queue(&relay_q, &status_work);
    }
    if (art_relay_link() < 2 || right_read_at == 0 || right.version < 2 ||
        right_len < sizeof(right)) {
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

static int art_relay_init(void) {
    k_work_queue_start(&relay_q, relay_stack, K_THREAD_STACK_SIZEOF(relay_stack),
                       K_LOWEST_APPLICATION_THREAD_PRIO, NULL);
    return 0;
}

SYS_INIT(art_relay_init, APPLICATION, CONFIG_APPLICATION_INIT_PRIORITY);

/* Underglow: the right half follows ours (rgb_idle.c) */

#if IS_ENABLED(CONFIG_NICE_VIEW_RGB_IDLE)
// Right after the link comes up, ZMK's own setup traffic goes first.
#define RGB_SETTLE K_MSEC(100)
// While active, check now and then.
#define RGB_POLL K_SECONDS(5)
// A send that failed is retried this often, idle or not, up to RGB_RETRIES times per change.
#define RGB_RETRY K_SECONDS(1)
#define RGB_RETRIES 10

static int16_t rgb_sent = -1; // last on | follow << 1 the right half confirmed, -1 = unknown
static uint8_t rgb_retries;

static void rgb_sync_cb(struct k_work *work) {
    struct k_work_delayable *dwork = k_work_delayable_from_work(work);
    bool on;
    bool synced = true;
    if (zmk_rgb_underglow_get_state(&on) == 0) {
        bool follow = rgb_idle_sync();
        int16_t now = on | follow << 1;
        if (now != rgb_sent) {
            // A write request, so the right's answer confirms it applied the state: one small
            // request per change, with write_req's timeout.
            struct bt_conn *conn = job_conn();
            uint8_t buf[] = {ART_RELAY_RGB, on, follow};
            synced = conn != NULL && write_req(conn, buf, sizeof(buf)) == 0;
            if (conn != NULL) {
                bt_conn_unref(conn);
            }
            if (synced) {
                rgb_sent = now;
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
    if (zmk_activity_get_state() == ZMK_ACTIVITY_ACTIVE) {
        k_work_schedule_for_queue(&relay_q, dwork, RGB_POLL);
    }
}
static K_WORK_DELAYABLE_DEFINE(rgb_sync_work, rgb_sync_cb);

static void rgb_sync_link_up(void) {
    rgb_sent = -1;
    rgb_retries = 0;
    k_work_reschedule_for_queue(&relay_q, &rgb_sync_work, RGB_SETTLE);
}

void art_relay_rgb_kick(void) {
    if (art_relay_link() == 2) {
        rgb_retries = 0;
        k_work_reschedule_for_queue(&relay_q, &rgb_sync_work, K_NO_WAIT);
    }
}
#else
static void rgb_sync_link_up(void) {}
void art_relay_rgb_kick(void) {}
#endif
