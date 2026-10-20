/*
 * Left half: collects an art blob from the PC (host_link.c), then pushes it to the right half's
 * art relay service (art_relay_peripheral.c) over the split connection.
 * SPDX-License-Identifier: MIT
 *
 * The service isn't part of ZMK's split protocol, so this finds it on its own: once the split
 * connection is up (and ZMK has had time to secure it), it discovers the service and its two
 * characteristics. Pushes run on their own work queue, where blocking on the radio is fine.
 */

#include <string.h>

#include <zephyr/kernel.h>
#include <zephyr/bluetooth/bluetooth.h>
#include <zephyr/bluetooth/conn.h>
#include <zephyr/bluetooth/gatt.h>
#include <zephyr/bluetooth/uuid.h>
#include <zephyr/sys/byteorder.h>
#include <zephyr/sys/crc.h>

#include <zephyr/logging/log.h>
LOG_MODULE_DECLARE(zmk, CONFIG_ZMK_LOG_LEVEL);

#include "art_relay.h"

#define DISCOVER_DELAY K_SECONDS(3)
#define GATT_TIMEOUT K_SECONDS(3)

static uint8_t blob[CONFIG_NICE_VIEW_ART_RELAY_MAX_SIZE];
static uint16_t blob_len;
static uint32_t blob_crc;
static uint16_t pushed;
static uint8_t state = ART_LEFT_IDLE;
static uint8_t error;
static struct art_relay_status right;

static struct bt_conn *peer;
static uint16_t data_handle, status_handle;

static const struct bt_uuid_128 service_uuid = BT_UUID_INIT_128(ART_RELAY_SERVICE_UUID);
static const struct bt_uuid_128 data_uuid = BT_UUID_INIT_128(ART_RELAY_DATA_UUID);
static const struct bt_uuid_128 status_uuid = BT_UUID_INIT_128(ART_RELAY_STATUS_UUID);

/* Discovery */

static struct bt_gatt_discover_params discover_params;

static uint8_t chrc_found(struct bt_conn *conn, const struct bt_gatt_attr *attr,
                          struct bt_gatt_discover_params *params) {
    if (attr == NULL) {
        LOG_INF("art relay: right half %s", data_handle ? "ready" : "has no relay");
        return BT_GATT_ITER_STOP;
    }
    const struct bt_gatt_chrc *chrc = attr->user_data;
    if (bt_uuid_cmp(chrc->uuid, &data_uuid.uuid) == 0) {
        data_handle = chrc->value_handle;
    } else if (bt_uuid_cmp(chrc->uuid, &status_uuid.uuid) == 0) {
        status_handle = chrc->value_handle;
    }
    return BT_GATT_ITER_CONTINUE;
}

static uint8_t service_found(struct bt_conn *conn, const struct bt_gatt_attr *attr,
                             struct bt_gatt_discover_params *params) {
    if (attr == NULL) {
        LOG_INF("art relay: right half has no relay service");
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
    }
    return BT_GATT_ITER_STOP;
}

static void discover_cb(struct k_work *work) {
    if (peer == NULL) {
        return;
    }
    discover_params.uuid = &service_uuid.uuid;
    discover_params.func = service_found;
    discover_params.start_handle = 0x0001;
    discover_params.end_handle = 0xffff;
    discover_params.type = BT_GATT_DISCOVER_PRIMARY;
    int err = bt_gatt_discover(peer, &discover_params);
    if (err) {
        LOG_ERR("art relay: service discovery failed (%d)", err);
    }
}
static K_WORK_DELAYABLE_DEFINE(discover_work, discover_cb);

static void connected(struct bt_conn *conn, uint8_t err) {
    struct bt_conn_info info;
    if (err || peer != NULL || bt_conn_get_info(conn, &info) || info.role != BT_CONN_ROLE_CENTRAL) {
        return; // only the split link has us as central; host links have us as peripheral
    }
    peer = bt_conn_ref(conn);
    data_handle = status_handle = 0;
    k_work_reschedule(&discover_work, DISCOVER_DELAY);
}

static void disconnected(struct bt_conn *conn, uint8_t reason) {
    if (conn != peer) {
        return;
    }
    k_work_cancel_delayable(&discover_work);
    bt_conn_unref(peer);
    peer = NULL;
    data_handle = status_handle = 0;
}

BT_CONN_CB_DEFINE(art_relay_conn_cb) = {
    .connected = connected,
    .disconnected = disconnected,
};

/* GATT calls, made synchronous for the push work queue */

static K_SEM_DEFINE(gatt_done, 0, 1);
static uint8_t gatt_err;

