/*
 * SET_EFFECT's maths (lamp_effect.h), line for line after crates/scene: effects.rs (band,
 * band_centre, the four effects), space.rs (along, extent) and colour.rs (mixing in Oklab with
 * palette's matrices, gradient). Floats as there (f32), with the phase's division in double as
 * SceneTime::phase does it. The simulator holds it to crates/scene's vectors within 2 per channel.
 * SPDX-License-Identifier: MIT
 */

#include <math.h>
#include <string.h>

#include "lamp_effect.h"

/* Where each lamp of the left half sits, in thousandths of the half's width and height from its
 * top-left corner, in chain order. PROVISIONAL (#529): the board profile's led_keys guess
 * (app/resources/boards/eyelash-corne.toml, a column snake) at the key centres of its drawing
 * (6.5 x 4.5 key units). The #90 row G sitting corrects both. The right half mirrors it. */
static const uint16_t lamps_permille[][2] = {
    {77, 222},  {77, 444},  {77, 667},  {231, 667}, {231, 444}, {231, 222}, {385, 167},
    {385, 389}, {385, 611}, {538, 556}, {538, 333}, {538, 111}, {692, 139}, {692, 361},
    {692, 583}, {846, 611}, {846, 389}, {846, 167}, {615, 889}, {769, 889}, {923, 889},
};

/* sRGB 8-bit encoding: thresholds[i - 1] is the linear value where i takes over from i - 1
 * (the decode of (i - 0.5) / 255), so a binary search gives the correctly rounded byte without a
 * powf per channel per frame. */
