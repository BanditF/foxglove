/*
 * relay-sim: coroutines for the simulated threads: Windows fibers, or POSIX ucontext.
 * Built on its own (no /Zp1, no relay headers): it only passes pointers.
 * SPDX-License-Identifier: MIT
 */
#if !defined(_WIN32)
#define _XOPEN_SOURCE 700 // ucontext
#endif
#include "sim_coro.h"

#include <stdint.h>
#include <stdlib.h>

#if defined(_WIN32)
#include <windows.h>

static LPVOID main_fiber;
struct coro {
    LPVOID fiber;
    void (*fn)(void *);
    void *arg;
};

static void WINAPI entry(LPVOID p) {
    struct coro *c = p;
    c->fn(c->arg);
}

void sim_coro_init(void) { main_fiber = ConvertThreadToFiber(NULL); }

void *sim_coro_create(size_t stack, void (*fn)(void *), void *arg) {
    struct coro *c = calloc(1, sizeof(*c));
    c->fn = fn;
    c->arg = arg;
    c->fiber = CreateFiber(stack, entry, c);
    return c;
}

void sim_coro_switch(void *from, void *to) {
    (void)from;
    SwitchToFiber(to ? ((struct coro *)to)->fiber : main_fiber);
}
#else
#include <ucontext.h>

static ucontext_t main_ctx;
struct coro {
    ucontext_t ctx;
    void (*fn)(void *);
    void *arg;
};

static void entry(unsigned int hi, unsigned int lo) {
    struct coro *c = (struct coro *)(((uintptr_t)hi << 32) | (uintptr_t)lo);
    c->fn(c->arg);
}

void sim_coro_init(void) {}

void *sim_coro_create(size_t stack, void (*fn)(void *), void *arg) {
    struct coro *c = calloc(1, sizeof(*c));
    c->fn = fn;
    c->arg = arg;
    getcontext(&c->ctx);
    c->ctx.uc_stack.ss_sp = malloc(stack);
    c->ctx.uc_stack.ss_size = stack;
    c->ctx.uc_link = NULL;
    uintptr_t p = (uintptr_t)c;
    makecontext(&c->ctx, (void (*)(void))entry, 2, (unsigned int)(p >> 32),
                (unsigned int)(p & 0xffffffffu));
    return c;
}

void sim_coro_switch(void *from, void *to) {
    ucontext_t *f = from ? &((struct coro *)from)->ctx : &main_ctx;
    ucontext_t *t = to ? &((struct coro *)to)->ctx : &main_ctx;
    swapcontext(f, t);
}
#endif
