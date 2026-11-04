/*
 * The LampArray over Bluetooth (CONFIG_FOXGLOVE_LAMPARRAY_BLE, #88, #239): another HID service on
 * the left, next to ZMK's own and the Raw HID module's, built like the Raw HID module's hog.c,
 * with the same report map as the USB interface (lamparray.c). Whether Windows Dynamic Lighting
 * takes a LampArray over Bluetooth is unverified (#90 row G); dev builds only.
 * SPDX-License-Identifier: MIT
 *
 * All six reports are feature reports: a read is a get-feature, a write a set-feature. There are
 * no input reports, so no CCC and nothing in the key report queue. Every report fits in one
 * write once the host has raised the ATT MTU (MultiUpdate is 50 bytes); writes that need a
 * prepared (long) write are refused. The ATT MTU must be at least 53.
 *
 * The lights go back (host_frames.c) on a disconnect of the host (here), an endpoint or a profile
 * change (host_frames.c's listener).
 */

#include <string.h>

#include <zephyr/bluetooth/conn.h>
#include <zephyr/bluetooth/gatt.h>
#include <zephyr/bluetooth/uuid.h>
#include <zephyr/kernel.h>

#include "host_frames.h"
#include "lamparray.h"

enum {
    HIDS_REMOTE_WAKE = BIT(0),
    HIDS_NORMALLY_CONNECTABLE = BIT(1),
};
#define HIDS_FEATURE 0x03

struct hids_info {
    uint16_t version;
    uint8_t code;
    uint8_t flags;
} __packed;

struct hids_report {
    uint8_t id;
    uint8_t type;
} __packed;

static const struct hids_info info = {
    .version = 0x0111,
    .code = 0x00,
    .flags = HIDS_NORMALLY_CONNECTABLE,
};

static const struct hids_report refs[LAMPARRAY_REPORTS] = {
    {LAMPARRAY_REPORT_ATTRIBUTES, HIDS_FEATURE},
    {LAMPARRAY_REPORT_ATTR_REQUEST, HIDS_FEATURE},
    {LAMPARRAY_REPORT_ATTR_RESPONSE, HIDS_FEATURE},
    {LAMPARRAY_REPORT_MULTI_UPDATE, HIDS_FEATURE},
    {LAMPARRAY_REPORT_RANGE_UPDATE, HIDS_FEATURE},
    {LAMPARRAY_REPORT_CONTROL, HIDS_FEATURE},
};

static uint8_t ctrl_point;

// A read longer than the MTU comes as several reads with an offset; the report is made at offset
// 0 only, so the attributes response moves on one lamp per read, not per piece.
static uint8_t read_buf[LAMPARRAY_REPORT_MAX];
static int read_len;

static ssize_t read_info(struct bt_conn *conn, const struct bt_gatt_attr *attr, void *buf,
                         uint16_t len, uint16_t offset) {
    return bt_gatt_attr_read(conn, attr, buf, len, offset, &info, sizeof(info));
}

static ssize_t read_map(struct bt_conn *conn, const struct bt_gatt_attr *attr, void *buf,
                        uint16_t len, uint16_t offset) {
    return bt_gatt_attr_read(conn, attr, buf, len, offset, lamparray_report_desc,
                             lamparray_report_desc_len);
}

static ssize_t read_ref(struct bt_conn *conn, const struct bt_gatt_attr *attr, void *buf,
                        uint16_t len, uint16_t offset) {
    return bt_gatt_attr_read(conn, attr, buf, len, offset, attr->user_data,
                             sizeof(struct hids_report));
}

static ssize_t read_report(struct bt_conn *conn, const struct bt_gatt_attr *attr, void *buf,
                           uint16_t len, uint16_t offset) {
    const struct hids_report *ref = attr->user_data;
    if (offset == 0) {
        read_len = lamparray_get_report(LAMPARRAY_BLE, ref->id, read_buf);
        if (read_len < 0) { // a set-only report reads as zeros of its size
            read_len = lamparray_report_len(ref->id);
            memset(read_buf, 0, sizeof(read_buf));
        }
    }
    return bt_gatt_attr_read(conn, attr, buf, len, offset, read_buf, read_len);
}

