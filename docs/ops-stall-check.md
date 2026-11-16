# Stall check (ops)

`scripts/ops/stall-check.ps1` decides whether Foxglove work has stalled. It is a script, not an
LLM run: below-normal priority, a 120 s hard limit, no network, no GUI, no launches (#1126).
`scripts/ops/register-stall-check.ps1` registers it hourly in Task Scheduler (a person runs it
once; `-WhatIf` shows the plan, `-Unregister` removes it).

Output, in `<folder holding worktrees\>\worktrees\_tools\`:

- `stall-status.json`: `last_check`, `stalled[]` (`rule`, `what`, `since`, `confirmed`), `action`
  (`none`, `alarm`, `pause`, `freeze_lifted`), plus the facts (`progress`, `grace`, `jobs`,
  `disk`, `open_prs`, `strikes`, `alarm`).
- `stall-alarm.txt`: the alarm lines, only while an alarm is confirmed.
- `stall-state.json`: two-strikes memory. `game-freeze.flag`: if present and no game runs, it is
  removed and `action` is `freeze_lifted`.

Rules: a (no progress >90 min while work exists; grace 60 min after a session start or cleared
freeze), b (open non-draft PR idle >120 min, no cargo/rustc), c (hideout-fuzz or python past 3 h),
e (C: under 50 GB), f (possible usage-limit stop, once). a, b, c need two runs in a row. A running
game means `none`. Open PRs and the #90 count are read from `_tools\open-prs.json` and
`user-queue-count.txt` (written by a job that has network); when missing or older than 180 min,
`prs_known` is false and the script never pauses or raises rule b. Tests: `scripts\ops\test-ops.ps1`.
