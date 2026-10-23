/*
 * Underglow idle auto-off that survives a reboot, and the right half following the left's.
 * SPDX-License-Identifier: MIT
 */

#pragma once

#include <stdbool.h>

// Left: whether the right half's underglow follows the left's (persisted, default on).
bool rgb_idle_sync(void);
void rgb_idle_set_sync(bool sync);

// Right: the left's underglow state, from the art relay link. follow = false hands the right its
// own idle auto-off back.
void rgb_idle_remote(bool on, bool follow);
// Right: the split link dropped; go by the right's own activity until the left is back.
void rgb_idle_remote_lost(void);
