# Hideout shared test vectors (#906)

Vendored by the hub, the Foxglove modules and Vulpes. Spec: [`docs/hideout-contract.md`](../../docs/hideout-contract.md)
(section 16 sets this folder). Contract version 1.13. **No implementation lives here.**

Rules (spec 16, 13.2):
- A reader MUST pass every vector. A spec change that changes a result changes the vector in the same PR.
- Placeholder ids only (`foxglove.example-*`, `example.vendor.*`, `example.private.*`). Never a real private module name.
- Every key is a **TEST-ONLY** key, derived from a public label. It protects nothing and is never pinned in a release.

## How to read the expected results

[`index.json`](index.json) has one entry per vector file, keyed by its path in this folder.

| Field | Meaning |
|---|---|
| `result` | `valid`, `invalid`, or `table` (a file whose rows each carry their own result) |
| `id` | the section 15 error id a reader must report when `invalid` |
| `normalized` / `reads_as` | the fields a reader must produce for a valid manifest, after defaults, the `arete` to `vulpes` alias, and the legacy mapping (4.10) |
| `context` | what the reader is told (clock, pinned root, `seen` state, source, anchor, private ids); the default clock is `now` |
| `spec` | the spec sections the vector covers |
| `final` | for transcripts: the assertions at the end; the step list is the rest of the contract |

Each manifest and legacy file is **one whole file in, one result out**. Error ids are compared; message text is free.

## Map

