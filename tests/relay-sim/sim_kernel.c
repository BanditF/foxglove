/*
 * relay-sim: a small deterministic kernel. Threads are coroutines (Windows fibers, POSIX
 * ucontext) with Zephyr priorities (lower runs first; negative = cooperative). Code takes no
 * virtual time; time moves only when every thread is blocked, to the next timer. A preemptible
 * thread that readies a higher priority thread (a semaphore give, a work submit) yields to it
 * at once, as Zephyr's scheduler would; a cooperative one runs on until it blocks.
 * SPDX-License-Identifier: MIT
 */
#include <stdarg.h>
#include <stdio.h>
#include <stdlib.h>

#include "sim_kernel.h"

#include "sim_coro.h"

#define STACK_SIZE (256 * 1024)
#define MAX_THREADS 32

enum { T_READY, T_BLOCKED, T_DEAD };

struct sim_thread {
    const char *name;
    int prio;
    int state;
    uint64_t ready_seq; // FIFO among equal priorities
    void (*fn)(void *);
    void *arg;
    int wake_reason;
    struct sim_timer *timeout;
    struct sim_waitq *waiting_on;
    struct sim_thread *wait_next;
    uint64_t wait_seq;
    void *coro;
};

struct sim_timer {
    int64_t at;
    uint64_t seq;
    void (*cb)(void *);
    void *arg;
    struct sim_timer *next;
    bool fired;
};

static struct sim_thread threads[MAX_THREADS];
static int n_threads;
static struct sim_thread *current;
static int64_t now_us;
static uint64_t seq;
static struct sim_timer *timers;

int64_t sim_now_us(void) { return now_us; }
int64_t k_uptime_get(void) { return now_us / 1000; }
k_tid_t k_current_get(void) { return current; }
const char *sim_thread_name(struct sim_thread *t) { return t ? t->name : "isr"; }
int sim_thread_prio(struct sim_thread *t) { return t ? t->prio : -100; }

static void to_scheduler(void) { sim_coro_switch(current->coro, NULL); }

static void thread_entry(void *arg) {
    struct sim_thread *t = arg;
    t->fn(t->arg);
    t->state = T_DEAD;
    to_scheduler();
}

void sim_kernel_init(void) { sim_coro_init(); }

struct sim_thread *sim_thread_create(const char *name, int prio, void (*fn)(void *), void *arg) {
    if (n_threads == MAX_THREADS) {
        fprintf(stderr, "relay-sim: too many threads\n");
        exit(2);
    }
    struct sim_thread *t = &threads[n_threads++];
    memset(t, 0, sizeof(*t));
    t->name = name;
    t->prio = prio;
    t->fn = fn;
    t->arg = arg;
    t->state = T_READY;
    t->ready_seq = ++seq;
    t->coro = sim_coro_create(STACK_SIZE, thread_entry, t);
    return t;
}

/* Timers */

struct sim_timer *sim_timer_start(int64_t at_us, void (*cb)(void *), void *arg) {
    struct sim_timer *t = calloc(1, sizeof(*t));
    t->at = at_us < now_us ? now_us : at_us;
    t->seq = ++seq;
    t->cb = cb;
    t->arg = arg;
    struct sim_timer **p = &timers;
    while (*p && ((*p)->at < t->at || ((*p)->at == t->at && (*p)->seq < t->seq))) {
        p = &(*p)->next;
    }
    t->next = *p;
    *p = t;
    return t;
}

void sim_timer_cancel(struct sim_timer *t) {
    if (t == NULL) {
        return;
    }
    for (struct sim_timer **p = &timers; *p; p = &(*p)->next) {
        if (*p == t) {
            *p = t->next;
            free(t);
            return;
        }
    }
}

/* Scheduling */

static struct sim_thread *pick(void) {
    struct sim_thread *best = NULL;
    for (int i = 0; i < n_threads; i++) {
        struct sim_thread *t = &threads[i];
        if (t->state == T_READY &&
            (!best || t->prio < best->prio ||
             (t->prio == best->prio && t->ready_seq < best->ready_seq))) {
            best = t;
        }
    }
    return best;
}

static void run_thread(struct sim_thread *t) {
    current = t;
    sim_coro_switch(NULL, t->coro);
    current = NULL;
}

bool sim_run(int64_t end_us, bool (*done)(void *), void *arg) {
    for (;;) {
        if (done && done(arg)) {
            return true;
        }
        struct sim_thread *t = pick();
        if (t) {
            run_thread(t);
            continue;
        }
        if (!timers) {
            return false;
        }
        if (timers->at > end_us) {
            now_us = end_us;
            return true;
        }
        now_us = timers->at;
        struct sim_timer *first = timers;
        timers = first->next;
        first->cb(first->arg);
        free(first);
    }
}

