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
- **Gates, in order:** no game running (`local-llm.ps1` exits 3; check the same before a CPU job)
  and `python scripts/lab/labkit.py idle --min 120` passes. If either blocks, leave the issue labelled,
  say "deferred: game/user active" once, and wait for the next event. No retry loop.
- **Limits:** `-TimeoutSec 300` per LLM prompt, one batch at most 30 minutes; CPU jobs at BelowNormal
  priority with a wall-clock limit written in the issue (default 30 minutes). Never leave a model
  loaded or a job running past its limit.
- **Return:** On-call posts at most 20 lines on the issue (the result, the counts, failing lines),
  puts large output in the Hub's notes folder, and swaps the label to `local-review`.
- **Review:** the Planner reads it, uses or corrects it, and closes the issue or relabels it
  `compute:claude`. Record one line in `tools\local-llm-quality.csv` for LLM jobs.
- One local job at a time; they count against the same 3-agent limit only when a Claude session runs
  them, and On-call runs them itself.
