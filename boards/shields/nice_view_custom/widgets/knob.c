/*
 * &knob and &knob_press (#719; knob.h): an encoder's turns and click, for any ZMK encoder board.
 *
 * - &knob C_VOLUME_UP C_VOLUME_DOWN: each detent taps its fallback key (the turn's maths is ZMK's
 *   own sensor-rotate, behavior_sensor_rotate_common.c), unless the PC holds the knob with an app
 *   as its target: then the detents go to the PC as one event per burst (D7 94 01 01 steps).
 *   With master as the target the keys go, so Windows' own volume and its popup work as they
 *   always did, unless the PC holds master volume (D7 16, #855): then master's turns are events
 *   too (target master), and the PC sets the level and shows its own card.
 * - &knob_press C_MUTE: its fallback key, pressed and released with the switch, unless the PC
 *   holds the knob: then a click (sent once double-tap-ms passed without a second), a double
 *   click (sent on the second press) and a hold (sent when hold-ms is reached) are events. A
 *   double click with master as the target also taps the fallback key here: master's mute is
 *   Windows' own, the same key as without Foxglove; with master volume held it doesn't (the PC
 *   mutes master itself).
 *
 * Start/stop: a hold timer starts on a held press and stops on its release or when it fires; the
 * click timer starts on a held release and stops on the next press or when it fires. Nothing
 * else runs; no boot work.
 * SPDX-License-Identifier: MIT
 */

#include <zephyr/device.h>
#include <zephyr/drivers/sensor.h>
#include <zephyr/kernel.h>
#include <zephyr/sys/atomic.h>

#include <drivers/behavior.h>
#include <zmk/behavior.h>
#include <zmk/behavior_queue.h>
#include <zmk/events/position_state_changed.h>
#include <zmk/matrix.h>
#include <zmk/sensors.h>
#include <zmk/virtual_key_position.h>

#include "health.h"
#include "knob.h"

#if defined(FOXGLOVE_HAS_HOST_LINK)
#include "host_link.h"
#else
// No host link on this board: the knob is its fallback keys only.
static inline enum knob_target host_link_knob_target(void) { return KNOB_MASTER; }
static inline bool host_link_knob_held(void) { return false; }
static inline bool host_link_knob_volume(void) { return false; }
static inline int host_link_knob_event(enum knob_event kind, int8_t steps) {
    ARG_UNUSED(kind);
    ARG_UNUSED(steps);
    return -ENOTCONN;
}
#endif

/* The turns */

// One remainder per encoder (a board without one still builds, e.g. the simulator).
#define KNOB_SENSORS MAX(ZMK_KEYMAP_SENSORS_LEN, 1)

struct knob_config {
    const char *cw;  // the behaviour the clockwise detent taps (param1)
    const char *ccw; // and the counter-clockwise one (param2)
    int tap_ms;
};

struct knob_data {
    struct sensor_value remainder[KNOB_SENSORS];
    int triggers[KNOB_SENSORS];
};

static int sensor_of(uint32_t position) {
    int i = (int)ZMK_SENSOR_POSITION_FROM_VIRTUAL_KEY_POSITION(position);
    return i >= 0 && i < KNOB_SENSORS ? i : 0;
}

// Detents the PC hasn't been sent yet: a fast spin goes as one event per system queue turn.
static atomic_t turn_steps;

static void turn_send(struct k_work *work) {
    atomic_val_t n = atomic_set(&turn_steps, 0);
    while (n != 0) {
        int8_t steps = (int8_t)CLAMP(n, -127, 127);
        if (host_link_knob_event(KNOB_TURN, steps) != 0) {
            return; // the hold went meanwhile: these detents are dropped
        }
        n -= steps;
    }
}
static K_WORK_DEFINE(turn_work, turn_send);

// ZMK's sensor-rotate maths (behavior_sensor_rotate_common.c): degrees to detents.
static int knob_accept_data(struct zmk_behavior_binding *binding,
                            struct zmk_behavior_binding_event event,
                            const struct zmk_sensor_config *sensor_config,
                            size_t channel_data_size,
                            const struct zmk_sensor_channel_data *channel_data) {
    const struct device *dev = zmk_behavior_get_binding(binding->behavior_dev);
    struct knob_data *data = dev->data;
    const struct sensor_value value = channel_data[0].value;
    int i = sensor_of(event.position);
    int triggers;

    if (value.val1 == 0) {
        triggers = value.val2; // the old encoder behaviour: ticks in val2
    } else {
        struct sensor_value r = data->remainder[i];
        r.val1 += value.val1;
        r.val2 += value.val2;
        if (r.val2 >= 1000000 || r.val2 <= -1000000) {
            r.val1 += r.val2 / 1000000;
            r.val2 %= 1000000;
        }
        int per = 360 / MAX(sensor_config->triggers_per_rotation, 1);
        triggers = r.val1 / per;
        r.val1 %= per;
        data->remainder[i] = r;
    }
    data->triggers[i] = triggers;
    return 0;
}

