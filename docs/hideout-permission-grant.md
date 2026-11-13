# Module permission grant (#1178)

Status: the registry layer, the control pipe with its caller check, the `hideout modules`
CLI, router narrowing and the hub-side updater hook are built and tested. The Modules-page dialog
and Permissions section are **not built yet**; this page records what they must do, from
`docs/hideout-permission-dialog-ux.md`.

## What exists

- `Registry::permission_list(id)` returns the list the dialog and CLI show: items with group,
  plain line, state (asked, never, overlap) and the **list code** (16 hex digits of the SHA-256
  over id, version, manifest hash, program path, program hash and the items).
- `Registry::approve_listed(id, code, chosen, declined, now, by)` records a grant only if `code`
  is the list now (else `permission.list_changed`, nothing written). Never-listed and overlapping
  items are refused and recorded as declined. Items in neither set stay pending.
- `Registry::revoke_items(id, items)` only narrows.
- `Registry::attest_update(id, version, by, now)` is reached only through
  `Supervisor::attest_verified_update(id, &Verified)` (hub-side, no path argument: the new program is
  `run[0]`), and a `Verified` exists only for a `latest.json` whose Ed25519 signature checked out; it keeps a grant across a new program file in the same install
  folder and records the history (`bound.attested`). Any other program change voids the grant.
  This is weaker than binding to one hash; see contract section 6.4.
- `Supervisor::turn_on` refuses with "Hideout needs your OK first. Press Turn on… to review."
  while permissions are pending or the binding is void.
- `Supervisor::review_approved(id, code, chosen, declined, by)` grants for the list the user saw.

## Still to build

- The dialog on the Modules page (reuse `keyboard_grant_ui.rs`), the Permissions section
  (Allowed / Off / Can't be allowed, Turn off, Turn all off, Change...) and the attestation history,
  with Review then Allow for a voided binding.
- Handing the signed `latest.json` and its signature from the app's updater to the hub after a
  successful install, so the hub calls `attest_verified_update` (today the hook exists and is tested;
  nothing in the shipped updater sends it yet, so an update still voids until then).
- Signer-bound grants (#383 / row D), the tightening of contract section 6.4.
