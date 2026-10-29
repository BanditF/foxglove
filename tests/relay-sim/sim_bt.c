/*
 * relay-sim: the split link under art_relay_central.c. The left's side follows Zephyr 3.5's
 * Bluetooth host (v3.5.0+zmk-fixes, subsys/bluetooth/host att.c, gatt.c, conn.c):
 *  - ATT has one request on the bearer (chan->req); later ones queue (att->reqs) and go when its
 *    response comes, from the RX thread, in att_handle_rsp before the callback runs.
 *  - Every PDU takes ATT TX meta data when it is made and a connection TX context when it is
 *    sent (CONFIG_BT_CONN_TX_MAX, 5 with ZMK's central). Both wait forever (K_FOREVER) on any
 *    thread but the system work queue, which gets K_NO_WAIT (tx_meta_data_alloc, conn_tx_alloc);
 *    both come back on the system work queue when the controller reports the packet sent
 *    (tx_notify), which then runs bt_att_sent: a queued request goes then if the bearer is free.
 *  - A request sent from the system work queue without a TX context goes back on att->reqs
 *    (chan_req_send fails with -ENOBUFS) and only the next sent PDU tries it again.
 *  - A request on the air that gets no response for 30 s kills the bearer (att_timeout): every
 *    request's callback gets BT_ATT_ERR_UNLIKELY, subscriptions go, nothing more is sent.
 *  - gatt_read_rsp: a chunk of MTU - 1 bytes whose callback says CONTINUE is read on with a Read
 *    Blob from the RX thread; a shorter one ends the read with a NULL callback.
 *  - The link has ZMK's slave latency (ZMK_SPLIT_BLE_PREF_LATENCY 30, #662): the right listens
 *    at an event right after one that carried data either way, when it has a PDU of its own due,
 *    and at least every 31st event; at the others nothing moves. A request sent while the right
 *    sleeps waits for its next listen (up to ~230 ms). sim_bt_faults.no_latency turns it off.
 * The right's side is a model of art_relay_peripheral.c's service (status, data, screen, CCC),
 * with Zephyr's CCC rules for a bonded peer (gatt.c bt_gatt_attr_write_ccc, update_ccc): the
 * CCC's cfg_changed runs only when the aggregate value changes, and its screen push (#662).
 * SPDX-License-Identifier: MIT
 */
#include <stdio.h>
#include <stdlib.h>

#include "sim_bt.h"

#include "art_relay.h"
#include "half_marker.h"
#include "screen_snap.h"
#include "sim_kernel.h"

struct sim_bt_stats sim_bt_stats;
struct sim_bt_faults sim_bt_faults = {.late_op = 0, .late_us = 0, .late_count = 0};
struct sim_right sim_right;

extern const struct bt_conn_cb art_relay_conn_cb;

/* Handles of the right's relay service */
#define H_SVC 0x0020
#define H_DATA_DECL 0x0021
#define H_DATA 0x0022
#define H_STATUS_DECL 0x0023
#define H_STATUS 0x0024
#define H_CCC 0x0025
#define H_SCREEN_DECL 0x0026
#define H_SCREEN 0x0027
#define H_END 0x0027

static const struct bt_uuid_128 svc_uuid = BT_UUID_INIT_128(ART_RELAY_SERVICE_UUID);
static const struct bt_uuid_128 data_uuid = BT_UUID_INIT_128(ART_RELAY_DATA_UUID);
static const struct bt_uuid_128 status_uuid = BT_UUID_INIT_128(ART_RELAY_STATUS_UUID);
static const struct bt_uuid_128 screen_uuid = BT_UUID_INIT_128(ART_RELAY_SCREEN_UUID);

int bt_uuid_cmp(const struct bt_uuid *a, const struct bt_uuid *b) {
    if (a->type != b->type) {
        return 1;
    }
    return memcmp(((const struct bt_uuid_128 *)a)->val, ((const struct bt_uuid_128 *)b)->val, 16);
}

/* Connections */

struct bt_conn {
    int refs;
    int state;
    bt_security_t sec;
    int id;
};

static struct bt_conn *link; // the current split link, or the last one
static bool link_up;
static int link_ids;

struct bt_conn *bt_conn_ref(struct bt_conn *conn) {
    conn->refs++;
    return conn;
}

void bt_conn_unref(struct bt_conn *conn) {
    if (--conn->refs < 0) {
        fprintf(stderr, "relay-sim: bt_conn_unref below zero\n");
        exit(2);
    }
}

int bt_conn_get_info(const struct bt_conn *conn, struct bt_conn_info *info) {
    memset(info, 0, sizeof(*info));
    info->role = BT_CONN_ROLE_CENTRAL;
    info->state = conn->state;
    return 0;
}

bt_security_t bt_conn_get_security(const struct bt_conn *conn) { return conn->sec; }
uint16_t bt_gatt_get_mtu(struct bt_conn *conn) { return conn ? SIM_MTU : 0; }

static bool conn_ok(struct bt_conn *conn) {
    return conn == link && link_up && conn->state == BT_CONN_STATE_CONNECTED;
}

/* PDUs and the left's TX pools */

enum {
    P_REQ_WRITE,
    P_REQ_READ,
    P_REQ_SUB,
    P_REQ_DISC_PRIM,
    P_REQ_DISC_CHRC,
    P_CMD,
    P_RSP,
    P_NOTIFY,
};

