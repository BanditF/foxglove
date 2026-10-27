# Foxglove roles (`.claude/agents/`)

Each file in `.claude/agents/` is one role on the team. Instead of a session staying open all
day, a role is a file, and work runs as one short job per issue that starts from the right file and ends with
a PR. Standing sessions: **1 · Hub** (the user's contact) and **7 · On-call** (hands on the
keyboard). Everything else starts on demand.

| Role | File | Runs as |
|---|---|---|
| Planner: roadmap, briefs, merges | `planner.md` | a session when there is merging or triage to do |
| App engineer | `app-engineer.md` | a job session per issue |
| Firmware engineer | `firmware-engineer.md` | a job session per issue |
| Art & content | `art.md` | a job session per issue |
| On-call: flashing, sittings, PC chores | `on-call.md` | the standing On-call session only, never a subagent |
| Test lab | `test-lab.md` | a subagent, or a job session for harness work |
| Scout: hardware and prior art | `scout.md` | a subagent (research comes back as a report) |
| Host health: disk, memory, GPU, builds | `host-health.md` | a subagent, or a job session for tooling |
| Code reviewer | `code-reviewer.md` | a subagent, the merge gate |
| Release | `release.md` | a subagent |

## Feature requests: research & scope gate

Every feature request goes through three steps before a job is briefed:
1. **Scope:** the Planner checks it against [`scope.md`](scope.md). Out of scope is closed with
   the reason; unclear gets `needs-decision` and goes to the user through the Hub.
2. **Research:** the Scout writes it in the issue body's Research section, or as a comment headed
   "Research" (either counts), in order: **Reuse** (open-market code or
   crates and their licence), **Emulate** (patterns in existing software), **Design our own**
   (only when the first two come up empty).
3. **Design and build:** only then does the Planner brief an engineer.

New feature issues use the Feature template (`.github/ISSUE_TEMPLATE/feature.yml`). Bug fixes
skip the gate.

## Starting a job

The Planner (or the Hub) starts one session per issue with a brief like:

> You are the **firmware-engineer** for BanditF/foxglove. Read `.claude/agents/firmware-engineer.md`
> and your memory `~/.claude/agent-memory/firmware-engineer/MEMORY.md` first. Task: #NNN ...
> Done = a PR that passes `scripts/check.ps1`, reported "ready to merge" to the Planner.

Each job makes its own worktree in `C:\Users\fuchs\splitkeeb\worktrees\<name>`:
`git -C C:/Users/fuchs/splitkeeb/foxglove worktree add -b <branch> ../worktrees/<name> origin/main`.
Never put one inside the repo, including `.claude/worktrees/`: from a nested `app/`, cargo finds the
main checkout's workspace and refuses to build. `scripts/clean-worktrees.ps1` frees merged ones.
Logs and scratch files go in `worktrees\_logs\`, not loose in `worktrees\`; the cleanup prunes
both after an hour.
After a `git worktree move`, delete that worktree's target dir (or `CARGO_TARGET_DIR`) before the
next gate. Test binaries bake `env!("CARGO_MANIFEST_DIR")` in at the old path and fail only in the
full run (plugin-api's `schema_is_current`, #361).
From a terminal the same job is
`claude --agent firmware-engineer`. A subagent role (test-lab, scout, host-health,
code-reviewer, release) is called from any session with the Agent tool.

## Build cost

The PC is shared with the user, and every gate is minutes of full CPU (the PC impact report,
2026-10-26). So:
- **Iterate with fast checks only:** `cargo check` or `cargo clippy` on the crate you changed
  (`-p <crate>`, or `--manifest-path app/Cargo.toml`), through `scripts/build-slot.ps1`.
- **Run the full `scripts/check.ps1` once**, just before opening the PR. Run it again only after
  a code change, never for a PR description, docs or comment edit.
- **Reuse one warm worktree** (and its target dir) for a series of small fixes: branch each fix
  from `origin/main` inside it (`git switch -c <branch> origin/main`) instead of adding a fresh
  worktree per tiny change.
- **Builds run at BelowNormal priority:** `scripts/build-slot.ps1` (and so `check.ps1` and
  `merge-pr.sh`) drops the slot's process to it and cargo inherits it, so the desktop stays
  smooth during a gate. Jobs and the two slots are unchanged.
- **Guards in the gate** (code governor): `cargo fmt --check` (crates and `app/src/foxglove`),
  `cargo deny` (licences, advisories; `deny.toml`, `app/deny.toml`), and a size budget that lists
  functions over 120 lines and files over 1,500 (warnings only). PRs use
  `.github/pull_request_template.md`.
- **Remove a finished worktree on merge:** `git worktree remove <dir>` plus its target dir, or
  `scripts/clean-worktrees.ps1 -Apply`.

## This PC's values (`tools/local.toml`)

The dev tooling's machine-specific values live in one place, `scripts/local_config.py`: the
halves' USB serials (`foxglove_flash.py`, foxglove-lab), the flasher's rollback folder, the
workspace folder (for scheduled-task prompts and personal scripts) and the worktree root
(`merge-pr.sh`). Its defaults are this PC's, so nothing changes here. On another PC, copy
`tools/local.example.toml` to `tools/local.toml` in the workspace folder (next to the main
checkout) and edit it, or set the environment variable named next to each key.
`python scripts/local_config.py show` prints what the tools will use. A scheduled-task prompt
can start with `python <repo>\scripts\local_config.py get workspace.root` instead of a fixed path.

## Memory

Every role keeps `memory: user`: `~/.claude/agent-memory/<role>/MEMORY.md` on this PC, shared by
every job in that role whatever its worktree. A subagent or `--agent` run loads it
automatically; a desktop job session reads it as its first step. Add a lesson when you learn
one, prune what is stale, keep it under 200 lines. Open work goes in issues, not memory.

## Rules the hooks enforce

`.claude/settings.json` runs `scripts/hooks/guard.py` before every shell command. It blocks a
flash outside On-call, a Foxglove launch without On-call's OK, a second listener on a busy port,
a PR or push to the wrong base, a worktree outside `splitkeeb\worktrees\`, and a cargo build
outside a build slot (`scripts/build-slot.ps1`; `check.ps1` takes one itself). Each message says
the one deliberate way past it.
