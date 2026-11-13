# Hideout: the module permission dialog and its CLI path (UX plan, #1178)

The §6.4 permission dialog for every module, first used by the adopted module `foxglove`, which
`--adopt` registers with 13 `pending_permissions` and which `Supervisor::turn_on`
(`crates/hideout-supervisor/src/driver.rs`, "permissions wait for the permission dialog") can't
start today. Also the headless path the Hub and scripts use, and a security look. Plan only, no
production code. Review image:
`C:\Users\fuchs\splitkeeb\worktrees\h-perm-ux\review\ux-permission-dialog.png` (a Pillow mockup
with the hub's theme tokens, not an egui render; drawn by `review/ux-permission-dialog.py`).

## Research and scope

- **Reuse:** the #1155-B keyboard grant (`hideout-shell/src/keyboard_grant.rs`,
  `keyboard_grant_ui.rs`, `keyboard_actions.rs` `GrantUi`, `review/ux-1155-legacy-grant.md`): a
  hub-drawn `egui::Modal`, Not now focused and the accent fill, Enter/Esc/close answer Not now,
  focus trapped, the binding line with the path, the header line for "started without your act",
  path + SHA-256 binding, `void` that never revives, router-first narrowing. The registry's
  `Inventory::grant` (never-list via `check_grantable`, the adopted `provide:` overlap refusal)
  and `Inventory::ask`. The supervisor's "Update waiting for you" line (`shell_states.rs`
  `update_line_ui`, `modules_list.rs` `Does::Review`), whose greyed Review this dialog switches
  on. Contract §6.1 (sensitive list), §6.2 (capability lines, risk classes), §6.6 (never-list),
  §7.4 (`hideout modules <step>`, exit codes 0-5), §8.8 (`modules.*` need `modules.manage`).
- **Emulate:** Android's runtime permission screen (one item per line, revocable later, grouped),
  Windows privacy prompts (one question, plain list, Settings to undo), `apt`/`winget`'s
  "these will be installed, continue?" with a non-interactive `--yes` that still prints the list,
  and Terraform's `plan` / `apply <planfile>`: a grant applies only the exact list that was shown.
- **In scope:** the dialog (per-permission choice, groups, defaults, never-grantable and
  extra-confirm items), what Allow does through `turn_on`, narrowing after a grant, binding to
  the manifest and the program (void on change), revocation, wording, keyboard and screen reader,
  contrast, failure states, the CLI path with exact commands, text and exit codes, the security
  look (TOCTOU, manifest change, who may grant, logs).
- **Out of scope:** the catalog path (§12/§13; a signed catalog entry for `foxglove`, #90 row D);
  changing `FIRST_PARTY_AREAS` or the §6.3 exception; Foxglove's own window; MCP grants (§6.6:
  MCP clients are approved in `initialize`, not here); outside tools' once-per-tool approval
  (§6.4, a separate small prompt); firmware.

## 1. When the dialog opens

