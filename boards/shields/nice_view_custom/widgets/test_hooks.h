/* SPDX-License-Identifier: MIT */
#pragma once
#include <stdbool.h>
#include <stdint.h>

/* Returns 2 if the single request slot is busy, 5 if invalid, otherwise queues it. */
uint8_t test_hooks_press(uint8_t pos, uint8_t state);
void test_hooks_host_seen(void);
/* Left: handles a D7 08 report. Fills reply (7 bytes, D7 88 status ...) and returns false, or
 * returns true when the reply comes later from the relay. */
bool test_hooks_bootloader(const uint8_t *d, int len, uint8_t reply[7]);
/* Right: art relay op BOOT accepted; reboots into the bootloader after the ATT reply. */
void test_hooks_reboot_soon(void);
/* Left: the answer to BOOT for the right; route says how it went (HOST_LINK_BOOT_ROUTE_*). */
void test_hooks_boot_relay_done(uint8_t status, uint8_t route);
/* Relay completion is returned to the system queue before touching injected state. */
void test_hooks_relay_done(uint8_t status);
