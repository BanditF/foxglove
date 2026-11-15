# check_hub_deps.py: Hub dependency licence gate (#932, contract §11)

The Hideout hub is MIT and clean-room, so every package in the dependency graph of the hub crates must carry a permissive licence from an exact allowlist, and no path dependency may point under `app/` (GPL-3.0).

## Running it

```
python scripts/check_hub_deps.py
python scripts/check_hub_deps.py --metadata saved.json
python scripts/check_hub_deps.py --platform x86_64-pc-windows-msvc
```

The first form runs `cargo metadata --format-version 1 --locked --all-features --filter-platform <platform>` with the repository root as the working directory (`cwd`). `--locked` means `Cargo.lock` must already be up to date; cargo will not change it. The second form reads a saved `cargo metadata` JSON instead of running cargo.

## Flags

| Flag | Default | Description |
|---|---|---|
| `--metadata PATH` | (none) | A saved `cargo metadata` JSON file instead of running cargo |
| `--platform TRIPLE` | `x86_64-pc-windows-msvc` | The target triple to resolve the graph for (ignored with `--metadata`) |

The graph starts at the two hub roots (`hideout-shell`, `hideout`) and follows normal and build dependencies; the roots' own dev-dependencies count too, other packages' dev-dependencies do not. The hub's GUI dependencies are Windows-only, which is why Windows x64 is the default platform.

## Licence allowlist

The allowlist is an exact set of SPDX identifiers, not a family match:

`MIT`, `MIT-0`, `Apache-2.0`, `Apache-2.0 WITH LLVM-exception`, `BSD-2-Clause`, `BSD-3-Clause`, `ISC`, `Zlib`, `Unicode-3.0`, `Unicode-DFS-2016`, `Unlicense`, `0BSD`, `BSL-1.0`, `CC0-1.0`.

Copyleft of any strength (GPL, LGPL, AGPL, MPL, EPL, OFL) is not on it. A licence is read as an SPDX expression: an `OR` passes when one side passes, an `AND` when both do, and the old `A/B` spelling counts as `OR`. A trailing `+` is ignored. A package with no `license` field fails.

Local crates (those with no registry source) must read exactly `MIT`; anything else, even an allowlisted licence, is a problem.

## Exit codes

| Code | Meaning |
|---|---|
| 0 | No problems; only the summary line is printed |
| 1 | One or more problems; one `hub deps:` line per problem (sorted), then the summary |

## What it reads and writes

**Reads:** the workspace's Cargo metadata (from cargo or the saved JSON). No other files.

**Writes:** to stdout only: one `hub deps: <package> <version>: <problem>` line per problem, then `hub deps: N packages checked, M problem(s)`.

Problems include a missing licence field, a licence not on the allowlist, an invalid SPDX expression, a local crate that is not exactly `MIT`, and a path dependency under `app/`. Missing hub roots in the workspace raise an error instead.

## Example

```
$ python scripts/check_hub_deps.py --metadata saved.json
hub deps: other-lib 0.5.0: licence 'GPL-3.0' is not on the hub allowlist
hub deps: some-crate 1.2.3: no licence field (licence file LICENSE.txt)
hub deps: 47 packages checked, 2 problem(s)
```
