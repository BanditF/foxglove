#!/usr/bin/env bash
# Planner merge helper: merge one PR after the local gate passes.
#
#   scripts/merge-pr.sh <worktree> <PR> <base>
#   scripts/merge-pr.sh fox-mm 123 main
#   scripts/merge-pr.sh fox-rm 124 release/0.1
#
# Steps: retarget the PR to <base> if it is stacked on another branch, check out the PR
# head detached in C:\Users\fuchs\splitkeeb\worktrees\<worktree>, merge <base> in, run
# scripts/check.ps1 (which takes its own build slot), push the merge to the PR branch,
# `gh pr merge --merge`, then run clean-worktrees.ps1 -Apply.
#
# Rules: one job per worktree at a time; fox-mm for main and fox-rm for release/0.1, each
# with its own CARGO_TARGET_DIR; merge only PRs their author marked "ready to merge".
set -u

wt=${1:?worktree}; n=${2:?PR}; base=${3:?base}
R=BanditF/foxglove
WT_ROOT=/c/Users/fuchs/splitkeeb/worktrees
LOGS=${MERGE_PR_LOGS:-$WT_ROOT/.merge-logs}
mkdir -p "$LOGS"
cd "$WT_ROOT/$wt" || { echo "no worktree $WT_ROOT/$wt"; exit 1; }
export CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-$WT_ROOT/target-${wt#fox-}}

br=$(gh pr view "$n" --repo $R --json headRefName --jq .headRefName)
pb=$(gh pr view "$n" --repo $R --json baseRefName --jq .baseRefName)
if [ "$pb" != "$base" ]; then
  echo "#$n base $pb -> $base"
  gh pr edit "$n" --repo $R --base "$base" >/dev/null || exit 1
fi

git fetch -q origin && git checkout -q --detach "origin/$br" || exit 1
if git merge -q --no-edit "origin/$base" 2>&1 | grep -qi conflict; then
  echo "#$n CONFLICT"; git merge --abort; exit 1
fi

if ! timeout 2400 powershell -ExecutionPolicy Bypass -File scripts/check.ps1 > "$LOGS/check-$n.log" 2>&1; then
  echo "#$n CHECK FAILED (log: $LOGS/check-$n.log)"
  grep -E "\.\.\. FAILED|^error" "$LOGS/check-$n.log" | head -4
  exit 1
fi

git push -q origin "HEAD:refs/heads/$br" 2>&1 | grep -v '^remote'
gh pr merge "$n" --repo $R --merge --match-head-commit "$(git rev-parse HEAD)" 2>&1 | tail -1
st=$(gh pr view "$n" --repo $R --json state --jq .state)
echo "#$n $st"
if [ "$st" = MERGED ] && [ -f scripts/clean-worktrees.ps1 ]; then
  powershell -ExecutionPolicy Bypass -File scripts/clean-worktrees.ps1 -Apply > "$LOGS/clean-$n.log" 2>&1
fi
[ "$st" = MERGED ]
