# Local compute: what runs on the user's PC instead of Claude tokens

Issues are the tracker (`gh issue list` from inside the repo; the #90 queue is only for human steps,
Planner open state is issue #358). Every issue carries at most one `compute:` label. No label means
`compute:claude`.

| Label | Runs | By | Reviewed by |
|---|---|---|---|
| `compute:local-llm` | rote text work on LM Studio (`qwen3.6-35b-a3b`) via `tools\local-llm.ps1` | On-call | Planner (or the filing engineer) |
| `compute:cpu` | a deterministic command, no model: fuzzing, soak/lab runs, long test runs | On-call | the filer reads the summary |
| `compute:claude` | design, code, judgement (the default) | a job session | code-reviewer |
| `local-review` | state: a local job finished, its output waits for review | set by On-call | Planner |

## What qualifies for `compute:local-llm`
All four must hold, else it is `compute:claude`:
1. **Rote:** a transform with a clear recipe (summarise a log, triage WARNs into buckets, draft test
   vectors from a spec, draft release notes or a PR body from a diff).
2. **Verifiable:** the output is checked by a machine (run vectors against the parser, compare counts
   to the log) or fits a short human read.
3. **No authorship:** the model writes no product code, firmware, design, or security-relevant text.
   Data files (test vectors) are fine once a test run accepts them.
4. **Reviewed before use:** a Claude agent reads every output before it is merged or posted anywhere
   outward-facing. A failed check sends the issue back to `compute:claude` with one line why.

## When to queue a local job (user, 11/09)
Queue a local job only when ALL hold:
1. **A named consumer:** an open issue or PR that uses the output.
2. **A mechanical accept check** (it parses, a test passes, a diff applies) or a short Claude review.
3. **A hard cap:** one pass, N items, or a time limit; no re-runs to improve the output.
4. **Expected value beats review cost.**

Idle beats busywork: if no job meets all four, the machine stays idle.

### Accept rate per job type
On-call is the wrangler and keeps this table current after each job. A type accepted under 50% of
the time is retired or narrowed.

| Job type | Accepted | Status |
|---|---|---|
| Spec-to-vector drafts (§8.1) | 4 of 6 (2 were spec gaps) | 67%, keep |
| Trust edge-case vectors | 6 of 14 (8 duplicates of existing vectors) | 43%, narrow: only cases the crate can run |
| Install vectors | 5 of 10 (the registry test rejected 5) | 50%, keep, narrow to the test as the accept check |
| Log cause analysis (#942-#946) | 0 of 5 | retired 11/09, generic causes |
| Log-line extraction | accepted (mechanical) | keep |
| WARN triage (cause guessing) | n/a | retired 11/09, marked everything harmless |
| Terms and stale-path greps | correct, but no consumer | do not queue |

## What qualifies for `compute:cpu`
A command with a pass/fail or counted result that costs no tokens while it runs: fuzz a parser, a
soak or lab round, a full test run on another tree. State the command, the time limit and the
artifact in the issue. Only the failing lines come back, never the log.

## Routing (no polling)
- **Tag:** whoever files an issue (or the Planner at triage) adds the label. Add it when you write
  the brief; do not spend Claude tokens first to find out whether it is rote.
- **Hand over:** the Planner sends On-call ONE message per batch ("local batch: #A #B #C"), at an event
  it already has: after a merge batch, at the end of a sitting step, or on the Hub's go. On-call
  also runs one `gh issue list --label compute:local-llm,compute:cpu` at its own existing wakes, never
  on a timer or loop.
- **Gate:** local jobs run whenever the user isn't gaming, at below-normal priority (`local-llm.ps1`
  exits 3 while a game runs; check the same before a CPU job). If a game is running, leave the issue
  labelled, say "deferred: game active" once, and wait for the next event. No retry loop.
- **Limits:** `-TimeoutSec 300` per LLM prompt, one batch at most 30 minutes; CPU jobs at BelowNormal
  priority with a wall-clock limit written in the issue (default 30 minutes). Never leave a model
  loaded or a job running past its limit.
- **Return:** On-call posts at most 20 lines on the issue (the result, the counts, failing lines),
  puts large output in the Hub's notes folder, and swaps the label to `local-review`.
- **Review:** the Planner reads it, uses or corrects it, and closes the issue or relabels it
  `compute:claude`. Record one line in `tools\local-llm-quality.csv` for LLM jobs.
- One local job at a time; they count against the same 3-agent limit only when a Claude session runs
  them, and On-call runs them itself.

## Recurring ops jobs are scripts (#949)
`scripts/ops/` holds the recurring jobs as PowerShell, run by Windows Task Scheduler, no Claude:
`pc-health-check`, `foxglove-digest` (optional `-Summarize` through the local LLM, at most 10
lines, from the facts file only), `monthly-driver-check`, `weekly-repo-janitor` (proposals only),
`logi-leak-probe` (before/after each flash) and `lab-health` (after a batch flash). All of them
share `game-check.ps1` (exit 3 while a game runs; the list is `games.json`), run at below-normal
priority under a hard time limit, launch no GUI, and write one summary line to
`%LOCALAPPDATA%\Foxglove\ops\summary.log`. Tests: `scripts/ops/test-ops.ps1` (part of `check.ps1`).

## The queue script: `tools/local-queue.ps1` (#1128)
One batch per run (no loop, no timer). It reads `gh issue list -l compute:local-llm --state open`,
runs each runnable issue through `tools\local-llm.ps1`, posts ONE comment of at most 20 lines headed
`Local LLM draft (needs review): <model>` and adds `local-review`. It never closes an issue, merges or
edits code. Run it at the "hand over" event above, never on a timer:
`powershell -File tools\local-queue.ps1` (`-List` prints the queue, `-DryRun` / `-WhatIf` also shows the
game state and does nothing else).

**Tag an issue** with `compute:local-llm` and put these markers on their own lines in the body:
```
type: log-triage          (or pr-body-draft, release-notes, test-vectors, docs-prescreen)
input: C:\...\gate.log    (a file inside the repo, the sibling worktrees folder or _logs)
prompt: one-line extra instruction
```
`log-triage` needs `input:` (the last 24,000 characters of a gate or lab log; the model lists failures
and causes quoted from it). The other types need `input:` or `prompt:` and pass them through a thin
template. Skipped with a note: no or unknown `type:`, a path outside the allowed folders, and issues
already labelled `local-review` or `needs-claude`.

**Limits:** a game running holds the whole batch (exit 0, `held: game running`), checked at the start and
between jobs; BelowNormal priority; 10 minutes per job (`-JobTimeoutSec`), 45 minutes per batch
(`-BatchTimeoutSec`), 5 jobs (`-MaxJobs`). A job that overruns is killed by exact PID and the model is
unloaded with `lms.exe` (the server stopped if the run found it down), so nothing stays resident. The
wrapper writes the `local-llm-usage.csv` row (task `local-queue:<type>`). The summary line goes to
`%LOCALAPPDATA%\Foxglove\ops\summary.log` like the other ops scripts. Tests: `scripts/ops/test-local-queue.ps1`
(stubs only, part of `test-ops.ps1`).

**Review step:** a Sonnet agent at low effort reads every `local-review` comment against its input
before anything is used or posted outward; then the Planner closes the issue or relabels it
`compute:claude` as above. Update the accept-rate table after each batch.