static const float thresholds[255] = {
    1.517634918e-04f, 4.552904753e-04f, 7.588174589e-04f, 1.062344442e-03f, 1.365871426e-03f,
    1.669398410e-03f, 1.972925393e-03f, 2.276452377e-03f, 2.579979360e-03f, 2.883506344e-03f,
    3.188300904e-03f, 3.509259350e-03f, 3.848314933e-03f, 4.205748030e-03f, 4.581832741e-03f,
    4.976837250e-03f, 5.391024160e-03f, 5.824650784e-03f, 6.277969427e-03f, 6.751227633e-03f,
    7.244668422e-03f, 7.758530499e-03f, 8.293048455e-03f, 8.848452952e-03f, 9.424970891e-03f,
    1.002282557e-02f, 1.064223685e-02f, 1.128342126e-02f, 1.194659215e-02f, 1.263195981e-02f,
    1.333973160e-02f, 1.407011200e-02f, 1.482330280e-02f, 1.559950311e-02f, 1.639890952e-02f,
    1.722171611e-02f, 1.806811463e-02f, 1.893829446e-02f, 1.983244280e-02f, 2.075074465e-02f,
    2.169338291e-02f, 2.266053845e-02f, 2.365239016e-02f, 2.466911500e-02f, 2.571088806e-02f,
    2.677788263e-02f, 2.787027021e-02f, 2.898822059e-02f, 3.013190190e-02f, 3.130148060e-02f,
    3.249712161e-02f, 3.371898824e-02f, 3.496724235e-02f, 3.624204428e-02f, 3.754355296e-02f,
    3.887192588e-02f, 4.022731918e-02f, 4.160988767e-02f, 4.301978482e-02f, 4.445716284e-02f,
    4.592217266e-02f, 4.741496402e-02f, 4.893568542e-02f, 5.048448422e-02f, 5.206150661e-02f,
    5.366689765e-02f, 5.530080130e-02f, 5.696336045e-02f, 5.865471691e-02f, 6.037501146e-02f,
    6.212438386e-02f, 6.390297287e-02f, 6.571091626e-02f, 6.754835085e-02f, 6.941541251e-02f,
    7.131223618e-02f, 7.323895588e-02f, 7.519570475e-02f, 7.718261504e-02f, 7.919981813e-02f,
    8.124744458e-02f, 8.332562409e-02f, 8.543448553e-02f, 8.757415699e-02f, 8.974476575e-02f,
    9.194643832e-02f, 9.417930042e-02f, 9.644347704e-02f, 9.873909241e-02f, 1.010662700e-01f,
    1.034251327e-01f, 1.058158025e-01f, 1.082384007e-01f, 1.106930482e-01f, 1.131798648e-01f,
    1.156989699e-01f, 1.182504822e-01f, 1.208345198e-01f, 1.234512000e-01f, 1.261006396e-01f,
    1.287829547e-01f, 1.314982609e-01f, 1.342466731e-01f, 1.370283056e-01f, 1.398432722e-01f,
    1.426916860e-01f, 1.455736597e-01f, 1.484893052e-01f, 1.514387341e-01f, 1.544220573e-01f,
    1.574393851e-01f, 1.604908274e-01f, 1.635764935e-01f, 1.666964922e-01f, 1.698509319e-01f,
    1.730399202e-01f, 1.762635645e-01f, 1.795219715e-01f, 1.828152475e-01f, 1.861434984e-01f,
    1.895068294e-01f, 1.929053454e-01f, 1.963391508e-01f, 1.998083496e-01f, 2.033130451e-01f,
    2.068533405e-01f, 2.104293382e-01f, 2.140411405e-01f, 2.176888490e-01f, 2.213725650e-01f,
    2.250923893e-01f, 2.288484224e-01f, 2.326407643e-01f, 2.364695145e-01f, 2.403347723e-01f,
    2.442366365e-01f, 2.481752054e-01f, 2.521505770e-01f, 2.561628489e-01f, 2.602121184e-01f,
    2.642984823e-01f, 2.684220370e-01f, 2.725828787e-01f, 2.767811030e-01f, 2.810168053e-01f,
    2.852900806e-01f, 2.896010235e-01f, 2.939497283e-01f, 2.983362889e-01f, 3.027607989e-01f,
    3.072233515e-01f, 3.117240396e-01f, 3.162629557e-01f, 3.208401921e-01f, 3.254558406e-01f,
    3.301099928e-01f, 3.348027400e-01f, 3.395341729e-01f, 3.443043823e-01f, 3.491134583e-01f,
    3.539614910e-01f, 3.588485700e-01f, 3.637747846e-01f, 3.687402239e-01f, 3.737449765e-01f,
    3.787891310e-01f, 3.838727754e-01f, 3.889959976e-01f, 3.941588852e-01f, 3.993615253e-01f,
    4.046040051e-01f, 4.098864111e-01f, 4.152088298e-01f, 4.205713473e-01f, 4.259740495e-01f,
    4.314170219e-01f, 4.369003499e-01f, 4.424241185e-01f, 4.479884124e-01f, 4.535933162e-01f,
    4.592389142e-01f, 4.649252902e-01f, 4.706525280e-01f, 4.764207111e-01f, 4.822299226e-01f,
    4.880802457e-01f, 4.939717629e-01f, 4.999045567e-01f, 5.058787093e-01f, 5.118943028e-01f,
    5.179514188e-01f, 5.240501388e-01f, 5.301905441e-01f, 5.363727156e-01f, 5.425967342e-01f,
    5.488626805e-01f, 5.551706346e-01f, 5.615206767e-01f, 5.679128866e-01f, 5.743473441e-01f,
    5.808241284e-01f, 5.873433188e-01f, 5.939049942e-01f, 6.005092333e-01f, 6.071561148e-01f,
    6.138457168e-01f, 6.205781175e-01f, 6.273533947e-01f, 6.341716262e-01f, 6.410328894e-01f,
    6.479372614e-01f, 6.548848195e-01f, 6.618756404e-01f, 6.689098006e-01f, 6.759873768e-01f,
    6.831084450e-01f, 6.902730814e-01f, 6.974813617e-01f, 7.047333615e-01f, 7.120291564e-01f,
    7.193688216e-01f, 7.267524320e-01f, 7.341800626e-01f, 7.416517880e-01f, 7.491676827e-01f,
    7.567278210e-01f, 7.643322770e-01f, 7.719811246e-01f, 7.796744376e-01f, 7.874122894e-01f,
    7.951947535e-01f, 8.030219030e-01f, 8.108938110e-01f, 8.188105503e-01f, 8.267721935e-01f,
    8.347788132e-01f, 8.428304815e-01f, 8.509272707e-01f, 8.590692527e-01f, 8.672564993e-01f,
    8.754890821e-01f, 8.837670725e-01f, 8.920905419e-01f, 9.004595613e-01f, 9.088742016e-01f,
    9.173345337e-01f, 9.258406282e-01f, 9.343925555e-01f, 9.429903859e-01f, 9.516341896e-01f,
    9.603240364e-01f, 9.690599963e-01f, 9.778421388e-01f, 9.866705335e-01f, 9.955452497e-01f,
};

