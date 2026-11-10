# Hideout supervisor: what the user sees (UX plan)

Issue #966 (H2.5, light supervisor), UX gate step. The supervisor restarts, stops and takes back
modules (contract §7.7, §7.9); this plan is the crash bar and the module states it causes, and
where each shows. Clean-room: written from [hideout-contract.md](hideout-contract.md) §7.7–7.10,
[hideout-tabs-ux.md](hideout-tabs-ux.md) and public docs; no code under `app/` was read.

Mockups (Python + Pillow, [`design/mockups/hideout_supervisor_mockups.py`](design/mockups/hideout_supervisor_mockups.py),
the tab mockups' helpers and `hideout-shell` DARK tokens; the script fails under 4.5:1, lowest 5.4:1):
- `C:\Users\fuchs\splitkeeb\review\hideout-supervisor\ux-supervisor-proposed-states.png`: sidebar
  marks, the crash bar, the stopped card, tray menu and icon, the notification, the rollback note
- `C:\Users\fuchs\splitkeeb\review\hideout-supervisor\ux-supervisor-proposed-safe-mode.png`: the
  safe-mode window and the two steps of finding the module that breaks Hideout

## Research

- **Reuse:** the §7.8 state list and its words (one list for panel header, Modules page and tray),
  the tab strip's marks and stopped card (hideout-tabs-ux.md: orange ring, red warning mark,
  greyed last page), `state_words` from #956 for the plain names, `hub.notify`'s notification path,
  the tray status line (§2, root cause only). Nothing new is invented where §7.8 has a word.
- **Emulate:**
  - **Chrome's "Aw, Snap!" / crashed tab:** the page stays where it was, says what happened in one
    line and offers one main button (Reload = Start again).
  - **VS Code's extension host:** "Extension host terminated unexpectedly 3 times within the last
    5 minutes" and it stops restarting after a bound; a notification with one action. Our crash
    bar is its in-page, quieter twin.
  - **systemd `Restart=` with `StartLimitBurst`:** restart with backoff, then give up and say so;
    the user starts it again by hand, which resets the count.
  - **Windows Safe Mode / Firefox Troubleshoot Mode, `git bisect`:** start with nothing loaded,
    one banner to go back, halve to find the culprit, never change saved settings.
- **Not taken:** a modal on crash (interrupts a game), a countdown in the tray (noise), automatic
  "Start with default settings" (loses settings without asking).

## Scope

In: Hideout's own window, tray and notifications (`docs/scope.md`: one place to see the keyboard,
lights and screens and know they work). Every module and outside tool gets the same states; nothing
is per module. Out: per-module health views (modules own those), resource caps the user can set,
log viewers beyond Open log, any wording inside a module's own page.

## States: where and what

