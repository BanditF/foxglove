/*
 * relay-sim: the slice of Zephyr 3.5 (v3.5.0+zmk-fixes) and ZMK that art_relay_central.c uses,
 * on a host. The kernel parts run on the simulated kernel in sim_kernel.c (coroutine threads,
 * priorities, virtual time); the Bluetooth parts on the fake host and peer in sim_bt.c, which
 * follow Zephyr 3.5's ATT client rules (one request on the bearer, the rest queued; TX
 * buffers and contexts taken with K_FOREVER off the system workqueue, K_NO_WAIT on it).
 * SPDX-License-Identifier: MIT
 */
#pragma once

#include <errno.h>
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#include <string.h>

/* Toolchain */
#if defined(_MSC_VER)
#define __packed /* the build passes /Zp1: every struct is packed */
#define __attribute__(x)
#else
#define __packed __attribute__((packed))
#endif
#define __used
#define __maybe_unused
#define BUILD_ASSERT(cond, ...) _Static_assert(cond, "BUILD_ASSERT")
#define CONTAINER_OF(ptr, type, field) ((type *)(((char *)(ptr)) - offsetof(type, field)))

/* sys/util.h */
#define BIT(n) (1UL << (n))
#define ARRAY_SIZE(a) (sizeof(a) / sizeof((a)[0]))
#ifndef MIN
#define MIN(a, b) (((a) < (b)) ? (a) : (b))
#endif
#ifndef MAX
#define MAX(a, b) (((a) > (b)) ? (a) : (b))
#endif
#define CLAMP(v, lo, hi) (((v) <= (lo)) ? (lo) : (((v) >= (hi)) ? (hi) : (v)))

/* IS_ENABLED, as Zephyr does it */
#define Z_SIM_XXXX1 Z_SIM_YYYY,
#define IS_ENABLED(config_macro) Z_SIM_IS_ENABLED1(config_macro)
#define Z_SIM_IS_ENABLED1(config_macro) Z_SIM_IS_ENABLED2(Z_SIM_XXXX##config_macro)
#define Z_SIM_IS_ENABLED2(one_or_two_args) Z_SIM_IS_ENABLED3(one_or_two_args 1, 0)
#define Z_SIM_IS_ENABLED3(ignore_this, val, ...) val

/* devicetree: no chosen underglow; the right's LED count comes from the config */
#define DT_HAS_CHOSEN(x) 0

/* sys/byteorder.h */
static inline void sys_put_le16(uint16_t v, uint8_t *p) {
    p[0] = (uint8_t)v;
    p[1] = (uint8_t)(v >> 8);
}
static inline void sys_put_le32(uint32_t v, uint8_t *p) {
    sys_put_le16((uint16_t)v, p);
    sys_put_le16((uint16_t)(v >> 16), p + 2);
}
static inline uint16_t sys_get_le16(const uint8_t *p) { return (uint16_t)(p[0] | (p[1] << 8)); }
static inline uint32_t sys_get_le32(const uint8_t *p) {
    return sys_get_le16(p) | ((uint32_t)sys_get_le16(p + 2) << 16);
}
uint32_t crc32_ieee(const uint8_t *data, size_t len);

/* logging */
void sim_log(int level, const char *fmt, ...);
#define LOG_MODULE_DECLARE(...)
#define LOG_MODULE_REGISTER(...)
#define LOG_ERR(...) sim_log(1, __VA_ARGS__)
#define LOG_WRN(...) sim_log(2, __VA_ARGS__)
#define LOG_INF(...) sim_log(3, __VA_ARGS__)
#define LOG_DBG(...) sim_log(4, __VA_ARGS__)

/* atomics: one host thread runs at a time (coroutines), so plain reads and writes are atomic */
typedef long atomic_t;
typedef long atomic_val_t;
#define ATOMIC_INIT(v) (v)
#define ATOMIC_BITMAP_SIZE(n) (((n) + 31) / 32)
#define ATOMIC_DEFINE(name, n) atomic_t name[ATOMIC_BITMAP_SIZE(n)]
static inline atomic_val_t atomic_get(const atomic_t *a) { return *a; }
static inline atomic_val_t atomic_set(atomic_t *a, atomic_val_t v) {
    atomic_val_t o = *a;
    *a = v;
    return o;
}
static inline atomic_val_t atomic_clear(atomic_t *a) { return atomic_set(a, 0); }
static inline bool atomic_cas(atomic_t *a, atomic_val_t old, atomic_val_t v) {
    if (*a != old) {
        return false;
    }
    *a = v;
    return true;
}
static inline atomic_val_t atomic_inc(atomic_t *a) { return (*a)++; }
static inline atomic_val_t atomic_or(atomic_t *a, atomic_val_t v) {
    atomic_val_t o = *a;
    *a |= v;
    return o;
}
static inline atomic_val_t atomic_and(atomic_t *a, atomic_val_t v) {
    atomic_val_t o = *a;
    *a &= v;
    return o;
}
static inline void atomic_set_bit(atomic_t *a, int bit) { a[bit / 32] |= (1L << (bit % 32)); }
static inline bool atomic_test_bit(const atomic_t *a, int bit) {
    return (a[bit / 32] >> (bit % 32)) & 1;
}
static inline void atomic_clear_bit(atomic_t *a, int bit) { a[bit / 32] &= ~(1L << (bit % 32)); }

