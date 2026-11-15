# footprint_compare.py: idle-footprint sample summary and baseline compare (#935)

`scripts/lab/footprint_compare.py` is a stdlib-only library module (no Windows calls, no process
launches) that turns two samples of the same `hideout.exe` process into per-metric readings and
compares them against a stored baseline to decide PASS / FAIL / BLOCKED. It has no CLI entry point.

## Metrics measured

All metrics concern the hub process while idle:

| Metric | Unit | Notes |
|---|---|---|
| `private_mb` | MB | private working-set bytes at end of window |
| `working_set_mb` | MB | working-set bytes at end of window |
| `cpu_pct` | % of one core | CPU time over the sampling window / wall time |
| `loop_wakes_per_s` | wakes/s | hub's event-loop returns per second (from log lines) |
| `wakeups_per_s` | switches/s | thread context-switches summed; INFO only, never fails. `None` where no counter exists |
| `gdi_handles`, `user_handles` | count | GDI and USER handle counts from the last sample (`gdi`, `user`), at end of window |
| `vram_mb` | MB | dedicated GPU memory; `None` on builds without a counter |

Optional (may be `None`, never FAIL): `wakeups_per_s`, `vram_mb`. INFO-only (never compared or used in verdict): `wakeups_per_s`.

## Window states and limits

Loop-wake limits are per state: `hidden` compares `loop_wakes_per_s`, `shown` compares `loop_wakes_shown_per_s`. In the real baseline the hidden limit is `"max": 2` (at most 2 wakes/s, window closed to the tray) and the shown limit is `"max": 5`.

A baseline entry is either `{"value": v, "pct": p, "slack": s}` (limit = value * (1 + pct/100) + slack) or has a `"max"` key, a fixed limit that replaces the formula for any metric (`value` is then only the expected reading).

## API functions

| Function | Purpose |
|---|---|
| `summarize(first, last, wall_s)` | Two process-sample dicts (`private_b`, `working_set_b`, `cpu_100ns`, `threads`, `gdi`, `user`, optional `vram_b`) seconds apart -> metric dict. Raises `ValueError` if `wall_s <= 0`. |
| `top_threads(first, last, wall_s, names=None, n=3)` | Top `n` threads by context-switches per second: `[{"tid", "name", "switches_per_s"}]`. Empty when no per-thread data. |
| `compare(measured, baseline, state='hidden')` | Per-metric rows `(name, value, limit, status)` — PASS / FAIL / SKIP / MISSING / INFO. `state` picks the loop-wake key. |
| `verdict(rows)` | FAIL if any metric past its limit, BLOCKED if a baselined reading is missing (not OPTIONAL), else PASS. |
| `record(batch, results)` | JSON text: `{"batch": ..., "sets": {...}}`. |
| `loop_wakes(text)` | From hub log lines -> `(wakes_per_s or None, {cause: count})`; drops the first report. |
| `load_sets(text)` | Parses a JSON `{"sets": {...}}` config into `{name: [folder strings]}` with validation. |

## Example usage

The baseline file (`tests/hil/hideout-footprint-baseline.json`) is `{"sets": {"none": {...}, "default": {...}}}`; `compare` takes one set's entries, as `scripts/lab/hideout_footprint.py` does with `baseline["sets"][name]`.

```python
import json
import footprint_compare as fc

metrics = fc.summarize(sample_first, sample_last, wall_s=60)
baseline = json.load(open('tests/hil/hideout-footprint-baseline.json'))
rows = fc.compare(metrics, baseline['sets']['none'], state='hidden')
print(fc.verdict(rows))   # PASS / FAIL / BLOCKED
```
