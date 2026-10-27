/*
 * SET_EFFECT (D7 0F, #490): this half draws an effect from its lamp positions and the shared
 * clock, on either half. The left takes it from the PC (host_effects.c), the right from the left
 * over the art relay (art_relay.h ART_RELAY_EFFECT).
 * SPDX-License-Identifier: MIT
 */

#pragma once

#include <stdbool.h>
#include <stdint.h>

#include "lamp_effect.h"

// Takes this half's LEDs from its own effect (lamp_strip.c) and draws w, its clock at at_ms now
// (the scene's time modulo the period). A new call replaces the effect and re-anchors the clock.
// Any thread; the drawing runs on ZMK's low-priority queue. Copies w.
void effect_run_start(const struct lamp_effect_wire *w, uint32_t at_ms);
// Stops drawing. hand_back: the LEDs go back to the keyboard's own effect; without it they stay
// with the PC (SET_FRAME takes over). Once it returns, no frame of the effect draws any more.
void effect_run_stop(bool hand_back);
// Whether this half draws an effect.
bool effect_run_active(void);
// This half's effect clock now, modulo the period: the anchor for the other half.
uint32_t effect_run_clock(void);
// rgb_idle.c: the user's on/off (or idle) may have changed while the PC has the LEDs. Off draws
// black and stops the frames; on starts them again. Any thread.
void effect_run_lights_changed(void);