struct req;
struct pdu {
    int kind;
    uint16_t handle, offset;
    uint8_t data[32];
    uint16_t len;
    uint8_t err;
    struct req *req;
    int64_t due; // a late response goes on the air from then
    bool on_air; // a request: handed to the controller (its TX context taken)
    bool push; // a screen chunk the right pushed (its sent callback paces the push)
    bt_gatt_complete_func_t sent; // a command's complete callback (att_on_sent_cb)
    void *sent_data;
    struct pdu *next;
};

struct req {
    int kind;
    void *params;
    struct pdu *pdu;
    struct req *next;
};

static int meta_free = SIM_TX_BUFS, ctx_free = SIM_TX_CTX;
static struct sim_waitq meta_wait, ctx_wait;

static bool on_syswq(void) { return k_current_get() == k_sys_work_q.thread; }

// 1: taken; 0: none and this thread may not wait (the system work queue: K_NO_WAIT);
// -1: the link went while it waited (no slot).
static int take(int *free, struct sim_waitq *wq) {
    if (*free > 0) {
        (*free)--;
        return 1;
    }
    if (k_current_get() == NULL || on_syswq()) {
        return 0;
    }
    return sim_block(wq, -1) == 0 ? 1 : -1; // K_FOREVER; the giver hands its slot over
}

static void give(int *free, struct sim_waitq *wq) {
    if (!sim_wake_one(wq, 0)) {
        (*free)++;
    }
}

/* The air: one queue per direction, a connection event every 7.5 ms */

struct pdu_list {
    struct pdu *head, *tail;
};

static void push(struct pdu_list *l, struct pdu *p) {
    p->next = NULL;
    if (l->tail) {
        l->tail->next = p;
    } else {
        l->head = p;
    }
    l->tail = p;
}

static struct pdu *pop(struct pdu_list *l) {
    struct pdu *p = l->head;
    if (p) {
        l->head = p->next;
        if (!l->head) {
            l->tail = NULL;
        }
        p->next = NULL;
    }
    return p;
}

static struct pdu_list left_air, right_air;   // waiting for a connection event
static struct pdu_list left_done;             // sent, tx_notify pending (left syswq)
static struct pdu_list right_in, left_in;     // received, RX pending
static struct pdu_list right_done;            // sent by the right, its tx_notify pending
static struct pdu_list left_cmd_wait;         // commands made without a TX context (att->tx_queue)

static struct k_work_q bt_rx_q = {.name = "bt_rx"};
static struct k_work_q right_rx_q = {.name = "right_rx"};
static struct k_work_q right_sys_q = {.name = "right_sys"};
static void left_rx_cb(struct k_work *w);
static void left_tx_notify_cb(struct k_work *w);
static void right_rx_cb(struct k_work *w);
static void right_tx_notify_cb(struct k_work *w);
static K_WORK_DEFINE(left_rx_work, left_rx_cb);
static K_WORK_DEFINE(left_tx_notify_work, left_tx_notify_cb);
static K_WORK_DEFINE(right_rx_work, right_rx_cb);
static K_WORK_DEFINE(right_tx_notify_work, right_tx_notify_cb);

static struct sim_timer *event_timer;
static int right_push_sent; // push chunks on the air whose sent callback hasn't run on the right
static int right_slept;        // connection events the right has slept through in a row
static bool right_awake_next; // the last event the right listened at carried data

// Whether the right has a PDU of its own due now (it then skips its latency).
static bool right_has_due(void) {
    for (struct pdu *p = right_air.head; p; p = p->next) {
        if (p->due <= sim_now_us()) {
            return true;
        }
    }
    return false;
}

static void conn_event(void *arg) {
    (void)arg;
    event_timer = NULL;
    if (!link_up) {
        return;
    }
    // Peripheral latency (ZMK_SPLIT_BLE_PREF_LATENCY 30): the right listens right after an event
    // that carried data, when it has data of its own, and at least every SIM_PERIPH_LATENCY + 1st
    // event. At an event it sleeps through nothing moves either way.
    if (!sim_bt_faults.no_latency && !right_awake_next && !right_has_due() &&
        right_slept < SIM_PERIPH_LATENCY) {
        right_slept++;
        event_timer = sim_timer_start(sim_now_us() + SIM_CONN_INTERVAL_US, conn_event, NULL);
        return;
    }
    right_slept = 0;
    int moved = 0;
    bool stalled = sim_bt_faults.left_tx_stall_at_us &&
                   sim_now_us() >= sim_bt_faults.left_tx_stall_at_us;
    for (int i = 0; i < SIM_PDUS_PER_EVENT && left_air.head && !stalled; i++) {
        struct pdu *p = pop(&left_air);
        struct pdu *copy = malloc(sizeof(*copy));
        *copy = *p;
        copy->req = NULL;
        push(&right_in, copy);
        push(&left_done, p);
        moved++;
    }
    if (left_done.head) {
        k_work_submit(&left_tx_notify_work);
    }
    if (right_in.head) {
        k_work_submit_to_queue(&right_rx_q, &right_rx_work);
    }
    // The right's PDUs: a late one holds back only itself.
    int sent = 0;
    struct pdu_list keep = {0};
    while (right_air.head) {
        struct pdu *p = pop(&right_air);
        if (sent < SIM_PDUS_PER_EVENT && p->due <= sim_now_us()) {
            sent++;
            right_push_sent += p->push;
            push(&left_in, p);
        } else {
            push(&keep, p);
        }
    }
    right_air = keep;
    right_awake_next = moved || sent || (left_air.head && !stalled);
    if (sent) {
        k_work_submit_to_queue(&bt_rx_q, &left_rx_work);
        k_work_submit_to_queue(&right_sys_q, &right_tx_notify_work);
    }
    event_timer = sim_timer_start(sim_now_us() + SIM_CONN_INTERVAL_US, conn_event, NULL);
}