static void write_cb(struct bt_conn *conn, uint8_t err, struct bt_gatt_write_params *params) {
    gatt_err = err;
    k_sem_give(&gatt_done);
}

static int write_req(const uint8_t *data, uint16_t len) {
    static struct bt_gatt_write_params params;
    params.func = write_cb;
    params.handle = data_handle;
    params.offset = 0;
    params.data = data;
    params.length = len;
    k_sem_reset(&gatt_done);
    if (bt_gatt_write(peer, &params) || k_sem_take(&gatt_done, GATT_TIMEOUT)) {
        return -EIO;
    }
    return gatt_err ? -EIO : 0;
}

static int write_cmd(const uint8_t *data, uint16_t len) {
    for (int tries = 0; tries < 50; tries++) {
        int err = bt_gatt_write_without_response(peer, data_handle, data, len, false);
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
        return BT_GATT_ITER_CONTINUE;
    }
    gatt_err = err;
    k_sem_give(&gatt_done);
    return BT_GATT_ITER_STOP;
}

static int read_status(void) {
    static struct bt_gatt_read_params params;
    params.func = read_cb;
    params.handle_count = 1;
    params.single.handle = status_handle;
    params.single.offset = 0;
    k_sem_reset(&gatt_done);
    if (bt_gatt_read(peer, &params) || k_sem_take(&gatt_done, GATT_TIMEOUT)) {
        return -EIO;
    }
    return gatt_err ? -EIO : 0;
}

/* Pushes */

K_THREAD_STACK_DEFINE(relay_stack, 1024);
static struct k_work_q relay_q;
static int8_t select_set;

static void fail(uint8_t err) {
    error = err;
    state = ART_LEFT_FAILED;
}

static void push_cb(struct k_work *work) {
    if (art_relay_link() < 2) {
        fail(ART_ERR_NO_LINK);
        return;
    }
    uint8_t buf[3 + 244];
    buf[0] = ART_RELAY_BEGIN;
    sys_put_le16(blob_len, &buf[1]);
    sys_put_le32(blob_crc, &buf[3]);
    if (write_req(buf, 7)) {
        fail(ART_ERR_BLE);
        return;
    }
    int chunk = CLAMP((int)bt_gatt_get_mtu(peer) - 3 - 3, 16, (int)sizeof(buf) - 3);
    for (pushed = 0; pushed < blob_len;) {
        int n = MIN(chunk, blob_len - pushed);
        buf[0] = ART_RELAY_DATA;
        sys_put_le16(pushed, &buf[1]);
        memcpy(&buf[3], &blob[pushed], n);
        if (peer == NULL || write_cmd(buf, 3 + n)) {
            fail(ART_ERR_BLE);
            return;
        }
        pushed += n;
    }
    buf[0] = ART_RELAY_COMMIT;
    if (write_req(buf, 1) || read_status()) {
        fail(ART_ERR_BLE);
        return;
    }
    if (right.state != ART_RIGHT_PLAYING) {
        fail(ART_ERR_RIGHT);
        return;
    }
    state = ART_LEFT_DONE;
}
static K_WORK_DEFINE(push_work, push_cb);

static void select_cb(struct k_work *work) {
    uint8_t buf[] = {ART_RELAY_SELECT, select_set};
    if (art_relay_link() < 2 || write_req(buf, sizeof(buf)) || read_status()) {
        fail(art_relay_link() < 2 ? ART_ERR_NO_LINK : ART_ERR_BLE);
        return;
    }
    state = ART_LEFT_DONE;
}
static K_WORK_DEFINE(select_work, select_cb);

static void status_cb(struct k_work *work) {
    if (art_relay_link() == 2) {
        read_status();
    }
}
static K_WORK_DEFINE(status_work, status_cb);

/* Called from host_link.c on the USB thread */

int art_relay_begin(uint16_t len, uint32_t crc) {
    if (state == ART_LEFT_PUSHING) {
        return -EBUSY; // leave the running push's state alone
    }
    if (len > sizeof(blob)) {
        fail(ART_ERR_TOO_BIG);
        return -ENOMEM;
    }
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
        fail(ART_ERR_RANGE);
        return -EINVAL;
    }
    memcpy(&blob[offset], data, len);
    return 0;
}

int art_relay_commit(void) {
    if (state != ART_LEFT_RECEIVING) {
        return -EINVAL;
    }
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
    select_set = set;
    state = ART_LEFT_PUSHING;
    error = ART_ERR_NONE;
    pushed = blob_len = 0;
    k_work_submit_to_queue(&relay_q, &select_work);
    return 0;
}

uint8_t art_relay_link(void) {
    if (peer == NULL) {
        return 0;
    }
    return data_handle && status_handle ? 2 : 1;
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
