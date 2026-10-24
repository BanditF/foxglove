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
