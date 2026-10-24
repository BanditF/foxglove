/*
 * The art relay's board-side step, for the simulator (which builds no relay): &art_next /
 * &art_prev fall back to it when the app isn't subscribed (#302), so printing it shows that path.
 * SPDX-License-Identifier: MIT
 */
#include <zephyr/sys/printk.h>

void art_relay_step(int dir) { printk("hostlink: -- art relay steps the board's art by %d\n", dir); }
