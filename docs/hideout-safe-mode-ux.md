# Hideout safe mode: finding the module that breaks Hideout (UX plan)

Issue #1033, UX gate step for the bisect. Safe mode itself (banner, **Start normally**) is #1032
and [hideout-supervisor-ux.md](hideout-supervisor-ux.md) "Safe mode and finding the culprit"; the
trigger (3 unclean closes in 5 minutes) is built separately. This plan is what happens after the
user clicks **Find the module that breaks Hideout**. Clean-room: written from
[hideout-contract.md](hideout-contract.md) §7.10, the supervisor, tabs and modules UX plans and
public docs; no code under `app/` was read.

Mockups (Python + Pillow, [`design/mockups/hideout_bisect_mockups.py`](design/mockups/hideout_bisect_mockups.py),
the supervisor and tab mockups' helpers and `hideout-shell` DARK tokens; the script fails under 4.5:1):
- `C:\Users\fuchs\splitkeeb\worktrees\_review\h1033\ux-bisect-proposed-rounds.png`: the banner
  link, a round in progress (bar, sidebar, Modules page), between rounds, the tray
- `C:\Users\fuchs\splitkeeb\worktrees\_review\h1033\ux-bisect-proposed-results.png`: found it,
  couldn't find one, closed with nothing on, one module only, the "did it close by itself" question

This replaces the earlier sketch (an always-on-top window asking "Still broken?" each step): the
signal we look for is Hideout closing, which Hideout can see by itself, so the user is only asked
when it can't tell.

## Research

- **Reuse:** the safe-mode banner and its link (#1032); the clean-exit marker the trigger writes on
  Quit (#1033), which is also how a round sees "Hideout closed"; the supervisor's amber family
  (banner, tray dot) and its quiet rules; the Modules page rows and their §7.8 words; **Copy
  diagnostics** and its redaction (#1032, the space-in-path fix in #1033); **Turn off** as the
  Modules page already saves it.
- **Emulate:**
  - **VS Code Extension Bisect:** halves the enabled extensions, keeps a bar saying how many are
    off, three answers ("Good now", "This is bad", "Stop Bisect"), ends with the one extension and
    an offer to disable it, never changes the saved list. We take the bar, the end offer and Stop;
    we drop the per-step question because a close is detected.
  - **`git bisect`:** good / bad / skip, and "roughly N steps left". Our "Round 2 of 4" is the
    same count, shown as an upper bound.
  - **Windows clean boot (msconfig) and modded-game "half your mods" advice:** the same halving
    done by hand across restarts; the user has to keep notes. We keep the notes.
  - **Firefox Troubleshoot Mode, Android safe mode:** nothing is changed until the user picks the
    fix at the end.
- **Not taken:** an always-on-top window (covers a game, and a round is meant to be ordinary use);
  starting the next round by itself after a close (modules, lights and screens would change while
  the user is doing something else); a round per module (n rounds instead of log₂ n).

## Scope

In: Hideout's own window, tray and one notification (`docs/scope.md`: know that the keyboard,
lights and screens work). Works the same for every module and outside tool: the search knows only
"which modules Start normally would start" and "did Hideout close". Out: finding a broken page
inside a module (the supervisor's crash bar and stopped card cover modules that crash on their own),
freezes and slowness, a log viewer, editing the saved on/off list during the search.

## Who takes part

- **In the search:** exactly the modules **Start normally** would start (saved On, installed,
  startable). Modules that need another one go in the same half as it, and count as one entry
  ("Lights with Art"); the catalog's provider links decide this, nothing is per module.
- **Already off:** stay off and are not part of the search. Their row reads "Off · not part of the
  search".
- **Can't start** (needs a newer Hideout, files don't match): not part of the search, row keeps its
  own words.
- **None in the search:** the link is not shown. The banner line reads "No modules are on, so the
  problem is in Hideout itself." with **Copy diagnostics** · Start normally.
- **One in the search:** the link reads **Check whether Art breaks Hideout** and there is one round,
  "Round 1 of 1 · only Art is on".

## How it starts

1. User clicks **Find the module that breaks Hideout** in the safe-mode banner.
2. The banner becomes the round bar (below) for round 1 and the round's modules start at once. No
   intro screen: the bar's own lines say what to do, and **Stop finding** is right there.

## A round

Hideout starts only that round's half. The round bar sits at the top of every Hideout page (the
Modules page and the started modules' pages), in the safe-mode amber:

> **Finding the module that breaks Hideout · Round 2 of 4**
> 3 of 6 modules are on: Lights, Art, Chat.
> Use Hideout as usual. If it closes, it'll pick up here.
> This round ends in 4 min if Hideout stays open.
> **It's fine** · It closed · Stop finding

- **Round count:** "of 4" is the most rounds it can take (log₂ of the entries, plus one round with
  the last suspect alone when it was never on by itself in a round that closed). It only ever drops;
  when it finishes early it just shows the result.
- **Which modules are on:** named up to three, then "Lights, Art, Chat and 2 more"; the full list is
  the Modules page.
- **Sidebar:** the started modules as usual; the others muted with the tooltip and accessible name
  "Art, off for this round". Their pages say "Off for this round" with no Turn on.
- **Modules page rows:** "On in this round" / "Off for this round" / "Off · not part of the search".
  Turn on/off buttons are locked during the search, with a visible line under the list and an
  accessible description: "Locked while finding the module. Stop finding to change it."
- **The timer** counts only while Hideout is open and the PC is awake; it pauses for sleep and
  resumes. Repainted once a minute, only while visible.

### How a round ends

| What happens | Counted as | Next |
|---|---|---|
| Hideout closes without a Quit (crash, killed, restart by itself) | closed | on the next start: between-rounds card |
| 5 minutes pass with Hideout open | fine | next round starts at once, bar says "Round 2 was fine." |
| User clicks **It's fine** | fine | same as above |
| User clicks **It closed** (the hub came back by itself and missed it, or a window vanished) | closed | between-rounds card |
| User quits Hideout (tray Quit) or Windows shuts down cleanly | not counted | on the next start: "Round 2 was paused" · **Go on with round 2** · Stop finding |
| Exit Hideout can't read (no marker, but Windows restarted) | asked | "Hideout closed during round 2. Did it close by itself?" **It closed** · I closed it · Stop finding |

One close is enough to count a round; the 3-in-5-minutes rule is for entering safe mode and is
off while a search runs (a close during a round never starts a second safe mode on top).

### Between rounds (after a close)

Hideout starts with nothing on (safe mode) and the bar reads:

> **Round 2: Hideout closed.** That narrows it to Lights, Art or Chat.
> **Start round 3** · Stop finding

One click starts the next half. Nothing starts by itself after a close, so a game or a call is never
interrupted by modules coming back. The tray says the same (below).

## The result

Shown in the bar's place on the Modules page, focus on the primary button. The other modules are
not started yet; the choice starts them.

**Found it** (a round closed and the suspect was on in it, alone or confirmed):
> **Art seems to make Hideout close.**
> Hideout closed in rounds 1 and 3 with Art on, and stayed open without it.
> **Turn off Art** · Copy diagnostics · Start normally anyway

- **Turn off Art:** saves Art off, leaves safe mode, starts every other saved-On module. The row
  then says "Off" with Turn on, as always. When an older version is kept, the line "Art 0.4.2
  worked before." and a fourth link **Go back to Art 0.4.2** appear (the supervisor's ladder step 3).
- **Copy diagnostics:** the rounds (which entries were on, closed or fine, exit kinds), redacted as
  #1032; never the user's file paths.
- **Start normally anyway:** leaves safe mode with the saved list unchanged.

**Couldn't find one module** (every round fine, or the last suspect stayed up alone):
> **Hideout stayed open in every round.**
> It may need two modules on together, or the problem has passed.
> **Start normally** · Copy diagnostics · Try again

**Closed with nothing on** (a close while no module was started, e.g. between rounds):
> **Hideout closed with no modules on.**
> The problem is in Hideout itself, not a module.
> **Copy diagnostics** · Start normally anyway

One notification, only when the result is ready and the window is closed: "Found the module" /
"Art seems to make Hideout close." / Open Hideout. No notification per round.

## Stopping

**Stop finding** (bar, tray menu, between-rounds card, the paused and asked questions) stops the
round's modules and returns to the safe-mode banner with the line "Stopped finding. Nothing was
changed." No confirmation: nothing is saved, and the banner still has **Start normally**. Stop goes
back to safe mode, not to a normal start, because the cause is still unknown.

The search never edits the saved on/off list; only **Turn off <Module>** or **Go back** at the end
does. Its progress (round, entries, results) survives restarts and is cleared by the result choice
or Stop.

## Tray during the search

- Icon keeps the amber safe-mode dot.
- Tooltip and status line: "Hideout · finding the module (round 2 of 4)"; between rounds "Hideout ·
  round 2 closed, start round 3"; at the result "Hideout · found the module".
- Menu: the status line, **Open Hideout**, then **Start round 3** (between rounds only), **Stop
  finding**, Quit. No It's fine in the tray: answering needs the bar's context.

## Keyboard and screen reader

- The bar is a landmark region named "Finding the module that breaks Hideout", first in tab order
  on every page: It's fine, It closed, Stop finding.
- A polite live region announces once per change: round start ("Round 2 of 4 started. Lights, Art
  and Chat are on."), round end ("Round 2 was fine."), result. The minute timer is not announced.
- The between-rounds card, the paused and asked questions and the result take focus on their
  primary button. Enter = primary. Esc does nothing (Stop is a deliberate click, and a stray Esc
  must not end a long search).
- Locked toggles keep focus and announce the lock line as their description (not a hover-only
  tooltip; #1033 follow-up 5).

## Wording

| Where | Text |
|---|---|
| Banner link | "Find the module that breaks Hideout" / one entry: "Check whether <Module> breaks Hideout" |
| Bar title | "Finding the module that breaks Hideout · Round <k> of <n>" |
| Bar lines | "<a> of <b> modules are on: <names>." / "Use Hideout as usual. If it closes, it'll pick up here." / "This round ends in <m> min if Hideout stays open." |
| Bar buttons | It's fine · It closed · Stop finding |
| Round fine | "Round <k> was fine." |
| After a close | "Round <k>: Hideout closed." / "That narrows it to <names>." / Start round <k+1> |
| Paused | "Round <k> was paused." / Go on with round <k> |
| Asked | "Hideout closed during round <k>. Did it close by itself?" / It closed · I closed it |
| Rows | "On in this round" / "Off for this round" / "Off · not part of the search" |
| Lock line | "Locked while finding the module. Stop finding to change it." |
| Found | "<Module> seems to make Hideout close." / "Hideout closed in rounds <list> with <Module> on, and stayed open without it." |
| Not found | "Hideout stayed open in every round." / "It may need two modules on together, or the problem has passed." |
| Hideout itself | "Hideout closed with no modules on." / "The problem is in Hideout itself, not a module." |
| Stopped | "Stopped finding. Nothing was changed." |
| Tray | "Hideout · finding the module (round <k> of <n>)" |

"Seems" stays: one close can be chance. Never shown: exit codes, marker files, state ids, "bisect".

## Hand-off to app-engineer (#1033)

Behaviour: entries = Start normally's set grouped by provider links; halving with the upper-bound
count above; one close (no clean marker) or **It closed** = closed, 5 minutes open or **It's fine**
= fine, clean Quit = paused, unreadable exit = asked; progress persisted outside the saved on/off
list; next round only on click after a close; the 3-in-5 trigger off during a search.

Acceptance (lab, window-free where possible): with 6 entries and one module that panics the hub,
the result names it in at most 4 rounds and the saved list is unchanged until the result choice;
with no culprit, every round fine → "Hideout stayed open in every round"; a clean Quit mid-round
resumes the same round; a close between rounds → "closed with no modules on"; Stop at each state
returns to the banner with nothing saved; a string test over the bar and result formatters finds
no exit code, path or "bisect".

## Open questions for the user (via the Hub)

1. Where a round shows: **a bar at the top of Hideout's pages + the tray** / a small always-on-top
   window (the earlier sketch). Image: `ux-bisect-proposed-rounds.png`.
2. Round length when nothing happens: **5 minutes** (the trigger's window) / 10 minutes.
3. After a close, the next round: **starts on one click (Start round 3)** / starts by itself when
   Hideout comes back. Image: `ux-bisect-proposed-rounds.png`.