void sim_preempt(void) {
    if (current == NULL || current->prio < 0) {
        return; // interrupt context, or a cooperative thread: runs on
    }
    struct sim_thread *t = pick();
    if (t && t != current && t->prio < current->prio) {
        current->ready_seq = 0; // it resumes ahead of its equals
        to_scheduler();
    }
}

static void wait_timeout(void *arg) {
    struct sim_thread *t = arg;
    t->timeout = NULL;
    if (t->state != T_BLOCKED) {
        return;
    }
    if (t->waiting_on) {
        for (struct sim_thread **p = &t->waiting_on->head; *p; p = &(*p)->wait_next) {
            if (*p == t) {
                *p = t->wait_next;
                break;
            }
        }
        t->waiting_on = NULL;
    }
    t->wake_reason = -EAGAIN;
    t->state = T_READY;
    t->ready_seq = ++seq;
}

int sim_block(struct sim_waitq *q, int64_t timeout_us) {
    if (current == NULL) {
        fprintf(stderr, "relay-sim: blocking outside a thread\n");
        exit(2);
    }
    struct sim_thread *t = current;
    t->state = T_BLOCKED;
    t->wait_next = NULL;
    t->wait_seq = ++seq;
    t->waiting_on = q;
    if (q) {
        struct sim_thread **p = &q->head;
        while (*p) {
            p = &(*p)->wait_next;
        }
        *p = t;
    }
    t->timeout = timeout_us >= 0 ? sim_timer_start(now_us + timeout_us, wait_timeout, t) : NULL;
    to_scheduler();
    return t->wake_reason;
}

static void make_ready(struct sim_thread *t, int reason) {
    sim_timer_cancel(t->timeout);
    t->timeout = NULL;
    t->waiting_on = NULL;
    t->wake_reason = reason;
    t->state = T_READY;
    t->ready_seq = ++seq;
}

bool sim_wake_one(struct sim_waitq *q, int reason) {
    struct sim_thread **best = NULL;
    for (struct sim_thread **p = &q->head; *p; p = &(*p)->wait_next) {
        if (!best || (*p)->prio < (*best)->prio) {
            best = p;
        }
    }
    if (!best) {
        return false;
    }
    struct sim_thread *t = *best;
    *best = t->wait_next;
    make_ready(t, reason);
    return true;
}

void sim_wake_all(struct sim_waitq *q, int reason) {
    while (sim_wake_one(q, reason)) {
    }
}

int32_t k_msleep(int32_t ms) {
    sim_block(NULL, (int64_t)ms * 1000);
    return 0;
}

/* Semaphores */

int k_sem_take(struct k_sem *s, k_timeout_t timeout) {
    if (s->count > 0) {
        s->count--;
        return 0;
    }
    if (timeout.us == 0) {
        return -EBUSY;
    }
    return sim_block(&s->wait, timeout.us) == 0 ? 0 : -EAGAIN;
}

void k_sem_give(struct k_sem *s) {
    if (!sim_wake_one(&s->wait, 0)) {
        if (s->count < s->limit) {
            s->count++;
        }
    }
    sim_preempt();
}

void k_sem_reset(struct k_sem *s) {
    s->count = 0;
    sim_wake_all(&s->wait, -EAGAIN);
}

/* Work queues */

struct k_work_q k_sys_work_q = {.name = "sysworkq"};

static void work_q_thread(void *arg) {
    struct k_work_q *q = arg;
    for (;;) {
        while (q->head == NULL) {
            sim_block(&q->wait, -1);
        }
        struct k_work *w = q->head;
        q->head = w->next;
        if (q->head == NULL) {
            q->tail = NULL;
        }
        w->next = NULL;
        w->flags &= ~SIM_WORK_QUEUED;
        w->flags |= SIM_WORK_RUNNING;
        w->handler(w);
        w->flags &= ~SIM_WORK_RUNNING;
    }
}

void k_work_queue_start(struct k_work_q *q, char *stack, size_t size, int prio,
                        const struct k_work_queue_config *cfg) {
    (void)stack;
    (void)size;
    if (cfg && cfg->name) {
        q->name = cfg->name;
    }
    q->thread = sim_thread_create(q->name ? q->name : "workq", prio, work_q_thread, q);
}

static int submit(struct k_work_q *q, struct k_work *w) {
    if (w->flags & SIM_WORK_QUEUED) {
        return 0;
    }
    if (q == NULL || q->thread == NULL) {
        fprintf(stderr, "relay-sim: submit to a queue that isn't started\n");
        exit(2);
    }
    w->flags |= SIM_WORK_QUEUED;
    w->queue = q;
    w->next = NULL;
    if (q->tail) {
        q->tail->next = w;
    } else {
        q->head = w;
    }
    q->tail = w;
    sim_wake_one(&q->wait, 0);
    return 1;
}

