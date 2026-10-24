/* SPDX-License-Identifier: MIT */
#pragma once
#include <stdint.h>

/* Returns 2 if the single request slot is busy, 5 if invalid, otherwise queues it. */
uint8_t test_hooks_press(uint8_t pos, uint8_t state);
void test_hooks_host_seen(void);
/* Relay completion is returned to the system queue before touching injected state. */
void test_hooks_relay_done(uint8_t status);