- **Only on the user's own act in the hub window:** **Turn on** on the Modules page for a module
  with `pending_permissions`, **Review…** on "Update waiting for you", **Review…** on a "needs your
  OK again" header line, or **Change…** in the module's Permissions section. Never at sign-in,
  autostart, a hub restart, an update, a file change, a module's own request or a panel event
  (same rule as §6.4 #1155-B).
- **Started without that act** (autostart with a grant voided, an upgrade that adds permissions):
  no modal. The module's page and its Modules row get the hub-drawn header line (§5 wording) with
  **Review…** · Not now.
- **One dialog, two sections, when both are needed.** For `foxglove`, the #1155-B keyboard grant is
  the dialog's last section, collapsed, as one choice that is **off** by default: "Also let
  Foxglove handle your keyboard for other apps". Expanding it shows the built #1155-B items and
  binding line unchanged. When only the keyboard grant asks, the built #1155-B dialog opens as
  today. (Taste, Hub pick: this replaces "keyboard grant first" in §6.4's last bullet.)

## 2. The dialog

Drawn by the hub (`hideout-shell`), never by a module panel; a module can't draw, focus or click
its own permission. Modal, 620 px wide, scrolls inside when taller than 80 % of the window.

Top to bottom (see the image):

1. Title, intro.
2. **All N** checkbox (tri-state: all, some, none).
3. The groups, in this order, each a heading then one checkbox per permission:
   - **Sensitive** (danger colour heading, §6.1 sensitive list and `keys`-class channels):
     unchecked by default; checking one opens the extra confirmation (below).
   - **Make changes when an app asks** (attention colour heading).
   - **Answer questions from apps**, **Share its status** (muted headings).
   - **Can't be allowed** (no checkbox): never-list items (§6.6) and refused `provide:` overlaps,
     each with its reason line. Shown so the user sees the whole ask; they are recorded as refused.
4. For `foxglove`: the keyboard grant section (collapsed, off).
5. Binding: "Only for this copy of <name>:" + the canonical path of `run[0]` (adopted modules) or
   the install folder (catalog/sideload), selectable monospace; the reasons it asks again; where
   to undo; **List code** (16 hex digits of the list hash, §8), the same one the CLI prints.
6. **Allow N** (outline; N = how many are checked; "Allow" alone is never shown) · **Not now**
   (accent fill, focused). With nothing checked the first button reads **Allow none** and still
   works: it records every item as declined, so the module can run with nothing (§4).

**Default state.** Normal-risk items start **checked** (the manifest asked for them and the
module does nothing useful without them); sensitive items and the keyboard grant start
**unchecked**. The safe default is the button, not the boxes: Not now is focused and Enter, Esc
and closing all answer it; nothing is recorded by Not now. After a void (§6), items start in the
state the user last chose, still needing Allow.

**Extra confirmation** (sensitive items, any module): checking one shows an inline confirm under
it (no second modal): "<plain line>. This lets <name> <risk sentence>." + checkbox "I understand"
that must also be ticked; Allow counts the item only with both. Applies to `key.events`,
`hid.raw`, `net`, `net.connect:*:*`, `ai.ask:*`, `secrets.ref:*`, `keys`-class channels and any
`provide:` a catalog does not assign (a sideload offering methods other apps will call).

**Never grantable here** (shown under Can't be allowed, never a checkbox): §6.6's list (Confirm
behaviours, another module's secrets, gaming reservations, `key.events`/`hid.raw`/`keys` and
`wallpaper.control` over MCP, `modules.manage` to anything but `hideout.modules` and the
first-party CLI, `facts.write:*`, `facts.write:device.*`), a `provide:` that overlaps another
adopted module's grant (`permission.provider`), a `provide:` wildcard for the §6.3 legacy methods
(only exact names carry them), and anything in `FIRST_PARTY_AREAS` for a non-first-party holder.

### The 13 `foxglove` permissions (from `app/src/foxglove/hub_link/module.toml`)

None is on the sensitive or never list; all start checked. Plain names (dialog) and the CLI's
group column:

| Group | Permission | Dialog line |
|---|---|---|
| Share its status | `facts.write:keyboard.*` | Share your keyboard's status in Hideout (sub-line: which keyboard, battery, layer, lights) |
| Share its status | `facts.read:module.*` | See which modules are installed and running |
| Answer questions | `provide:health.read` | The keyboard's health report |
| Answer questions | `provide:lights.page` | What the Lights page shows |
| Answer questions | `provide:scenes.page` | Which scenes and wallpaper looks are listed |
| Answer questions | `provide:overlay.window` | Where the layer overlay window is |
| Answer questions | `provide:overlay.shown` | Whether the layer overlay is on |
| Answer questions | `provide:tray.panel` | Whether the tray panel is open |
| Answer questions | `provide:pc_lighting.list` | Your PC's lighting zones |
| Make changes | `provide:overlay.restore` | Bring the layer overlay back on screen |
| Make changes | `provide:overlay.set_shown` | Turn the layer overlay on or off |
| Make changes | `provide:tray.set_panel` | Open or close the tray panel |
| Make changes | `provide:pc_lighting.set_zone` | Change a PC lighting zone's colour |

Group headings say the verb, so a `provide:` line is a noun phrase ("Answer questions from apps:
Your PC's lighting zones"). The lines are data: a `permission_lines` table next to the capability
grammar (one row per permission body; `provide:<method>` rows come from the method table's own
description; an unknown body falls back to §6.2's "Unknown: <raw string>" in the sensitive
group). A test keeps every permission in `module.toml` covered by a row.

## 3. What Allow does (and `turn_on` after it)

1. The dialog sends `(id, list_hash, chosen, declined, keyboard: bool)` to the registry.
2. `Registry::approve_listed` (replaces `approve`, which grants every pending item with no
   binding) takes `inventory.lock`, re-reads the hub's manifest copy, re-hashes `run[0]`,
   recomputes the list hash and **refuses unless it equals `list_hash`** (`permission.list_changed`,
   nothing written). `chosen` and `declined` must each be a subset of the shown list and together
   cover it. It then runs `Inventory::grant(chosen)` (never-list and overlap refusals still apply),
   moves `declined` and refused items to a new `declined_permissions` list, empties
   `pending_permissions`, and writes the binding record (§6). Widening writes the file first, then
   tells the router (a failed write grants nothing).
3. The router widens its in-memory grants for this module (the only widening besides hub start).
4. If the keyboard section was ticked, `allow_keyboard` runs with the same pinned path and hash.
5. `turn_on` then runs as today: revocation check, **pending is now empty**, `set_enabled(true)`,
   safe mode → "On · not started in safe mode", else `Ev::Enable`: shallow verify, launch with the
   program pinned (write-locked) from its hash, `initialize` checks the exe hash **against the
   binding record** (a mismatch here voids and does not start, §6), `module.register` gets only
   the granted `provide:` routes, ready.
6. Not now: nothing written; the module stays off; the row keeps **Turn on…**.

`turn_on`'s refusal stays, but its condition becomes "pending non-empty **or** binding void";
`declined_permissions` never blocks a start. Its message becomes the row line in §5.

## 4. After the grant: narrowing only, changes, revocation

- **Permissions section** on the module's page (and Modules row expander): each item with its
  state word, never colour alone: **Allowed** (ok), **Off** (muted, declined or turned off),
  **Can't be allowed** (danger), plus "Allowed in Hideout on <date>" or "Allowed from the command
  line on <date>" (§7). Buttons: **Turn off** per allowed item, **Turn all off**, and **Change…**
  (opens the dialog with the current states; turning an Off item on is a widening and goes
  through the full dialog, list code and all).
- **Turn off** (one or all) has no confirm (the safe direction): the router drops the grant first
  (routes removed, leases through them end `lease.closed` `revoked`, calls needing it answer
  error 5), then the file is written; a failed write is reported and not undone. The module keeps
  running with less, as #1155-B. Turning off `facts.write:keyboard.*` leaves Foxglove running with
  its tiles empty; the row line says "Foxglove can't share its status. Its tiles stay empty."
- **The file only narrows a running hub.** The inventory watcher applies a change to
  `inventory.json` as `effective = in_memory ∩ file` for `granted`, the binding and the keyboard
  record; a permission that appears in the file is ignored until the user's Allow or the next
  hub start, and is logged at warn ("ignored a grant added to inventory.json by another process").
  New `pending_permissions` from the file are taken (they only ask). This extends #1155-B's rule
  from the keyboard record to every grant.
- **Manifest change after adoption** (`--adopt` again, or an update): `Inventory::ask` puts new
  items in pending as today; items the new manifest no longer asks for are **dropped** from
  `granted` (narrowing; today they stay); the list hash changes, so any open dialog or CLI code is
  stale. A running module keeps its version until the swap ("Update waiting for you").

## 5. Wording (exact)

| Where | Text |
|---|---|
| Title | "Let <name> run in Hideout?" (after a void: same title) |
| Intro | "<name> asks for these <N> things. Each app that uses them still needs its own permission. Nothing leaves this PC." (sensitive present: drop "Nothing leaves this PC." when `net*` is asked) |
| All | "All <N>" |
| Group headings | "Sensitive" · "Make changes when an app asks" · "Answer questions from apps" · "Share its status" · "Can't be allowed" |
| Can't be allowed reasons | "Hideout never allows this for <name>." · "Another program already answers these." · "Only Hideout's own parts can do this." |
| Sensitive confirm | "I understand" |
| Keyboard section | "Also let <name> handle your keyboard for other apps" (state word "(off)"/"(on)"), then the #1155-B items and binding unchanged |
| Binding | "Only for this copy of <name>:" + path / "If the program or this list changes, Hideout asks again." / "You can turn each one off any time in Modules, <name>." / "List code <16 hex>" |
| Buttons | **Allow <N>** · **Allow none** · **Not now** (default, focused) |
| Reason on top, list changed | "The list changed while it was open. Nothing was allowed." / "Check the new list below, then choose again." |
| Reason on top, void | "<name>'s program file is new or was moved since you allowed it." |
| Header line, never asked (adopted, started without act) | "<name> needs your OK to run in Hideout" / "Until you allow it, <name> stays off." · **Review…** · Not now |
| Header line, void | "<name> changed since you allowed it" / "Its program file is new or was moved, so Hideout stopped it until you allow it again." · **Review…** · Not now |
| Header line, CLI grant (once, until seen) | "<name> was allowed from the command line" / "<N> permissions, <time>." · **Review…** · OK |
| Modules row, waiting | "Waits for your OK" · **Turn on…** (replaces "Comes in a later Hideout.") |
| Turn on refused (should not happen once built) | "Hideout needs your OK first. Press Turn on… to review." |
| Errors (inline, attention) | "Hideout is busy. Try again in a moment." (lock held) · "Hideout couldn't save your choice. Nothing changed." (write failed) · "<name>'s program isn't at <path> any more." (run[0] missing) · "This version was withdrawn." (revoked; no dialog) · "<name> needs a newer Hideout." (`manifest.hub`; no dialog) |

Sentence case. No "permission string", "provide", "method", "router", "hash" or ids in the
window except the selectable path and the list code; the CLI shows both the raw string and the
line.

## 6. Binding and void-on-change

- The grant record (new, per module, in `inventory.json` next to `granted`): `bound: {manifest_sha256,
  exe_path, exe_sha256, list_hash, at, by: "dialog"|"cli"}`. `exe_*` for adopted modules (the
  only files the hub can't hash through a catalog); catalog and sideload modules bind
  `manifest_sha256` and their `files` map (already hashed at install).
- **At every launch** the supervisor compares the pinned program's path and hash (the one
  `initialize` checks, §7.5) with `bound`. A difference **voids**: `granted` moves back to
  `pending_permissions` with the old choices kept as `last_choice` for the dialog's default, the
  keyboard record goes `void` as #1155-B, the module is **not started**, the header line says why.
  Putting the old file back does not revive (the record's hash is cleared on void).
- Consequence for `foxglove`: every Foxglove update asks again (one click on Review…, then Allow).
  Until then the hub-mode keyboard tiles and routes are off; standalone Foxglove is unaffected.
  **This needs the user's go** (decision 1 below).

## 7. CLI and headless path (the Hub, scripts)

The §7.4 front end: `hideout modules <step> <id>`. Until a separate `hideout` CLI binary exists,
`hideout.exe --modules <step> <id> …` takes the same words (as `--adopt` does today); it opens no
window and starts nothing. `foxglove-cli` never grants (it can't hold `modules.manage`); its
`status` says "Foxglove waits for your OK in Hideout: hideout modules permissions foxglove" when
`module.foxglove.state` is off with pending permissions.

**Grants go through the running hub.** `grant`, `deny` and `revoke` are requests over the hub's
control pipe (new, `hideout-control-<user><suffix>`, same naming and pid checks as the capture
and lab pipes), so the router widens from the user's act and narrowing is router-first. With no
hub running they exit 5 and change nothing (a direct file write would only take effect at the
next hub start anyway, and two doors are harder to audit). `permissions` is read-only and works
without the hub.

| Command | Does | Exit |
|---|---|---|
| `hideout modules permissions <id> [--json]` | Prints the list (group, raw string, plain line, state), the program path and hash, and the list code. Changes nothing. | 0; 1 not installed |
| `hideout modules grant <id> <perm>… --listed <code>` | Grants exactly the named items, each of which must be in the listed set; everything else in the list stays pending. | 0; 3 code missing/stale; 1 refused |
| `hideout modules grant <id> --all-listed --listed <code>` | Grants every grantable item of that exact list; prints each. | same |
| `hideout modules grant <id> … --keyboard` | Also records the #1155-B keyboard grant (only with `--listed`; never implied by `--all-listed`). | same |
| `hideout modules deny <id> (<perm>…│--all-listed) --listed <code>` | Records items as declined (lets `turn_on` start without them). | same |
| `hideout modules revoke <id> (<perm>…│--all) [--keyboard]` | Narrows; no code needed. | 0; 1; 4; 5 |
| `hideout modules enable <id>` | `turn_on` (§3). With pending items: exit 3, "Foxglove waits for your OK: hideout modules permissions foxglove". | 0; 3; 1 |

Exit codes are §7.4's: 0 done; 1 refused (error id printed); 2 usage; 3 needs approval; 4 busy
(`inventory.lock`); 5 the hub isn't running. `--json` gives `{ok, error: {id, message}, list_hash,
granted, declined, refused}`.

Rules that keep it never silent:
- No `--yes`, no `--all` for grant. `--all-listed` is valid only with `--listed <code>`, and
  `<code>` must equal the hash of the list as it is at grant time (16 hex digits or the full 64
  from `--json`). A missing code prints the full list and exits 3 without granting; a stale one
  prints "Not allowed: the list changed since you read it (permission.list_changed). Read it
  again." and exits 3.
- Every grant prints every item it granted, declined or refused, one per line, with the path it
  is bound to. Nothing is read from stdin (no interactive prompt to script around).
- The hub shows the one-time "allowed from the command line" header line and records `by: "cli"`
  with the client's process image path in the log.
- The Hub agent runs `permissions` freely (read-only) but `grant` only after the user's go in
  chat, quoting the printed list; and only against the folder the user named (decision 2).

Text of `permissions foxglove` (13 lines in full in the real output):

```
Foxglove 0.3.0 (adopted) asks for 13. None allowed yet.
Program: C:\Program Files\Foxglove\foxglove.exe  sha256 3c0d…9e21
  Share its status        facts.write:keyboard.*        Share your keyboard's status in Hideout
  …
  Make changes when asked provide:pc_lighting.set_zone  Change a PC lighting zone's colour
List code 7f3a91c20b4e55d1. To allow: hideout modules grant foxglove --all-listed --listed 7f3a91c20b4e55d1
```

## 8. Security look

**TOCTOU between the shown list and the granted list.** Today `Registry::approve` grants
whatever is in `pending_permissions` when it runs; between the dialog opening and Allow,
`--adopt` (or any same-user write to `inventory.json` or the hub's manifest copy) could add items
and they would be granted under the user's click. Fix: the **list hash** = SHA-256 of canonical
JSON `{format: "hideout-permission-list-v1", id, version, manifest_sha256, exe_path, exe_sha256,
items: [sorted canonical strings with their state: asked | never | overlap]}`. The dialog and the
CLI show the code; `approve_listed` recomputes it under `inventory.lock` and refuses on any
difference, and checks `chosen ⊆ shown`. The program hash in the list also closes "swap the exe
after the dialog": the launch compares against `bound.exe_sha256`, and the supervisor pins the
file from that hash until the instance ends. 64 bits of code against a deliberate second
preimage is out of reach; the full 64-digit form is accepted too.

**Manifest change after adoption.** The hub reads its own copy (`modules\<id>\<version>\
module.toml`), not the external folder, after `--adopt`; a re-adopt is a new list (new hash),
only asks for new items, and drops items no longer asked (§4). The version check at `initialize`
(§7.4 verify) stays.

**Who may grant.**
- In the window: only from egui input events in the hub's own dialog. No pipe method, panel
  `ui.event`, `action.invoke`, `settings.set`, tray menu item, lab control verb (outside lab
  hubs) or module call reaches `approve_listed`. A test: the router's method table has no grant
  method, and `approve_listed` is called from `hideout-shell` and the control-pipe handler only.
- Over the control pipe: the hub checks the client process (`GetNamedPipeClientProcessId`) is
  **not in any module's Job Object** (`IsProcessInJob` against each job the supervisor holds; job
  members' children inherit the job, breakaway is not allowed) and that its image path and SHA-256
  equal the hub's own image (the CLI is `hideout.exe`; later the signed `hideout` CLI by its
  catalog hash, §8.2 v1.3). `modules.*` over the router keep needing `modules.manage`, which an
  adopted module, a sideload and `foxglove` can never hold (§6.6).
- Honest limit (as #1155-B already records): a process of the same user outside the hub's jobs
  (standalone Foxglove, any app the user runs) can run `hideout.exe --modules grant` or edit
  `inventory.json`. Same-user is not a boundary on Windows. What holds: grants never widen from
  the file in a running hub, a CLI grant is always visible in the window and the log, every grant
  is bound to the exact list and program, and the user can see and revoke each item.

**Logs** (`hideout.log`, info unless noted; no tokens, no secrets; paths are the user's own):
- `permissions: <id>: dialog shown, list <code>, <N> asked, <k> can't be allowed`
- `permissions: <id>: allowed <a>, declined <d>, refused <r> (<ids>) by dialog|cli pid <p> <image>; bound <path> <hash8>`
- `permissions: <id>: not allowed: list changed (shown <code>, now <code>)` (warn)
- `permissions: <id>: turned off <list> by user` · `void: program changed|moved (<hash8> → <hash8>)` (warn)
- `permissions: <id>: ignored a grant added to inventory.json by another process: <list>` (warn)
- `permissions: control pipe: refused client pid <p> <image>: in module job <id>|not hideout` (warn)

## 9. Keyboard and screen reader

- Dialog: role dialog, name = title, description = intro + binding lines. Reading order as drawn.
  Focus starts on **Not now**; Tab: All, each item in order (Space toggles), the keyboard
  section's expander, the list code (selectable), Allow N, Not now; focus trapped, Shift+Tab back;
  closing returns focus to the button that opened it.
- Each group: role group, name = heading ("Make changes when an app asks, 4 items"). Each item: a
  checkbox whose accessible name is group verb + line + risk word, e.g. "Make changes when an app
  asks: Change a PC lighting zone's colour, checked". Sensitive items add "sensitive"; Can't be
  allowed items are plain text with the reason, not disabled checkboxes (so they aren't skipped).
- The Allow button's name follows the count ("Allow 9"); the change is announced politely once
  per toggle burst, not per keystroke.
- Header lines: polite live region, announced once; no focus taken. State words always in text.
- **Contrast** (WCAG, computed from `theme.rs` `LIGHT`/`DARK`, not measured on a render); every
  text pair ≥ 4.5:1, the checkbox outline uses `text_muted` (not `border`, which is 1.5:1 / 1.3:1)
  for the 3:1 non-text rule:

  | Pair | Light | Dark |
  |---|---|---|
  | text / surface | 17.1 | 13.0 |
  | muted / surface (sub-lines, binding, list code; checkbox outline) | 7.3 | 5.8 |
  | accent / surface (Allow N outline, keyboard expander) | 5.2 | 6.4 |
  | on_accent / accent (Not now, Review…) | 5.2 | 8.0 |
  | attention / surface ("Make changes" heading, list-changed line) | 6.3 | 8.3 |
  | danger / surface (Sensitive, Can't be allowed) | 6.9 | 5.4 |
  | ok / surface ("Allowed") | 6.5 | 7.9 |
  | text / chrome (header line) | 14.5 | 14.5 |
  | muted / chrome | 6.2 | 6.5 |

## 10. Failure states

| Case | Window | CLI | Written |
|---|---|---|---|
| List changed while open | reason line on top, new list, choose again | exit 3 `permission.list_changed` | nothing |
| `inventory.lock` held | "Hideout is busy…", dialog stays open | exit 4 | nothing |
| Write failed (Allow) | "Hideout couldn't save your choice. Nothing changed." | exit 1 | nothing; router not widened |
| Write failed (Turn off) | "Turned off. Hideout couldn't save it; it will ask again after a restart." | exit 1, says turned off in the running hub | router narrowed, file not |
| `run[0]` missing | no dialog; row "<name>'s program isn't at <path> any more." | exit 1, the path printed (the registry's `Failure::Io(NotFound)`; no error id exists for it yet) | nothing |
| Revoked version / needs newer hub | no dialog; existing row lines | exit 1 `trust.revoked` / `manifest.hub` | nothing |
| Item refused at grant (never, overlap) | under Can't be allowed | listed as refused, exit 0 if anything else was granted, else 1 | refused recorded in `declined_permissions` |
| Program hash differs at launch | header line void, not started | `enable` exit 1 `trust.mismatch` | void (§6) |
| Hub not running | n/a | exit 5 | nothing |
| Control pipe client in a module job / not hideout.exe | n/a | pipe closed, exit 1 `permission.caller` | nothing |

## 11. Hand-off acceptance (app-engineer, then an Opus security review)

- `Registry::approve_listed(id, list_hash, chosen, declined, now, by)` replaces `approve`; the
  list-hash function lives in `hideout-registry` and is shared by the dialog and the CLI.
  `review_approved` and `turn_on` go through it. Tests: added item between show and Allow is
  refused; `chosen ⊄ shown` refused; exe swapped after show refused at launch; Not now writes
  nothing.
- Watcher narrowing test: a grant added to `inventory.json` by hand is not honoured until restart
  and is logged; a removal acts at once.
- `turn_on` starts `foxglove` after a dialog Allow and after a CLI grant (window-free, lab hub,
  `HIDEOUT_CONFIG_DIR`); the #1178 rerun's check 1 passes without hand-editing the inventory.
- Control-pipe caller test: a client inside a supervised job is refused.
- New error ids in `hideout-contract` `error.rs` and contract §6/§7.4: `permission.list_changed`
  (the shown list is not the list now) and `permission.caller` (a grant request from a caller
  that may not grant). Not now from the dialog stays `lifecycle.declined` for `enable`.
- Render test writes review PNGs of the dialog, the list-changed and void states, and the
  Permissions section in both themes; a test keeps the 13 lines of §2 equal to the manifest.

## Decisions

Hub picks (taste and order, per standing rules): one dialog with the keyboard grant as a
collapsed, off section (§1); normal items pre-checked with Not now as the default button (§2);
grants through the running hub only (§7); `foxglove-cli` never grants.

**Needs the user's go:**
1. **Void on program change for adopted modules (§6).** (A, recommended) every Foxglove update
   voids the grants and Foxglove stays off in Hideout until one Review → Allow. (B) carry grants
   to a new hash named by Foxglove's Ed25519-signed `latest.json` (fewer asks, more to review),
   or (C) void only the keyboard grant and keep the 13 across updates when the list is unchanged.
2. **May a Claude session run `hideout modules grant` for the user?** Recommended: only on a
   staging/lab hub folder the user names, after the user's go in chat with the printed list; never
   on the user's real hub folder without a go each time. This also covers the #1178 rerun.

## Not verified

- Mockup only; text widths are Segoe UI estimates, not egui measurements.
- The control pipe and `IsProcessInJob` caller check are designed, not prototyped; whether the
  supervisor's jobs forbid breakaway was not traced in `sandbox.rs`.
- Whether removing no-longer-asked items on re-adopt (§4) breaks any lab fixture.
