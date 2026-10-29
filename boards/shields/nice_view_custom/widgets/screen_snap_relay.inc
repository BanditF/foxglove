/* Included by art_relay_central.c: shares relay_q and job_conn(), no extra queue or discovery.
 * SPDX-License-Identifier: MIT
 */
static K_SEM_DEFINE(screen_done, 0, 1);
static atomic_t screen_flight;
static atomic_t screen_accept;
static atomic_t screen_pending;
static atomic_t screen_replied;
static uint8_t screen_err;
static uint16_t screen_received;
static int64_t screen_deadline;
static struct bt_gatt_read_params screen_read_params;
static struct bt_gatt_write_params screen_write_params;
static const uint8_t screen_op = ART_RELAY_SNAP;

static void screen_timeout(struct k_work *work) {
    unsigned int key = irq_lock();
    bool expired = atomic_get(&screen_pending) && k_uptime_get() >= screen_deadline;
    bool send = expired && atomic_cas(&screen_replied, 0, 1);
    irq_unlock(key);
    if (send) {
        atomic_clear(&screen_accept);
        screen_snap_complete(1, SCREEN_TIMEOUT);
    }
}
static K_WORK_DELAYABLE_DEFINE(screen_timeout_work, screen_timeout);

static void screen_request_done(uint8_t err) {
    unsigned int key = irq_lock();
    screen_err = err;
    atomic_clear(&screen_flight);
    k_sem_give(&screen_done);
    irq_unlock(key);
    if (atomic_cas(&stuck_screen, 1, 0)) {
        unstick_answered(); // a late answer to a let-go readback
    } else if (!err || err == BT_ATT_ERR_UNLIKELY) {
        atomic_clear(&stuck_in_row);
    }
}

static void screen_write_cb(struct bt_conn *conn, uint8_t err,
                            struct bt_gatt_write_params *params) {
    screen_request_done(err);
}

static uint8_t screen_read_cb(struct bt_conn *conn, uint8_t err,
                             struct bt_gatt_read_params *params, const void *data,
                             uint16_t length) {
    if (!atomic_get(&screen_accept)) {
        screen_request_done(BT_ATT_ERR_UNLIKELY);
        return BT_GATT_ITER_STOP;
    }
    if (err || data == NULL) {
        screen_request_done(err);
        return BT_GATT_ITER_STOP;
    }
    if (length > SCREEN_SNAP_WIRE - screen_received) {
        screen_request_done(BT_ATT_ERR_INVALID_ATTRIBUTE_LEN);
        return BT_GATT_ITER_STOP;
    }
    screen_snap_remote_data(screen_received, data, length);
    screen_received += length;
    if (screen_received == SCREEN_SNAP_WIRE) {
        screen_request_done(0);
        return BT_GATT_ITER_STOP;
    }
    if (!atomic_get(&screen_accept) || k_uptime_get() >= screen_deadline) {
        screen_request_done(BT_ATT_ERR_UNLIKELY);
        return BT_GATT_ITER_STOP;
    }
    /* Zephyr advances single.offset and issues Read Blob requests, even at MTU 23. */
    return BT_GATT_ITER_CONTINUE;
}

static int screen_wait(int ret) {
    if (ret) {
        screen_err = 0;
        atomic_clear(&screen_flight);
        return -EIO;
    }
    int64_t remaining = screen_deadline - k_uptime_get();
    if (remaining <= 0 || k_sem_take(&screen_done, K_MSEC(remaining))) {
        if (atomic_get(&screen_flight) && atomic_cas(&stuck_screen, 0, 1)) {
            unstick_arm(); // let go: the right may yet answer; if not, the unstick acts
        }
        return -ETIMEDOUT;
    }
    return screen_err ? -EIO : 0;
}

static void screen_job(struct k_work *work) {
    uint8_t result = SCREEN_TIMEOUT;
    struct bt_conn *conn = job_conn();
    if (conn == NULL || !screen_handle) {
        result = SCREEN_NO_LINK;
        goto done;
    }
    if (atomic_get(&screen_flight) || k_uptime_get() >= screen_deadline) {
        goto done;
    }
    atomic_set(&screen_accept, 1);
    if (atomic_get(&screen_replied) || k_uptime_get() >= screen_deadline) {
        goto done;
    }
    screen_write_params.func = screen_write_cb;
    screen_write_params.handle = data_handle;
    screen_write_params.offset = 0;
    screen_write_params.data = &screen_op;
    screen_write_params.length = 1;
    k_sem_reset(&screen_done);
    atomic_set(&screen_flight, 1);
    if (screen_wait(bt_gatt_write(conn, &screen_write_params))) {
        goto done;
    }
    int poll_ms = 25;
    while (k_uptime_get() < screen_deadline) {
        screen_received = 0;
        screen_read_params.func = screen_read_cb;
        screen_read_params.handle_count = 1;
        screen_read_params.single.handle = screen_handle;
        screen_read_params.single.offset = 0;
        k_sem_reset(&screen_done);
        atomic_set(&screen_flight, 1);
        int err = screen_wait(bt_gatt_read(conn, &screen_read_params));
        if (!err && screen_received == SCREEN_SNAP_WIRE) {
            result = screen_snap_remote_valid() ? SCREEN_OK : SCREEN_TIMEOUT;
            break;
        }
        if (atomic_get(&screen_flight) || screen_received ||
            screen_err != BT_ATT_ERR_UNLIKELY) {
            break;
        }
        /* Right's display job has not run yet. Only relay_q sleeps, within the same deadline. */
        int64_t remaining = screen_deadline - k_uptime_get();
        if (remaining > 0) {
            k_msleep(MIN(remaining, poll_ms));
            poll_ms = MIN(poll_ms * 2, 500);
        }
    }
done:
    atomic_clear(&screen_accept);
    k_work_cancel_delayable(&screen_timeout_work);
    if (conn != NULL) {
        bt_conn_unref(conn);
    }
    if (atomic_cas(&screen_replied, 0, 1)) {
        screen_snap_complete(1, result);
    }
    atomic_clear(&screen_pending);
}
static K_WORK_DEFINE(screen_work, screen_job);

uint8_t art_relay_screen_snap(void) {
    if (art_relay_link() != 2 || !screen_handle) {
        return SCREEN_NO_LINK;
    }
    /* Timed-out callbacks own their static params until they arrive. */
    if (atomic_get(&screen_flight) || !atomic_cas(&screen_pending, 0, 1)) {
        return SCREEN_BUSY;
    }
    unsigned int key = irq_lock();
    screen_deadline = k_uptime_get() + 3000;
    atomic_clear(&screen_replied);
    irq_unlock(key);
    /* Only replies on the system queue; all radio work stays on relay_q. */
    k_work_reschedule(&screen_timeout_work, K_SECONDS(3));
    k_work_submit_to_queue(&relay_q, &screen_work);
    return SCREEN_OK;
}
