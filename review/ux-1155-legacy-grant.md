# #1155-B Foxglove keyboard grant (UX plan)

The per-install, revocable user grant that lets the adopted legacy module `foxglove`
(foxglove.exe) provide the reserved v1 areas through the hub's router: `devices.*`,
`lighting.*`, `art.*`, `screens.*`, `behaviors.*`, `profiles.*`, `focus.*`. Hub decision (11/12,
#1155): option B, bound to the verified exe path and identity, recorded in `inventory.json`, off by
default in module mode; A (a catalog entry) is the long-term path. Plan only, no code. Review
image: `C:\Users\fuchs\splitkeeb\worktrees\h-1155-ux\review\ux-1155-legacy-grant-proposed.svg`
(drawn mockup with the hub's theme tokens, not an egui render).

## Research and scope

- **Reuse:** the §6.4 approval rules ("Never a modal at sign-in", "carries every existing grant
  over silently. Only new permissions ask"); the supervisor's header line for **Update waiting
  for you** (`shell_states.rs` `update_line_ui`: title, reason, **Review** · Not now, no focus
  taken, live polite announcement); the Modules page rows (`hideout_shell::modules_list`, Turn
  on / Turn off); P8's Foxglove page and its contrast table (same tokens); the router's existing
  check (`checks.rs` `FIRST_PARTY_AREAS`, `legacy_foxglove_ok`), which this grant extends for one
  module only.
- **Emulate:** Windows privacy prompts ("Let <app> access your location?": one question, a plain
  list, Yes/No, revocable in Settings) and UAC's "this copy" binding (path shown, a changed file
  asks again).
- **In scope:** when and where the user is asked, the dialog wording, the default, the one
  Permissions row, revoke and its effect, identity change, all states, keyboard and screen reader,
  both themes.
- **Out of scope:** the general §6.4 permission dialog for every module (this is its first use
  and should be built so it can grow into it, but the other sections are not designed here);
  loosening `FIRST_PARTY_AREAS` for adopted modules in general (refused); option A (catalog
  entry); `wallpaper-engine.*` through the router (needs a contract change first, below); any
  change to Foxglove's own window or to standalone mode; firmware.

## Decisions

### 1. When and where the user is asked

- **Only on the user's own act.** The dialog opens when the user presses **Turn on** for
  Foxglove in Modules for the first time (§7.4 *enable* shows the permission dialog), or presses
  **Review…** / **Turn on…** later. Never at sign-in, autostart, a hub restart or an update.
- **Started without that act** (autostart, an upgrade from a hub that had module mode on
  before the grant existed, a file change): no modal. The module starts without the grant, and
  the Foxglove page gets a hub-drawn header line, the same frame as Update waiting for you, with
  **Review…** · Not now. Not now hides the line until the next hub start; the Permissions row
  stays.
- **Existing grants carry over silently** (§6.4): the 11 `provide:` methods of P6 and the
  `facts.write:keyboard.*` grant are not asked again. Only this new grant asks, and it is never
  given silently, including on upgrade.
- **Declined** (Not now in the dialog): nothing is recorded in `granted`; the dialog does not
  come back by itself. The row shows Off with **Turn on…**.

### 2. Identity binding

- The grant is recorded with the canonical full path of `run[0]` and the SHA-256 of that file at
  the moment the user pressed Allow (the same exe hash `initialize` checks, §7.5). It is checked
  at every launch and at `initialize`, before any route in these areas is registered.
- **Path differs** (moved, or another copy registered with `--adopt`) or **hash differs**
  (swapped or updated file): the grant is void. The module still starts with its other grants;
  the routes are not registered; the header line and row say why ("is new or was moved").
  Asking again shows the reason on top of the same dialog.
- A void grant is never revived by putting the old file back: once void, only Allow records it
  again (no silent flip-flop).
- Foxglove updates change the hash, so each update asks again until A or open question 1 lands.
  That is non-modal and one click; it is the price of "a swapped exe does not inherit it".

### 3. After revoke (Turn off)

- **Foxglove keeps running, with less.** Its own window, the P8 tiles and page, `keyboard.*`
  facts and the 11 methods granted before keep working. Other apps and `foxglove-cli` calls into
  these areas through Hideout get what they get today with no provider (P6 behaviour); to them it
  is as if Foxglove weren't running.
- **Why not stop it:** the grant protects the keyboard from other apps' calls; stopping Foxglove
  would take the keyboard away from the user too. The routes are dropped at once (a registry
  reload, as §6.3 already does for a route no longer allowed); calls already forwarded finish.
- Turn off has no confirm: it is the safe direction and Turn on… restores it.

### 4. States

| State | Header line on Foxglove page | Permissions row (state · line · button) | Routes in the areas |
|---|---|---|---|
| Module not installed / not adopted, or module mode off | none | no row (Foxglove runs standalone with its own pipe) | n/a |
| Not asked yet (adopted, never turned on) | none | Off · "Not allowed yet." · **Turn on…** | none |
| Not asked, started without the user's act (upgrade, autostart) | "Foxglove needs your OK to handle your keyboard for other apps" · **Review…** · Not now | Off · "Not allowed yet." · **Turn on…** | none |
| Granted | none | On (ok) · "Keyboard, lights, art and screens, actions. Allowed <date>." · Turn off | registered after `initialize` checks path + hash |
| Declined (Not now in the dialog) | none (no nagging) | Off · "Not allowed yet." · **Turn on…** | none |
| Revoked (Turn off) | none | Off · "You turned this off. Foxglove keeps running on its own." · **Turn on…** | dropped at once |
| Identity mismatch (path or hash changed) | "Foxglove changed since you allowed it" · **Review…** · Not now | Asks again (attention) · "Foxglove's program file changed on <date>." or "…was moved on <date>." · **Review…** (main) | none; grant void |
| Module turned off / crash-stopped | the supervisor's own card (P8) | row unchanged, read-only state | none while not running |

### 5. Wording (exact)

| Where | Text |
|---|---|
| Dialog title | "Let Foxglove handle your keyboard in Hideout?" |
| Dialog intro | "Apps you have allowed to use your keyboard will reach it through Foxglove. With this on, Foxglove can do these for them:" |
| Item: Keyboard | "See which keyboard is connected, its battery, layer" (`devices.*`) |
| Item: Lights | "Change its colours, effects and brightness" (`lighting.*`) |
| Item: Art and screens | "Change what the keyboard's screens show" (`art.*`, `screens.*`) |
| Item: Actions | "Run its key actions, switch Foxglove profiles, pick the keyboard in front (never reset or bootloader)" (`behaviors.*`, `profiles.*`, `focus.*`; Confirm behaviours stay never-list, §6.6) |
| Dialog footer | "Each app still needs its own permission. Nothing leaves this PC." / "You can turn this off any time in Modules, Foxglove." |
| Binding line | "Only for this copy of Foxglove:" + full path / "If the program file changes or moves, Hideout asks again." |
| Buttons | **Allow** · **Not now** (default, focused, Esc) |
| Reason on top (Review… after a change) | "Foxglove's program file is new or was moved since you allowed it." |
| Header line, not asked | "Foxglove needs your OK to handle your keyboard for other apps" / "Until you allow it, apps that use your keyboard through Hideout can't reach it." |
| Header line, changed | "Foxglove changed since you allowed it" / "Its program file is new or was moved, so Hideout turned keyboard sharing off." |
| Row title | "Handle your keyboard for other apps" |
| Row states | "On" · "Off" · "Asks again" |
| Row buttons | "Turn off" · "Turn on…" · "Review…" (the ellipsis means a dialog follows) |

Sentence case. No "module mode", "method", "area", "router", "provide", "grant", "hash" or ids in
the UI. "Nothing leaves this PC" is true: the grant routes local pipe calls only and records
nothing outside `inventory.json`.

### 6. Default and safety

- **Not now** is the default: focused when the dialog opens, Enter and Esc both answer it.
  **Allow** is a plain (outline) button; Not now carries the accent fill. Closing the dialog any
  other way is Not now.
- The dialog and the row are drawn by the hub, never by a module panel: a module must not be able
  to draw or click its own permission. The dialog takes the focus only because the user just
  pressed Turn on / Review…; it is never raised from a background event.

### 7. Keyboard and screen reader

- Dialog: role dialog, name = title, description = intro + footer. Reading order: title, intro,
  the four items (a list, "4 items"), footer, binding line with the path, Allow, Not now. Focus
  starts on Not now; Tab / Shift+Tab cycle Allow ↔ Not now inside the dialog (focus trapped);
  closing returns focus to the button that opened it.
- Header line: polite live region, announced once per change ("Foxglove changed since you allowed
  it"); takes no focus. Tab order: Review…, Not now.
- Row: one group, "Handle your keyboard for other apps, On. Turn off, button." State words always
  appear in text, never by colour only.
- Contrast, computed (WCAG, `theme::contrast`) from `LIGHT` / `DARK` tokens and the panel tones,
  not measured on a render; every pair used is ≥ 4.5:1:

  | Pair | Light | Dark |
  |---|---|---|
  | text / surface | 17.1 | 13.0 |
  | muted / surface | 7.3 | 5.8 |
  | text / chrome (header line) | 14.5 | 14.5 |
  | muted / chrome | 6.2 | 6.5 |
  | accent (outline button text) / surface | 5.2 | 6.4 |
  | on_accent / accent (Not now, Review…) | 5.2 | 8.0 |
  | attention / surface ("Asks again") | 6.2 | 9.2 |
  | ok / surface ("On") | 6.3 | 8.4 |

  The amber `warn` token (3.8:1 light) is not used; "Asks again" uses `attention`.

## Hand-off acceptance (app-engineer, then Opus security review)

- One record in `inventory.json` under `modules.foxglove` (through `hideout-core`, sorted,
  canonical): the grant, the path, the SHA-256 and the time. Never in `hideout.toml` (§6.4). A
  test: the router registers a reserved-area route for `foxglove` only with a valid record whose
  path and hash match the connecting instance; any other adopted module, or `foxglove` with a
  mismatch, is refused (`permission.provider`) exactly as today.
- `FIRST_PARTY_AREAS` unchanged; `legacy_foxglove_ok` stays the only door, now also requiring the
  record (closes the "publisher or exe path" open item of §6.3).
- Window-free tests for each row of §4 and the strings of §5; a hideout-shell render test writes
  review PNGs of the dialog, both header lines and the four rows in both themes.
- Turn off drops the routes with no restart of the module (test: Foxglove's facts keep updating,
  a `devices.list` from another tool gets the no-provider answer).

## Open questions for the Hub (recommendation first)

1. **Updates and the hash.** **(A, pick) Ask again after every Foxglove update** (non-modal, one
   click) until the catalog entry exists. / (B) Carry the grant to a new hash when Foxglove's own
   Ed25519-signed `latest.json` names it: fewer asks, but the hub must read Foxglove's update
   manifest, more to review.
2. **Declined: ask again ever?** **(A, pick) Never by itself**: the row's Turn on… is the way back.
   / (B) The header line once more on the next Foxglove update.
3. **Where the "asked" memory lives** (so a decline is not re-asked): **(A, pick) `hideout.toml`
   `[modules.foxglove]`**, as a hub preference, not a grant. / (B) a `declined` mark in
   `inventory.json` next to the grant.
4. **Permission dialog scope.** **(A, pick) build this dialog as the first section of the §6.4
   dialog** (title, plain list, footer, Allow / Not now), so Review on Update waiting for you can
   reuse it. / (B) a one-off dialog for #1155.
5. **One switch or per area.** **(A, pick) one grant for all seven areas**, as the Hub decided ("a
   single hub setting"). / (B) per area toggles: more control, more to explain, and Foxglove's
   answers mix areas (`devices.read` gates several).

## Contract open items to record

- **A, long term:** when the Hideout root and catalog keys exist (#90 row D), a catalog-signed
  entry for the legacy module replaces this user grant; the grant is then migrated silently
  (§6.4: existing grants carry over) and the row goes away.
- **§6.3 temporary exception:** replace "the adopted module `foxglove` … the id alone grants
  nothing" with the per-install record (path + SHA-256 at grant time), checked at launch and
  `initialize`.
- **What callers get with no provider in a reserved area** (after revoke or mismatch): name the
  exact answer (error 2 vs -32601 and its `data.id`) so `foxglove-cli status` can say
  "Foxglove isn't sharing your keyboard in Hideout" instead of a bare failure.
- **`a11y_label` on a panel button:** P8's "Open Foxglove settings, Lights page" needs an
  accessible name different from the label; the panel schema has no field for it yet.
- **`wallpaper-engine.*` through the router:** string lease ids and a `lease_closed` (`lease.closed`,
  §8.5) the provider can see, so a lease held behind the router belongs to the caller, not to the
  hub's connection. Until then `wallpaper-engine.*` stays standalone-only and out of this grant.

## Not verified

- No render: the SVG is a hand-drawn mockup; text widths are estimated, not measured in egui.
- Whether foxglove.exe is installed per-user (user-writable folder) or per-machine; the hash
  binding is what makes a per-user folder safe either way.
- Whether `initialize`'s exe hash is already available to the router for adopted modules (§7.5
  says it is checked; the code path for `adopted` was not traced).