/* The left's ATT client */

static struct req *reqs_head; // att->reqs
static struct req *chan_req;  // chan->req
static bool att_alive;
static struct bt_gatt_subscribe_params *subs;
static struct k_work_delayable att_timeout_work;
static struct pdu *chan_req_pdu_on_air; // the request whose sent starts the ATT timeout

static void reqs_append(struct req *r) {
    r->next = NULL;
    struct req **p = &reqs_head;
    while (*p) {
        p = &(*p)->next;
    }
    *p = r;
}

int sim_bt_queued_requests(void) {
    int n = 0;
    for (struct req *r = reqs_head; r; r = r->next) {
        n++;
    }
    return n;
}

bool sim_bt_request_on_air(void) { return chan_req != NULL; }
bool sim_bt_att_alive(void) { return att_alive && link_up; }

// att_req_send_process: one request onto the bearer if it is free.
static void send_req_process(void) {
    if (!att_alive || chan_req || !reqs_head) {
        return;
    }
    struct req *r = reqs_head;
    reqs_head = r->next;
    r->next = NULL;
    chan_req = r;
    int got = take(&ctx_free, &ctx_wait);
    if (got == 0) {
        // chan_req_send -> bt_conn_send_cb -> -ENOBUFS: back on the list for the next sent.
        chan_req = NULL;
        r->next = reqs_head;
        reqs_head = r;
        sim_bt_stats.stuck_requests++;
        return;
    }
    if (got < 0 || !link_up || !att_alive || chan_req != r) {
        if (got > 0) {
            give(&ctx_free, &ctx_wait);
        }
        return; // the link or the bearer went while it waited; att_kill had the request
    }
    sim_bt_stats.requests_sent++;
    r->pdu->on_air = true;
    push(&left_air, r->pdu);
}

static struct pdu *new_pdu(int kind, uint16_t handle) {
    struct pdu *p = calloc(1, sizeof(*p));
    p->kind = kind;
    p->handle = handle;
    return p;
}

// gatt_req_send: the PDU is made (and its data copied) now; it is queued for the bearer.
static int req_send(struct bt_conn *conn, int kind, void *params, uint16_t handle, uint16_t offset,
                    const void *data, uint16_t len) {
    if (!conn_ok(conn) || !att_alive) {
        return -ENOTCONN;
    }
    int got = take(&meta_free, &meta_wait);
    if (got <= 0) {
        return got == 0 ? -ENOMEM : -ENOTCONN;
    }
    if (!conn_ok(conn) || !att_alive) {
        give(&meta_free, &meta_wait);
        return -ENOTCONN;
    }
    struct req *r = calloc(1, sizeof(*r));
    r->kind = kind;
    r->params = params;
    r->pdu = new_pdu(kind, handle);
    r->pdu->offset = offset;
    r->pdu->req = r;
    if (len > sizeof(r->pdu->data)) {
        len = sizeof(r->pdu->data);
    }
    if (data) {
        memcpy(r->pdu->data, data, len);
    }
    r->pdu->len = len;
    reqs_append(r);
    send_req_process();
    return 0;
}

int bt_gatt_write(struct bt_conn *conn, struct bt_gatt_write_params *params) {
    if (params->length > SIM_MTU - 3) {
        return -EINVAL;
    }
    return req_send(conn, P_REQ_WRITE, params, params->handle, 0, params->data, params->length);
}

int bt_gatt_read(struct bt_conn *conn, struct bt_gatt_read_params *params) {
    return req_send(conn, P_REQ_READ, params, params->single.handle, params->single.offset, NULL,
                    0);
}

int bt_gatt_subscribe(struct bt_conn *conn, struct bt_gatt_subscribe_params *params) {
    uint8_t v[2];
    sys_put_le16(params->value, v);
    int err = req_send(conn, P_REQ_SUB, params, params->ccc_handle, 0, v, 2);
    if (!err) {
        params->sim_next = subs;
        subs = params;
    }
    return err;
}

int bt_gatt_discover(struct bt_conn *conn, struct bt_gatt_discover_params *params) {
    int kind = params->type == BT_GATT_DISCOVER_PRIMARY ? P_REQ_DISC_PRIM : P_REQ_DISC_CHRC;
    return req_send(conn, kind, params, params->start_handle, 0, NULL, 0);
}

int bt_gatt_write_without_response(struct bt_conn *conn, uint16_t handle, const void *data,
                                   uint16_t length, bool sign) {
    return bt_gatt_write_without_response_cb(conn, handle, data, length, sign, NULL, NULL);
}

// The callback runs on the system work queue once the command is sent (tx_notify, then
// att_on_sent_cb). Zephyr 3.5 drops it for a command still queued when the link goes ("Bearer
// not connected, dropping ATT cb"): free_list and sim_bt_drop don't call it.
int bt_gatt_write_without_response_cb(struct bt_conn *conn, uint16_t handle, const void *data,
                                      uint16_t length, bool sign, bt_gatt_complete_func_t func,
                                      void *user_data) {
    (void)sign;
    if (!conn_ok(conn) || !att_alive) {
        return -ENOTCONN;
    }
    if (length > SIM_MTU - 3) {
        return -EINVAL;
    }
    int got = take(&meta_free, &meta_wait);
    if (got <= 0) {
        return got == 0 ? -ENOMEM : -ENOTCONN;
    }
    if (!conn_ok(conn) || !att_alive) {
        give(&meta_free, &meta_wait);
        return -ENOTCONN;
    }
    struct pdu *p = new_pdu(P_CMD, handle);
    memcpy(p->data, data, length);
    p->len = length;
    p->sent = func;
    p->sent_data = user_data;
    got = take(&ctx_free, &ctx_wait);
    if (got == 0) {
        push(&left_cmd_wait, p); // att->tx_queue: bt_att_sent sends it later
        return 0;
    }
    if (got < 0 || !link_up) {
        if (got > 0) {
            give(&ctx_free, &ctx_wait);
        }
        give(&meta_free, &meta_wait);
        free(p);
        return -ENOTCONN;
    }
    sim_bt_stats.cmds_sent++;
    push(&left_air, p);
    return 0;
}

