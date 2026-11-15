# labkit.py: shared utilities for app-level board checks

scripts/lab/labkit.py is a stdlib-only library module (no third-party dependencies) that provides helpers for Foxglove's Windows lab tests: input-injection tracking (`injections`, `injecting`, `record_injection`, `user_idle`, `require_user_idle`), temp config creation (`temp_config`), the `App` context manager that launches and restores the build under test, the `Api` client over Foxglove's local named pipe (`connect`), the `Checks` harness (PASS/FAIL/BLOCKED, exit 0/1/2), the `Windows` window inspector (ctypes), live feed parsing (`feed_messages`, `feed_art_facts`) and `restart_parity`. Its only command-line entry point is the idle gate.

Importing labkit raises `KeyError` if the APPDATA environment variable is unset (it builds the real config paths from it at import time).

## Running it from the CLI

    python scripts/lab/labkit.py idle [--min SECONDS]

The positional argument `what` is required and must be `idle` (the only choice).

The `idle` subcommand checks whether the PC has been idle long enough for a soak or typing step to proceed, leaving out input injected by the harness itself.

## Flags

| Flag | Default | Description |
|---|---|---|
| --min SECONDS | 120 | Float; seconds of user-idle time required before the gate passes |

## Exit codes

| Code | Meaning |
|---|---|
| 0 | The PC has been idle at least --min seconds (user input, harness-injected input excluded) |
| 2 BLOCKED | User had input within the threshold; prints a reason and `RESULT BLOCKED` to stdout |

## What it reads and writes

**Reads:** the injection log (the path in FOXGLOVE_LAB_INJECTIONS, else `%LOCALAPPDATA%\Foxglove\lab-injections.log`; the temp directory stands in if LOCALAPPDATA is unset), and the local user's last-input timestamp via the Windows API. As a CLI, `idle` writes no files (recording injections is done by other lab scripts through `injecting`/`record_injection`).

**Writes:** to stdout. On BLOCKED it prints two lines: one explaining why (e.g. `BLOCKED: the PC had user input 45 s ago (needs 120 s idle)`) and the status line `RESULT BLOCKED`. On success it prints `user idle: <seconds> s` (or `user idle: unknown`); when the latest input was discounted as the harness's, it appends ` (later input was the harness's: <what>)`.

## Example

    $ python scripts/lab/labkit.py idle --min 300
    user idle: 487 s (later input was the harness's: H2 Win key in the full-screen game)

    $ python scripts/lab/labkit.py idle --min 120
    BLOCKED: the PC had user input 5 s ago (needs 120 s idle)
    RESULT BLOCKED
