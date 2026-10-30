/* Included by art_relay_central.c: shares relay_q and job_conn(), no extra queue or discovery.
 * SPDX-License-Identifier: MIT
 */
/* Budget (#662): SCREEN_BUDGET_MS from the D7 06 to the reply, within foxglove-lab's and the
 * app's 6 s.
 *
 * Push (the d265853 hardware failure): the split link has ZMK's slave latency (30 x 7.5 ms), so
 * the right listens only after an event that carried data, when it has data of its own, or every
 * 31st event. Paced requests (REQ_GAP_MS after each answer, which the response-buffer rule needs,
 * art_relay_central.c) each found it asleep and waited for its next listen: ~63 Read Blob
 * requests ran past 5 s and every right snapshot timed out, with no relay timeout. So the SNAP
 * write now asks the right to push the image (ART_RELAY_SNAP_PUSH, art_relay.h): it sends it as
 * notifications of the status value, which the doorbell's subscription brings here
 * (screen_push_chunk), in (offset, bytes) chunks. Notifications take no response buffer, and a
 * right with data to send skips its latency. The left keeps a map of the bytes it has; once the
 * chunks stop (SCREEN_PUSH_QUIET_MS, or SCREEN_PUSH_FIRST_MS for the first one while the right's
 * display queue captures), it reads each gap with a paced Read Blob at its offset, until every
 * byte is here or the deadline. relay-sim: snap-latency.
 *
 * A right before the push refuses the two-byte SNAP with ATT 0x0d (invalid attribute length); the
 * left remembers that for the link, sends the one-byte SNAP and reads the image one chunk per
 * request, spaced REQ_GAP_MS, as before. Under the link's latency that takes far longer than the
 * budget, so against an older right the reply is a timeout at the 5 s deadline, inside the lab's
 * and the app's 6 s (relay-sim snap-latency-old). Every read is answered within a connection
 * event or two of the right listening, so a slow snapshot holds no request for long, and one let
 * go at the deadline arms the unstick from its own send time (unstick_arm_after), never sooner
 * than STUCK_AFTER_MS after it went out. */
#define SCREEN_BUDGET_MS 5000
#define SCREEN_PUSH_FIRST_MS 1500 // the SNAP's answer to the first chunk: the capture waits its turn
#define SCREEN_PUSH_QUIET_MS 300  // no chunk for this long: read the gaps

static K_SEM_DEFINE(screen_done, 0, 1);
static K_SEM_DEFINE(screen_chunk, 0, 1); // a pushed chunk came
static atomic_t screen_flight;
static atomic_t screen_accept;
static atomic_t screen_pending;
static atomic_t screen_replied;
static atomic_t screen_push_live;  // pushed chunks are taken (from the SNAP's answer to the end)
static atomic_t screen_no_push_on; // cmd_epoch + 1 of a link whose right refused the push
static uint8_t screen_err;
static uint16_t screen_read_at; // the offset of the read in flight
static uint8_t screen_have[(SCREEN_SNAP_WIRE + 7) / 8]; // the image's bytes here (irq_lock)
static uint16_t screen_have_n;
static int64_t screen_deadline;
static int64_t screen_sent_at; // when the request in flight went out
static struct bt_gatt_read_params screen_read_params;
static struct bt_gatt_write_params screen_write_params;
static const uint8_t screen_op[] = {ART_RELAY_SNAP, ART_RELAY_SNAP_PUSH};

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

// Bytes [at, at + n) of the image came: a pushed chunk or a read (Bluetooth RX thread).
static void screen_take(uint16_t at, const uint8_t *data, uint16_t n) {
    screen_snap_remote_data(at, data, n);
    unsigned int key = irq_lock();
    for (uint16_t i = at; i < at + n; i++) {
        if (!(screen_have[i / 8] & BIT(i % 8))) {
            screen_have[i / 8] |= BIT(i % 8);
            screen_have_n++;
        }
    }
    irq_unlock(key);
}

static uint16_t screen_have_count(void) {
    unsigned int key = irq_lock();
    uint16_t n = screen_have_n;
    irq_unlock(key);
    return n;
}

// The first byte not here yet, or SCREEN_SNAP_WIRE.
static uint16_t screen_gap(void) {
    unsigned int key = irq_lock();
    uint16_t at = 0;
    while (at < SCREEN_SNAP_WIRE && (screen_have[at / 8] & BIT(at % 8))) {
        at++;
    }
    irq_unlock(key);
    return at;
}

// A pushed chunk: the doorbell's notify with byte 0 ART_RELAY_RANG_SCREEN (Bluetooth RX thread).
static void screen_push_chunk(const uint8_t *d, uint16_t len) {
    if (len <= ART_RELAY_SNAP_PUSH_HEADER || !atomic_get(&screen_push_live)) {
        return; // empty, or not asked for (an earlier snapshot's)
    }
    uint16_t at = sys_get_le16(&d[1]);
    uint16_t n = len - ART_RELAY_SNAP_PUSH_HEADER;
    if (at >= SCREEN_SNAP_WIRE || n > SCREEN_SNAP_WIRE - at) {
        return;
    }
    screen_take(at, &d[ART_RELAY_SNAP_PUSH_HEADER], n);
    k_sem_give(&screen_chunk);
}