Surfaces: **S** sidebar module list, **T** its tab (#953), **P** the page area (crash bar or card),
**M** the Modules page row, **Tr** tray (icon, status line, menu), **N** notification.

| State (§7.8 id) | S and T | P | M | Tr | N |
|---|---|---|---|---|---|
| Running (`running`) | nothing | the page | "On" | nothing | — |
| Starting (`starting`) | orange ring | cached page, header "Starting…" | "Starting…" | nothing | — |
| Crashed, restarting (`restarting`) | orange ring; tooltip "Restarting… (2 of 5)" (once per crash; the countdown is the bar's) | **crash bar** (below) over the greyed last page | "Restarting in 4 s (2 of 5)" | nothing | — |
| Stopped after 5 crashes (`crash-stopped`) | red warning mark | **stopped card** (below) over the greyed last page | "Stopped after 5 crashes" + **Start again** | red dot on the icon; menu line "Art stopped" / "Open Art to start it again" | once per module per sign-in |
| Updating (`updating`) | orange ring; tooltip "Updating…" | header line "Updating… Your place on this page is kept." | "Updating…" | nothing | — |
| Off (`off`) | muted label, no mark | "Off" + **Turn on** | "Off" + Turn on | nothing | — |
| Needs a newer Hideout (`needs-hub`) | red warning mark | card "Needs a newer Hideout" + **Update Hideout**, Go back to <previous> | same words + Update Hideout | red dot; menu line "Art needs a newer Hideout" | once, at the update that caused it |
| Safe mode (§7.10) | only Modules and Settings in the sidebar | banner (below) on the Modules page | each enabled module "On · not started in safe mode" | amber dot; tooltip "Hideout · safe mode"; menu item **Start normally** | once at start |

Rules shared by all rows:
- **One mark per entry**, the tabs doc's precedence: stopped mark > ring > nothing. The red mark
  always has its words next to it (tooltip, accessible name: "Art, stopped after 5 crashes").
- **Root cause once** (§7.8): a module waiting on a stopped one shows "Waiting for Art" with
  **Go to Art**, never its own crash bar; the tray names only Art.
- **Tray quiet rule:** the icon's dot is only for states that need the user (stopped, needs a
  newer Hideout, safe mode). Restarts, updates and starts never touch the tray.
- **Notifications** only for: stopped after 5 crashes, automatic rollback, needs a newer Hideout,
  safe mode. One per module per sign-in (§7.7). Never for a restart.

### The crash bar (restarting)

Amber bar at the top of the module's page and of its pop-out, only while `restarting`:

> **Art stopped unexpectedly. Restarting in 4 s (2 of 5).**
> Your place on this page is kept. · Open log

- The seconds are the §7.7 backoff step (1, 2, 4 … 60 s); the bar shows the deadline as a
  countdown, repainted once a second only while the page is visible, and never past 60 s.
- "(2 of 5)" counts crashes in the current ten-minute window (§7.7's crash bar).
- When the module is back the bar goes without a message. A crash caused by an oversized message
  (`rpc.frame-too-large`, v1.2) reads exactly the same; the id is only in the log.
- One link, **Open log**. No "Restart now": the backoff exists so a crashing module can't spin.

### The stopped card (crash-stopped)

> ⚠ **Stopped after 5 crashes**
> Art crashed 5 times in 10 minutes.
> **Start again** · Start with default settings · Go back to 0.4.2
> Copy diagnostics

### Recovery ladder (one action per step)

1. **Start again** (primary). The count starts fresh. If Art runs for ten minutes, done.
2. If it stops again in the same sign-in, the card returns with **Start with default settings**
   as the primary button and the line "Starting again didn't help last time." That renames the
   settings folder (`settings.bad-<time>`, §7.8) and starts Art.
3. If that stops too: **Go back to <previous>** is primary ("Art 0.4.2 worked before.").
   Hidden when there is no previous version.
4. Last: **Copy diagnostics** (state id, exit codes, error ids, log tail) and **Turn off Art**.

Each step is one click on the same card; the card never asks two things at once.

### Automatic rollback

When Art is stopped within ten minutes of an update, Hideout goes back by itself (§7.8) and says
so, on the page (dismissable note) and as the notification:

> **Went back to Art 0.4.2** / 0.5.0 crashed 5 times after the update. / **OK** · Open log

The update shows again in Modules as "Update available" with the line "0.5.0 crashed here before."

### Safe mode and finding the culprit

Banner on the Modules page, the only page that opens:

> **Safe mode**
> Hideout closed 3 times in 5 minutes, so no modules were started.
> **Start normally** · Find the module that breaks Hideout

- **Start normally** clears the flag and starts the enabled modules. One click.
- **Find the module that breaks Hideout** runs §7.10's halving. Each step is a small always-on-top
  window with one question and two answers:
  - "Step 1 of about 3" / "Half your modules are off for now. Use Hideout as before." /
    "Is the problem still there?" / **Still broken** · Works now · Stop
  - End: "Found it" / "Art seems to break Hideout." / "Your other modules are back on." /
    **Turn off Art** · Go back to Art 0.4.2
- **Stop** at any step puts every module back as it was saved. Bisect never edits the saved list;
  only the last choice does.
- Turning a module off from the safe-mode Modules page is saved as usual.

### Hub restart and adoption

Taking modules back after the hub restarts (§7.9) shows nothing: no crash bar, no count, no
notification. Pages draw from the panel cache with "Starting…" until each module answers. A module
not back within the 30-second grace period is started fresh and reads "Starting…".

## Never shown

State ids (`crash-stopped`), error ids (`rpc.frame-too-large`, `rpc.*`), exit codes, pids, exe
hashes, Job Object or AppContainer words, backoff tables, "initialize". They go to Copy
diagnostics and the module's log only. A failed pid or exe-hash check at `initialize` (#959) shows
as `install-failed`: "Couldn't start Art" / "Its files don't match what was installed." /
**Try again** · Open log.

## Wording

| Where | Text |
|---|---|
| Crash bar | "<Module> stopped unexpectedly. Restarting in <n> s (<k> of 5)." / "Your place on this page is kept." / Open log |
| Stopped card | "Stopped after 5 crashes" / "<Module> crashed 5 times in 10 minutes." |
| Ladder lines | "Starting again didn't help last time." / "<Module> <version> worked before." |
| Tray menu | "<Module> stopped" / "Open <Module> to start it again" |
| Notification | "<Module> stopped" / "It crashed 5 times in 10 minutes." / Start again · Open <Module> |
| Rollback | "Went back to <Module> <old>" / "<new> crashed 5 times after the update." |
| Safe mode | "Safe mode" / "Hideout closed 3 times in 5 minutes, so no modules were started." |
| Needs hub | "Needs a newer Hideout" / Update Hideout |

#956's banner said "Stopped after repeated crashes"; this plan uses §7.8's "Stopped after 5
crashes" everywhere (state_words should change to match, or both to the other; see question 3).

## Keyboard and screen reader

- The crash bar is a polite live region, announced once per crash ("Art stopped unexpectedly,
  restarting"), not every second. The countdown is not announced.
- The stopped card's primary button takes focus when the user opens the page; Esc does nothing.
- Bisect windows: Enter = the first answer, Esc = Stop.

## Hand-off to app-engineer (H2.5)

Acceptance: each table row is reachable in the lab (kill a module: ring + crash bar with 1, 2, 4,
8 s; five kills: card, tray dot, one notification; Start again resets; hub kill: no bar, modules
back); no id from "Never shown" appears in any drawn string (a string test over `state_words` and
the bar's formatter); countdown repaint stops when the page is hidden.

## Open questions for the user (via the Hub)

1. Notification for "stopped": **Start again + Open <Module>** buttons / Open only (one click to
   the card, which has the ladder).
2. Crash bar colour: **amber, quiet** / red like the stopped card.
3. Stopped wording: **"Stopped after 5 crashes"** (§7.8, tabs doc) / "Stopped after repeated
   crashes" (#956's banner).

Open for the contract owner (not the user): what a clean exit (code 0, §7.7 "a stop, not a crash")
looks like while the module is still on; §7.8 has no state for it. Proposal: treat it as `starting`
on next activation and show nothing until then.
