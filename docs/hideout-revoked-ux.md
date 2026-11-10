# Hideout: a module version its publisher withdrew (UX plan)

Issue #1026, UX gate step. After #1025 the registry knows which running versions an accepted
catalog revoked (with the catalog's `reason`), and the router ends their leases and publishes the
hub fact `module.<id>.revoked = {version, reason}` (contract §7.8, §8.5, v1.6). This plan is what
the user sees when the supervisor then stops that module. Clean-room: written from
[hideout-contract.md](hideout-contract.md) §7.8, §8.5, §12.4–12.5, PR #1025's description,
[hideout-supervisor-ux.md](hideout-supervisor-ux.md), [hideout-tabs-ux.md](hideout-tabs-ux.md),
[hideout-plugin-modules-ux.md](hideout-plugin-modules-ux.md) and public docs; no code under `app/`
was read.

Mockups (Python + Pillow, [`design/mockups/hideout_revoked_mockups.py`](design/mockups/hideout_revoked_mockups.py),
the earlier mockups' helpers and `hideout-shell` DARK tokens; the script fails under 4.5:1, lowest
5.4:1, and checks the note-cleaning rule):
- `C:\Users\fuchs\splitkeeb\worktrees\_review\h1026\ux-revoked-proposed-states.png`: the page
  card, sidebar and tab mark, the tile, the Modules rows, tray menu and icon, the notification
- `C:\Users\fuchs\splitkeeb\worktrees\_review\h1026\ux-revoked-proposed-card.png`: the card with
  no note, with a long note cleaned and capped, while checking, after "nothing newer", and with
  no previous version

## Research

- **Reuse:** the stopped card, the red warning mark and its precedence, the tray quiet rule and
  dot, the notification path, the tile's "Open only" rule (hideout-plugin-modules-ux.md decision
  3), "Root cause once", Copy diagnostics, the §7.8 "Ways out" column. The update path
  (`update`, `update-waiting`) and rollback (§7.4, refused for a revoked version) already exist;
  this state only points at them.
- **Emulate:**
  - **Firefox's add-on blocklist (hard block):** the add-on is disabled, can't be turned back on,
    and the add-ons page says so in one line. We take "can't be turned on" and the in-place line.
  - **Chrome's "This extension was turned off because…" (Web Store takedown):** a fixed line by
    the browser, never the developer's own words in the alert. We take the browser-owned title.
  - **npm `deprecate` / crates.io yank messages:** the publisher's message is shown as written,
    as plain text, beside the tool's own words. We take "shown as written, labelled as theirs".
  - **Windows Security / Play Protect "App removed":** one notification when it happens, not at
    every sign-in.
