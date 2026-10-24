/* SPDX-License-Identifier: MIT */
#include <zephyr/toolchain.h>
#include <zephyr/sys/util.h>

#ifndef FOXGLOVE_BUILD_SHA
#define FOXGLOVE_BUILD_SHA "unknown"
#endif
#if IS_ENABLED(CONFIG_ZMK_SPLIT_ROLE_CENTRAL)
#define FOXGLOVE_HALF "left"
#else
#define FOXGLOVE_HALF "right"
#endif
/* Exact bytes: FOXGLOVE_HALF=left;SHA=<sha> or FOXGLOVE_HALF=right;SHA=<sha>.
 * scripts/flash/foxglove_flash.py searches for FOXGLOVE_HALF=. */
const char foxglove_half_marker[] __used __attribute__((section(".foxglove_half"))) =
    "FOXGLOVE_HALF=" FOXGLOVE_HALF ";SHA=" FOXGLOVE_BUILD_SHA;
