/* relay-sim: coroutines (sim_coro.c), built without the relay's struct packing. MIT */
#pragma once
#include <stddef.h>

void sim_coro_init(void);
void *sim_coro_create(size_t stack, void (*fn)(void *), void *arg);
// Switches from `from` (NULL: the scheduler) to `to` (NULL: the scheduler).
void sim_coro_switch(void *from, void *to);
