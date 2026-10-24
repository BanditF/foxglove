/*
 * Build identity: which half, firmware version, profile and git SHA (half_marker.c).
 * SPDX-License-Identifier: MIT
 */

#pragma once

#include <stdint.h>

#include <zephyr/toolchain.h>

struct foxglove_build_info {
    uint8_t major, minor, patch;
    uint8_t profile; // 0 dev (test hooks on), 1 GA
    char sha[8];     // short git SHA, ASCII, zero-padded ("unknown" if the build had none)
} __packed;

void foxglove_build_info(struct foxglove_build_info *out);

// Each test hook compiled in leaves "FOXGLOVE_TEST_HOOK=<name>" in the UF2, next to the half
// marker, so CI can prove a GA build has none (scripts/firmware/check_hooks.py, #81).
#define FOXGLOVE_TEST_HOOK_MARK(name)                                                                  const char foxglove_test_hook_##name[] __used                                                          __attribute__((section(".foxglove_half"))) = "FOXGLOVE_TEST_HOOK=" #name
