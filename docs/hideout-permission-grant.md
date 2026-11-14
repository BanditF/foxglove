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

## Built in the Modules page

- `crates/hideout/src/permission_actions.rs` (`PermUi`) opens the dialog from Turn on, the row's
  Review button, the Permissions header line and Change... Allow calls `grant_listed_in` (router
  widened after the file write) and then turns the module on or lets the pending update swap run
  (`Supervisor::review`). Turn off and Turn all off call `revoke_items_in` (router narrowed first).
- A list that changed while the dialog was open is refused (`permission.list_changed`) and the
  dialog reopens on the new list with the reason on top. After a void, Review then Allow starts
  from the last choice.
- The Permissions section shows Allowed / Off / Can't be allowed. Tests live in
  `permission_actions.rs` and `crates/hideout-registry/tests/permission_grant.rs`; the render test
  writes `review/built-permission-*.png`.

## Still to build

- The attestation history in the section, the "Waits for your OK" row wording, and the Review
  button on the module page's own update line (still greyed).
- Signer-bound grants (#383 / row D), the tightening of contract section 6.4.

## The updater's hand-off (built)

The app can't call the hub to attest (it runs inside the module's Job Object, and the control
pipe and router refuse that). Instead, before an install in module mode, `update.rs` leaves the
verified release's `latest.json` and `latest.json.sig` in `%LOCALAPPDATA%/Foxglove/updates/attest-handoff.json`
(`foxglove_update::handoff`). The file is only a carrier. After the legacy `foxglove` module's
planned exit, once the installer helper has finished and before the restart, the supervisor
(`attest::attest_from_handoff`) takes the file (read once, deleted, ignored when older than an hour),
checks the Ed25519 signature itself with the update key, requires the release to be newer than any
version already attested for the module, and calls `attest_installed`, which still needs a bound,
not void grant and the same install folder. Any other module's planned exit never reads it. A failure
anywhere means the change voids at the next check as before (the user is asked again).

Guards added after the security review: the version rule always applies (strictly newer than the
installed and every attested version; if neither parses, refused); the hand-off must be newer than the
module's current instance; and when the signed manifest carries `exe_sha256` the program at `run[0]`
must match it. The control pipe keeps the caller check on retired jobs (children that outlive their
module), opens the client once, refuses a bad caller without reading, and reads with a 2 s deadline.
An Allow widens the router only by what it granted, never by what `inventory.json` says, and a
binding also records the program's arguments (a change voids).

Open item (pipeline): the hash-in-manifest is the real fix. Until the release pipeline emits
`exe_sha256` in `latest.json` (`foxglove-update manifest --exe-sha256 <hex>` is ready), a release
without it gets only the version rule and a warning in the log.

### Design limit

A module outside an AppContainer has the user's token: it can inject into or drive `hideout.exe`,
synthesise input to the hub's dialog, or write `inventory.json`. "A module never grants itself" is
full only for AppContainer modules unless the hub home and install folders get ACLs and every grant
needs a real click in the hub window. Follow-ups: DACL on the hub home; job UI-limit flags;
queue-and-confirm for CLI grants; a pipe-server identity check (`GetNamedPipeServerProcessId`).

Known limit: this proves "a signed release was installed by the updater", not that the file on disk is
that release's file (the hash isn't in `latest.json`); the signer-bound grant below closes that.
