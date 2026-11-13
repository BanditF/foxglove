# Module permission grant (#1178)

Status: the registry layer is built (list code, `approve_listed`, narrowing, void, attestation,
`turn_on` refusal). The dialog and the `hideout modules` CLI are **not built yet**; this page
records what they must do, from `docs/hideout-permission-dialog-ux.md`.

## What exists

- `Registry::permission_list(id)` returns the list the dialog and CLI show: items with group,
  plain line, state (asked, never, overlap) and the **list code** (16 hex digits of the SHA-256
  over id, version, manifest hash, program path, program hash and the items).
- `Registry::approve_listed(id, code, chosen, declined, now, by)` records a grant only if `code`
  is the list now (else `permission.list_changed`, nothing written). Never-listed and overlapping
  items are refused and recorded as declined. Items in neither set stay pending.
- `Registry::revoke_items(id, items)` only narrows.
- `Registry::attest_update(id, version, by, now)` is called by the update or installer path after
  it verified the signed package; it keeps a grant across a new program file in the same install
  folder and records the history (`bound.attested`). Any other program change voids the grant.
  This is weaker than binding to one hash; see contract section 6.4.
- `Supervisor::turn_on` refuses with "Hideout needs your OK first. Press Turn on… to review."
  while permissions are pending or the binding is void.
- `Supervisor::review_approved(id, code, chosen, declined, by)` grants for the list the user saw.

## Still to build

- The dialog on the Modules page (reuse `keyboard_grant_ui.rs`), the Permissions section
  (Allowed / Off / Can't be allowed, Turn off, Turn all off, Change…) and the attestation history.
- The control pipe (`\\.\pipe\hideout-control-<user>`) with the caller check (`permission.caller`),
  and `hideout modules permissions|grant|deny|revoke` with `--listed <code>`, exit codes 0 done,
  1 refused, 2 usage, 3 needs approval or stale list code, 4 busy, 5 hub not running.
- The narrowing of the running router (`narrow_module_grants`) and the inventory watcher
  intersecting file and memory.
- Calling `attest_update` from the updater after it verifies `latest.json`.