int k_work_submit_to_queue(struct k_work_q *q, struct k_work *w) {
    int r = submit(q, w);
    sim_preempt();
    return r;
}

int k_work_submit(struct k_work *w) { return k_work_submit_to_queue(&k_sys_work_q, w); }

void k_work_init(struct k_work *w, k_work_handler_t h) {
    memset(w, 0, sizeof(*w));
    w->handler = h;
}

void k_work_init_delayable(struct k_work_delayable *dw, k_work_handler_t h) {
    memset(dw, 0, sizeof(*dw));
    dw->work.handler = h;
}

static void delayable_fire(void *arg) {
    struct k_work_delayable *dw = arg;
    dw->timer = NULL;
    dw->work.flags &= ~SIM_WORK_DELAYED;
    submit(dw->target, &dw->work);
}

static int schedule_locked(struct k_work_q *q, struct k_work_delayable *dw, k_timeout_t d) {
    dw->target = q;
    if (d.us == 0) {
        return submit(q, &dw->work);
    }
    dw->work.flags |= SIM_WORK_DELAYED;
    dw->timer = sim_timer_start(now_us + (d.us < 0 ? INT64_MAX / 4 : d.us), delayable_fire, dw);
    return 1;
}

int k_work_schedule_for_queue(struct k_work_q *q, struct k_work_delayable *dw, k_timeout_t d) {
    if (dw->work.flags & (SIM_WORK_QUEUED | SIM_WORK_DELAYED)) {
        return 0;
    }
    int r = schedule_locked(q, dw, d);
    sim_preempt();
    return r;
}

int k_work_schedule(struct k_work_delayable *dw, k_timeout_t d) {
    return k_work_schedule_for_queue(&k_sys_work_q, dw, d);
}

static void unschedule(struct k_work_delayable *dw) {
    if (dw->work.flags & SIM_WORK_DELAYED) {
        sim_timer_cancel(dw->timer);
        dw->timer = NULL;
        dw->work.flags &= ~SIM_WORK_DELAYED;
    }
}

int k_work_reschedule_for_queue(struct k_work_q *q, struct k_work_delayable *dw, k_timeout_t d) {
    unschedule(dw);
    int r = schedule_locked(q, dw, d);
    sim_preempt();
    return r;
}

int k_work_reschedule(struct k_work_delayable *dw, k_timeout_t d) {
    return k_work_reschedule_for_queue(&k_sys_work_q, dw, d);
}

static void dequeue(struct k_work *w) {
    if (!(w->flags & SIM_WORK_QUEUED)) {
        return;
    }
    struct k_work_q *q = w->queue;
    struct k_work *prev = NULL;
    for (struct k_work *p = q->head; p; prev = p, p = p->next) {
        if (p == w) {
            if (prev) {
                prev->next = p->next;
            } else {
                q->head = p->next;
            }
            if (q->tail == p) {
                q->tail = prev;
            }
            break;
        }
    }
    w->next = NULL;
    w->flags &= ~SIM_WORK_QUEUED;
}

int k_work_cancel_delayable(struct k_work_delayable *dw) {
    unschedule(dw);
    dequeue(&dw->work);
    return (dw->work.flags & SIM_WORK_RUNNING) ? 1 : 0;
}

int k_work_cancel(struct k_work *w) {
    dequeue(w);
    return (w->flags & SIM_WORK_RUNNING) ? 1 : 0;
}

bool k_work_delayable_is_pending(const struct k_work_delayable *dw) {
    return (dw->work.flags & (SIM_WORK_QUEUED | SIM_WORK_DELAYED | SIM_WORK_RUNNING)) != 0;
}

/* Misc */

uint32_t crc32_ieee(const uint8_t *data, size_t len) {
    uint32_t crc = 0xffffffffu;
    for (size_t i = 0; i < len; i++) {
        crc ^= data[i];
        for (int b = 0; b < 8; b++) {
            crc = (crc >> 1) ^ (0xedb88320u & (0u - (crc & 1u)));
        }
    }
    return ~crc;
}

int sim_log_level = 2;

void sim_log(int level, const char *fmt, ...) {
    if (level > sim_log_level) {
        return;
    }
    static const char *names[] = {"", "ERR", "WRN", "INF", "DBG"};
    printf("  [%9.3f %-8s] %s: ", now_us / 1e6, sim_thread_name(current), names[level]);
    va_list ap;
    va_start(ap, fmt);
    vprintf(fmt, ap);
    va_end(ap);
    printf("\n");
}