/* kernel: time */
typedef struct {
    int64_t us; // -1 forever
} k_timeout_t;
#define K_NO_WAIT ((k_timeout_t){0})
#define K_FOREVER ((k_timeout_t){-1})
#define K_USEC(t) ((k_timeout_t){(int64_t)(t)})
#define K_MSEC(t) ((k_timeout_t){(int64_t)(t) * 1000})
#define K_SECONDS(t) ((k_timeout_t){(int64_t)(t) * 1000000})
#define K_TIMEOUT_EQ(a, b) ((a).us == (b).us)
int64_t k_uptime_get(void);
int32_t k_msleep(int32_t ms);
static inline unsigned int irq_lock(void) { return 0; }
static inline void irq_unlock(unsigned int key) { (void)key; }

struct k_spinlock {
    int unused;
};
typedef int k_spinlock_key_t;
static inline k_spinlock_key_t k_spin_lock(struct k_spinlock *l) {
    (void)l;
    return 0;
}
static inline void k_spin_unlock(struct k_spinlock *l, k_spinlock_key_t k) {
    (void)l;
    (void)k;
}

/* kernel: threads (opaque; sim_kernel.c) */
struct sim_thread;
typedef struct sim_thread *k_tid_t;
k_tid_t k_current_get(void);
#define CONFIG_NUM_COOP_PRIORITIES 16
#define CONFIG_NUM_PREEMPT_PRIORITIES 15
#define K_PRIO_COOP(x) (-(CONFIG_NUM_COOP_PRIORITIES - (x)))
#define K_LOWEST_APPLICATION_THREAD_PRIO (CONFIG_NUM_PREEMPT_PRIORITIES - 1)
#define K_THREAD_STACK_DEFINE(sym, size) char sym[size]
#define K_THREAD_STACK_SIZEOF(sym) sizeof(sym)

/* kernel: semaphores */
struct sim_waitq {
    struct sim_thread *head;
};
struct k_sem {
    unsigned int count, limit;
    struct sim_waitq wait;
};
#define K_SEM_DEFINE(name, init, lim) struct k_sem name = {(init), (lim), {0}}
int k_sem_take(struct k_sem *s, k_timeout_t timeout);
void k_sem_give(struct k_sem *s);
void k_sem_reset(struct k_sem *s);
static inline unsigned int k_sem_count_get(struct k_sem *s) { return s->count; }

/* kernel: work queues */
struct k_work;
typedef void (*k_work_handler_t)(struct k_work *work);
struct k_work_q;
struct sim_timer;
struct k_work {
    k_work_handler_t handler;
    struct k_work *next;
    struct k_work_q *queue; // where it is queued or last ran
    unsigned int flags;     // SIM_WORK_*
};
struct k_work_delayable {
    struct k_work work;
    struct sim_timer *timer;
    struct k_work_q *target;
};
struct k_work_q {
    struct k_work *head, *tail;
    struct sim_thread *thread;
    struct sim_waitq wait;
    const char *name;
};
#define SIM_WORK_QUEUED 1u
#define SIM_WORK_RUNNING 2u
#define SIM_WORK_DELAYED 4u
#define K_WORK_INITIALIZER(h) {(h), NULL, NULL, 0}
#define K_WORK_DEFINE(name, h) struct k_work name = K_WORK_INITIALIZER(h)
#define K_WORK_DELAYABLE_DEFINE(name, h) \
    struct k_work_delayable name = {K_WORK_INITIALIZER(h), NULL, NULL}
extern struct k_work_q k_sys_work_q;
struct k_work_queue_config {
    const char *name;
};
void k_work_queue_start(struct k_work_q *q, char *stack, size_t size, int prio,
                        const struct k_work_queue_config *cfg);