static void screen_request_done(uint8_t err) {
    answered_now();
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
    if (!err && params->length == sizeof(screen_op) && atomic_get(&screen_accept)) {
        // The right's chunks come after this answer on the link: take them from here on.
        atomic_set(&screen_push_live, 1);
    }
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
        screen_request_done(err ? err : BT_ATT_ERR_UNLIKELY); // no data: the value ended early
        return BT_GATT_ITER_STOP;
    }
    if (length > SCREEN_SNAP_WIRE - screen_read_at) {
        screen_request_done(BT_ATT_ERR_INVALID_ATTRIBUTE_LEN);
        return BT_GATT_ITER_STOP;
    }
    screen_take(screen_read_at, data, length);
    /* One chunk per request: screen_job sends the next (a Read Blob at the next gap) after
     * REQ_GAP_MS. Going on from here (BT_GATT_ITER_CONTINUE) sent it the moment this answer came,
     * before the right could free its response buffer (#662). */
    screen_request_done(0);
    return BT_GATT_ITER_STOP;
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
            // Let go: the right may yet answer; if not, the unstick acts STUCK_AFTER_MS after
            // this request went out.
            unstick_arm_after(k_uptime_get() - screen_sent_at);
        }
        return -ETIMEDOUT;
    }
    return screen_err ? -EIO : 0;
}

// The SNAP write: with the push byte, or one byte (a right that refused the push).
static int screen_snap_write(struct bt_conn *conn, bool push) {
    screen_write_params.func = screen_write_cb;
    screen_write_params.handle = data_handle;
    screen_write_params.offset = 0;
    screen_write_params.data = screen_op;
    screen_write_params.length = push ? sizeof(screen_op) : 1;
    req_space();
    k_sem_reset(&screen_done);
    atomic_set(&screen_flight, 1);
    screen_sent_at = k_uptime_get();
    return screen_wait(bt_gatt_write(conn, &screen_write_params));
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
    unsigned int key = irq_lock();
    memset(screen_have, 0, sizeof(screen_have));
    screen_have_n = 0;
    irq_unlock(key);
    k_sem_reset(&screen_chunk);
    atomic_set(&screen_accept, 1);
    if (atomic_get(&screen_replied) || k_uptime_get() >= screen_deadline) {
        goto done;
    }
    // The push comes through the doorbell's subscription: without one, read as before.
    bool push = atomic_get(&doorbell_live) &&
                atomic_get(&screen_no_push_on) != atomic_get(&cmd_epoch) + 1;
    int err = screen_snap_write(conn, push);
    if (err && push && !atomic_get(&screen_flight) &&
        screen_err == BT_ATT_ERR_INVALID_ATTRIBUTE_LEN) {
        // A right before the push: remember it for this link, and snap the old way.
        atomic_set(&screen_no_push_on, atomic_get(&cmd_epoch) + 1);
        push = false;
        err = screen_snap_write(conn, false);
    }
    if (err) {
        goto done;
    }
    int64_t quiet_until = push ? k_uptime_get() + SCREEN_PUSH_FIRST_MS : 0;
    int poll_ms = 25;
    while (k_uptime_get() < screen_deadline) {
        uint16_t have = screen_have_count();
        if (have == SCREEN_SNAP_WIRE) {
            result = screen_snap_remote_valid() ? SCREEN_OK : SCREEN_TIMEOUT;
            break;
        }
        int64_t now = k_uptime_get();
        if (now < quiet_until) {
            // The push runs: wait for its chunks; the gaps are read once they stop.
            k_sem_take(&screen_chunk, K_MSEC(MIN(quiet_until, screen_deadline) - now));
            if (screen_have_count() != have) {
                quiet_until = k_uptime_get() + SCREEN_PUSH_QUIET_MS;
            }
            continue;
        }
        req_space();
        screen_read_at = screen_gap();
        screen_read_params.func = screen_read_cb;
        screen_read_params.handle_count = 1;
        screen_read_params.single.handle = screen_handle;
        screen_read_params.single.offset = screen_read_at;
        k_sem_reset(&screen_done);
        atomic_set(&screen_flight, 1);
        screen_sent_at = k_uptime_get();
        err = screen_wait(bt_gatt_read(conn, &screen_read_params));
        if (!err) {
            continue; // the next gap
        }
        if (atomic_get(&screen_flight) || screen_err != BT_ATT_ERR_UNLIKELY ||
            screen_have_count() != 0) {
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
    atomic_clear(&screen_push_live);
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
    screen_deadline = k_uptime_get() + SCREEN_BUDGET_MS;
    atomic_clear(&screen_replied);
    irq_unlock(key);
    /* Only replies on the system queue; all radio work stays on relay_q. */
    k_work_reschedule(&screen_timeout_work, K_MSEC(SCREEN_BUDGET_MS));
    k_work_submit_to_queue(&relay_q, &screen_work);
    return SCREEN_OK;
}
