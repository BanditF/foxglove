# Foxglove roles

Each file here is one role on the team. Instead of a session staying open all day, a role is
a file, and work runs as one short job per issue that starts from the right file and ends with
a PR. Standing sessions: **1 · Hub** (the user's contact) and **7 · On-call** (hands on the
keyboard). Everything else starts on demand.

| Role | File | Runs as |
|---|---|---|
| Planner: roadmap, briefs, merges | `planner.md` | a session when there is merging or triage to do |
| App engineer | `app-engineer.md` | a job session per issue |
| Firmware engineer | `firmware-engineer.md` | a job session per issue |
| Art & content | `art.md` | a job session per issue |
| On-call: flashing, sittings, PC chores | `on-call.md` | the standing On-call session |
| Test lab | `test-lab.md` | a subagent, or a job session for harness work |
| Scout: hardware and prior art | `scout.md` | a subagent (research comes back as a report) |
| Host health: disk, memory, GPU, builds | `host-health.md` | a subagent, or a job session for tooling |
| Code reviewer | `code-reviewer.md` | a subagent, the merge gate |
| Release | `release.md` | a subagent |

## Starting a job

The Planner (or the Hub) starts one session per issue with a brief like:

> You are the **firmware-engineer** for BanditF/foxglove. Read `.claude/agents/firmware-engineer.md`
> and your memory `~/.claude/agent-memory/firmware-engineer/MEMORY.md` first. Task: #NNN ...
> Done = a PR that passes `scripts/check.ps1`, reported "ready to merge" to the Planner.

Each job makes its own worktree in `C:\Users\fuchs\splitkeeb\worktrees\<name>`:
`git -C C:/Users/fuchs/splitkeeb/foxglove worktree add -b <branch> ../worktrees/<name> origin/main`.
Never put one inside the repo, including `.claude/worktrees/`: from a nested `app/`, cargo finds the
main checkout's workspace and refuses to build. `scripts/clean-worktrees.ps1` frees merged ones.
From a terminal the same job is
`claude --agent firmware-engineer`. A subagent role (test-lab, scout, host-health,
code-reviewer, release) is called from any session with the Agent tool.

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