static void remove_sub(struct bt_gatt_subscribe_params *params) {
    for (struct bt_gatt_subscribe_params **p = &subs; *p; p = &(*p)->sim_next) {
        if (*p == params) {
            *p = params->sim_next;
            return;
        }
    }
}

static void req_failed(struct req *r, uint8_t err);

// Zephyr's discovery of the relay service, one attribute per response (MTU 23, 128-bit UUIDs).
static void disc_rsp(struct req *r, struct pdu *p) {
    struct bt_gatt_discover_params *dp = r->params;
    static struct bt_gatt_service_val svc;
    static struct bt_gatt_chrc chrc;
    static struct bt_gatt_attr attr;
    if (p->err) {
        dp->func(link, NULL, dp);
        return;
    }
    if (r->kind == P_REQ_DISC_PRIM) {
        svc.uuid = &svc_uuid.uuid;
        svc.end_handle = H_END;
        attr.uuid = &svc_uuid.uuid;
        attr.handle = H_SVC;
        attr.user_data = &svc;
        if (dp->func(link, &attr, dp) == BT_GATT_ITER_CONTINUE) {
            dp->func(link, NULL, dp);
        }
        return;
    }
    uint16_t decl = sys_get_le16(p->data);
    chrc.value_handle = decl + 1;
    chrc.uuid = decl == H_DATA_DECL     ? &data_uuid.uuid
                : decl == H_STATUS_DECL ? &status_uuid.uuid
                                        : &screen_uuid.uuid;
    attr.handle = decl;
    attr.user_data = &chrc;
    if (dp->func(link, &attr, dp) == BT_GATT_ITER_STOP) {
        return;
    }
    dp->start_handle = decl + 1;
    if (bt_gatt_discover(link, dp) != 0) {
        dp->func(link, NULL, dp);
    }
}

static void read_rsp(struct req *r, struct pdu *p) {
    struct bt_gatt_read_params *rp = r->params;
    if (p->err || p->len == 0) {
        rp->func(link, p->err, rp, NULL, 0);
        return;
    }
    if (rp->func(link, 0, rp, p->data, p->len) == BT_GATT_ITER_STOP) {
        return;
    }
    if (p->len < SIM_MTU - 1) {
        rp->func(link, 0, rp, NULL, 0);
        return;
    }
    rp->single.offset += p->len;
    if (bt_gatt_read(link, rp) < 0) {
        rp->func(link, BT_ATT_ERR_UNLIKELY, rp, NULL, 0);
    }
}

static void dispatch(struct req *r, struct pdu *p) {
    switch (r->kind) {
    case P_REQ_WRITE: {
        struct bt_gatt_write_params *wp = r->params;
        wp->func(link, p->err, wp);
        break;
    }
    case P_REQ_READ:
        read_rsp(r, p);
        break;
    case P_REQ_SUB: {
        struct bt_gatt_subscribe_params *sp = r->params;
        if (p->err) {
            remove_sub(sp);
        }
        if (sp->subscribe) {
            sp->subscribe(link, p->err, sp);
        }
        break;
    }
    default:
        disc_rsp(r, p);
        break;
    }
}

static void req_failed(struct req *r, uint8_t err) {
    struct pdu p = {.err = err};
    if (r->kind == P_REQ_DISC_PRIM || r->kind == P_REQ_DISC_CHRC) {
        struct bt_gatt_discover_params *dp = r->params;
        dp->func(link, NULL, dp);
    } else {
        dispatch(r, &p);
    }
    free(r);
}

// att_reset + bt_gatt_disconnected: every request fails, subscriptions go.
static void att_kill(void) {
    if (!att_alive) {
        return;
    }
    att_alive = false;
    k_work_cancel_delayable(&att_timeout_work);
    if (chan_req) {
        struct req *r = chan_req;
        chan_req = NULL;
        if (!r->pdu->on_air) {
            give(&meta_free, &meta_wait); // it was still waiting for a TX context
        }
        req_failed(r, BT_ATT_ERR_UNLIKELY);
    }
    while (reqs_head) {
        struct req *r = reqs_head;
        reqs_head = r->next;
        give(&meta_free, &meta_wait);
        req_failed(r, BT_ATT_ERR_UNLIKELY);
    }
    while (subs) {
        struct bt_gatt_subscribe_params *sp = subs;
        subs = sp->sim_next;
        sp->notify(link, sp, NULL, 0);
    }
}

static void att_timeout_cb(struct k_work *w) {
    (void)w;
    sim_bt_stats.att_timeouts++;
    LOG_ERR("sim: ATT Timeout: the bearer is dead until the link drops");
    att_kill();
}

