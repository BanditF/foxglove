# uia_start.py: the Settings window's UI Automation tree has the Start button (#1376)

`scripts/lab/uia_start.py` starts a lab Foxglove on a hidden desktop (`quiet-launch.ps1`, a temp `FOXGLOVE_CONFIG_DIR`, a hard time limit), has `uia_tree.ps1` select the Scenes page through UI Automation (no mouse, no keys) and checks the tree. No window, focus change or tray icon reaches the user's desktop. It is the live counterpart of the unit test `the_scenes_page_has_an_effect_group_a_start_button_and_a_live_region` (`app/src/foxglove/a11y_tree_tests.rs`).

## How to run (On-call)

```
$env:FOXGLOVE_APP_OK = '1'; python scripts/lab/uia_start.py --app <foxglove.exe>
```

Run `scripts/ops/game-check.ps1` first. Exit 0 PASS, 1 FAIL, 2 BLOCKED; the last line is `RESULT ...`. The user's own Foxglove must not be running: `quiet-launch.ps1` refuses (BLOCKED, with its words) and the case never stops it. Stop it yourself first and start it again after (`FOXGLOVE_CONFIG_DIR` is the lab's, never the user's).

| Flag | Default | Description |
|---|---|---|
| `--app PATH` | required | the Foxglove build under test. |
| `--seconds N` | 150 | the lab Foxglove's hard limit, 60..600 (also the cleanup if the case is killed). |
| `--keep-tree FILE` | none | copy the last dumped tree (JSON) there. |

## Checks

| Check | PASS when |
|---|---|
| the tree is not empty | at least 10 nodes under the `Foxglove` window |
| the Scenes page is open | sidebar `Button` "Scenes" is on |
| the Start button | a `Button` named `Start` |
| the Effect group | a `Group` named `Effect` (AccessKit's radio group) |
| the live region | a node whose UIA LiveSetting is polite; BLOCKED, not FAIL, when the .NET UIA client cannot read LiveSetting (then Narrator is the proof) |

If the Scenes page cannot be opened (`-Page Scenes` finds no sidebar entry), the case reads the window as it is: the tree and the live region are checked, Start and Effect are BLOCKED and the missing page is printed.

Waits: one deadline. The first UI Automation read turns the accessibility tree on in that process, so `uia_tree.ps1` is tried up to 4 times, 4 s apart, each bounded by its own `-TimeoutSec` (there is no "window drawn" event to wait on).

Window-free tests: `python -m unittest scripts/test_uia_start.py` (run by `scripts/check.ps1`). `uia_tree.ps1` gained a `liveSetting` field per node (UIA LiveSetting, `null` when unreadable) for the live check; its `-SelfTest` covers it.

## Human-only row

- **Narrator reads the Start button on the Scenes page.** Human only (a screen reader speaks; no agent can hear it): Narrator on (Win+Ctrl+Enter), Settings > Scenes, Tab to Start; it says "Start, button". Also hear that a notice is read once (live region).