- **Not taken:** a modal (interrupts a game), the publisher's text in the notification or tray
  (outside the window, no label, no cap), going back automatically (runs other code without a
  click), "for safety" when the catalog gave no reason (Hideout doesn't know why).

## Scope

In: Hideout's window, tray panel, tray menu and notifications for any catalog module whose running
version is revoked. Same for every module; nothing per module. Out: yanked versions (they keep
running and show nothing on a running module; Modules may say "Not offered any more" in its
install list, not here), sideloads and lab modules (no catalog, never revoked), the catalog
editor or anything a publisher sees, uninstalling.

## The state

- **State id:** `revoked` (new §7.8 row, additive; ask for it in #1026's contract change). Set by
  the supervisor when it stops a module because `Registry::revoked()` lists it, at hub start
  (shallow verify refuses the launch, §12.5) and on catalog acceptance. Cleared when another
  version of the module connects, the same moment `module.<id>.revoked` clears.
- **UI name:** **Withdrawn** (short, for the Modules row and tooltips). Card title:
  "<Module> <version> was withdrawn". The version is in the title because only that version is.
- **What happens to the module:** stopped (drain and stop, §7.6), leases already ended with
  `"revoked"`, panels hidden, files, data, settings, secrets and grants kept (as **disable**,
  §7.4), so an update picks up where it left off. It stays **enabled** unless the user turns it
  off, so a fixed version starts by itself once installed.
- **Can't start:** no Start again, no Turn on. A Turn on that arrives some other way (settings
  file, MCP, CLI) is refused with `trust.revoked` and the state stays Withdrawn. Rollback to the
  withdrawn version is refused (§7.4).
- **Lighting modules** add the line "Your keyboard is back on its own lighting." (as stopped).

§7.8 row to add:

| State id | UI name | Reason line | Ways out |
|---|---|---|---|
| `revoked` | Withdrawn ("<Module> <version> was withdrawn") | "Its publisher withdrew this version, so Hideout stopped it." + the publisher's note | **Check for updates**, Go back to <previous>, Turn off <Module>, Copy diagnostics |

## Where it shows

Surfaces as in hideout-supervisor-ux.md: **S** sidebar, **T** tab, **P** page, **Ti** tray tile,
**M** Modules row, **Tr** tray, **N** notification.

| Surface | What |
|---|---|
| S and T | red warning mark (stopped family, same precedence: stopped mark > ring > nothing). Tooltip and accessible name: "Art, withdrawn by its publisher" |
| P | the **withdrawn card** (below) over the greyed last page; also in the module's pop-out |
| Ti | red mark, red outline, "Version withdrawn" / "by its publisher", **Open** (to the card) |
| M | "Withdrawn · 0.5.0" + **Check for updates** (primary). After Turn off: "Off · 0.5.0 was withdrawn" + Check for updates (no Turn on) |
| Tr | red dot on the icon; menu line "Art was withdrawn" / "Open Art to check for updates". Root cause only. The dot goes when the module is updated, gone back or turned off |
| N | once per withdrawn version, when it happens: "Art 0.5.0 was withdrawn" / "Its publisher withdrew it, so Hideout stopped it." / **Check for updates** · Open Art |

- **Notification once per version, not per sign-in:** a crash is an event each sign-in; a
  withdrawal is one event that stays true. Later sign-ins show the dot and the card, no second
  notification. Remembered in Hideout's own state (module id + version), not the module's.
- **Root cause once:** a module that waits on Art reads "Waiting for Art" with **Go to Art**, never
  a card of its own.

## The withdrawn card

> ⚠ **Art 0.5.0 was withdrawn**
> Its publisher withdrew this version, so Hideout stopped it.
> Publisher's note:
> "This build can send your screen captures to a test server. Please update to 0.5.1."
> **Check for updates** · Go back to 0.4.2 · Turn off Art
> Copy diagnostics

No reason in the catalog (empty, or empty after cleaning): the "Publisher's note" lines are left
out. No "for safety": Hideout doesn't know the reason, and the fixed line already says enough.

### Buttons

- **Check for updates** (primary, focused). Fetches this module's source's catalog now (the same
  path as the routine refresh, one fetch, no retry loop). The button reads "Checking…" and is
  disabled while it runs; the status line under the reason says "Checking Art's source…".
  Outcomes:
  - A newer version that isn't withdrawn: the normal update starts; the card gives way to
    "Updating…" (or "Update waiting for you" with **Review** when it asks for a new permission).
  - Nothing newer: status "No new version yet. Checked just now." The card stays. A later
    catalog with a fix updates as any update does; nothing to watch by hand.
  - The source can't be reached: status "Couldn't check. Try again later."
- **Go back to <previous>** (secondary). Shown only when a previous version is kept and isn't
  itself withdrawn (yanked still runs, §12.4). The normal rollback (shallow verify first). It's
  the user's click, never automatic.
- **Turn off <Module>**: **disable** (§7.4). The red dot and the tray line go; the page shows the
  Off card "Off · 0.5.0 was withdrawn" with **Check for updates** instead of Turn on.
- **Copy diagnostics** (link): copies the block below.

### The publisher's note (decision 1)

The catalog's `reason` is publisher text. Hideout draws it on the card only, as plain text:
1. **Plain text only.** Never parsed as markup, Markdown or rich text; a URL is drawn as text and
   is never a link. The label "Publisher's note:" and the quotes mark it as theirs, not Hideout's.
2. **Cleaned:** control and format characters are dropped (C0/C1, bidi overrides and isolates,
   zero-width), every run of whitespace (newlines, tabs) becomes one space, the ends are trimmed.
3. **Capped at 160 characters** (whole characters, never mid-code point; an ellipsis "…" when cut).
   About two lines on the card. The full text is in Copy diagnostics.
4. **Only on the card.** Never in the tile, sidebar tooltip, tray menu, notification, Modules row
   or a log line's summary. These are outside the window or too small to carry a label.
5. The fact is the hub's (`module.*` facts can't be written by modules), so a module can't put
   words on its own card.