static void left_rx_cb(struct k_work *w) {
    (void)w;
    struct pdu *p;
    while ((p = pop(&left_in)) != NULL) {
        if (!link_up) {
            free(p);
            continue;
        }
        if (p->kind == P_RSP) {
            sim_bt_stats.responses++;
            if (!att_alive || !chan_req) {
                free(p);
                continue;
            }
            k_work_cancel_delayable(&att_timeout_work);
            struct req *r = chan_req;
            chan_req = NULL;
            send_req_process();
            dispatch(r, p);
            free(r);
        } else if (p->kind == P_NOTIFY) {
            struct bt_gatt_subscribe_params *sp = subs;
            while (sp && sp->value_handle != p->handle) {
                sp = sp->sim_next;
            }
            if (!att_alive || sp == NULL) {
                sim_bt_stats.notifies_dropped++;
            } else {
                sp->notify(link, sp, p->data, p->len);
            }
        }
        free(p);
    }
}

// tx_notify on the system work queue, then bt_att_sent.
static void left_tx_notify_cb(struct k_work *w) {
    (void)w;
    struct pdu *p;
    while ((p = pop(&left_done)) != NULL) {
        give(&ctx_free, &ctx_wait);
        if (p->kind != P_CMD && chan_req && chan_req->pdu == p && att_alive) {
            k_work_reschedule(&att_timeout_work, K_USEC(SIM_ATT_TIMEOUT_US));
        }
        give(&meta_free, &meta_wait);
        if (p->kind == P_CMD && p->sent && link_up) {
            p->sent(link, p->sent_data);
        }
        if (p->req == NULL) {
            free(p); // request PDUs stay with their request (a few bytes each)
        }
        // bt_att_sent: queued requests first, then the commands left waiting.
        if (att_alive && link_up) {
            send_req_process();
            while (left_cmd_wait.head && take(&ctx_free, &ctx_wait) > 0) {
                sim_bt_stats.cmds_sent++;
                push(&left_air, pop(&left_cmd_wait));
            }
        }
    }
}

/* The right half */

static bool rsp_meta_busy; // the right's response buffer, until its tx_notify
static void right_redraw(void);
static void ring(uint8_t what);

static void right_send(struct pdu *p) {
    if (!link_up) {
        free(p);
        return;
    }
    push(&right_air, p);
}

static void respond(uint8_t err, const void *data, uint16_t len) {
    if (sim_bt_faults.rsp_meta_rule && rsp_meta_busy) {
        sim_bt_stats.rsp_dropped++; // "already processing a REQ/RSP": no response at all
        return;
    }
    rsp_meta_busy = true;
    struct pdu *p = new_pdu(P_RSP, 0);
    p->err = err;
    if (data) {
        memcpy(p->data, data, len);
    }
    p->len = len;
    right_send(p);
}

static uint16_t status_value(uint8_t *out) {
    struct art_relay_status s = {0};
    s.version = 9;
    s.state = sim_right.state;
    s.received = sim_right.received;
    s.capacity = 16384;
    s.set = sim_right.set;
    // One built-in since #671 (Bandit_Emblem); a SELECT above it is a kept slot (set_count + n),
    // which this right plays like any other set.
    s.set_count = 1;
    s.rgb.on = sim_right.rgb_on;
    s.rgb.hue = sys_get_le16(sim_right.look);
    s.rgb.saturation = sim_right.look[2];
    s.rgb.brightness = sim_right.look[3];
    s.rgb.effect = sim_right.look[4];
    s.rgb.speed = 3;
    s.flash_slots = ART_RELAY_FLASH_SLOTS;
    uint16_t n = 0;
    memcpy(out, &s, sizeof(s));
    n += sizeof(s);
    memset(out + n, 0, ART_RELAY_FLASH_SLOTS * sizeof(struct art_relay_slot_info));
    n += ART_RELAY_FLASH_SLOTS * sizeof(struct art_relay_slot_info);
    struct foxglove_build_info b = {0, 1, 0, 0, "sim"};
    memcpy(out + n, &b, sizeof(b));
    n += sizeof(b);
    struct battery_dim_wire d = {0};
    d.level = 80;
    memcpy(out + n, &d, sizeof(d));
    n += sizeof(d);
    sys_put_le32(sim_right.redraws, out + n);
    n += 4;
    return n;
}

static uint8_t blob[16384];

static void right_update_lit(void) {
    bool lit = sim_right.follow ? (sim_right.rgb_on && sim_right.left_activity == 0)
                                : sim_right.rgb_on;
    if (lit != sim_right.lit) {
        sim_right.lit = lit;
        sim_right.redraws++;
        right_redraw();
        ring(ART_RELAY_RANG_RGB);
    }
}

static bool snap_push; // the capture under way is to be pushed (SNAP with ART_RELAY_SNAP_PUSH)