static int knob_process(struct zmk_behavior_binding *binding,
                        struct zmk_behavior_binding_event event,
                        enum behavior_sensor_binding_process_mode mode) {
    const struct device *dev = zmk_behavior_get_binding(binding->behavior_dev);
    const struct knob_config *cfg = dev->config;
    struct knob_data *data = dev->data;
    int i = sensor_of(event.position);
    int triggers = data->triggers[i];
    data->triggers[i] = 0;

    if (mode != BEHAVIOR_SENSOR_BINDING_PROCESS_MODE_TRIGGER || triggers == 0) {
        return ZMK_BEHAVIOR_TRANSPARENT;
    }
    if (host_link_knob_target() == KNOB_APP || host_link_knob_volume()) {
        // An app, or master with master volume held (#855): the PC sets the level.
        atomic_add(&turn_steps, triggers);
        k_work_submit(&turn_work);
        return ZMK_BEHAVIOR_OPAQUE;
    }
    // Master (unless the PC holds master volume), or no PC: the fallback keys, as &inc_dec_kp
    // taps them.
    struct zmk_behavior_binding key = {
        .behavior_dev = triggers > 0 ? cfg->cw : cfg->ccw,
        .param1 = triggers > 0 ? binding->param1 : binding->param2,
    };
#if IS_ENABLED(CONFIG_ZMK_SPLIT)
    event.source = ZMK_POSITION_STATE_CHANGE_SOURCE_LOCAL; // as ZMK's: always on the central
#endif
    for (int n = triggers > 0 ? triggers : -triggers; n > 0; n--) {
        zmk_behavior_queue_add(&event, key, true, cfg->tap_ms);
        zmk_behavior_queue_add(&event, key, false, 0);
        health_inc(HEALTH_KNOB_KEYS);
    }
    return ZMK_BEHAVIOR_OPAQUE;
}

static const struct behavior_driver_api knob_api = {
    .locality = BEHAVIOR_LOCALITY_CENTRAL,
    .sensor_binding_accept_data = knob_accept_data,
    .sensor_binding_process = knob_process,
};

#define DT_DRV_COMPAT foxglove_behavior_knob
#define KNOB_INST(n)                                                                               \
    static const struct knob_config knob_config_##n = {                                           \
        .cw = DEVICE_DT_NAME(DT_INST_PHANDLE_BY_IDX(n, bindings, 0)),                              \
        .ccw = DEVICE_DT_NAME(DT_INST_PHANDLE_BY_IDX(n, bindings, 1)),                             \
        .tap_ms = DT_INST_PROP(n, tap_ms),                                                         \
    };                                                                                             \
    static struct knob_data knob_data_##n;                                                         \
    BEHAVIOR_DT_INST_DEFINE(n, NULL, NULL, &knob_data_##n, &knob_config_##n, POST_KERNEL,          \
                            CONFIG_KERNEL_INIT_PRIORITY_DEFAULT, &knob_api);
DT_INST_FOREACH_STATUS_OKAY(KNOB_INST)
#undef DT_DRV_COMPAT

/* The click */

enum press_state {
    PRESS_IDLE,
    PRESS_KEY,         // not held: the fallback key is down
    PRESS_DOWN,        // held: pressed, the hold timer runs
    PRESS_DONE,        // held: this press was a hold or a double click; its release ends it
    PRESS_WAIT_SECOND, // held: released after a click, the click timer runs
};

struct knob_press_config {
    const char *key; // the behaviour the fallback key goes through
    int hold_ms;
    int double_ms;
    int tap_ms;
};

struct knob_press_data {
    const struct knob_press_config *cfg;
    struct k_work_delayable hold_work;
    struct k_work_delayable click_work;
    struct zmk_behavior_binding_event event; // the press's
    uint32_t param;                          // the fallback key, as bound
    uint8_t state;
};

static struct zmk_behavior_binding fallback_of(const struct knob_press_data *data) {
    return (struct zmk_behavior_binding){.behavior_dev = data->cfg->key, .param1 = data->param};
}

static void press_event(enum knob_event kind) {
    host_link_knob_event(kind, 0); // a hold dropped meanwhile: nothing goes
}

static void hold_reached(struct k_work *work) {
    struct k_work_delayable *d = k_work_delayable_from_work(work);
    struct knob_press_data *data = CONTAINER_OF(d, struct knob_press_data, hold_work);
    if (data->state == PRESS_DOWN) {
        data->state = PRESS_DONE;
        press_event(KNOB_HOLD);
    }
}

static void click_due(struct k_work *work) {
    struct k_work_delayable *d = k_work_delayable_from_work(work);
    struct knob_press_data *data = CONTAINER_OF(d, struct knob_press_data, click_work);
    if (data->state == PRESS_WAIT_SECOND) {
        data->state = PRESS_IDLE;
        press_event(KNOB_CLICK);
    }
}