int k_work_submit_to_queue(struct k_work_q *q, struct k_work *w);
int k_work_submit(struct k_work *w);
void k_work_init(struct k_work *w, k_work_handler_t h);
void k_work_init_delayable(struct k_work_delayable *dw, k_work_handler_t h);
int k_work_schedule_for_queue(struct k_work_q *q, struct k_work_delayable *dw, k_timeout_t d);
int k_work_schedule(struct k_work_delayable *dw, k_timeout_t d);
int k_work_reschedule_for_queue(struct k_work_q *q, struct k_work_delayable *dw, k_timeout_t d);
int k_work_reschedule(struct k_work_delayable *dw, k_timeout_t d);
int k_work_cancel_delayable(struct k_work_delayable *dw);
int k_work_cancel(struct k_work *w);
static inline struct k_work_delayable *k_work_delayable_from_work(struct k_work *w) {
    return CONTAINER_OF(w, struct k_work_delayable, work);
}
bool k_work_delayable_is_pending(const struct k_work_delayable *dw);

/* init.h: SYS_INIT entries become globals the harness calls in order */
#define SYS_INIT(fn, level, prio) int (*const sim_sys_init_##fn)(void) = fn

/* Bluetooth: uuid.h */
#define BT_UUID_TYPE_16 0
#define BT_UUID_TYPE_128 2
struct bt_uuid {
    uint8_t type;
};
struct bt_uuid_128 {
    struct bt_uuid uuid;
    uint8_t val[16];
};
#define BT_UUID_128_ENCODE(w32, w1, w2, w3, w48)                                                  \
    (uint8_t)(((w48) >> 0) & 0xFF), (uint8_t)(((w48) >> 8) & 0xFF),                               \
        (uint8_t)(((w48) >> 16) & 0xFF), (uint8_t)(((w48) >> 24) & 0xFF),                         \
        (uint8_t)(((w48) >> 32) & 0xFF), (uint8_t)(((w48) >> 40) & 0xFF),                         \
        (uint8_t)(((w3) >> 0) & 0xFF), (uint8_t)(((w3) >> 8) & 0xFF),                             \
        (uint8_t)(((w2) >> 0) & 0xFF), (uint8_t)(((w2) >> 8) & 0xFF),                             \
        (uint8_t)(((w1) >> 0) & 0xFF), (uint8_t)(((w1) >> 8) & 0xFF),                             \
        (uint8_t)(((w32) >> 0) & 0xFF), (uint8_t)(((w32) >> 8) & 0xFF),                           \
        (uint8_t)(((w32) >> 16) & 0xFF), (uint8_t)(((w32) >> 24) & 0xFF)
#define BT_UUID_INIT_128(...) {{BT_UUID_TYPE_128}, {__VA_ARGS__}}
int bt_uuid_cmp(const struct bt_uuid *a, const struct bt_uuid *b);

/* conn.h */
struct bt_conn; // sim_bt.c
typedef enum {
    BT_SECURITY_L0,
    BT_SECURITY_L1,
    BT_SECURITY_L2,
    BT_SECURITY_L3,
    BT_SECURITY_L4,
} bt_security_t;
enum bt_security_err { BT_SECURITY_ERR_SUCCESS };
enum { BT_CONN_ROLE_CENTRAL = 0, BT_CONN_ROLE_PERIPHERAL = 1 };
enum { BT_CONN_STATE_DISCONNECTED, BT_CONN_STATE_CONNECTED = 4 };
struct bt_conn_info {
    uint8_t type, role, id;
    int state;
};
struct bt_conn_cb {
    void (*connected)(struct bt_conn *conn, uint8_t err);
    void (*disconnected)(struct bt_conn *conn, uint8_t reason);
    void (*security_changed)(struct bt_conn *conn, bt_security_t level, enum bt_security_err err);
};
#define BT_CONN_CB_DEFINE(name) const struct bt_conn_cb name
struct bt_conn *bt_conn_ref(struct bt_conn *conn);
void bt_conn_unref(struct bt_conn *conn);
int bt_conn_get_info(const struct bt_conn *conn, struct bt_conn_info *info);
bt_security_t bt_conn_get_security(const struct bt_conn *conn);
int bt_conn_disconnect(struct bt_conn *conn, uint8_t reason);
#define BT_HCI_ERR_REMOTE_USER_TERM_CONN 0x13