// The relay op's ATT error (0: accepted).
static uint8_t right_op(const uint8_t *d, uint16_t len) {
    switch (d[0]) {
    case ART_RELAY_BEGIN:
        sim_right.blob_len = sys_get_le16(&d[1]);
        sim_right.blob_crc = sys_get_le32(&d[3]);
        sim_right.received = 0;
        sim_right.state = ART_RIGHT_RECEIVING;
        return 0;
    case ART_RELAY_DATA: {
        uint16_t at = sys_get_le16(&d[1]);
        uint16_t n = len - 3;
        if (sim_right.state == ART_RIGHT_RECEIVING && at + n <= sim_right.blob_len) {
            memcpy(&blob[at], &d[3], n);
            sim_right.received = MAX(sim_right.received, at + n);
        }
        return 0;
    }
    case ART_RELAY_COMMIT:
        if (sim_right.state == ART_RIGHT_RECEIVING && sim_right.received == sim_right.blob_len &&
            crc32_ieee(blob, sim_right.blob_len) == sim_right.blob_crc) {
            sim_right.state = ART_RIGHT_PLAYING;
            sim_right.set = -1;
            sim_right.pushes_ok++;
            sim_right.redraws++;
            right_redraw();
        } else {
            sim_right.state = ART_RIGHT_BAD_CRC;
        }
        return 0;
    case ART_RELAY_SELECT:
        sim_right.set = (int8_t)d[1];
        sim_right.state = ART_RIGHT_IDLE;
        sim_right.redraws++;
        right_redraw();
        return 0;
    case ART_RELAY_RGB:
        sim_right.rgb_on = d[1];
        sim_right.follow = d[2];
        if (len >= 13) {
            memcpy(sim_right.look, &d[8], 5);
        }
        right_update_lit();
        return 0;
    case ART_RELAY_ACTIVITY:
        sim_right.left_activity = d[1];
        right_update_lit();
        return 0;
    case ART_RELAY_PANEL_POLARITY: // saved and drawn (panel_polarity.c): the screen redraws
        if (len == 2) {
            sim_right.polarity = d[1];
            sim_right.polarity_writes++;
            sim_right.redraws++;
            right_redraw();
        }
        return 0;
    case ART_RELAY_SNAP:
        // art_relay_peripheral.c: one byte reads as before; a second, ART_RELAY_SNAP_PUSH (#662),
        // pushes the image once captured. A right before the push refuses two bytes.
        if (len != 1 && (sim_bt_faults.right_no_push || len != 2 || d[1] != 1)) {
            return BT_ATT_ERR_INVALID_ATTRIBUTE_LEN;
        }
        snap_push = len == 2;
        sim_right.snap_ready = 1;
        right_redraw(); // the capture runs on the right's display (system) queue
        return 0;
    default:
        return 0;
    }
}

static uint8_t frozen[SCREEN_SNAP_WIRE]; // the capture: reads come from it (screen_snap.c)
static uint16_t push_at; // the next byte of the push
static int push_out;     // push chunks not yet sent
static bool push_live;

static void screen_wire(uint8_t out[SCREEN_SNAP_WIRE]) {
    uint8_t *px = out + SCREEN_SNAP_HEADER;
    for (int i = 0; i < SCREEN_SNAP_BYTES; i++) {
        px[i] = (uint8_t)(sim_right.set * 31 + sim_right.frame * 7 + i);
    }
    sys_put_le16((uint16_t)sim_right.frame, out);
    sys_put_le32(crc32_ieee(px, SCREEN_SNAP_BYTES), out + 2);
}

static bool late_hit(uint8_t op) {
    if (sim_bt_faults.late_count <= 0) {
        return false;
    }
    if (sim_bt_faults.late_op && sim_bt_faults.late_op != op) {
        return false;
    }
    sim_bt_faults.late_count--;
    return true;
}

static void right_rx_cb(struct k_work *w) {
    (void)w;
    struct pdu *p;
    while ((p = pop(&right_in)) != NULL) {
        if (!link_up) {
            free(p);
            continue;
        }
        uint8_t op = p->len ? p->data[0] : 0;
        bool late = p->kind != P_CMD && late_hit(p->kind == P_REQ_WRITE ? op : 0xff);
        struct pdu_list before = right_air;
        switch (p->kind) {
        case P_CMD:
            if (p->handle == H_DATA && p->len) {
                right_op(p->data, p->len);
            }
            break;
        case P_REQ_WRITE:
            respond(p->handle == H_DATA && p->len ? right_op(p->data, p->len) : 0, NULL, 0);
            break;
        case P_REQ_SUB: {
            uint16_t v = sys_get_le16(p->data);
#ifdef ART_RELAY_RINGS_ON_SUBSCRIBE
            if (v & BT_GATT_CCC_NOTIFY) {
                ring(ART_RELAY_RANG_RGB); // doorbell_written (cfg_write): every subscribe
            }
#endif
            sim_right.ccc_cfg = v;
            if (sim_right.ccc_cfg != sim_right.ccc_value) { // bt_gatt_attr_write_ccc
                sim_right.ccc_value = v;
#ifndef ART_RELAY_RINGS_ON_SUBSCRIBE
                if (v & BT_GATT_CCC_NOTIFY) {
                    ring(ART_RELAY_RANG_RGB); // doorbell_subscribed (cfg_changed)
                }
#endif
            }
            respond(0, NULL, 0);
            break;
        }
        case P_REQ_READ: {
            static uint8_t value[2048];
            uint16_t n;
            if (p->handle == H_SCREEN) {
                if (sim_right.snap_ready != 2) {
                    respond(BT_ATT_ERR_UNLIKELY, NULL, 0);
                    break;
                }
                memcpy(value, frozen, SCREEN_SNAP_WIRE);
                n = SCREEN_SNAP_WIRE;
            } else {
                n = status_value(value);
            }
            if (p->offset > n) {
                respond(BT_ATT_ERR_INVALID_OFFSET, NULL, 0);
            } else {
                respond(0, value + p->offset, MIN(SIM_MTU - 1, n - p->offset));
            }
            break;
        }
        case P_REQ_DISC_PRIM:
            respond(0, NULL, 0);
            break;
        case P_REQ_DISC_CHRC: {
            uint16_t decl = p->handle <= H_DATA_DECL     ? H_DATA_DECL
                            : p->handle <= H_STATUS_DECL ? H_STATUS_DECL
                            : p->handle <= H_SCREEN_DECL ? H_SCREEN_DECL
                                                         : 0;
            uint8_t v[2];
            sys_put_le16(decl, v);
            if (decl) {
                respond(0, v, 2);
            } else {
                respond(0x0a, NULL, 0); // attribute not found
            }
            break;
        }
        }
        if (late) {
            // The response this request made goes on the air late, or never.
            struct pdu *r = before.tail ? before.tail->next : right_air.head;
            if (r && r->kind == P_RSP) {
                if (sim_bt_faults.late_err) {
                    r->err = sim_bt_faults.late_err;
                }
                if (sim_bt_faults.late_us < 0) {
                    r->due = INT64_MAX / 2;
                } else {
                    r->due = sim_now_us() + sim_bt_faults.late_us;
                }
            }
        }
        free(p);
    }
}