static ssize_t write_report(struct bt_conn *conn, const struct bt_gatt_attr *attr, const void *buf,
                            uint16_t len, uint16_t offset, uint8_t flags) {
    const struct hids_report *ref = attr->user_data;
    if (offset != 0) {
        return BT_GATT_ERR(BT_ATT_ERR_INVALID_OFFSET);
    }
    int err = lamparray_set_report(LAMPARRAY_BLE, ref->id, buf, len);
    if (err == -EINVAL) {
        return BT_GATT_ERR(BT_ATT_ERR_INVALID_ATTRIBUTE_LEN);
    }
    if (err) {
        return BT_GATT_ERR(BT_ATT_ERR_NOT_SUPPORTED);
    }
    return len;
}

static ssize_t write_ctrl_point(struct bt_conn *conn, const struct bt_gatt_attr *attr,
                                const void *buf, uint16_t len, uint16_t offset, uint8_t flags) {
    if (offset + len > sizeof(ctrl_point)) {
        return BT_GATT_ERR(BT_ATT_ERR_INVALID_OFFSET);
    }
    memcpy(&ctrl_point + offset, buf, len); // suspend / exit suspend: nothing to do
    return len;
}

#define PERM_RW (BT_GATT_PERM_READ_ENCRYPT | BT_GATT_PERM_WRITE_ENCRYPT)
#define FEATURE_REPORT(i)                                                                          \
    BT_GATT_CHARACTERISTIC(BT_UUID_HIDS_REPORT, BT_GATT_CHRC_READ | BT_GATT_CHRC_WRITE, PERM_RW,   \
                           read_report, write_report, (void *)&refs[i]),                           \
        BT_GATT_DESCRIPTOR(BT_UUID_HIDS_REPORT_REF, BT_GATT_PERM_READ_ENCRYPT, read_ref, NULL,     \
                           (void *)&refs[i])

BT_GATT_SERVICE_DEFINE(lamparray_hog_svc, BT_GATT_PRIMARY_SERVICE(BT_UUID_HIDS),
                       BT_GATT_CHARACTERISTIC(BT_UUID_HIDS_INFO, BT_GATT_CHRC_READ,
                                              BT_GATT_PERM_READ, read_info, NULL, NULL),
                       BT_GATT_CHARACTERISTIC(BT_UUID_HIDS_REPORT_MAP, BT_GATT_CHRC_READ,
                                              BT_GATT_PERM_READ_ENCRYPT, read_map, NULL, NULL),
                       FEATURE_REPORT(0), FEATURE_REPORT(1), FEATURE_REPORT(2), FEATURE_REPORT(3),
                       FEATURE_REPORT(4), FEATURE_REPORT(5),
                       BT_GATT_CHARACTERISTIC(BT_UUID_HIDS_CTRL_POINT,
                                              BT_GATT_CHRC_WRITE_WITHOUT_RESP, BT_GATT_PERM_WRITE,
                                              NULL, write_ctrl_point, NULL));

// A host link, not the split link (the left is the split's central).
static bool host_conn(struct bt_conn *conn) {
    struct bt_conn_info conn_info;
    return bt_conn_get_info(conn, &conn_info) == 0 && conn_info.role == BT_CONN_ROLE_PERIPHERAL;
}

static void connected(struct bt_conn *conn, uint8_t err) {
    if (err == 0 && host_conn(conn)) {
        lamparray_transport_ready(LAMPARRAY_BLE); // a host can see the service
    }
}

// The host that took over went away: the lights go back.
static void disconnected(struct bt_conn *conn, uint8_t reason) {
    if (host_conn(conn)) {
        host_frames_lamparray(HOST_FRAMES_OWNER_LAMPARRAY_BLE, false);
    }
}

BT_CONN_CB_DEFINE(lamparray_hog_conn_cb) = {
    .connected = connected,
    .disconnected = disconnected,
};
