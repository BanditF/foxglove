/* relay-sim: the simulated kernel (sim_kernel.c). SPDX-License-Identifier: MIT */
#pragma once

#include "zephyr_sim.h"

struct sim_thread *sim_thread_create(const char *name, int prio, void (*fn)(void *), void *arg);
const char *sim_thread_name(struct sim_thread *t);
int sim_thread_prio(struct sim_thread *t);

// Virtual time in microseconds.
int64_t sim_now_us(void);

// A timer callback runs in "interrupt" context: it must not block.
struct sim_timer *sim_timer_start(int64_t at_us, void (*cb)(void *), void *arg);
void sim_timer_cancel(struct sim_timer *t);

// Runs every thread and timer until virtual time reaches end_us, or until done() says so.
// Returns false if nothing was left to run (every thread blocked forever, no timers).
bool sim_run(int64_t end_us, bool (*done)(void *), void *arg);

// Blocks the calling thread on q (NULL: just sleeps) for timeout_us (-1 forever).
// Returns 0 when woken by sim_wake_one, -EAGAIN on timeout.
int sim_block(struct sim_waitq *q, int64_t timeout_us);
// Wakes the waiter that blocked first among those of highest priority; false if none.
bool sim_wake_one(struct sim_waitq *q, int reason);
void sim_wake_all(struct sim_waitq *q, int reason);
// A preemption point: lets a higher priority ready thread run first.
void sim_preempt(void);

void sim_kernel_init(void);
void sim_kernel_reset(void);