static void push_more(void);

static void right_tx_notify_cb(struct k_work *w) {
    (void)w;
    rsp_meta_busy = false; // chan_rsp_sent -> tx_meta_data_free (rsp_meta)
    if (right_push_sent) {
        push_out -= right_push_sent; // bt_gatt_notify_cb's func: the next chunks go
        right_push_sent = 0;
        push_more();
    }
}

static void right_busy_cb(struct k_work *w) {
    (void)w;
    if (sim_bt_faults.right_redraw_busy_us > 0) {
        sim_block(NULL, sim_bt_faults.right_redraw_busy_us); // the display redraw holds the queue
    }
    if (sim_right.snap_ready == 1) {
        screen_wire(frozen);
        sim_right.snap_ready = 2;
        if (snap_push) { // screen_snap_captured: the push starts over from offset 0
            snap_push = false;
            push_at = 0;
            push_live = true;
            sim_bt_stats.screen_pushes++;
            push_more();
        }
    }
}
static K_WORK_DEFINE(right_busy_work, right_busy_cb);

static void right_redraw(void) { k_work_submit_to_queue(&right_sys_q, &right_busy_work); }

static void right_frame_cb(struct k_work *w) {
    (void)w;
    sim_block(NULL, sim_bt_faults.right_frame_busy_us); // an art frame holds the queue
}
static K_WORK_DEFINE(right_frame_work, right_frame_cb);

/* The right's screen push (art_relay_peripheral.c, #662): the frozen image as notifications of the
 * status value, ART_RELAY_RANG_SCREEN, offset u16 LE, then up to MTU - 6 bytes; at most PUSH_OUT
 * unsent at a time, each sent callback (on the right's system queue) sends the next. */
#define PUSH_OUT 2

static void push_more(void) {
    while (push_live && push_out < PUSH_OUT && push_at < SCREEN_SNAP_WIRE) {
        if (!(sim_right.ccc_value & BT_GATT_CCC_NOTIFY) || !link_up) {
            push_live = false; // nobody subscribed: bt_gatt_notify_cb refuses, the push ends
            return;
        }
        uint16_t n = MIN(SIM_MTU - 3 - 3, SCREEN_SNAP_WIRE - push_at);
        struct pdu *p = new_pdu(P_NOTIFY, H_STATUS);
        p->data[0] = 0x80; // ART_RELAY_RANG_SCREEN
        sys_put_le16(push_at, &p->data[1]);
        memcpy(&p->data[3], &frozen[push_at], n);
        p->len = 3 + n;
        p->push = true;
        push_at += n;
        push_out++;
        sim_bt_stats.screen_chunks++;
        right_send(p);
    }
    if (push_at >= SCREEN_SNAP_WIRE) {
        push_live = false;
    }
}

static uint8_t ring_what;
static void ring_cb(struct k_work *w) {
    (void)w;
    uint8_t what = ring_what;
    ring_what = 0;
    if (!what || !(sim_right.ccc_value & BT_GATT_CCC_NOTIFY) || !link_up) {
        return;
    }
#ifdef RGB_SHOWN_LIT
    uint8_t d[1 + sizeof(struct rgb_idle_wire) + sizeof(struct rgb_shown_wire)] = {what};
    struct rgb_idle_wire rgb = {sim_right.rgb_on, sys_get_le16(sim_right.look), sim_right.look[2],
                                sim_right.look[3], sim_right.look[4], 3};
    struct rgb_shown_wire shown = {0};
    shown.flags = RGB_SHOWN_VALID | (sim_right.lit ? RGB_SHOWN_LIT : 0) | RGB_SHOWN_WANTED |
                  (sim_right.follow ? RGB_SHOWN_FOLLOWING : 0) | RGB_SHOWN_IDLE;
    shown.value = sim_right.lit ? sim_right.look[3] : 0;
    shown.dim = 100;
    memcpy(&d[1], &rgb, sizeof(rgb));
    memcpy(&d[1 + sizeof(rgb)], &shown, sizeof(shown));
#else
    uint8_t d[1] = {what}; // a right before #605 rings with the bits only
#endif
    struct pdu *p = new_pdu(P_NOTIFY, H_STATUS);
    memcpy(p->data, d, sizeof(d));
    p->len = sizeof(d);
    sim_bt_stats.rings++;
    right_send(p);
}
static K_WORK_DEFINE(ring_work, ring_cb);

static void ring(uint8_t what) {
    ring_what |= what;
    k_work_submit_to_queue(&right_sys_q, &ring_work);
}

static void right_animate(void *arg) {
    (void)arg;
    if (sim_right.lit) {
        sim_right.frame++;
        if (sim_bt_faults.right_frame_busy_us > 0) {
            k_work_submit_to_queue(&right_sys_q, &right_frame_work);
        }
    }
    int64_t every = sim_bt_faults.right_frame_us > 0 ? sim_bt_faults.right_frame_us : 250000;
    sim_timer_start(sim_now_us() + every, right_animate, NULL);
}

/* Link up and down */