The mockup script carries the cleaning rule as a reference (`clean_note`) and asserts it.

### Copy diagnostics

Plain text, one item per line, nothing secret:
- `state: revoked`, module id, the withdrawn version, the previous version (and whether it's
  withdrawn or yanked)
- source name, catalog `seq` and when it was accepted, the revocation `action`
- the full `reason` as written, with control and format characters escaped (`\u{202e}`)
- when Hideout stopped the module, how many leases ended with `"revoked"`
- the last Check for updates: time and outcome (found <version> / nothing newer / couldn't reach,
  with the error id)

## Never shown

`revoked`, `trust.revoked`, catalog URLs, `seq`, signatures, lease ids, "lease.closed", "catalog".
They go to Copy diagnostics only. The word in the UI is "withdrawn", for both the state and the
action ("its publisher withdrew").

## Keyboard and screen reader

- The card is a region named by its title. When the user opens the page, focus goes to
  **Check for updates**; Tab order: Check for updates, Go back, Turn off, Copy diagnostics. Esc
  does nothing.
- When the state arrives while the page is visible: one polite announcement, "Art 0.5.0 was
  withdrawn by its publisher and stopped". The note is read when focus or the reader reaches it,
  as "Publisher's note: …", never in the announcement.
- The status line is a polite live region ("Checking…", "No new version yet").
- The red mark is never the only signal: the tooltip, accessible name and tile words say it.

## Hand-off to app-engineer (#1026)

Acceptance:
- In the lab, a catalog that revokes a running module's version: module stopped, card on its page
  and pop-out, red mark in sidebar and tab, the tile as drawn, tray dot and line, exactly one
  notification. Restart the hub or sign in again: card and dot, no second notification.
- Turn on is not offered; a Turn on request is refused with `trust.revoked`.
- Check for updates with a fixed version in the catalog updates and the card goes; with none, the
  status line reads "No new version yet. Checked just now."
- Go back appears only when a previous non-withdrawn version is kept.
- A string test: the cleaning rule (bidi override, newline, zero-width, 300-character note → at
  most 160 characters with "…"), and no "Never shown" word in any drawn string.
- The note never appears in the notification, tray, tile or tooltip strings.

## Decisions (designer's picks, for the Hub)

1. The publisher's note: **on the card, plain text, cleaned, capped at 160 characters, full in
   Copy diagnostics** / only in Copy diagnostics / on the card uncapped. (#1025's open question.)
   Image: `ux-revoked-proposed-card.png`.
2. Ways out: **Check for updates (primary), Go back to <previous> when one is kept, Turn off** /
   #1025's two (Check for updates, Turn off) / go back automatically. Image:
   `ux-revoked-proposed-states.png`.
3. Notification: **once per withdrawn version, when it happens** / once per sign-in while it stays
   withdrawn (like "stopped"). Image: `ux-revoked-proposed-states.png`.

Wording change from #1025: no-reason line "Its publisher withdrew this version, so Hideout
stopped it." instead of "…for safety." (Hideout doesn't know why), and the version in the title.