static uint8_t encode(float linear) {
    if (!(linear >= thresholds[0])) { // NaN too
        return 0;
    }
    int lo = 0, hi = 254; // the answer is lo + 1
    while (lo < hi) {
        int mid = (lo + hi + 1) / 2;
        if (linear >= thresholds[mid]) {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    return lo + 1;
}

static float decode(uint8_t v) {
    float x = v / 255.0f;
    return x <= 0.04045f ? x / 12.92f : powf((x + 0.055f) / 1.055f, 2.4f);
}

static void to_oklab(const uint8_t rgb[3], float lab[3]) {
    float r = decode(rgb[0]), g = decode(rgb[1]), b = decode(rgb[2]);
    float l = cbrtf(0.4122214708f * r + 0.5363325363f * g + 0.0514459929f * b);
    float m = cbrtf(0.2119034982f * r + 0.6806995451f * g + 0.1073969566f * b);
    float s = cbrtf(0.0883024619f * r + 0.2817188376f * g + 0.6299787005f * b);
    lab[0] = 0.2104542553f * l + 0.7936177850f * m - 0.0040720468f * s;
    lab[1] = 1.9779984951f * l - 2.4285922050f * m + 0.4505937099f * s;
    lab[2] = 0.0259040371f * l + 0.7827717662f * m - 0.8086757660f * s;
}

static void from_oklab(const float lab[3], uint8_t out[3]) {
    float l = lab[0] + 0.3963377774f * lab[1] + 0.2158037573f * lab[2];
    float m = lab[0] - 0.1055613458f * lab[1] - 0.0638541728f * lab[2];
    float s = lab[0] - 0.0894841775f * lab[1] - 1.2914855480f * lab[2];
    l = l * l * l;
    m = m * m * m;
    s = s * s * s;
    out[0] = encode(4.0767416621f * l - 3.3077115913f * m + 0.2309699292f * s);
    out[1] = encode(-1.2684380046f * l + 2.6097574011f * m - 0.3413193965f * s);
    out[2] = encode(-0.0041960863f * l - 0.7034186147f * m + 1.7076147010f * s);
}

static float clamp01(float x) { return x < 0.0f ? 0.0f : x > 1.0f ? 1.0f : x; }

// colour.rs Rgb::mix: colour a at t = 0, b at t = 1, through Oklab.
static void mix(const struct lamp_effect *fx, int a, int b, float t, uint8_t out[3]) {
    t = isfinite(t) ? clamp01(t) : 0.0f;
    if (t == 0.0f) {
        memcpy(out, fx->rgb[a], 3);
        return;
    }
    if (t == 1.0f) {
        memcpy(out, fx->rgb[b], 3);
        return;
    }
    float lab[3];
    for (int i = 0; i < 3; i++) {
        lab[i] = fx->lab[a][i] + (fx->lab[b][i] - fx->lab[a][i]) * t;
    }
    from_oklab(lab, out);
}

// colour.rs gradient over the effect's colours.
static void gradient(const struct lamp_effect *fx, float t, bool cyclic, uint8_t out[3]) {
    int n = fx->n;
    if (n == 1) {
        memcpy(out, fx->rgb[0], 3);
        return;
    }
    if (!isfinite(t)) {
        t = 0.0f;
    }
    int spans;
    if (cyclic) {
        t = fmodf(t, 1.0f); // rem_euclid
        if (t < 0.0f) {
            t += 1.0f;
        }
        if (t >= 1.0f) { // -tiny + 1 rounds to 1
            t = 0.0f;
        }
        spans = n;
    } else {
        t = clamp01(t);
        spans = n - 1;
    }
    float at = t * spans;
    int i = (int)floorf(at);
    if (i > spans - 1) {
        i = spans - 1;
    }
    mix(fx, i, (i + 1) % n, at - i, out);
}

// effects.rs band: 1 at the centre, 0 from half away, smoothstep in between.
static float band(float distance, float half) {
    if (isnan(half) || half <= 0.0f) {
        return distance == 0.0f ? 1.0f : 0.0f;
    }
    float x = clamp01(fabsf(distance) / half);
    return 1.0f - x * x * (3.0f - 2.0f * x);
}

// effects.rs band_centre.
static float band_centre(float phase, float span, float width) {
    return phase * (span + width) - width / 2.0f;
}

// space.rs along: 0 at the first corner the direction meets, 1 at the last.
static float along(const struct lamp_effect *fx, float x, float y) {
    if (fx->hi - fx->lo <= 1.1920929e-7f) { // f32::EPSILON
        return 0.0f;
    }
    return clamp01((x * fx->c + y * fx->s - fx->lo) / (fx->hi - fx->lo));
}

static int16_t i16(const uint8_t *b) { return (int16_t)(b[0] | b[1] << 8); }

float lamp_effect_phase(uint64_t t_ms, uint32_t period_ms) {
    if (period_ms == 0) {
        return 0.0f;
    }
    return (float)((double)(t_ms % period_ms) / (double)period_ms);
}

bool lamp_effect_prepare(struct lamp_effect *fx, const struct lamp_effect_wire *w, bool mirror) {
    if (w->kind == LAMP_EFFECT_NONE || w->kind >= LAMP_EFFECT_KINDS || w->n < 1 ||
        w->n > LAMP_EFFECT_COLOURS || (w->kind != LAMP_EFFECT_SWEEP && w->n != 2)) {
        return false;
    }
    // Straight into fx (no copy on the stack: the low-priority queue's is small).
    struct lamp_effect *const f = fx;
    memset(f, 0, sizeof(*f));
    f->kind = w->kind;
    f->n = w->n;
    f->period_ms = w->period_ms[0] | w->period_ms[1] << 8 | (uint32_t)w->period_ms[2] << 16;
    f->width = (uint16_t)(w->width_dmm[0] | w->width_dmm[1] << 8) / 10.0f;
    f->ox = i16(&w->origin[0]);
    f->oy = i16(&w->origin[2]);
    float deg = (uint16_t)(w->angle_cdeg[0] | w->angle_cdeg[1] << 8) / 100.0f;
    float rad = deg * 0.017453292519943295f; // f32::to_radians
    f->s = sinf(rad);
    f->c = cosf(rad);
    float bx = i16(&w->room[0]), by = i16(&w->room[2]), bw = i16(&w->room[4]),
          bh = i16(&w->room[6]);
    const float corners[4][2] = {{bx, by}, {bx + bw, by}, {bx, by + bh}, {bx + bw, by + bh}};
    f->lo = INFINITY;
    f->hi = -INFINITY;
    for (int i = 0; i < 4; i++) {
        float p = corners[i][0] * f->c + corners[i][1] * f->s;
        f->lo = fminf(f->lo, p);
        f->hi = fmaxf(f->hi, p);
        f->reach = fmaxf(f->reach, hypotf(corners[i][0] - f->ox, corners[i][1] - f->oy));
    }
    f->span = bw * fabsf(f->c) + bh * fabsf(f->s);
    memcpy(f->rgb, w->rgb, sizeof(f->rgb));
    for (int i = 0; i < f->n; i++) {
        to_oklab(f->rgb[i], f->lab[i]);
    }
    float hx = i16(&w->half[0]), hy = i16(&w->half[2]), hw = i16(&w->half[4]),
          hh = i16(&w->half[6]);
    for (int i = 0; i < LAMP_EFFECT_LAMPS; i++) {
        uint16_t pm[2];
        if (w->lamps_set) {
            pm[0] = (uint16_t)(w->lamp[i][0] | w->lamp[i][1] << 8);
            pm[1] = (uint16_t)(w->lamp[i][2] | w->lamp[i][3] << 8);
            if (pm[0] == LAMP_EFFECT_UNSEEN || pm[1] == LAMP_EFFECT_UNSEEN) {
                pm[0] = pm[1] = 500; // not seen: the half's centre
            }
        } else {
            lamp_effect_table(i, mirror, pm);
        }
        f->lamp[i][0] = hx + hw * (pm[0] / 1000.0f);
        f->lamp[i][1] = hy + hh * (pm[1] / 1000.0f);
    }
    return true;
}

void lamp_effect_table(int i, bool mirror, uint16_t out[2]) {
    if (i < 0 || i >= (int)(sizeof(lamps_permille) / sizeof(lamps_permille[0]))) {
        out[0] = out[1] = 500; // not in the table: the half's centre
        return;
    }
    out[0] = mirror ? 1000 - lamps_permille[i][0] : lamps_permille[i][0];
    out[1] = lamps_permille[i][1];
}

void lamp_effect_at(const struct lamp_effect *fx, float x, float y, float phase, uint8_t out[3]) {
    switch (fx->kind) {
    case LAMP_EFFECT_SCANLINE: {
        float here = along(fx, x, y) * fx->span;
        float centre = band_centre(phase, fx->span, fx->width);
        mix(fx, 1, 0, band(here - centre, fx->width / 2.0f), out);
        break;
    }
    case LAMP_EFFECT_SWEEP:
        gradient(fx, along(fx, x, y) - phase, true, out);
        break;
    case LAMP_EFFECT_RIPPLE: {
        float radius = band_centre(phase, fx->reach, fx->width);
        float d = hypotf(x - fx->ox, y - fx->oy);
        mix(fx, 1, 0, band(d - radius, fx->width / 2.0f), out);
        break;
    }
    case LAMP_EFFECT_BREATHE: {
        float a = phase * 6.2831855f; // f32::consts::TAU
        mix(fx, 0, 1, 0.5f - 0.5f * cosf(a), out);
        break;
    }
    default:
        memset(out, 0, 3);
    }
}

void lamp_effect_render(const struct lamp_effect *fx, float phase,
                        uint8_t out[LAMP_EFFECT_LAMPS][3]) {
    for (int i = 0; i < LAMP_EFFECT_LAMPS; i++) {
        lamp_effect_at(fx, fx->lamp[i][0], fx->lamp[i][1], phase, out[i]);
    }
}

bool lamp_effect_still(const struct lamp_effect *fx) { return fx->period_ms == 0; }