static void connected_cb(struct k_work *w);
static void secured_cb(struct k_work *w);
static void disconnected_cb(struct k_work *w);
static K_WORK_DEFINE(connected_work, connected_cb);
static K_WORK_DELAYABLE_DEFINE(secured_work, secured_cb);
static K_WORK_DEFINE(disconnected_work, disconnected_cb);
static struct bt_conn *gone;
static int64_t back_after;
static struct sim_timer *back_timer;

static void connected_cb(struct k_work *w) {
    (void)w;
    art_relay_conn_cb.connected(link, 0);
}

static void secured_cb(struct k_work *w) {
    (void)w;
    if (!link_up) {
        return;
    }
    link->sec = BT_SECURITY_L2;
    // update_ccc on encryption: the bonded left's stored CCC comes back, cfg_changed rings.
    if (sim_right.ccc_cfg != sim_right.ccc_value) {
        sim_right.ccc_value = sim_right.ccc_cfg;
#ifndef ART_RELAY_RINGS_ON_SUBSCRIBE
        if (sim_right.ccc_value & BT_GATT_CCC_NOTIFY) {
            ring(ART_RELAY_RANG_RGB); // cfg_changed; the left hasn't subscribed yet
        }
#endif
    }
    art_relay_conn_cb.security_changed(link, BT_SECURITY_L2, BT_SECURITY_ERR_SUCCESS);
}

static int64_t secure_after;

void sim_bt_connect(int64_t secure_after_us) {
    link = calloc(1, sizeof(*link));
    link->refs = 1; // the stack's own
    link->state = BT_CONN_STATE_CONNECTED;
    link->sec = BT_SECURITY_L1;
    link->id = ++link_ids;
    link_up = true;
    att_alive = true;
    chan_req = NULL;
    reqs_head = NULL;
    rsp_meta_busy = false;
    secure_after = secure_after_us;
    k_work_submit_to_queue(&bt_rx_q, &connected_work);
    k_work_reschedule_for_queue(&bt_rx_q, &secured_work, K_USEC(secure_after_us));
    if (!event_timer) {
        event_timer = sim_timer_start(sim_now_us() + SIM_CONN_INTERVAL_US, conn_event, NULL);
    }
}

static void reconnect(void *arg) {
    (void)arg;
    back_timer = NULL;
    sim_bt_connect(secure_after ? secure_after : 500000);
}

static void disconnected_cb(struct k_work *w) {
    (void)w;
    att_kill();
    art_relay_conn_cb.disconnected(gone, BT_HCI_ERR_REMOTE_USER_TERM_CONN);
    back_timer = sim_timer_start(sim_now_us() + back_after, reconnect, NULL);
}

static void free_list(struct pdu_list *l, bool left) {
    struct pdu *p;
    while ((p = pop(l)) != NULL) {
        if (left) {
            give(&ctx_free, &ctx_wait);
            give(&meta_free, &meta_wait);
        }
        if (p->req == NULL) {
            free(p);
        }
    }
}

void sim_bt_stall_left_tx(void) { sim_bt_faults.left_tx_stall_at_us = sim_now_us(); }

void sim_bt_drop(int64_t back_after_us) {
    if (!link_up) {
        return;
    }
    sim_bt_stats.link_drops++;
    sim_bt_faults.left_tx_stall_at_us = 0; // a new link starts clean
    link_up = false;
    link->state = BT_CONN_STATE_DISCONNECTED;
    gone = link;
    back_after = back_after_us;
    free_list(&left_air, true);
    free_list(&left_done, true);
    struct pdu *p;
    while ((p = pop(&left_cmd_wait)) != NULL) {
        give(&meta_free, &meta_wait);
        free(p);
    }
    free_list(&right_air, false);
    free_list(&right_in, false);
    free_list(&left_in, false);
    sim_right.ccc_value = 0; // disconnected_cb in gatt.c: the value goes, the bond's cfg stays
    sim_right.snap_ready = 0;
    push_live = false;
    push_out = 0;
    right_push_sent = 0;
    snap_push = false;
    right_awake_next = false;
    right_slept = 0;
    sim_wake_all(&ctx_wait, 1); // "Disconnected while allocating context"
    sim_wake_all(&meta_wait, 1);
    k_work_submit_to_queue(&bt_rx_q, &disconnected_work);
}

int bt_conn_disconnect(struct bt_conn *conn, uint8_t reason) {
    (void)reason;
    if (conn != link || !link_up) {
        return -ENOTCONN;
    }
    sim_bt_drop(3000000); // ZMK reconnects; the halves re-encrypt
    return 0;
}

void sim_bt_init(void) {
    k_work_queue_start(&bt_rx_q, NULL, 0, K_PRIO_COOP(8), NULL);
    k_work_queue_start(&right_rx_q, NULL, 0, K_PRIO_COOP(8), NULL);
    k_work_queue_start(&right_sys_q, NULL, 0, -1, NULL);
    k_work_init_delayable(&att_timeout_work, att_timeout_cb);
    att_timeout_work.target = &k_sys_work_q;
    sim_right.set = 0;
    sim_right.polarity = -1;
    sim_right.follow = true;
    sim_right.rgb_on = true;
    sim_right.ccc_cfg = BT_GATT_CCC_NOTIFY; // bonded and subscribed before (the left's CCC)
    uint8_t look[5] = {23, 0, 100, 42, 3};
    memcpy(sim_right.look, look, 5);
    sim_right.lit = true;
    sim_timer_start(250000, right_animate, NULL);
}

/* ZMK */
int zmk_split_central_invoke_behavior(uint8_t source, struct zmk_behavior_binding *binding,
                                      struct zmk_behavior_binding_event event, bool state) {
    (void)source;
    (void)binding;
    (void)event;
    (void)state;
    return 0;
}
