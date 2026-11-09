# Hideout shared test vectors (#906)

Vendored by the hub, the Foxglove modules and Vulpes. Spec: [`docs/hideout-contract.md`](../../docs/hideout-contract.md)
(section 16 sets this folder). Contract version 1.2. **No implementation lives here.**

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
| `manifests/valid/*.toml` | every kind, `[[ui.panels]]` (five slots), `[[ui.windows]]`, `[[settings]]`, `[[actions]]`, `[[secrets]]`, `[[mcp.tools]]`, `hub = "1.0"`, activation events, `when` clauses (with `!`), `requires` forms, `api` string form, `arete` alias, `[edition]`, `[[pin]]`, `[identity]`, private module | 3, 4, 9, 13 |
| `manifests/invalid/*.toml` | one broken thing each: syntax, unknown keys (top, nested, from a newer minor), missing keys, ids, family, kind, version, `hub` (shape and not served: `1.1`, `2.0`), `api`, `run`, visibility, `requires`, `enriches`, `provides`, activation and `when` grammar, UI, MCP, pin, edition, identity, actions, settings, secrets | 4, 15 |
| `legacy/*.plugin.toml`, `legacy/*.kit.toml` | what a legacy file reads as (kind service, stdio, `hub` 1.0, family by id prefix, `[lifecycle] data = "remove"`); legacy files with new keys fail | 4.10 |
| `legacy/precedence-*/` | `module.toml` before `plugin.toml` before `kit.toml` | 4.10 |
| `permissions/permissions.json` | the grammar (kept and new), unknown, bad argument, the never-list by holder | 6.1, 6.6 |
| `trust/root/` | good root, legacy `foxglove-plugin-root`, expired, rollback, bad signature, scope overlap, key reuse | 12.2, 12.3 |
| `trust/catalogs/` | one signed catalog per family, third-party in a family catalog, `plugins`/`kits` aliases, revocations, legacy v2 catalog, a user-anchored private catalog | 12.4, 13.4, 13.6 |
| `trust/catalogs/invalid/` | bad signature, unlisted key, wrong family key, out-of-scope entry or revocation, epoch, expired, rollback, kind or visibility mismatch, private in a public source, user scope breaches | 12.4, 13 |
| `trust/install/` (index only) | unknown archive, yanked, revoked, exact folder, extra or changed file, sideload scope | 12.5, 12.6 |
| `trust/archives/`, `trust/folders/` | the signed archives and unpacked folders the catalogs pin | 12.4 |
| `trust/keys/` | TEST-ONLY public keys | 12 |
| `config/` | `hideout.toml` `[[sources]]` and `[[trust.anchors]]` tables, valid, overlapping, and the four `config.*` errors | 13.4, 13.6 |
| `panels/` | `ui.register` trees: all node types, under the caps, over each cap, an unknown node type, one bad panel with a good one | 5.2, 5.5 |
| `frames/panel-frames.json` | binary stream frames as hex: image, canvas, text, length mismatches | 5.4, 8.4 |
| `transcripts/lifecycle/` | `initialize` (module, tool, bad token, hub not served, not first), `module.register`/`module.ready`, re-registration, `ui.undeclared`, 30 s ready timeout, errors -32601, 2, 4, 8, bad hub minor (7), late reply after timeout, malformed JSON (-32700), module crash mid-call, duplicate request id and oversized line (v1.2) | 7.5, 8.1, 8.8 |
| `transcripts/swap/` | hot swap overlap and handover, drain `done`, `busy`, Force, Not now, `module.idle`, key action queued during a swap | 7.6 |
| `transcripts/ui/` | `ui.patch`, `ui.event` refusal, `ui.asset.put`, `ui.window.open`, `action.invoke`, `settings.get`/`set`, `hub.notify` | 5.3, 8.8 |
| `transcripts/stream/` | a live panel stream grant, `channel.open`/`channel.create`, denial, offer rejected | 5.4, 8.4 |
| `transcripts/facts/`, `modules/`, `mcp/`, `gpu/` | facts, `modules.*`, required-by, MCP and private modules, GPU methods | 8.8, 9, 8.9, 13.5 |
| `transcripts/supervisor/` | each of the 12 states (`state-<id>.json`), crash backoff and stop, exit 0, rollback, root-cause-once, update waiting | 7.7, 7.8 |
| `transcripts/hub/` | hub restart takes modules back, 30 s grace, safe mode, quarantine, bisect | 7.9, 7.10, 5.5 |

A transcript is `{name, spec, description, steps}`. A step is a wire message `{from, to, msg}` (one JSON-RPC 2.0
message), an `{event}` outside the wire, or an `{expect}` assertion. `old`/`new` are two instances during a swap.

## Regenerating

`python tests/hideout-vectors/tools/generate.py` rewrites everything (needs the `cryptography` package, only to make the
TEST-ONLY signatures). The files are authoritative; the tooling is a convenience. `.gitattributes` keeps every file byte-exact,
since signatures are over exact bytes (minisign, prehashed `ED`).

## Spec gaps

The nine gaps found while writing these vectors were decided in [#918](https://github.com/BanditF/foxglove/issues/918) and written into the spec as contract version 1.1 (1.2 adds the duplicate-request-id and oversized-line transcripts); the vectors above carry the decided results.