| Path | Covers | Spec |
|---|---|---|
| `manifests/valid/*.toml` | every kind, `[[ui.panels]]` (five slots), `[[ui.windows]]`, `[[settings]]`, `[[actions]]`, `[[secrets]]`, `[[mcp.tools]]`, `hub = "1.0"`, activation events, `when` clauses (with `!`, and `apps.seen.<name>`, v1.4), `kind = "file"` settings (v1.4, hub 1.1), `[limits]` (v1.10, hub 1.2), a stdio module with panels (v1.4), `requires` forms, `api` string form, `arete` alias, `[edition]`, `[[pin]]`, `[identity]`, private module | 3, 4, 9, 13 |
| `manifests/invalid/*.toml` | one broken thing each: syntax, unknown keys (top, nested, from a newer minor), missing keys, ids, family, kind, version, `hub` (shape and not served: `1.3`, `2.0`; `[limits]` under `1.1`), `[limits]` values, `api`, `run`, visibility, `requires`, `enriches`, `provides`, activation and `when` grammar, UI, MCP, pin, edition, identity, actions, settings, secrets | 4, 15 |
| `legacy/*.plugin.toml`, `legacy/*.kit.toml` | what a legacy file reads as (kind service, stdio, `hub` 1.0, family by id prefix, `[lifecycle] data = "remove"`); legacy files with new keys fail | 4.10 |
| `legacy/precedence-*/` | `module.toml` before `plugin.toml` before `kit.toml` | 4.10 |
| `permissions/permissions.json` | the grammar (kept and new), unknown, bad argument, the never-list by holder | 6.1, 6.6 |
| `trust/root/` | good root, legacy `foxglove-plugin-root`, expired, rollback, bad signature, scope overlap, key reuse | 12.2, 12.3 |
| `trust/catalogs/` | one signed catalog per family, third-party in a family catalog, `plugins`/`kits` aliases, revocations, legacy v2 catalog, a user-anchored private catalog | 12.4, 13.4, 13.6 |
| `trust/catalogs/invalid/` | bad signature, unlisted key, wrong family key, out-of-scope entry or revocation, epoch, expired, rollback, kind or visibility mismatch, private in a public source, user scope breaches | 12.4, 13 |
| `trust/hub-release.json` | (v1.11) a catalog entry's `min_hub_release`: read or ignored, and the page Update Hideout opens (the allow-listed release page, else the releases page) | 7.8, 12.4 |
| `trust/install/` (index only) | unknown archive, yanked, revoked, exact folder, extra or changed file, sideload scope | 12.5, 12.6 |
| `trust/archives/`, `trust/folders/` | the signed archives and unpacked folders the catalogs pin | 12.4 |
| `trust/keys/` | TEST-ONLY public keys | 12 |
| `config/` | `hideout.toml` `[[sources]]` and `[[trust.anchors]]` tables, valid, overlapping, and the four `config.*` errors; (v1.6) the `[windows]` pop-out placement table (`windows.*`, read on its own), valid and `config.windows`, and a broken `[windows]` the sources reader passes over | 2.5, 13.4, 13.6 |
| `panels/` | `ui.register` trees: all node types, under the caps, over each cap, an unknown node type, one bad panel with a good one; (v1.4) `tone`, button `opens` and `action` (an index `context.actions` lists the module's `[[actions]]`), the tile caps; (v1.5) canvas and image sizes, `scaling`, `stale_after_ms`, `mono`, a `markdown` stream, a streamed image's `label`, no streams in a tile; (v1.8) `at-cap-*` and `over-cap-*`: each page cap (nodes, depth, text bytes, canvas shapes, table rows, streams) at the cap and one over | 5.2, 5.4, 5.5 |
| `facts/supervisor-facts.json` | (v1.4) the supervisor's `module.<id>.restart`, `.recovery`, `.previous` and `.failed` values, valid and invalid rows | 7.8 |
| `frames/panel-frames.json` | binary stream frames as hex: image, canvas, text, length mismatches; (v1.5) a multibyte character, and a text frame split inside one (dropped) | 5.4, 8.4 |
| `channels/channel-wire.json` | (v1.4) the channel wire as hex streams (the nonce first, `u32` LE framing, the 256-byte and 64 MiB caps, truncation) with the end each reaches, and which pipe names a consumer may connect to | 8.4 |
| `transcripts/lifecycle/` | `initialize` (module, tool, bad token, hub not served, not first), `module.register`/`module.ready`, re-registration, `ui.undeclared`, 30 s ready timeout, errors -32601, 2, 4, 8, bad hub minor (7), late reply after timeout, malformed JSON (-32700), module crash mid-call, duplicate request id and oversized line (v1.2) | 7.5, 8.1, 8.8 |
| `transcripts/swap/` | hot swap overlap and handover, drain `done`, `busy`, Force, Not now, `module.idle`, key action queued during a swap | 7.6 |
| `transcripts/ui/` | `ui.patch`, `ui.event` refusal, `ui.asset.put`, `ui.window.open`, `action.invoke`, `settings.get`/`set`, `hub.notify`, `ui.badge`, `ui.visibility` (v1.3; tiles too, v1.4) | 5.3, 5.9, 8.8 |
| `transcripts/stream/` | a live panel stream grant, `channel.open`/`channel.create`, denial, offer rejected, a provider's bad pipe (v1.4); (v1.5) text restored on visible, reopens then "Stopped updating" | 5.4, 8.4 |
| `transcripts/facts/`, `modules/`, `mcp/`, `gpu/` | facts, `modules.*`, required-by, MCP and private modules, GPU methods | 8.8, 9, 8.9, 13.5 |
| `transcripts/net/`, `audio/` | (v1.12) the host services' refusals: `net.open` without its grant (`net.denied`), with a bad argument (`net.argument`), and `net.send` to a stream the caller doesn't hold (`net.stream`); `audio.inputs.set_mute` without `audio.inputs.mute` (`audio.denied`) | 8.10, 8.11 |
| `transcripts/apps/` | (v1.13) `apps.watch` on the hub: the `{exe, open}` answer from `apps.running.<name>`, `app.window.appeared` and `app.window.closed` on each flip, `apps.denied` and `apps.argument` | 8.12, 4.6 |
| `transcripts/supervisor/` | each of the 13 states (`state-<id>.json`; (v1.9) `state-revoked.json`), crash backoff and stop, exit 0, rollback, root-cause-once, update waiting, an outside problem that is not a crash (v1.4), the backoff run resetting after a quiet ten minutes (v1.8) | 7.7, 7.8 |
| `transcripts/hub/` | hub restart takes modules back, 30 s grace, safe mode, quarantine, bisect | 7.9, 7.10, 5.5 |

A transcript is `{name, spec, description, steps}`. A step is a wire message `{from, to, msg}` (one JSON-RPC 2.0
message), an `{event}` outside the wire, or an `{expect}` assertion. `old`/`new` are two instances during a swap.

## Regenerating

There is no generator in the repository. The one #914 used (`tools/generate.py` here) was never committed (`**/tools/*`
is git-ignored) and no copy survives, so the vectors are edited by hand, each in the PR that changes its result (#918).
The files are authoritative. Re-signing a changed catalog or archive needs a new TEST-ONLY key generator, committed at a
tracked path (`scripts/`), not under a `tools/` folder. `.gitattributes` keeps every file byte-exact,
since signatures are over exact bytes (minisign, prehashed `ED`).

## Spec gaps

The nine gaps found while writing these vectors were decided in [#918](https://github.com/BanditF/foxglove/issues/918) and written into the spec as contract version 1.1 (1.2 adds the duplicate-request-id and oversized-line transcripts; 1.8 closes the rest: the canvas-shape and table-row caps with boundary vectors at and one over every page cap, and the backoff reset after a quiet ten minutes); the vectors above carry the decided results.