/* gatt.h */
#define BT_ATT_ERR_INVALID_OFFSET 0x07
#define BT_ATT_ERR_INVALID_ATTRIBUTE_LEN 0x0d
#define BT_ATT_ERR_UNLIKELY 0x0e
#define BT_GATT_ERR(e) (-(e))
enum { BT_GATT_ITER_STOP = 0, BT_GATT_ITER_CONTINUE };
#define BT_GATT_CCC_NOTIFY 0x0001
enum {
    BT_GATT_DISCOVER_PRIMARY,
    BT_GATT_DISCOVER_SECONDARY,
    BT_GATT_DISCOVER_INCLUDE,
    BT_GATT_DISCOVER_CHARACTERISTIC,
};
enum {
    BT_GATT_SUBSCRIBE_FLAG_VOLATILE,
    BT_GATT_SUBSCRIBE_FLAG_NO_RESUB,
    BT_GATT_SUBSCRIBE_FLAG_WRITE_PENDING,
    BT_GATT_SUBSCRIBE_FLAG_SENT,
    BT_GATT_SUBSCRIBE_NUM_FLAGS
};
struct bt_gatt_attr {
    const struct bt_uuid *uuid;
    void *user_data;
    uint16_t handle;
};
struct bt_gatt_chrc {
    const struct bt_uuid *uuid;
    uint16_t value_handle;
    uint8_t properties;
};
struct bt_gatt_service_val {
    const struct bt_uuid *uuid;
    uint16_t end_handle;
};
struct bt_gatt_discover_params;
typedef uint8_t (*bt_gatt_discover_func_t)(struct bt_conn *conn, const struct bt_gatt_attr *attr,
                                           struct bt_gatt_discover_params *params);
struct bt_gatt_discover_params {
    const struct bt_uuid *uuid;
    bt_gatt_discover_func_t func;
    uint16_t start_handle, end_handle;
    uint8_t type;
};
struct bt_gatt_read_params;
typedef uint8_t (*bt_gatt_read_func_t)(struct bt_conn *conn, uint8_t err,
                                       struct bt_gatt_read_params *params, const void *data,
                                       uint16_t length);
struct bt_gatt_read_params {
    bt_gatt_read_func_t func;
    size_t handle_count;
    struct {
        uint16_t handle;
        uint16_t offset;
    } single;
};
struct bt_gatt_write_params;
typedef void (*bt_gatt_write_func_t)(struct bt_conn *conn, uint8_t err,
                                     struct bt_gatt_write_params *params);
struct bt_gatt_write_params {
    bt_gatt_write_func_t func;
    uint16_t handle, offset;
    const void *data;
    uint16_t length;
};
struct bt_gatt_subscribe_params;
typedef uint8_t (*bt_gatt_notify_func_t)(struct bt_conn *conn,
                                         struct bt_gatt_subscribe_params *params,
                                         const void *data, uint16_t length);
typedef void (*bt_gatt_subscribe_func_t)(struct bt_conn *conn, uint8_t err,
                                         struct bt_gatt_subscribe_params *params);
struct bt_gatt_subscribe_params {
    bt_gatt_notify_func_t notify;
    bt_gatt_subscribe_func_t subscribe;
    uint16_t value_handle, ccc_handle, value;
    ATOMIC_DEFINE(flags, BT_GATT_SUBSCRIBE_NUM_FLAGS);
    struct bt_gatt_subscribe_params *sim_next;
};
int bt_gatt_discover(struct bt_conn *conn, struct bt_gatt_discover_params *params);
int bt_gatt_read(struct bt_conn *conn, struct bt_gatt_read_params *params);
int bt_gatt_write(struct bt_conn *conn, struct bt_gatt_write_params *params);
int bt_gatt_write_without_response(struct bt_conn *conn, uint16_t handle, const void *data,
                                   uint16_t length, bool sign);
int bt_gatt_subscribe(struct bt_conn *conn, struct bt_gatt_subscribe_params *params);
uint16_t bt_gatt_get_mtu(struct bt_conn *conn);

/* zmk */
enum zmk_activity_state { ZMK_ACTIVITY_ACTIVE, ZMK_ACTIVITY_IDLE, ZMK_ACTIVITY_SLEEP };
enum zmk_activity_state zmk_activity_get_state(void);
struct zmk_behavior_binding {
    const char *behavior_dev;
    uint32_t param1, param2;
};
struct zmk_behavior_binding_event {
    int layer;
    uint32_t position;
    int64_t timestamp;
    uint8_t source;
};
int zmk_split_central_invoke_behavior(uint8_t source, struct zmk_behavior_binding *binding,
                                      struct zmk_behavior_binding_event event, bool state);
