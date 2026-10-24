# Release-gate report format

`foxglove-lab run` and `scripts/release-test.ps1` write one folder per run under
`lab-reports/<UTC>-<git sha>[-release]/`:

| File | For | Notes |
|---|---|---|
| `report.html` | people | Self-contained. The first screen is the summary: verdict, areas, firmware build of each half, app version, case table with links to captures. Details follow, one section per case. |
| `report.json` | the release agent, scripts | Schema `foxglove-lab-report`, version 2: [report.schema.json](report.schema.json). |
| `junit.xml` | CI | `<testsuites>` with one `<testsuite>` per area. Properties carry the suite, verdict, firmware builds, app version and git SHA. |
| `*.png` | evidence | Screen captures, portrait 68×160 scaled 4×, as a person sees the screen; diffs mark differing pixels red. |
| `preflight.json`, `usb.txt`, `summary.txt` | release gate only | From `scripts/release-test.ps1`: UF2 preflight, USB state of both halves, one-line verdict. |

## Verdict

- **PASS** only if no case failed and no case tagged `release` is BLOCKED.
- **BLOCKED** means a precondition outside the test isn't met, for example:
  - the firmware lacks a capability bit;
  - the right half's relay isn't up;
  - a manual step in a non-interactive run;
  - pushed art that a test would destroy.

  A blocked release case blocks the release; it's never counted as a pass.
- **Exit code:** 0 when the verdict is PASS and every non-manual case passed.

## Areas

A case's area is its first tag other than `release`: `smoke`, `positions`, `screens`, `app`, and so on. Each area is:
- FAIL if any of its cases failed;
- BLOCKED if one of its `release` cases is blocked;
- PASS otherwise.

## Firmware and app

- `firmware.left` / `firmware.right`: version, profile (`dev` with test hooks, `ga` without) and short SHA, from D7 09 BUILD_INFO.
- `null` when the firmware doesn't report it (no HELLO bit 12) or the right half isn't reachable. A release report for GA must show both halves `ga` with the release SHA.
- `app`: the running `foxglove.exe` path and product version, or `running: false`.

## Stability

- Readers check `schema` and `version`.
- Within version 2, fields may be added but are never removed or renamed. A breaking change bumps `version` and updates the schema file and this page in the same PR.
- The schema's `required` list is checked by `crates/foxglove-lab` tests.