static int knob_press_pressed(struct zmk_behavior_binding *binding,
                              struct zmk_behavior_binding_event event) {
    const struct device *dev = zmk_behavior_get_binding(binding->behavior_dev);
    struct knob_press_data *data = dev->data;
    data->event = event;
    data->param = binding->param1;

    if (!host_link_knob_held()) {
        k_work_cancel_delayable(&data->click_work);
        data->state = PRESS_KEY;
        zmk_behavior_queue_add(&event, fallback_of(data), true, 0);
        health_inc(HEALTH_KNOB_KEYS);
        return ZMK_BEHAVIOR_OPAQUE;
    }
    if (data->state == PRESS_WAIT_SECOND) {
        // The second click: a double click, sent now.
        k_work_cancel_delayable(&data->click_work);
        data->state = PRESS_DONE;
        press_event(KNOB_DOUBLE);
        if (host_link_knob_target() == KNOB_MASTER && !host_link_knob_volume()) {
            // Master's mute is the system's: the same key as without Foxglove (with master
            // volume held the PC mutes master itself, #855).
            zmk_behavior_queue_add(&event, fallback_of(data), true, data->cfg->tap_ms);
            zmk_behavior_queue_add(&event, fallback_of(data), false, 0);
            health_inc(HEALTH_KNOB_KEYS);
        }
        return ZMK_BEHAVIOR_OPAQUE;
    }
    data->state = PRESS_DOWN;
    k_work_reschedule(&data->hold_work, K_MSEC(data->cfg->hold_ms));
    return ZMK_BEHAVIOR_OPAQUE;
}

static int knob_press_released(struct zmk_behavior_binding *binding,
                               struct zmk_behavior_binding_event event) {
    const struct device *dev = zmk_behavior_get_binding(binding->behavior_dev);
    struct knob_press_data *data = dev->data;

    switch (data->state) {
    case PRESS_KEY:
        data->state = PRESS_IDLE;
        zmk_behavior_queue_add(&event, fallback_of(data), false, 0);
        break;
    case PRESS_DOWN:
        k_work_cancel_delayable(&data->hold_work);
        data->state = PRESS_WAIT_SECOND;
        k_work_reschedule(&data->click_work, K_MSEC(data->cfg->double_ms));
        break;
    case PRESS_DONE:
        data->state = PRESS_IDLE;
        break;
    default:
        break;
    }
    return ZMK_BEHAVIOR_OPAQUE;
}

#if IS_ENABLED(CONFIG_ZMK_BEHAVIOR_METADATA)
// The fallback key is a key, as &kp's parameter (Studio and the app's keymap editor show it so).
static const struct behavior_parameter_value_metadata press_values[] = {
    {
        .display_name = "Key",
        .type = BEHAVIOR_PARAMETER_VALUE_TYPE_HID_USAGE,
    },
};

static const struct behavior_parameter_metadata_set press_sets[] = {{
    .param1_values = press_values,
    .param1_values_len = ARRAY_SIZE(press_values),
}};

static const struct behavior_parameter_metadata press_metadata = {
    .sets_len = ARRAY_SIZE(press_sets),
    .sets = press_sets,
};
#endif

static const struct behavior_driver_api knob_press_api = {
    .locality = BEHAVIOR_LOCALITY_CENTRAL,
    .binding_pressed = knob_press_pressed,
    .binding_released = knob_press_released,
#if IS_ENABLED(CONFIG_ZMK_BEHAVIOR_METADATA)
    .parameter_metadata = &press_metadata,
#endif
};

#define DT_DRV_COMPAT foxglove_behavior_knob_press
#define KNOB_PRESS_INST(n)                                                                         \
    static const struct knob_press_config knob_press_config_##n = {                               \
        .key = DEVICE_DT_NAME(DT_INST_PHANDLE_BY_IDX(n, bindings, 0)),                             \
        .hold_ms = DT_INST_PROP(n, hold_ms),                                                       \
        .double_ms = DT_INST_PROP(n, double_tap_ms),                                               \
        .tap_ms = DT_INST_PROP(n, tap_ms),                                                         \
    };                                                                                             \
    static struct knob_press_data knob_press_data_##n = {                                         \
        .cfg = &knob_press_config_##n,                                                             \
        .hold_work = Z_WORK_DELAYABLE_INITIALIZER(hold_reached),                                   \
        .click_work = Z_WORK_DELAYABLE_INITIALIZER(click_due),                                     \
    };                                                                                             \
    BEHAVIOR_DT_INST_DEFINE(n, NULL, NULL, &knob_press_data_##n, &knob_press_config_##n,           \
                            POST_KERNEL, CONFIG_KERNEL_INIT_PRIORITY_DEFAULT, &knob_press_api);
DT_INST_FOREACH_STATUS_OKAY(KNOB_PRESS_INST)
#undef DT_DRV_COMPAT
