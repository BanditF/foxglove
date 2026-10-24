/* SPDX-License-Identifier: MIT */
#include <string.h>

#include <zephyr/toolchain.h>
#include <zephyr/sys/util.h>

#include "half_marker.h"

#ifndef FOXGLOVE_BUILD_SHA
#define FOXGLOVE_BUILD_SHA "unknown"
#endif
#ifndef FOXGLOVE_FW_VERSION
#define FOXGLOVE_FW_VERSION "0.0.0"
#define FOXGLOVE_FW_MAJOR 0
#define FOXGLOVE_FW_MINOR 0
#define FOXGLOVE_FW_PATCH 0
#endif
#if IS_ENABLED(CONFIG_ZMK_SPLIT_ROLE_CENTRAL)
#define FOXGLOVE_HALF "left"
#else
#define FOXGLOVE_HALF "right"
#endif
#if IS_ENABLED(CONFIG_FOXGLOVE_TEST_HOOKS)
#define FOXGLOVE_PROFILE "dev"
#else
#define FOXGLOVE_PROFILE "ga"
#endif
/* Exact bytes, e.g. FOXGLOVE_HALF=left;VER=0.1.0;PROFILE=dev;SHA=1a2b3c4.
 * scripts/flash/foxglove_flash.py searches for FOXGLOVE_HALF=. */
const char foxglove_half_marker[] __used __attribute__((section(".foxglove_half"))) =
    "FOXGLOVE_HALF=" FOXGLOVE_HALF ";VER=" FOXGLOVE_FW_VERSION ";PROFILE=" FOXGLOVE_PROFILE
    ";SHA=" FOXGLOVE_BUILD_SHA;

void foxglove_build_info(struct foxglove_build_info *out) {
    *out = (struct foxglove_build_info){
        .major = FOXGLOVE_FW_MAJOR,
        .minor = FOXGLOVE_FW_MINOR,
        .patch = FOXGLOVE_FW_PATCH,
        .profile = IS_ENABLED(CONFIG_FOXGLOVE_TEST_HOOKS) ? 0 : 1,
    };
    strncpy(out->sha, FOXGLOVE_BUILD_SHA, sizeof(out->sha)); // padded with zeros
}
